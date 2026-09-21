# webbluetooth

The [Web Bluetooth API](https://webbluetoothcg.github.io/web-bluetooth/) in Rust — on Apple platforms, Linux, Android and Windows.

`requestDevice`, `gatt.connect()`, `getPrimaryService`, `readValue`,
`startNotifications` — the same names, the same security model. Promises become
`async fn`s and events become `Stream`s; everything else reads like the
JavaScript original.

```rust
use webbluetooth::prelude::*;
use webbluetooth::uuid::{characteristics, services};

let bluetooth = Bluetooth::new();

let device = bluetooth.request_device(
    RequestDeviceOptions::new()
        .filter(DeviceFilter::new().service(services::HEART_RATE)?)
        .optional_service(services::BATTERY_SERVICE)?,
).await?;

let gatt = device.gatt();
gatt.connect().await?;

let service = gatt.get_primary_service(services::HEART_RATE).await?;
let characteristic = service
    .get_characteristic(characteristics::HEART_RATE_MEASUREMENT).await?;

let mut beats = characteristic.start_notifications().await?;
while let Some(value) = beats.next().await {
    println!("{} bpm", value[1]);
}
```

`Bluetooth::shared()` is the process-wide session — `navigator.bluetooth`,
which a page cannot ask for a second of. Prefer it: a device is only usable
through the session that found it, so two `new()` calls in one program produce
a device that will not connect. `new()` and `with_chooser()` open separate
sessions deliberately, for when that is what you want.

Already holding `uuid::Uuid` constants, as anything arriving from `btleplug`
is? Turn on the `uuid` feature and pass them straight to `get_primary_service`
and the rest:

```toml
webbluetooth = { version = "0.0.1", features = ["uuid"] }
```

## Start here

```sh
cargo run -p webbluetooth --example doctor    # why Bluetooth is or isn't usable
cargo run -p webbluetooth --example scan      # list everything in range
cargo run -p webbluetooth --example battery   # read the nearest battery level
cargo run -p webbluetooth --example heart_rate --features terminal-chooser
cargo run -p webbluetooth --example sensors -- battery_service  # every stream at once
cargo run -p webbluetooth --example peripheral # act as a GATT server instead
cargo run -p webbluetooth --example dual_role  # both roles at once
cargo run -p webbluetooth --example watch      # advertisements, without connecting
cargo run -p webbluetooth --example l2cap      # a byte pipe, not attributes
cargo run -p webbluetooth --example roundtrip  # exercise a peer and report what worked
```

Run `doctor` first. Almost every "it finds nothing" report is an authorization
problem rather than anything to do with the radio.

Coming from [btleplug](https://github.com/deviceplug/btleplug)?
[MIGRATING.md](MIGRATING.md) is the call-by-call mapping, taken from a real
port rather than from reading two API docs side by side.

## Authorization, which is the part that bites

macOS and iOS gate Bluetooth behind TCC. A process with no
`NSBluetoothAlwaysUsageDescription` is reported **`Unauthorized` and is never
prompted** — it just sees no adapter, silently.

**In an app bundle**, put the key in `Contents/Info.plist`. On iOS it is
required; sandboxed macOS apps also need the
`com.apple.security.device.bluetooth` entitlement.

**In a bare CLI binary** there is no bundle, so the plist is linked into the
Mach-O image instead. Do it from a build script, where the path can be
absolute:

```rust
// build.rs, with Info.plist beside it
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo::rerun-if-changed=Info.plist");
        println!("cargo::rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{dir}/Info.plist");
    }
}
```

This workspace does exactly that, in `crates/webbluetooth/build.rs`.

It is tempting to put the flag in `.cargo/config.toml` instead — this
repository did, for a while — but a cargo config can only name a path relative
to *the directory cargo was invoked from*, and is itself discovered by walking
up from that same directory. Build from anywhere else, with `--manifest-path`
or from an IDE or a parent workspace, and the flag quietly does not apply: the
build succeeds and the binary has no section, which surfaces later as
`Unauthorized` with nothing pointing back at the build. `cargo package`, which
verifies each archive in a directory of its own, fails outright instead.

One more thing that is easy to lose an afternoon to: **the prompt is attributed
to the responsible process**, not to your binary. Launched from an editor, a
CI runner, or an agent that has no Bluetooth grant of its own, the request is
denied without a dialog. Run it straight from Terminal.app the first time.

## Peripheral mode

Web Bluetooth is central-only — a web page is always the GATT *client*.
CoreBluetooth also offers `CBPeripheralManager`, and the same
runtime-synthesised-delegate machinery serves it, so `webbluetooth::peripheral`
is there as a sibling API rather than a port of anything.

```rust
use webbluetooth::peripheral::{Advertising, Characteristic, Peripheral, Request, Service};
use webbluetooth::uuid::{characteristics, services};

let (peripheral, mut requests) = Peripheral::new();

let published = peripheral.publish(
    Service::new(services::BATTERY_SERVICE)?.characteristic(
        Characteristic::new(characteristics::BATTERY_LEVEL)?.read().notify(),
    ),
).await?;

peripheral.start_advertising(
    Advertising::new().local_name("Rust").service(services::BATTERY_SERVICE)?,
).await?;

let level = published.characteristic(characteristics::BATTERY_LEVEL).unwrap();

while let Some(request) = requests.next().await {
    match request {
        Request::Read(read)               => read.respond(&[87])?,
        Request::Write(write)             => write.accept(),
        Request::Subscribed { .. }        => level.notify(&[87]).await?,
        _ => {}
    }
}
```

The request stream is single-consumer by construction: a read must be answered
exactly once, so it cannot be handed to two listeners. **An unanswered request
is answered for you** — dropping a `ReadRequest` or `WriteRequest` responds
`RequestNotSupported` rather than stalling the central until the ATT timeout.

Two rules CoreBluetooth enforces by raising an Objective-C exception — which
would abort the process — are checked in Rust and returned as errors instead:

- **A characteristic with a fixed value must be read-only.** Anything writeable
  or changing leaves the value unset and answers reads on demand.
- **Only user description (`0x2901`) and presentation format (`0x2904`)
  descriptors can be created.** The Client Characteristic Configuration
  descriptor is added automatically when a characteristic can notify.

Apple honours only a local name and a service UUID list when advertising — no
manufacturer data, no service data — and the packet holds 28 bytes for
everything.

The GATT blocklist does **not** apply here: it exists to stop a *client*
reaching dangerous attributes on someone else's device, and publishing a
service of your own is a different act.

## Scanning without asking for a device

`requestDevice()` grants access to one device. The specification also has two
ways to watch advertisements while granting nothing, and both are here:

```rust
// requestLEScan() — every advertisement in range, repeats included.
let mut scan = bluetooth
    .request_le_scan(LeScanOptions::accept_all_advertisements().keep_repeated_devices(true))
    .await?;
while let Some(event) = scan.next().await {
    println!("{} {:?} dBm", event.id, event.rssi());
}

// watchAdvertisements() — one granted device, and nothing else.
let mut watch = device.watch_advertisements().await?;
```

Both are useful for peripherals whose payload lives in the advertisement rather
than in a characteristic, and for noticing a device is back in range without
connecting to it.

**The radio scan is shared.** Every platform here scans globally — one
`scanForPeripheralsWithServices:`, one `StartDiscovery`, one watcher, one
`BluetoothLeScanner` — so the scan is reference-counted and its sightings are
published to everyone interested. The first watcher starts the radio and the
last one stops it. Filters are combined rather than intersected: the radio has
to be at least as wide as the widest watcher, and each watcher then applies its
own filter to what arrives.

That matters because the obvious implementation is wrong in a way that is hard
to notice. With one scan owned by whoever started it, a `requestDevice` stops
the radio when its chooser returns — cancelling a scan somebody else is still
reading. `examples/watch.rs` asserts against exactly that, and on real hardware
it sees 275 further sightings after a concurrent `request_device` completes.

## Staying at parity with the standard

"Is this still the whole API?" is not a question to answer from memory. That is
how the GATT blocklist ended up with three wrong entries and the JNI table with
four wrong slots — both times the fix was to find something authoritative and
ask it.

`./scripts/update.sh surface` takes the IDL the specification itself declares,
extracted by `w3c/webref` straight from the standard, and writes every member
to `crates/webbluetooth-core/spec/web-bluetooth-surface.txt`.
`tests/web_bluetooth_surface.rs` then
requires each of the 106 to be mapped to something here or excluded on purpose
with the reason recorded — and it checks the mapped item *exists*, because a
mapping table is trivially easy to fill in optimistically.

When the standard gains a member, the generated file changes and the test fails
until somebody decides what to do about it.

```
Web Bluetooth surface: 81 implemented, 25 excluded
```

The 25 are not gaps. Every one is something that cannot exist outside a
browser, and the test enforces that the reason names one of a short list of
accepted categories rather than letting "excluded" become a place to put
anything inconvenient:

| | |
|---|---|
| 10 | the Permissions API — a browser's grant store |
| 12 | `ValueEvent` and `BluetoothAdvertisingEventInit` — DOM event types a page constructs; streams carry values here |
| 1 | `Bluetooth.referringDevice` — nothing outside a browser has a referrer |
| 1 | `WatchAdvertisementsOptions.signal` — an `AbortSignal`; dropping the stream stops the watch |
| 1 | `writeValue` — deprecated by the specification in favour of the two explicit forms |

**Both oracles are vendored, because neither contains the other.** MDN's compat
data records what browsers have *shipped* — 49 members — so everything the
standard defines but nobody implements is absent from it, including
`getIncludedService`, which this crate had already implemented and MDN would
never have asked for. It cannot establish completeness.

But the IDL carries no status: the word "deprecated" does not appear in it
once. The `writeValue` exclusion below says the specification deprecates it,
and while the IDL was the only oracle, nothing in the repository could have
contradicted that — it was written from memory. MDN does mark it `deprecated`,
so the claim was right, but it was not *checked*. The test now requires any
deprecation claim to be corroborated by MDN, and requires the two to agree
wherever they overlap.

Five things were missing when the IDL check first ran, all of which had been
asserted rather than verified:

* **`onavailabilitychanged`** — now `Bluetooth::watch_availability`. Polled
  rather than pushed, and the documentation says so: CoreBluetooth and BlueZ
  report it, a raw HCI socket does not, and Android would need a
  `BroadcastReceiver` this crate does not install. One polled implementation
  behaves the same everywhere; two pushed ones and three silent ones would not.
* **`onserviceadded` / `onservicechanged` / `onserviceremoved`** — now
  `RemoteGattServer::watch_services_changed`. One stream, not three: the
  platforms report *that* the set changed, not which service, and splitting it
  would mean inventing detail nobody has.
* **`watchingAdvertisements`** — now on `BluetoothDevice`.
* **`appearance`** — now on `Advertisement`, read from AD type `0x19` on a raw
  HCI scan and from BlueZ's `Appearance` property.
* **The four back-pointers** — `characteristic.service`, `service.device`,
  `descriptor.characteristic`, `server.device`.

And one real bug: **`reliableWrite` was reading the wrong bit.** It returned the
properties byte's *Extended Properties* flag, which only says an Extended
Properties **descriptor exists**; `reliableWrite` and `writableAuxiliaries` are
bits *inside* that descriptor. The BlueZ mapping even collapsed
`"extended-properties" | "reliable-write"` into one value. Three separate bits
now, with `extended_properties()` for the flag itself, documented as
unavailable on CoreBluetooth and Android which expose nothing finer.

The check covers *accounting*, not behaviour: that a member has a counterpart,
not that the counterpart is right. Behaviour is what the round-trip examples and
the per-backend tests are for.

## What is different from a browser

**The chooser is yours.** `requestDevice()` is defined around a user gesture and
a picker dialog, and that consent step is why the API is safe to expose at all.
A library has no chrome to draw, so it is a trait:

| Chooser | Behaviour |
|---|---|
| `FirstMatch` (default) | Takes the first match automatically. Convenient — and a deliberate removal of the consent step. |
| `StrongestSignal` | Collects for a window, picks the strongest RSSI. |
| `TerminalChooser` | Numbered list on stdout, reads a line. Feature `terminal-chooser`. |
| `Predicate` | Your closure over each candidate. |
| *your own* | Implement `DeviceChooser` against whatever UI you have. Everything the signature names — `Candidates`, `Candidate`, `BoxFuture` — is in the prelude, so it costs no dependency of your own. |

**Everything else about the security model is kept.**

- The **GATT blocklist** is vendored from the [WebBluetoothCG registry](https://github.com/WebBluetoothCG/registries)
  and enforced: the HID service, unsigned firmware-update services, the serial
  number, and direct CCCD writes are all refused. Refresh it with
  `scripts/update.sh blocklist`.
- The **manufacturer data blocklist** — the CG's *second* registry — is
  vendored and enforced too. Today it holds one entry, Apple's iBeacon, because
  a beacon's proximity UUID is a location fix and reading one off a passing
  stranger's phone is not something a permission should be able to grant. It
  applies at two points: a `manufacturerData` filter wholly inside it is
  refused, and blocked data is stripped from advertisement events even for a
  caller who asked for that company.
- **Advertisement events are narrowed to the grant.** Three fields, not one:
  service UUIDs, service data and manufacturer data. Asking for no companies
  means seeing none — `optional_manufacturer_data` is a permission, not a
  preference.
- The **per-device service allowlist** from `filters` + `optional_services` is
  enforced on every `get_primary_service`. Reaching a service you did not ask
  for is a `SecurityError`, as in a browser.
- Blocklisted attributes are **filtered out of discovery**, not reported and
  then refused.

**No device addresses — on Apple.** `BluetoothDevice::id` is CoreBluetooth's
per-host identifier there: Apple does not expose hardware addresses, and the
value differs between machines for the same peripheral. Everywhere else the id
*is* the address, and `BluetoothDevice::address` returns it — `None` on Apple,
and only there.

**One radio, or the one you pick.** Web Bluetooth has no notion of choosing a
controller — a browser has one Bluetooth and that is the whole API. That is
right for a page and wrong for a machine with two dongles or a test rig driving
both ends of a link, so `adapters()`, `adapter()` and `select_adapter()` sit
alongside the standard rather than inside it. BlueZ, `linux-hci` and Windows
can enumerate and select; CoreBluetooth and Android structurally cannot, and
report one adapter and refuse any other by name rather than silently carrying
on with the wrong radio.

**Grants are per-process, and per instance.** `get_devices()` returns what this
`Bluetooth` has been granted, and nothing persists across restarts unless you
opt in with `Bluetooth::with_grants`. Every path that grants — `request_device`,
`request_device_with`, `adopt_device`, `adopt_candidate` — writes to that store,
and `forget()` removes from it.

Per *instance* matters as much: each `Bluetooth::new()` opens a separate adapter
session with its own grants and its own connections, and a `BluetoothDevice`
carries the session it was found through. A device discovered by one and
connected by another will not work. A browser has exactly one
`navigator.bluetooth`; make one, clone it.

## Testing

```sh
./scripts/check.sh            # host: fmt, clippy, tests
./scripts/check.sh docs       # rustdoc, per platform — see below
./scripts/check.sh packaging  # can every crate actually be published
./scripts/check.sh apple      # all 12 Apple targets
./scripts/check.sh linux      # Linux in Docker, both backends
./scripts/check.sh pi         # every Raspberry Pi architecture
./scripts/check.sh android    # all 4 ABIs, plus DEX and JNI checks
./scripts/check.sh windows    # all 4 targets, plus IID and COM tests
./scripts/check.sh wasm       # the browser: build, ABI cross-check, shim parses
./scripts/check.sh node       # loads and calls through, in both node and deno
./scripts/check.sh miri       # unit tests under Miri (needs nightly)

./scripts/update.sh all       # refresh the vendored oracles, then read the diff
./scripts/check.sh all        # everything

./scripts/ios.sh doctor            # build, sign, install and run on an iPad
./scripts/ios.sh ios-harness peripheral

./scripts/qemu/vm.sh setup         # a Linux VM with a real Bluetooth stack
./scripts/qemu/vm.sh radio         # virtual controllers, bluetoothd, a peer
./scripts/qemu/vm.sh test          # the same round-trip, over an emulated radio

./scripts/android/art.sh           # generated classes, loaded in real ART
./scripts/android/apk.sh peripheral   # a real app, on an emulator

cargo run -p webbluetooth --example watch   # requestLEScan + watchAdvertisements
```

No container has a Bluetooth controller — least of all on a macOS host, where
Docker runs in a VM with no passthrough at all. So the Linux backend is tested
against a **mocked BlueZ**: `python-dbusmock` serves the real `org.bluez`
interfaces and `docker/mock-bluez.py` builds a Heart Rate device with a full
GATT tree on top. Everything in `tests/linux_bluez.rs` is then real traffic over
a real bus — `GetManagedObjects`, discovery filters, `Connect`, `ReadValue`,
`WriteValue`, `StartNotify`, `PropertiesChanged` — driven through the public API
with nothing stubbed on the Rust side.

Documentation is checked **per platform**, not per host, because the host
cannot see a module gated to another target: `webbluetooth-windows`'s
peripheral adapter is `#[cfg(target_os = "windows")]`, so a broken intra-doc
link in it survives every check on a Mac until docs.rs builds it. Two such
links were doing exactly that.

The protocol layers that do not need a bus at all — the D-Bus codec, ATT PDUs,
HCI advertising reports — are unit-tested on any host, including macOS, which is
where the alignment and truncation cases live. So is the agreement between the
six backends: `cargo test` compares all six adapters as source wherever it runs,
which is the only check in the workspace that sees the five this build is not
compiling.

### Apple, on real radios

Everything above is static analysis or a mock. The Apple backend is also
exercised against two real devices — a Mac and an iPad — in both roles at once,
which is the only way to test a radio protocol: something else has to be on the
other end. A controller does not hear its own advertisements, so one machine
cannot do it alone.

`examples/ios-harness.rs` is the other end. `./scripts/ios.sh` builds an
example for `aarch64-apple-ios`, wraps it in an `.app`, discovers a device, a
provisioning profile that covers it and a signing identity in that profile's
team, installs it and runs it with the console attached. The harness publishes a
private service and answers reads, writes and subscriptions;
`examples/roundtrip.rs` is the central half and reports what worked:

```
  ✓ read                   level 86
  ✓ notify                 [85, 84]
  ✓ write                  control point accepted 0x2a
  ✓ blocklist              serial number refused
  ✓ included               0000180a-0000-1000-8000-00805f9b34fb
```

Both sides log the same conversation, so each line is confirmed twice — the
iPad recorded the read of `86`, the two notifications it pushed, the `[2a]`
write and the unsubscribe. Reversing the roles works too: the iPad as central
reads a battery level from the Mac's `CBPeripheralManager`. That covers scan,
advertisement parsing, connect, service and characteristic discovery, read,
write-with-response, notify, the GATT blocklist and disconnect — over the air,
with a runtime-synthesised delegate on both ends.

The blocklist line is worth calling out: an iPad really does publish a Serial
Number String, and the crate refuses to hand it over. That check would pass
vacuously against a peer that had no serial number to leak.

### Linux, on an emulated radio

Docker cannot help here: its kernel is built with `# CONFIG_BT is not set`, so a
container has no Bluetooth subsystem at all and the D-Bus mock is as far as it
goes. `./scripts/qemu/vm.sh` boots a Debian VM instead, with `hci_vhci` built
out of tree, two linked controllers from BlueZ's `btvirt`, a real `bluetoothd`,
and `scripts/qemu/peer.py` publishing the same fixture as the iOS harness — so
`examples/roundtrip.rs` runs unchanged:

```
  ✓ read                   level 51
  ✓ notify                 [50, 49]
  ✓ write                  control point accepted 0x2a
  ✓ blocklist              serial number refused
  ✓ included               0000180a-0000-1000-8000-00805f9b34fb
```

That is the **BlueZ backend** against a real daemon rather than a mock, with the
peer logging the matching reads, writes and subscription on the other side.

With two controllers, both ends can be this crate: the peripheral engine on
`hci1` and the central on `hci0`, which is how the Linux peripheral role is
tested. The central reads the battery level and is refused the serial number;
the peripheral logs the read from `00:AA:01:00:00:00`, the subscription, the
notification and the unsubscribe.

The **`linux-hci` backend** had never been run at all, and running it found a
bug that would have failed on every real Linux machine: the ATT socket was never
`bind`ed, so the kernel left its source type at `BDADDR_BREDR` and refused every
LE destination with `EINVAL`. With that fixed it scans, parses advertising data
and brings up a link — then deadlocks on its second ATT request. That one is
still open; both it and the emulator's two fidelity gaps are written up in
PLATFORMS.md under "Linux under QEMU".

Android gets the same treatment where it can. There is no emulator here, so no
BLE runs — but the two pieces that would fail silently are checked against the
platform's own tooling:

* **The generated DEX** is read back by Android's `dexdump`, which confirms the
  superclass, the constructor's disassembly, and every override as
  `PUBLIC NATIVE` with no code.
* **On a real device**, via `./scripts/android/art.sh`. `dexdump` parses a DEX
  and a desktop JVM has no `android.bluetooth` to inherit from, so neither
  links a class. An emulator does: each generated class resolves against the
  real superclass, every declared method is confirmed `native` with the right
  arity, and the hand-written constructor — four code units, the only
  `code_item` in these classes — actually runs.

  Then the crate's own loader does it: DEX into a direct `ByteBuffer`, through
  `InMemoryDexClassLoader`, `RegisterNatives` to bind a method, and instantiate.

  ```
    dev.webbluetooth.GattCallback: loaded, instantiated=true, natives bound=1
    dev.webbluetooth.ScanCallback: loaded, instantiated=true, natives bound=0
    ...
  ART loaded and bound every generated class
  ```

  `./scripts/android/apk.sh` goes further: a real APK, built without Gradle,
  holding a `Context` and the runtime scan and connect grants. The peripheral
  half runs for real there — it publishes a GATT service and advertises it —
  and it found two bugs at once, one of which was discarding *every* callback
  event the platform delivered. The central half gets as far as registering a
  scan; two emulators cannot be made to hear each other, for reasons in
  PLATFORMS.md.
* **The JNI vtable indices** are checked twice. Every constant is compared
  against the JDK's own `jni.h`, which declares the table in order — that covers
  all forty at once, including the ones no test thinks to call. Then the slots
  the backend actually uses are exercised against a real JVM through the
  Invocation API. JNI is JNI, so a table that works on OpenJDK works on ART.

  The header check exists because the behavioural one was not enough. A slot
  that is off by one calls the neighbouring function rather than crashing, and
  `REGISTER_NATIVES` was pointing at `UnregisterNatives` — which accepts any
  class and returns `JNI_OK`, so nothing looked wrong while no callback was ever
  bound. Four indices were wrong that way.

Windows has no equivalent oracle — no machine, no Wine with WinRT, and Docker
cannot run Windows containers on macOS. What *is* checked: the computed-IID
algorithm against a published vector, the signature strings against Microsoft's
own, the IIDs and vtable slots against metadata rather than memory, and the COM
delegate's reference counting, `QueryInterface` and panic containment by calling
through its vtable exactly as Windows would. The marshalling on top of that is
unverified.

## How it works

No Swift, no Objective-C, no Java, no `libdbus`, no code generation, no shim.
Every `build.rs` in the workspace only names libraries and sets one capability
flag. Where a platform demands a type that does not exist —
a CoreBluetooth delegate, an Android callback subclass — it is built at run
time rather than shipped.

```
┌──────────────────────────────────────────────────────────┐
│  webbluetooth          what you call                     │
│  Bluetooth · BluetoothDevice · RemoteGattServer/Service/ │
│  Characteristic/Descriptor · peripheral::Peripheral ·    │
│  L2capChannel — and the one `use` that picks a backend   │
└──────────────────────────────────────────────────────────┘
          │                                  │
          ▼                                  ▼
┌─────────────────────────────┐  ┌─────────────────────────────┐
│  webbluetooth-core          │  │  webbluetooth-apple         │
│  the portable model         │◀─┤  two delegate classes built │
│  errors · UUID registry ·   │  │  at RUNTIME · private       │
│  filters · both blocklists ·│  │  dispatch queues            │
│  grants · chooser ·         │  │                             │
│  the shared scan hub ·      │  │  backend.rs ─ central       │
│  the L2CAP channel          │  │  peripheral_backend.rs ─    │
│                             │  │  GATT server                │
└─────────────────────────────┘  └─────────────────────────────┘
                                              │
                                              ▼
                                 CoreBluetooth.framework (Obj-C)
```

Five more crates sit where `webbluetooth-apple` does — `-linux`, `-windows`,
`-android`, `-wasm` — each holding its platform's FFI *and* the adapters
written against it. `webbluetooth` links exactly one, and is 2,000 lines: the
Web Bluetooth objects, and the `use` that picks a platform.

### Why three layers and not one

They were one crate until the adapters were pulled out, and the move was worth
doing for what it turned up rather than for the tidiness.

**A backend could reach the public API, and three of them did.** While
everything was `crate::`, nothing distinguished "the shared model" from "the
types above me". `android` and `linux` called into `device.rs`; all six
imported from `gatt.rs`. Those two items — `CharacteristicProperties` and the
address predicate — were portable and simply filed in the wrong place, which is
the kind of thing that only becomes visible when reaching for it costs a
dependency edge.

**And portable files held platform code.** `state.rs` carried a
`CoreBluetooth`-shaped `From` impl, `filter.rs` carried two more. Each was
`#[cfg]`-gated, so on any one target it was invisible.

**Anything two backends both needed had nowhere to live.** `RestoredScan` —
three fields describing an interrupted scan, nothing platform-specific about it
— was written out **six times**, once per backend, because a shared type would
have had to go in a module every backend could see and no backend could be sure
of. It is declared once now.

**The duplicate nobody could see.** Linux's adapter declared its own
`extern "C" { write, close }` because it was in a different crate from the FFI
that already declared them. Putting them in one crate turned that into
`write redeclared with a different signature` — the two disagreed about whether
the buffer was `*const u8` or `*const c_void`.

**And the same thing again, three times, for L2CAP.** `Closed` and
`ChannelSink` were declared identically in `webbluetooth-apple`,
`webbluetooth-linux` and `webbluetooth-android`, and the machinery around them
— bounded backlog, async stream, close-once notification — was a fourth copy in
`webbluetooth`. Opening a channel is genuinely platform work; nothing else
about one is. So a platform crate now implements
`webbluetooth_core::l2cap::PlatformChannel` on whatever socket it opened, and
`L2capChannel` is generic over that. No dynamic dispatch — one channel type per
build — and `webbluetooth`'s own `l2cap.rs` went from 500 lines to 74.

That one was load-bearing beyond tidiness: the peripheral adapters could not
leave `webbluetooth` while `Request::ChannelOpened` carried a type assembled
above them. They are in their platform crates now.

**Four peripheral adapters carried a validator each, and three were the same.**
`validate_characteristic` on Linux, Android and Windows checked exactly what
the shared definition checked immediately before calling it. `validate_descriptor`
was the same two rules four times over, differing only in which stack the error
message blamed for managing the CCCD — they all manage it. What is genuinely
per-platform is one CoreBluetooth rule: a cached value must be read-only. That
one is applied where Apple publishes; the rest is in `webbluetooth-core`.

### What checks that the adapters agree

Only one backend compiles, so five of them are invisible to `cargo check`. A
trait was the obvious fix and does not work: three methods take
`self: &Arc<Self>` — `connect`, `request_device` and `set_radio_scanning` each
hand a weak reference to a platform callback that outlives the call — and
arbitrary self types are not stable in traits. A trait could only have them by
changing fourteen method definitions to suit the checking mechanism, and it
would still only ever check the backend already being compiled.

`webbluetooth-core/tests/backend_surface.rs` reads all six adapters as source
and compares the methods they declare — names, argument types, return types —
on whatever host it runs on:

```
Backend surface: 40 members across 6 backends
  apple      39 members     windows    38 members
  linux      39 members     wasm       39 members
  linux-hci  39 members     android    39 members
```

The two gaps are declared, with reasons the test requires: Windows and the web
have no `l2cap_target`, because neither WinRT nor the browser exposes an L2CAP
channel API, and only the browser has `watch_advertisements`, because the
native backends watch the radio and filter rather than watching one device. An
exception that stops applying fails the build, so the list cannot rot.

Renaming a method in five backends and missing the sixth now fails on a Mac.
So does changing one's return type:

```
write_descriptor is declared 2 ways:
  [android] async write_descriptor(self, &str, &Handle, &Vec<u8>) -> Result<()>
  [apple, linux, linux-hci, windows, wasm] async write_descriptor(self, &str, &Handle, &[u8]) -> Result<()>
```

Both Linux adapters — BlueZ and the raw-socket one behind `linux-hci` — are
compiled on every Linux build now, rather than only the one the feature
selects. Two adapters built one at a time are two adapters that drift, and the
`linux-hci` one is the one almost nobody builds.

`peripheral_surface.rs` does the same for the four peripheral adapters, and
writing it found three divergences that had been shipping:

* `PublishedCharacteristic::try_notify_centrals` exists on Apple, Android and
  Windows and **not on Linux** — so portable code that calls it does not
  compile there. That one is real and stays: BlueZ's GATT server has no
  per-central addressing, because a value reaches subscribers by emitting
  `PropertiesChanged` on the characteristic's own D-Bus object and every
  subscriber is watching it. It is recorded as an exception with that reason
  rather than papered over by notifying everybody.
* `PublishedService::characteristics` returned `&[PublishedCharacteristic]` on
  three platforms and `Vec<PublishedCharacteristic>` on Linux;
  `characteristic()` likewise returned a borrow or an owned value depending on
  where you built. Linux cannot lend — its `PublishedService` holds
  `Arc<CharacteristicState>` and pairs each with the manager on the way out —
  so the three that could lend now return owned values too. Cloning one is two
  refcount bumps.

### Why the Objective-C runtime and not the Swift one

`swiftui-native` reaches SwiftUI by resolving Swift-mangled symbols and
synthesising Swift type metadata, because SwiftUI is Swift-native. CoreBluetooth
is not:

| Framework | Swift-mangled symbols exported | Ships a `.swiftinterface`? |
|---|---:|---|
| SwiftUI | 19,561 | yes |
| CoreBluetooth | **0** | **no** |

`CoreBluetooth.framework` contains a `.tbd` listing Objective-C classes and
Objective-C `Headers`. No BLE framework in either SDK ships a Swift module.
`import CoreBluetooth` in Swift is the Clang importer synthesising Swift-shaped
declarations from Objective-C headers *at compile time*; the calls still lower
to `objc_msgSend`. A Swift bridge would add a `swiftc` step without removing a
single Objective-C call.

So the runtime that types are built in here is the Objective-C one, the same
way `swiftui-native`'s own `objc.rs` reaches AppKit and UIKit. The principle
is unchanged: nothing is compiled ahead of time, no source file in another
language is shipped, and the delegate class is produced at run time —
`objc_allocateClassPair`, Rust `extern "C"` functions installed as method
implementations, `class_addProtocol` for `CBCentralManagerDelegate` and
`CBPeripheralDelegate`, `objc_registerClassPair`.

Callbacks are delivered on a private `dispatch_queue`, not the main queue, so
this works in a plain `fn main()` with no `NSRunLoop` to spin.

### Correlating replies

CoreBluetooth's delegate methods carry no request identifier.
`readValueForCharacteristic:` returns nothing and, later,
`peripheral:didUpdateValueForCharacteristic:error:` arrives — through the *same*
callback a notification uses. Replies are matched to requests by the object they
concern: one FIFO queue of waiters per `(peripheral, attribute)` pair. **A value
that arrives with no queued reader is a notification** — which is exactly the
distinction the spec draws. GATT operations are also serialised per device with
an async mutex, as the spec's per-device operation queue requires.

A disconnect fails every request outstanding against that peripheral, so a
dropped link never leaves a future hanging or the device's lock held.

### Invalidation

`CBService` and `CBCharacteristic` objects do not survive a disconnect, and a
peripheral can replace its attribute table by re-advertising. Each device
carries a generation counter, bumped on both events; handles remember the
generation they were minted in and fail with `InvalidStateError` once it moves.
Without that, a stale handle is a use-after-free.

## Runtime

Runtime-agnostic. Built on `futures-channel` / `futures-util`, no tokio
dependency — works on any executor, including `futures_executor::block_on`.
Timers run on one background thread with a deadline heap.

## Platforms

Apple, Linux, Android and Windows, from one API. Apple is verified by
`./scripts/check.sh apple` (twelve targets, `--all-targets --all-features`);
Linux by `./scripts/check.sh linux` in Docker.

### Linux

Two backends, chosen by feature flag:

| | **BlueZ** (default) | **`linux-hci`** |
|---|---|---|
| needs `bluetoothd` | yes | **no** |
| transport | D-Bus to `org.bluez` | ATT over an L2CAP socket |
| scanning | `StartDiscovery` | raw HCI socket |
| privileges | D-Bus policy (`bluetooth` group) | none for GATT, `CAP_NET_RAW` to scan |
| pairing / bonding | `Device1.Pair` | **absent** — no Security Manager without a daemon |
| dependencies | none | none |

The default is BlueZ: it is the supported path, and it owns the Security
Manager, so encrypted characteristics work. Reach for `linux-hci` where no
daemon runs — containers, embedded images, minimal distributions — accepting
that anything needing pairing is out of reach.

```sh
cargo build                              # BlueZ
cargo build --features linux-hci         # direct to the controller
```

Neither has any dependencies. **D-Bus is spoken directly** — the wire protocol
over a Unix socket, SASL handshake and all — rather than binding `libdbus`, and
ATT and HCI are kernel sockets.

### Windows

Central and L2CAP— no, central and peripheral. **L2CAP does not exist on
Windows**: neither WinRT nor Win32 exposes an LE connection-oriented channel, so
`webbluetooth::l2cap` is absent there and `webbluetooth::L2CAP` is `false`.

WinRT is COM, and a COM object is a pointer to a table of function pointers — so
this needs no bindings crate, only the right interface identifiers and vtable
layouts. Both are **generated from Windows metadata** by
`scripts/update.sh winrt-iids`, which reads Microsoft's own generated bindings:
648 constants, including the slot index of every method the backend calls. A
slot off by one is a jump into a different function, and there is no runtime
here that would catch it, so none of them is counted by hand.

Parameterised interfaces — `IAsyncOperation<T>`, `TypedEventHandler<A, B>` —
have no IID in metadata; WinRT derives one by hashing a *signature string*. That
algorithm is implemented and pinned against a published UUID-5 vector, and the
signature strings are pinned against the exact byte sequences Microsoft's
bindings build.

Callbacks work as everywhere else: Windows wants an object it can call into, so
one is built — here a `#[repr(C)]` struct whose first field is a vtable, with an
atomic refcount and a Rust closure behind it.

### Raspberry Pi

Linux with BlueZ, so the Linux backend covers it unchanged — there is no
separate Pi build. What differs is the architecture: `aarch64-unknown-linux-gnu`
for 64-bit Pi OS, `armv7-unknown-linux-gnueabihf` for 32-bit, and
`arm-unknown-linux-gnueabihf` for the ARMv6 Zero and Pi 1. All are checked by
`./scripts/check.sh pi`, along with `musl` and `linux-hci` on ARM.

The one Pi-specific thing worth knowing: its controller is on a **UART**, not
USB, so it can go missing for reasons unique to the board —
`dtoverlay=disable-bt`, a stopped `hciuart` service, or the serial console
stealing the UART on a Pi 3 or Zero W. `doctor` reads `/proc/device-tree/model`
and explains those only when it is actually on a Pi. See
[PLATFORMS.md](PLATFORMS.md).

### Android

Central, peripheral and L2CAP, API 26+. Hand a `Context` over once, then use
the same API as everywhere else:

```rust
// From JNI_OnLoad, or an Activity.
webbluetooth::android::init(vm, context)?;
```

A null `context` falls back to `ActivityThread.currentApplication()`. Needs
`BLUETOOTH_SCAN` and `BLUETOOTH_CONNECT` on API 31+, `ACCESS_FINE_LOCATION` for
scanning on 23–30.

**Android's BLE callbacks are abstract classes, not interfaces**, so
`java.lang.reflect.Proxy` cannot help — something has to *subclass* them, and on
a device there is no compiler. This crate does what the Apple backend does with
`objc_allocateClassPair`: builds the class at run time. It emits a DEX defining
the subclass, loads it with `InMemoryDexClassLoader`, and binds the methods with
`RegisterNatives`.

That is far less work than it sounds, because a method declared `native` has no
bytecode at all — the writer emits constant tables, not instructions. The one
exception is the constructor, which must call `super()`, and that is four code
units written by hand. No Java is shipped, compiled, or required at build time.

Four classes are generated: the GATT client and scan callbacks, and for the
peripheral role the GATT server and advertising ones.

Two Android behaviours are smoothed over so the peripheral API matches Apple's:

* **Android does not create the Client Characteristic Configuration
  descriptor.** CoreBluetooth adds one to any characteristic that can notify;
  Android publishes exactly what it is given, so a notify characteristic
  without a CCCD is one nobody can subscribe to. The engine adds it.
* **Android does not tell you who is subscribed.** There is no
  `subscribedCentrals` — a subscription *is* a write to that descriptor, so the
  engine watches for it and keeps the list, surfacing it as
  `Request::Subscribed` rather than a raw write.

Android is also more permissive about what it will publish — a characteristic
may carry a value *and* be writable, and any descriptor UUID is accepted — so
validation lives in each engine rather than being shared.

### Apple

| Platform | Central | Peripheral | L2CAP | Restoration |
|---|:--:|:--:|:--:|:--:|
| macOS (arm64, x86_64) | ✅ | ✅ | ✅ | ⚠️ inert |
| iOS | ✅ | ✅ | ✅ | ✅ |
| Mac Catalyst | ✅ | ✅ | ✅ | ⚠️ inert |
| tvOS | ✅ | ❌ | ✅ | ⚠️ inert |
| watchOS | ✅ | ❌ | ✅ | ⚠️ inert |
| visionOS | ✅ | ❌ | ✅ | ⚠️ inert |
| simulators | ⚠️ no radio | | | |
| **Linux** (incl. Raspberry Pi) | ✅ | ✅ BlueZ only | ✅ central only | ❌ n/a |
| **Android** (API 26+) | ✅ | ✅ | ✅ (API 29+) | ❌ n/a |
| **Windows** (10 1709+) | ✅ | ✅ | ❌ none exists | ❌ n/a |

**The peripheral role is macOS and iOS only**, and that is CoreBluetooth's
constraint, not this crate's: `CBPeripheralManager` and the mutable-attribute
initialisers are annotated `API_UNAVAILABLE(watchos, tvos)` *and*
`API_UNAVAILABLE(visionos)`. The classes exist in every SDK's `.tbd`, so a
runtime check would not catch it — `build.rs` sets a `peripheral_role` cfg
instead, and the module is absent where the role is. Branch on
`webbluetooth::PERIPHERAL_ROLE`.

tvOS, watchOS and visionOS are Rust tier-3 targets and need nightly plus
`-Z build-std`; the script handles that. See [PLATFORMS.md](PLATFORMS.md) for
the full matrix, permissions per platform, and why restoration only does
anything on iOS.

## Both roles at once

`CBCentralManager` and `CBPeripheralManager` are independent objects, so one
process can scan and advertise simultaneously. Each gets its own
runtime-synthesised delegate class, its own delegate instance, its own sink
registry keyed by object address, and its own private dispatch queue — they
never contend. `tests/dual_role.rs` runs three of each in one process and checks
every delegate still delivers.

```rust
let bluetooth = Bluetooth::new();            // central
let (peripheral, mut requests) = Peripheral::new();  // peripheral

futures_util::future::join(scanning, serving).await;
```

**A radio cannot hear itself.** A device never discovers its own
advertisement, and two processes on the same Mac share one controller, so
neither can see the other. Loopback testing needs a second machine or a phone.

## L2CAP channels

The one place CoreBluetooth stops being request/response: a bidirectional byte
pipe with no 512-byte attribute ceiling and no ATT round trip per write. Not
part of Web Bluetooth. Available in both roles, on every platform.

```rust
// Central side
let channel = device.open_l2cap_channel(0x0080).await?;
let mut incoming = channel.take_incoming().unwrap();
channel.send(b"hello")?;
while let Some(chunk) = incoming.next().await { /* … */ }

// Peripheral side
let psm = peripheral.publish_l2cap_channel(false).await?;
// …a central connecting arrives as Request::ChannelOpened(channel)
```

`CBL2CAPChannel` hands back an `NSInputStream`/`NSOutputStream` pair, which is
runloop-driven — its async mode wants a scheduled runloop and a delegate, its
sync mode blocks. Each channel therefore gets one **pump thread** that owns both
streams, runs a `CFRunLoop`, and is the only place any stream method is called.
Writes from other threads queue and then `CFRunLoopWakeUp` the pump, so a send
does not wait for a timeout. **A chunk is not a message** — L2CAP CoC is a byte
stream; frame it yourself.

## State restoration

```rust
let bluetooth = Bluetooth::with_restoration(
    Restoration::new("com.example.app.central").allow_service(services::HEART_RATE)?,
    FirstMatch::default(),
);

if let Some(restored) = bluetooth.restored_session().await {
    for device in restored.devices { /* already connected */ }
}
```

Two things worth knowing. **A Web Bluetooth grant does not survive process
death**, and the system does not preserve which services a user allowed — so a
restored session's allowlist is what you declare on `Restoration`, up front.
And **only iOS relaunches a terminated process** for Bluetooth work, with the
right background mode; elsewhere the identifier is accepted and nothing ever
calls back.

## Status

**0.0.1 — the first release.**

Six central-role backends and four peripheral ones, each presenting the same
surface. Two tests check that by reading all ten as source, because only one is
ever compiled and the other nine are otherwise whatever they were last time
somebody targeted them.

What has run **against a real radio**: the Apple backend, in both roles at
once, between a Mac and an iPad — scan, advertisement parsing, connect,
discovery, read, write-with-response, notify, the GATT blocklist, disconnect,
**and an L2CAP channel**, each line confirmed from both ends:

```
  ✓ read                   level 81
  ✓ notify                 [80, 79]
  ✓ write                  control point accepted 0x2a
  ✓ blocklist              serial number refused
  ✓ included               none declared by this peer
  ✓ l2cap                  psm 0x00c0, 12 bytes echoed
```

The iPad's side of that last line reads `l2cap opened by B42410D9…` and
`l2cap echo 12 bytes`. A PSM is not discoverable — nothing in GATT advertises
one — so the harness publishes its number in a characteristic and the central
reads it before opening the channel, which is how a real peer does it too.

What has run **against an emulated or mocked one**: Linux, in a QEMU VM with
virtual controllers and a real `bluetoothd`, and in Docker against a mocked
BlueZ serving the real D-Bus interfaces; Android, with the generated classes
loaded into real ART and an APK on an emulator; the browser and Node, loaded
and called through.

What has **not** been exercised over the air: **Windows**. There is no radio in
any of its checks. The COM layer is tested through its own hand-built vtables
and the IIDs against Microsoft's metadata, which is not the same as having
talked to a device. That is a lack of a test rig rather than a known problem,
which is the distinction worth stating before somebody depends on it.

## Releasing

```sh
./scripts/release.sh            # package and verify all eight crates
./scripts/release.sh --publish  # upload, in dependency order
```

The dry run is the default because a version on crates.io can be yanked but
never reused.

## License

MIT — Copyright © 2025 [Eugene Hauptmann](https://github.com/eugenehp)

The full text is in [`LICENSE`](LICENSE). The SPDX identifier is `MIT`.
