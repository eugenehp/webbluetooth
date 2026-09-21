# Changelog

## 0.0.1 — unreleased

First release.

### The API

The Web Bluetooth specification's central role, in full: every one of the 106
members its IDL declares is either implemented or excluded with a recorded
reason, and a test fails when the standard gains a member. The 25 exclusions
are things that cannot exist outside a browser — the Permissions API, DOM event
constructors, a page's referrer — plus `writeValue`, which the specification
deprecates in favour of the two explicit forms.

Beyond the specification, because the platforms offer them and a page cannot
have them: the peripheral role (`CBPeripheralManager` and its equivalents),
L2CAP connection-oriented channels, connection parameters and PHY, adapter
selection, and iOS state preservation and restoration.

### Getting started

`webbluetooth::prelude` re-exports the traits and types a program needs in
scope, `StreamExt` above all: every event source here is a `Stream`, and
without that trait imported the first example in the README did not compile
without a `futures-util` dependency it never mentioned.

`Error` and `Result` are deliberately left out of it — a glob-imported `Result`
alias shadows `std::result::Result`.

### Common traits

`Availability` implements `Display` — its messages already existed, written
inside `Error`'s `Display` and reachable nowhere else, so a caller printing the
result of `availability()` got `{:?}` or nothing.

`RemoteGattServer`, `Notifications` and `DisconnectEvents` implement `Debug`,
which the handles either side of them already did. `tests/common_traits.rs`
pins the whole set, because the absence of a common trait is only ever visible
from outside the crate.

`Availability` implements `std::error::Error`, and converts into `Error` as
`NotAvailable`. It stays a separate type — an unusable radio is a state, not a
failed call — but a caller reporting one beside its other failures no longer
has to translate it by hand first, which was a `map_err` at every use of
`availability()`.

### Info.plist is embedded by a build script

macOS will not give a process Bluetooth without
`NSBluetoothAlwaysUsageDescription` in its `__TEXT,__info_plist` section, and a
bare CLI binary has no bundle to carry it — so it is linked in. That was a
`rustflags` entry in `.cargo/config.toml` naming `Info.plist`, and a cargo
config can only name a path relative to the directory cargo was invoked from.
It is also *discovered* by walking up from that same directory. Two
consequences, both of which had to be found rather than noticed:

* `cargo build --manifest-path …/webbluetooth/Cargo.toml` run from anywhere
  else — an IDE, a wrapper script, a parent workspace — never finds the config,
  so the flag never applies. The build succeeds and the binary has no section
  in it. There is no error: the program reports `Unauthorized`, is never
  prompted, and sees no adapter, with nothing connecting that back to how it
  was built. Confirmed with `otool -s __TEXT __info_plist`, which found no
  section at all on a binary built that way.
* `cargo package` verifies each archive by compiling it in
  `target/…/package/<crate>-<version>/`, a different directory again, so the
  link failed outright — on `webbluetooth-apple`'s *build script*, before a
  line of the crate was compiled. Packaging on a Mac could not pass.

`crates/webbluetooth/build.rs` now emits the link argument with an absolute
`CARGO_MANIFEST_DIR` path, and `Info.plist` sits beside it. A build script's
directives belong to the package rather than to the directory someone happened
to be standing in, and `rustc-link-arg` does not reach anyone's build script,
so both failure modes go together. `.cargo/config.toml` keeps a comment
explaining why the rustflag is not there, since the mistake invites itself
back.

The README was handing readers the `.cargo/config.toml` recipe to copy into
their own projects — the same silent failure, one repository at a time. It now
gives the build script.

### The prelude, and writing a chooser

`DeviceChooser` is the crate's central extension point — its own documentation
says to implement one for anything user-facing — and `choose` returns a
`BoxFuture`, which was named in the trait's signature and exported nowhere. So
implementing it meant adding `futures-util` to your own manifest. Both chooser
examples in this repository do exactly that, which is how it survived: from
inside the workspace the dependency is already there. It is the same gap the
prelude was built to close for `StreamExt`, one level down.

`BoxFuture` is re-exported from `chooser`, and the prelude gained it along with
`Candidate`, `Candidates`, `BluetoothUuid`, `Grant` and `LeScanOptions` — the
other types a caller has to name that it did not carry.

`tests/prelude_is_sufficient.rs` writes a chooser, a connect-and-notify loop
and a scan-and-adopt loop from `prelude::*` and nothing else, so the claim is
checked rather than asserted. A claim about what a caller needs in scope can
only be tested from a caller's position.

### A migration guide, from an actual migration

`MIGRATING.md` maps btleplug's calls onto these — `peripheral.subscribe` to
`start_notifications`, the `CentralEvent::DeviceDisconnected` filter to
`watch_disconnect`, the `adapter_state()` poll for `PoweredOn` to
`availability()`, and so on. Every row is a call a real port had to replace:
muse-rs, an EEG client that streams from eight characteristics at once, moved
across, and the table is what that took rather than what reading two API docs
side by side suggests.

Two things are genuinely different rather than renamed, and the guide leads
with both: a program must name the services it intends to reach, or
`get_primary_service` is a `SecurityError`; and notifications are per
characteristic rather than one stream per peripheral, so the dispatch loop a
btleplug program already has is recovered with `tagged()` and `select_all`.

Its code blocks were compiled verbatim from outside the workspace before being
written down, which is the only way a guide's claims are worth anything.

### Reading several characteristics at once

The case a real sensor is: a Muse headset streams EEG on four characteristics
and optical on three, simultaneously, and a program wants one loop over all of
them that can still tell them apart.

`Notifications::tagged()` returns `Tagged`, a named type rather than
`impl Stream`. That distinction is the whole point. Merging is what takes a
stream by value — `select_all` owns what it is given and hands back only what
the item type carries — so an opaque return type would have taken `lost()`
with it, and the merged set could have reported that notifications were
dropped but not *which characteristic* dropped them. Which is where the bare
`Vec<u8>` stream started. `SelectAll::iter()` reaches `Tagged`, so a merged
set stays diagnosable.

`examples/sensors.rs` is that program: it takes service UUIDs as arguments —
Web Bluetooth grants access to the services a request names, so there is no
"everything" to ask for — subscribes to every characteristic in them that will
notify or indicate, merges them, and prints a per-subscription received/lost
table at the end.

### Streams and futures are reachable from here

`webbluetooth::stream` re-exports `Stream`, `StreamExt`, `select_all`,
`SelectAll` and `BoxStream`; `webbluetooth::future` re-exports `select`,
`join`, `Either` and `BoxFuture`.

Every event source in this crate is a stream, and "stop when the device
disconnects" — `select` over a notification stream and `watch_disconnect` — is
the correct shape for almost every read loop here. Both of those require
combinators that lived only in `futures-util`, so using two things this crate
handed you meant taking a dependency of your own. Four examples in this
repository did exactly that, and three doc comments told readers to; all seven
now use the crate's own re-exports, which is also the check that they are
sufficient.

### One session per process

`Bluetooth::shared()` — the analogue of `navigator.bluetooth`, which a page
cannot ask for a second of. The first call opens an adapter and every call
after it returns a clone of the same one.

`new()` opens a *separate* session each time: its own grants, its own
connections. Since a `BluetoothDevice` carries the session it was found
through, two of them in one program is a bug that presents as a device which
simply will not connect — and the workaround a caller writes is a global, so
the crate may as well offer one.

This also closes the last member that was excluded as unimplementable rather
than as browser-only: `Navigator.bluetooth` now maps to `Bluetooth::shared`,
and the surface is 81 implemented, 25 excluded.

### The `uuid` crate

Optional feature `uuid`, off by default: `From` in both directions, `IntoUuid`,
and `as_uuid`. `BluetoothUuid` stays its own type — the specification defines
`BluetoothUUID` as canonical text and resolves `"battery_service"` through it,
neither of which a general-purpose UUID type does — but the rest of the Rust
Bluetooth world speaks `uuid::Uuid`, so a program arriving from `btleplug`
holds a module of `Uuid` constants and had to write a conversion helper on its
way in. Now they go straight to `get_primary_service`.

### Grants

**`request_device_with` now records the grant.** `persist_grants` was called
from `request_device` and `adopt_device` and not from the third path, so an
application with its own chooser — which is to say any application with a user
interface — granted devices that never reached the store. Nothing failed; the
next run simply went back through the chooser with nothing to explain why. The
routine moved to `Session`, where the grants actually live, and every path that
changes one calls it. `forget()` used to restate the same twenty lines and now
calls it too.

**A grant is a type.** `Grant` is re-exported and has a builder — `Grant::new()
.service(…)`, `.manufacturer_data(…)`, `.validate()` — and `adopt_device` takes
`impl Into<Grant>` rather than a `RequestDeviceOptions` whose filters it
documented itself as ignoring. `RequestDeviceOptions` still converts, so the
old spelling compiles. `remembered_grant` returns the `Grant` rather than a
`Vec` of its services, which dropped the manufacturer-data half on the floor,
and `grants::Stored.grant` is finally a type a caller can name.

**`adopt_candidate`** turns a sighting from `request_le_scan` into a device
without copying its identifier out by hand. Outside the standard, like
`adopt_device`, and with the same caveat: adopting re-resolves through the
platform, which can fail for something the scan reported a moment ago.

### Notifications carry their characteristic

`start_notifications()` returned a stream of `Vec<u8>` and nothing else, which
is less than the specification delivers: a `characteristicvaluechanged` event
carries its `target`, so JavaScript can always ask which characteristic a value
came from. `Notifications` now holds the `RemoteGattCharacteristic` it was made
from and exposes `characteristic()`, `uuid()` and `tagged()`.

This is only invisible on a device with one interesting characteristic. Merge
several subscriptions — which is what any sensor with more than one stream
requires — and the values no longer say what they are; worse, `lost()` is per
subscription, so a gap could not be attributed to a characteristic either.
`tagged()` pairs each value with its UUID, which is what makes `select_all`
over several subscriptions readable.

### Scanning

`LeScanOptions` is its own dictionary, as `BluetoothLEScanOptions` is in the
scanning draft: `filter`, `filters`, `keep_repeated_devices`,
`accept_all_advertisements`. It used to wrap a `RequestDeviceOptions`, which
meant a filtered scan was written by handing it a grant — and `optionalServices`
and the exclusion filters then did nothing at all, silently.

`LeScan` gained `filters()` and `accept_all_advertisements()`, so all five
members of `BluetoothLEScan` are now present rather than three.

`tests/le_scan_surface.rs` is why that gap is now visible: `requestLEScan` is
defined in a separate draft that `scripts/update.sh surface` does not fetch, so
the interface was implemented against nothing. The test vendors the IDL in its
own header and checks each member, plus the argument rules the algorithm
applies — neither filters nor `acceptAllAdvertisements`, or both at once, are
refused before the radio is touched.

### Platforms

| | central | peripheral | L2CAP |
|---|:--:|:--:|:--:|
| macOS, iOS, Mac Catalyst | ✅ | ✅ | ✅ |
| tvOS, watchOS, visionOS | ✅ | — | ✅ |
| Linux (BlueZ) | ✅ | ✅ | ✅ |
| Linux (`linux-hci`, no daemon) | ✅ | via BlueZ | ✅ |
| Android (API 26+) | ✅ | ✅ | ✅ (29+) |
| Windows 10 1709+ | ✅ | ✅ | — |
| Browser (WebAssembly) | ✅ | — | — |
| Node.js / Deno | ✅ | — | — |

No Swift, no Objective-C, no Java, no `libdbus`, no bindgen, no generated
projection. Each platform is reached through its own C ABI, and the classes
some of them demand — CoreBluetooth delegates, Android callback subclasses —
are built at run time.

### What has been run against a real radio

The Apple backend, in both roles at once, between a Mac and an iPad: scan,
advertisement parsing, connect, discovery, read, write-with-response, notify,
the GATT blocklist, disconnect, and an L2CAP channel carrying bytes both ways.
Each is confirmed from both ends rather than from the side that asked.

Windows has no radio in any of its checks, and says so.

### Crates

* `webbluetooth` — the API.
* `webbluetooth-core` — the portable model the backends share, and the
  vendored copies of the specification's IDL, MDN's compatibility data and the
  Web Bluetooth CG's two blocklists.
* `webbluetooth-apple`, `-linux`, `-windows`, `-android`, `-wasm` — each
  platform's FFI and the adapter written against it.
* `webbluetooth-node` — the Node.js and Deno addon.

Depend on `webbluetooth`. The rest are public because they have to name each
other's types.
