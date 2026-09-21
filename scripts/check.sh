#!/usr/bin/env bash
# Build, test and lint webbluetooth — everywhere it runs.
#
#   ./scripts/check.sh              # host: test + clippy + fmt
#   ./scripts/check.sh docs         # rustdoc, per platform
#   ./scripts/check.sh crates       # each crate's own tests, per platform
#   ./scripts/check.sh packaging    # can every crate be published
#   ./scripts/check.sh apple        # all 12 Apple targets
#   ./scripts/check.sh linux        # Linux in Docker, both backends
#   ./scripts/check.sh android      # all 4 Android ABIs, DEX + JNI checks
#   ./scripts/check.sh pi           # every Raspberry Pi architecture
#   ./scripts/check.sh windows      # all 4 Windows targets, IID + COM tests
#   ./scripts/check.sh all          # everything
#   ./scripts/check.sh shell        # a shell inside the Linux container
#
# Apple tier-3 targets (tvOS, watchOS, visionOS) need nightly and -Z build-std;
# they are skipped with a note if that is missing. Linux runs in Docker against
# a mocked BlueZ, because no container has a Bluetooth controller.
# ── Why the body is a function ──────────────────────────────────────────────
#
# Bash reads a script as it runs, one command at a time, from a byte offset.
# Edit the file while it is executing and the running shell resumes at an
# offset that no longer means what it did — it does not fail, it carries on
# and prints ticks. That happened here during a full run: a shifted line
# produced `line 411: ds: command not found`, a step ran out of order, and
# everything after it was noise that looked like success.
#
# A single `main` that is called on the last line means bash parses the whole
# file before any of the work starts, so an edit mid-run is a no-op for the
# copy already running.


step() { printf '\n\033[1m%s\033[0m\n' "$1"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=1; }
# Run a command, report it, and show why if it failed. The exit code is taken
# explicitly rather than through `if cmd`, so nothing downstream can eat it.
try() {
    local label="$1"; shift
    local output rc
    # `command`, so a shell function can never shadow the binary being asked
    # for. That is not hypothetical: `node()` was a function in this script and
    # `try "…" node …` called *it*, recursing until something killed the run,
    # with no node process ever starting. `host` is a function here too, and a
    # real binary, one `try "…" host …` away from the same trap. Fixing it here
    # covers every call site rather than the ones somebody remembered.
    #
    # Safe for everything this script runs: each `try` invokes a binary or a
    # script path, never a function, and the ones that need environment
    # variables go through `env` rather than a `VAR=value` prefix — which
    # `command` would not treat as an assignment.
    output=$(command "$@" 2>&1); rc=$?
    if [ "$rc" -eq 0 ]; then
        ok "$label"
    else
        bad "$label (exit $rc)"
        printf '%s\n' "$output" | tail -25 | sed 's/^/      /'
    fi
}

host() {
    step "host"
    try "fmt"    cargo fmt --all -- --check
    try "clippy" cargo clippy --workspace --all-targets --all-features -- -D warnings
    try "test"   cargo test --workspace --all-features
    # Broken intra-doc links are invisible to every other check here: they are
    # not compile errors, and `cargo doc` only warns by default.
    try "docs"   env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --all-features
}

apple() {
    step "apple — tier 2 (stable)"
    for t in aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios \
             aarch64-apple-ios-sim aarch64-apple-ios-macabi; do
        rustup target add "$t" >/dev/null 2>&1
        try "$t" cargo clippy --workspace --all-targets --all-features --target "$t"
    done

    if rustup component list --toolchain nightly 2>/dev/null | grep -q '^rust-src (installed)'; then
        step "apple — tier 3 (nightly, build-std)"
        for t in aarch64-apple-tvos aarch64-apple-tvos-sim aarch64-apple-watchos \
                 aarch64-apple-watchos-sim arm64_32-apple-watchos \
                 aarch64-apple-visionos aarch64-apple-visionos-sim; do
            try "$t" cargo +nightly clippy -Z build-std=std,panic_abort \
                --workspace --all-targets --all-features --target "$t"
        done
    else
        step "apple — tier 3 skipped"
        echo "  needs: rustup toolchain install nightly"
        echo "         rustup component add rust-src --toolchain nightly"
    fi
}

windows() {
    step "windows (cross-compile)"
    local targets=(x86_64-pc-windows-msvc aarch64-pc-windows-msvc
                   i686-pc-windows-msvc x86_64-pc-windows-gnu)
    for t in "${targets[@]}"; do
        rustup target add "$t" >/dev/null 2>&1
        # `clippy`, not `build`: linking needs MSVC or mingw, compiling does not.
        #
        # Both crates, not just the lower one. `webbluetooth-windows` is the
        # COM and IID layer; `webbluetooth` is the backend that calls it, and
        # checking only the former leaves every vtable slot, signature and
        # cast in the backend unverified on three of these four targets.
        try "$t" cargo clippy -p webbluetooth-windows --all-targets --target "$t" -- -D warnings
        try "$t backend" cargo clippy -p webbluetooth --all-targets --target "$t" -- -D warnings
    done
    # The computed-IID algorithm and the COM objects run on any host.
    try "guid + com behaviour" cargo test -p webbluetooth-windows
}

android() {
    step "android"
    local targets=(aarch64-linux-android armv7-linux-androideabi
                   x86_64-linux-android i686-linux-android)
    for t in "${targets[@]}"; do
        rustup target add "$t" >/dev/null 2>&1
        # `check`, not `build`: linking needs the NDK, compiling does not.
        try "$t" cargo clippy -p webbluetooth -p webbluetooth-android \
            --target "$t" -- -D warnings
    done
    # The generated DEX is checked with Android's own tooling, in Docker.
    if docker build -q -f docker/Dockerfile -t "$IMAGE" . >/dev/null 2>&1; then
        try "dexdump accepts the generated classes" \
            docker run --rm -v "$PWD":/work:ro "$IMAGE" bash -c '
                cargo run -q -p webbluetooth-android --example emit-dex -- /tmp/dex >/dev/null &&
                for f in /tmp/dex/*.dex; do dexdump -f "$f" >/dev/null || exit 1; done'
        try "jni slots match jni.h, and behave in a real jvm" \
            docker run --rm -v "$PWD":/work:ro "$IMAGE" cargo test -p webbluetooth-android
    else
        bad "docker build (needed for dexdump and the JVM check)"
    fi
}

# Undefined behaviour in the unsafe glue would not show up as a test failure:
# a wrong pointer, a bad transmute or a data race can all pass quietly. Miri
# watches for it.
#
# Miri cannot call foreign functions, so everything that reaches into the
# Objective-C runtime is marked `#[cfg_attr(miri, ignore)]` and skipped —
# `webbluetooth-apple` is mostly such code, and only its pure parts are covered
# here. What *is* covered is every byte-level layer: the D-Bus codec, ATT and
# HCI parsing, DEX generation, the WinRT IID arithmetic, and the COM delegate
# called through its own hand-built vtable.
miri() {
    if ! cargo +nightly miri --version >/dev/null 2>&1; then
        bad "cargo +nightly miri (rustup +nightly component add miri)"
        return
    fi
    # Leak checking stays on everywhere except `webbluetooth-core`, which owns a
    # singleton timer thread by design — one background thread with a deadline
    # heap, rather than a thread per sleep — and Miri counts a thread still
    # running at exit as a leak. `-Zmiri-ignore-leaks` also switches off
    # *memory* leak detection, which is a real loss, so it is confined to the
    # one crate that needs it. The byte-level code lives in the others, and
    # keeping it on there is what caught a reference leak in the COM tests.
    #
    # It used to be the top crate that needed the exception, because the timer
    # was there. Splitting the portable half out moved the thread with it, so
    # `webbluetooth` is checked for leaks now too.
    try "webbluetooth-core" env MIRIFLAGS=-Zmiri-ignore-leaks \
        cargo +nightly miri test -q -p webbluetooth-core --lib
    for c in webbluetooth webbluetooth-linux webbluetooth-android \
             webbluetooth-windows webbluetooth-apple; do
        try "$c" cargo +nightly miri test -q -p "$c" --lib
    done
    # The fuzzers use a small corpus under Miri; the point is the checking, not
    # the coverage.
    try "fuzzers under miri" \
        cargo +nightly miri test -q -p webbluetooth-linux --test fuzz_parsers
}

# Raspberry Pi is Linux with BlueZ, so the backend is the same one — what
# differs is the architecture. Pi OS ships 64-bit and 32-bit images, and the
# Zero/1 are ARMv6, which is a third target again.
pi() {
    step "raspberry pi (cross-compile)"
    local targets=(
        aarch64-unknown-linux-gnu        # Pi 5/4/3/Zero 2 W, 64-bit Pi OS
        armv7-unknown-linux-gnueabihf    # Pi 4/3/Zero 2 W, 32-bit Pi OS
        arm-unknown-linux-gnueabihf      # Pi Zero / Zero W / 1 / CM1 (ARMv6)
        aarch64-unknown-linux-musl       # static, 64-bit
        armv7-unknown-linux-musleabihf   # static, 32-bit
    )
    for t in "${targets[@]}"; do
        rustup target add "$t" >/dev/null 2>&1
        try "$t" cargo clippy --workspace --all-targets --target "$t" -- -D warnings
    done
    # The daemon-free backend matters most on a Pi, so check it on ARM too.
    try "armv7 + linux-hci" cargo clippy -p webbluetooth --all-targets \
        --features linux-hci --target armv7-unknown-linux-gnueabihf -- -D warnings
}

linux() {
    step "linux (docker, mocked bluez)"
    if ! docker build -q -f docker/Dockerfile -t "$IMAGE" . >/dev/null 2>&1; then
        bad "docker build"; return
    fi
    local run=(docker run --rm -v "$PWD":/work:ro "$IMAGE")
    try "bluez backend"     "${run[@]}" cargo test --workspace
    try "bluez clippy"      "${run[@]}" cargo clippy --workspace --all-targets -- -D warnings
    # `cargo test --workspace` above includes tests/fuzz_parsers.rs, which
    # hammers the hand-written ATT, HCI and D-Bus decoders with garbage.
    #
    # The daemon-free backend only gets built and linted here: end-to-end needs
    # a controller, and no container has one — Docker's kernel is built with
    # `# CONFIG_BT is not set`, so there is no Bluetooth subsystem at all.
    # Use ./scripts/qemu/vm.sh for that.
    try "linux-hci build"   "${run[@]}" cargo build -p webbluetooth --features linux-hci --all-targets
    try "linux-hci clippy"  "${run[@]}" cargo clippy -p webbluetooth --all-targets --features linux-hci -- -D warnings
}

# ── The web ─────────────────────────────────────────────────────────────────
#
# A browser is not an operating system: on wasm32 the target OS is the literal
# string "unknown", so a capability check phrased as "every OS except X"
# silently includes the web. That is worth a leg of its own.
wasm() {
    step "wasm (browser)"
    try "wasm32-unknown-unknown" cargo build -p webbluetooth --target wasm32-unknown-unknown
    try "clippy"                 cargo clippy -p webbluetooth --target wasm32-unknown-unknown -- -D warnings
    try "abi agrees with the shim" cargo test -p webbluetooth-wasm
    # The shim is the other half of the ABI and is not compiled, so nothing
    # else would notice it becoming unparseable.
    if command -v node >/dev/null 2>&1; then
        try "the shim parses" node --input-type=module -e \
            "import('./crates/webbluetooth-wasm/js/webbluetooth.js').then(()=>{},e=>{console.error(e);process.exit(1)})"
    else
        printf '  \033[2m- skipped: the shim is unchecked without node\033[0m\n'
    fi
}

# ── Node and Deno ───────────────────────────────────────────────────────────
#
# One addon for both: Deno implements Node-API, so the same `.node` file loads
# in each. The symbols come from the host process rather than a library, which
# is why this needs a linker flag and why a plain `cargo build` of a `cdylib`
# is the real test that the flag is right.
# Named `node_addon` rather than `node`, which is the bug this used to be.
#
# `try` runs its arguments as a command, and a shell function takes precedence
# over a binary of the same name — so `try "…" node -e …` inside `node()` called
# *this function* again, and did so from `wasm()` too. Both steps recursed until
# something killed them, forking a fresh copy of the script each time round, and
# because no `node` process ever started it looked like a shell waiting on a
# pipe rather than a runaway. The node and deno smoke checks never ran at all.
#
# `try` now runs everything through `command`, which closes this generally —
# the rename stays because a function named after a binary is a trap worth not
# leaving lying around, and because it is what makes the two fixes independent.
node_addon() {
    step "node + deno (native addon)"
    try "builds as a loadable addon" cargo build -p webbluetooth-node
    try "clippy"                     cargo clippy -p webbluetooth-node --all-targets -- -D warnings
    try "abi and conversions"        cargo test -p webbluetooth-node
    # Windows has no import library here: `raw-dylib` synthesises the import
    # stubs from the declarations, so the addon cross-compiles from anywhere.
    # Check-only, because the MSVC linker is not on this machine — the same
    # reason the `windows` leg above is check-only.
    try "cross-compiles for windows"  cargo clippy -p webbluetooth-node --target x86_64-pc-windows-msvc -- -D warnings
    try "cross-compiles for arm windows" cargo clippy -p webbluetooth-node --target aarch64-pc-windows-msvc -- -D warnings

    # A `.node` file is what both runtimes `require`; the built artefact has a
    # platform extension, so it is staged under the name they expect.
    local addon
    addon=$(ls target/debug/libwebbluetooth_node.dylib \
               target/debug/libwebbluetooth_node.so 2>/dev/null | head -1)
    if [ -z "$addon" ]; then
        bad "no addon was built"
        return
    fi
    mkdir -p target/node
    cp "$addon" target/node/webbluetooth.node
    # Re-sign after the rename, on macOS.
    #
    # The linker signs a `cdylib` ad-hoc as it produces it. Staging it under
    # another name leaves a signature the loader rejects, and the way it
    # rejects it is by killing the process that tried — SIGKILL, no message,
    # no exception, exit 137. That looks exactly like a crash in the addon,
    # which is the wrong place to spend an afternoon.
    if command -v codesign >/dev/null 2>&1; then
        codesign --force --sign - target/node/webbluetooth.node >/dev/null 2>&1 || true
    fi

    # Loading it is the real test of the ABI: it says the symbols resolved,
    # the entry point was found under the name the runtime looks up, and the
    # exports were installed. Calling through says the promise bridge works —
    # the answer comes from a worker thread, so a script that exits early
    # prints nothing and passes, which is the failure this catches.
    if command -v node >/dev/null 2>&1; then
        try "loads and calls through in node" node -e \
            "const m=require('./target/node/webbluetooth.node');
             const names=Object.keys(m);
             if(!names.includes('requestDevice'))throw new Error('exports missing: '+names);
             m.getAvailability().then(v=>{
               if(typeof v!=='boolean')throw new Error('expected a boolean, got '+typeof v);
             }).catch(e=>{console.error(e);process.exit(1)});"
    else
        printf '  \033[2m- skipped: node is not installed\033[0m\n'
    fi

    # The same binary, unchanged: Deno implements Node-API.
    if command -v deno >/dev/null 2>&1; then
        try "loads and calls through in deno" deno run --quiet --allow-ffi --allow-read \
            --unstable-ffi scripts/node/smoke.mjs

    else
        printf '  \033[2m- skipped: deno is not installed\033[0m\n'
    fi
}

# ── The real Linux stack ────────────────────────────────────────────────────
#
# Everything the `linux` leg does is against a D-Bus mock, because a container
# has no Bluetooth subsystem — Docker's kernel is built with
# `# CONFIG_BT is not set`. A mock exercises the code that talks to BlueZ and
# nothing below it.
#
# This leg uses the real thing: a VM with its own kernel, a real `bluetoothd`,
# virtual controllers, and a peer advertising a real GATT server. It is the
# only place the daemon-free backend can run at all, since that one speaks ATT
# over an L2CAP socket and HCI over a raw one.
#
# Skipped rather than failed when the VM is not provisioned: `vm.sh setup`
# downloads an image and builds an out-of-tree kernel module, which is not
# something to do implicitly inside a check.
vm() {
    step "linux (qemu, real bluez + real controller)"
    if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
        printf '  \033[2m- skipped: qemu is not installed\033[0m\n'
        return
    fi
    if [ ! -f target/qemu/debian.qcow2 ]; then
        printf '  \033[2m- skipped: no VM; run ./scripts/qemu/vm.sh setup\033[0m\n'
        return
    fi
    try "vm boots"              ./scripts/qemu/vm.sh start
    try "controllers and peer"  ./scripts/qemu/vm.sh radio
    try "both backends, real radio" ./scripts/qemu/vm.sh test
}

# Each crate's own tests, on its own platform.
#
# `cargo clippy -p webbluetooth --all-targets` builds the *library* of every
# dependency but not their test targets, so a broken `mod tests` inside
# `webbluetooth-windows` is invisible from the facade — and invisible on this
# host too, since the module is `#[cfg(target_os = "windows")]`. Two of them
# had been broken since the peripheral adapters moved out, referring to a
# `defs` module that only exists on the facade's side of the re-export.
crates() {
    step "crates — each on its own platform, with its own tests"
    local pairs=(
        "webbluetooth-core:x86_64-unknown-linux-gnu"
        "webbluetooth-apple:aarch64-apple-darwin"
        "webbluetooth-linux:x86_64-unknown-linux-gnu"
        "webbluetooth-windows:x86_64-pc-windows-msvc"
        "webbluetooth-android:aarch64-linux-android"
        "webbluetooth-wasm:wasm32-unknown-unknown"
        "webbluetooth-node:x86_64-unknown-linux-gnu"
    )
    for pair in "${pairs[@]}"; do
        local crate="${pair%%:*}" target="${pair##*:}"
        rustup target add "$target" >/dev/null 2>&1
        try "$crate on $target" cargo clippy -p "$crate" --all-targets \
            --all-features --target "$target" -- -D warnings
    done
}

# Documentation, per platform rather than per host.
#
# The `host` step's `cargo doc` cannot see a module gated to another target —
# `webbluetooth-windows`'s peripheral adapter is `#[cfg(target_os = "windows")]`
# and a Mac documents none of it. A broken intra-doc link in there is invisible
# until somebody builds docs for Windows, which is to say until docs.rs does.
#
# Each crate is documented for the target its `[package.metadata.docs.rs]`
# names, which is the one docs.rs will use.
docs() {
    step "docs — every crate on its own platform"
    local pairs=(
        "webbluetooth:aarch64-apple-darwin"
        "webbluetooth:x86_64-unknown-linux-gnu"
        "webbluetooth:x86_64-pc-windows-msvc"
        "webbluetooth:aarch64-linux-android"
        "webbluetooth:wasm32-unknown-unknown"
        "webbluetooth-core:x86_64-unknown-linux-gnu"
        "webbluetooth-apple:aarch64-apple-darwin"
        "webbluetooth-linux:x86_64-unknown-linux-gnu"
        "webbluetooth-windows:x86_64-pc-windows-msvc"
        "webbluetooth-android:aarch64-linux-android"
        "webbluetooth-wasm:wasm32-unknown-unknown"
        "webbluetooth-node:x86_64-unknown-linux-gnu"
    )
    for pair in "${pairs[@]}"; do
        local crate="${pair%%:*}" target="${pair##*:}"
        rustup target add "$target" >/dev/null 2>&1
        try "$crate on $target" env RUSTDOCFLAGS="-D warnings" \
            cargo doc --no-deps -p "$crate" --target "$target"
    done
}

# Can this be published at all? `scripts/release.sh` without `--publish`
# packages every crate and compiles each archive on its own, which is the only
# thing that catches a file the build reads but the package does not ship.
packaging() {
    step "packaging"
    try "cargo package --workspace" ./scripts/release.sh
}

main() {
    set -uo pipefail
    cd "$(dirname "$0")/.."

    FAILED=0
    IMAGE=webbluetooth-test

    case "${1:-host}" in
        host)  host ;;
        docs)  docs ;;
        crates) crates ;;
        packaging) packaging ;;
        vm)    vm ;;
        wasm)  wasm ;;
        node)  node_addon ;;
        miri)  miri ;;
        apple) apple ;;
        linux) linux ;;
        android) android ;;
        pi)    pi ;;
        windows) windows ;;
        # `miri` is not in `all`: it needs a nightly toolchain and takes minutes.
        all)   host; docs; crates; packaging; apple; linux; pi; android; windows; wasm; node_addon; vm ;;
        shell) docker build -q -f docker/Dockerfile -t "$IMAGE" . >/dev/null
               exec docker run --rm -it -v "$PWD":/work:ro "$IMAGE" bash ;;
        *)     echo "usage: $0 [host|docs|crates|packaging|apple|linux|pi|android|windows|wasm|node|vm|miri|all|shell]"; exit 2 ;;
    esac

    echo
    # The exit status has to come from $FAILED, not from the echo. Written as a
    # `&&`/`||` one-liner the script's status was the trailing echo's, which always
    # succeeds — so every run reported success to a caller no matter what failed.
    if [ "$FAILED" -eq 0 ]; then
        echo "all checks passed"
    else
        echo "some checks failed"
        exit 1
    fi
    exit "$FAILED"
}

# Both commands on one line, so bash has parsed the `exit` before `main`
# starts. The protection is not "bash reads the file first" — it reads each
# top-level command as it reaches it — it is that the process never returns to
# read another byte. Relying on `main` to exit on every path would put that
# back one careless `return` later, and the failure would look like success.
main "$@"; exit $?
