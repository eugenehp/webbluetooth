#!/usr/bin/env bash
# Check that this workspace can be published, and optionally publish it.
#
#   ./scripts/release.sh            # dry run: package and verify all eight
#   ./scripts/release.sh --publish  # actually upload, in dependency order
#
# Publishing is irreversible — a version number on crates.io can be yanked but
# never reused — so the dry run is the default and `--publish` has to be asked
# for.
set -uo pipefail
cd "$(dirname "$0")/.."

# Nothing to clear here any more. Embedding `Info.plist` used to be a
# `rustflags` entry in `.cargo/config.toml` with a path relative to wherever
# cargo was invoked, which made `cargo package` fail outright — verification
# builds each archive in its own directory, and the linker could not find the
# file. `crates/webbluetooth/build.rs` emits the link argument with an absolute
# path now, so packaging and publishing need no environment of their own.

# The order is the dependency order. Each crate's `version = "0.1.0"` on its
# path dependencies becomes a registry dependency in the archive, so a crate
# cannot be verified until everything below it is available.
ORDER=(
    webbluetooth-core
    webbluetooth-apple
    webbluetooth-linux
    webbluetooth-windows
    webbluetooth-android
    webbluetooth-wasm
    webbluetooth
    webbluetooth-node
)

say()  { printf '\n\033[1m── %s ──\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=1; }
FAILED=0

# ── The things that are easy to forget ──────────────────────────────────────
say "before anything is uploaded"

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
printf '  version   %s\n' "$version"

# A dirty tree is published as it is on disk, not as it is committed, which is
# how a debugging change ends up on crates.io.
if [ ! -d .git ]; then
    # Worth saying rather than passing quietly: what gets published is what is
    # on disk, and with no version control there is nothing to compare it to.
    bad "not a git repository — nothing records what this version contains"
else
    # Only changes that can reach an archive block the release, which is the
    # same line `cargo package` itself draws: it checks each package's own
    # directory and ignores the rest of the tree. A guard stricter than the
    # tool it is guarding stops a release over a file that cannot affect it —
    # this script edited itself, and `scripts/` ships in nothing.
    #
    # The root manifest and lockfile count as blocking: `[workspace.package]`
    # feeds every crate's version, licence and repository.
    dirty=$(git status --porcelain 2>/dev/null | sed 's/^...//; s/.* -> //')
    blocking=$(printf '%s\n' "$dirty" | grep -E '^(crates/|Cargo\.(toml|lock)$)' || true)
    if [ -n "$blocking" ]; then
        bad "uncommitted changes that would be published:"
        printf '%s\n' "$blocking" | sed 's/^/        /'
    elif [ -n "$dirty" ]; then
        ok "no uncommitted changes inside any crate"
        printf '        \033[2m(%s changed outside the crates; not published)\033[0m\n' \
            "$(printf '%s\n' "$dirty" | grep -c .)"
    else
        ok "working tree is clean"
    fi
fi

# The vendored oracles are a snapshot of a moving target; shipping a stale
# blocklist ships a security decision somebody has since changed.
if [ -n "$(find crates/webbluetooth-core/spec -name '*.txt' -mtime +90 2>/dev/null)" ]; then
    bad "some vendored oracles are over 90 days old — ./scripts/update.sh all"
else
    ok "vendored oracles are recent"
fi

# ── Package and verify ──────────────────────────────────────────────────────
#
# `--workspace` is what makes this possible before the first release: it
# understands that the sibling crates are being packaged together, and verifies
# each against the others' archives rather than against crates.io.
# ── Why a separate target directory ─────────────────────────────────────────
#
# Verification compiles each archive against the *packaged* copies of its
# siblings, which cargo serves from a temporary registry. Those copies carry a
# version number, and while a version is unpublished that number stays the same
# while its contents change — so a build artifact cached under it can be from an
# older tree. That failed a perfectly good release here: `webbluetooth-android`
# would not compile against a `webbluetooth-core` that had been rebuilt hours
# earlier. A directory of its own means the verification builds what it packaged.
say "package + verify (all eight)"
# Emptied first, and not reused. Verification serves the packaged siblings from
# a temporary registry *inside* this directory, keyed by a version number that
# does not change while 0.0.1 is unpublished — so a leftover index or a
# leftover rlib can be from an older tree. Both have happened here: once a good
# release failed, once `webbluetooth-apple` could not be downloaded at all. A
# gate that can pass on stale state is worse than one that fails on it.
rm -rf target/package-verify
if CARGO_TARGET_DIR=target/package-verify cargo package --workspace >/tmp/webbluetooth-package.log 2>&1; then
    ok "cargo package --workspace"
    printf '  %s archives in target/package\n' "$(grep -c 'Packaged' /tmp/webbluetooth-package.log)"
else
    bad "cargo package --workspace"
    tail -25 /tmp/webbluetooth-package.log | sed 's/^/      /'
fi

if [ "$FAILED" -ne 0 ]; then
    printf '\n\033[31mnot ready\033[0m\n'
    exit 1
fi

if [ "${1:-}" != "--publish" ]; then
    printf '\n\033[32mready\033[0m — re-run with --publish to upload\n'
    exit 0
fi

# ── Upload ──────────────────────────────────────────────────────────────────
#
# Resumable, because it has to be: publishing is eight separate uploads and
# crates.io rate-limits *new* crates hard. A first release stops partway
# through, and what is already up cannot be taken back — so a re-run has to
# pick up where it left off rather than fail on the first crate it meets.
#
# Already on crates.io, this exact version? Then skip it. That is the whole
# resume mechanism: the registry is the state, not a file here that could
# disagree with it.
published_already() {
    [ "$(curl -s -o /dev/null -w '%{http_code}' \
        -H "User-Agent: webbluetooth-release/$1" \
        "https://crates.io/api/v1/crates/$2/$1")" = "200" ]
}

# crates.io says exactly when it will accept the next new crate:
#
#   429 Too Many Requests … Please try again after Mon, 21 Sep 2026 19:29:09 GMT
#
# Honour that rather than guessing an interval or retrying into the wall.
# Echoes the seconds to wait, or nothing if this was some other failure.
seconds_until_allowed() {
    local stamp target now
    stamp=$(sed -n 's/.*try again after \(.*\) GMT.*/\1/p' "$1" | head -1)
    [ -n "$stamp" ] || return 1
    # BSD date first, then GNU: this runs on a Mac and in the Linux container.
    target=$(date -j -u -f "%a, %d %b %Y %H:%M:%S" "$stamp" +%s 2>/dev/null) \
        || target=$(date -u -d "$stamp GMT" +%s 2>/dev/null) || return 1
    now=$(date -u +%s)
    # A few seconds of slack: the limit is enforced on their clock, not ours.
    echo $(( target - now + 10 ))
}

say "publishing $version"
for crate in "${ORDER[@]}"; do
    if published_already "$version" "$crate"; then
        printf '  %-22s \033[2malready on crates.io\033[0m\n' "$crate"
        continue
    fi

    while :; do
        printf '  %-22s ' "$crate"
        # The same isolated target directory the verification used: `cargo
        # publish` verifies too, and would otherwise be exposed to the stale
        # artifacts described above.
        if CARGO_TARGET_DIR=target/package-verify \
            cargo publish -p "$crate" >/tmp/webbluetooth-publish.log 2>&1; then
            printf '\033[32mdone\033[0m\n'
            break
        fi

        wait_for=$(seconds_until_allowed /tmp/webbluetooth-publish.log) || wait_for=""
        if [ -n "$wait_for" ] && [ "$wait_for" -gt 0 ] 2>/dev/null; then
            printf '\033[33mrate limited\033[0m — crates.io will accept the next\n'
            printf '  %-22s new crate at %s. Waiting %dm%02ds.\n' "" \
                "$(date -u -r "$(( $(date -u +%s) + wait_for ))" '+%H:%M:%S GMT' 2>/dev/null \
                   || date -u -d "@$(( $(date -u +%s) + wait_for ))" '+%H:%M:%S GMT')" \
                "$(( wait_for / 60 ))" "$(( wait_for % 60 ))"
            sleep "$wait_for"
            continue
        fi

        printf '\033[31mfailed\033[0m\n'
        tail -20 /tmp/webbluetooth-publish.log | sed 's/^/      /'
        printf '\n  Stopped. Anything already uploaded stays uploaded —\n'
        printf '  re-run with --publish and it resumes from here.\n'
        exit 1
    done
done
printf '\n\033[32mpublished %s\033[0m\n' "$version"
