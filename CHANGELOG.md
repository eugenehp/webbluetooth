# Changelog

## 0.0.4 — 2026-10-15

RFCOMM on every native platform, and the Linux half of it working for the first
time. Most of this was found by porting a second consumer off `btleplug` —
[mw75](https://github.com/eugenehp/mw75), a headset that is activated over LE
and then streams EEG over RFCOMM — and then running what it needed, rather than
compiling it: in Docker against a mocked BlueZ, in a QEMU VM between two
emulated controllers, on macOS against a mocked IOBluetooth, and finally over
the air between a Mac and a Linux machine.

### Upgrading from 0.0.3

Three changes in behaviour, each a fix, but each something a caller could have
been relying on:

* **A notification stream now ends when its device disconnects**, after
  delivering what was already queued. It used to stay open and silent. A loop
  that reconnects inside `while let Some(..) = notifications.next()` has to
  subscribe again after the reconnect, which it always needed to — the old
  subscription could never produce another value.
* **On Linux, `ClassicSecurity::None` now asks for `BT_SECURITY_LOW`** rather
  than `BT_SECURITY_SDP`. The old level could not open a channel to an SSP peer
  on a cold link at all; see below.
* **`rfcomm` now links IOBluetooth on macOS**, where it used to compile to
  nothing. A macOS build that enables `rfcomm` without wanting Classic should
  stop enabling it there.

### RFCOMM on macOS

The `rfcomm` feature now covers macOS, as it does Linux, Android and Windows:
`Bluetooth::open_rfcomm_channel(address, channel)` and
`BluetoothDevice::open_rfcomm_channel(channel)` exist on a Mac, and
`RfcommChannel` is the same type callers hold everywhere else. Until now
`rfcomm` compiled to nothing there.

Underneath, `apple-classic` — previously a reserved name — is the IOBluetooth
backend, and `rfcomm` turns it on. On its own it provides
`Bluetooth::paired_classic_devices()`. In `webbluetooth-apple` it is split the
way the other platform crates are: a `classic` module that finds devices, and
an `rfcomm` module, behind an `rfcomm` feature, that opens channels on them.

Written the way the rest of `webbluetooth-apple` is — a delegate class
synthesised at run time, no bindings crate — with one pump thread per channel
that opens it and does every write, so a blocking write never stalls the
caller. CoreBluetooth and IOBluetooth share no identifier, so a
dual-mode device is bridged by name; see PLATFORMS.md for that and for the
entitlement macOS requires. The `rfcomm_macos` example lists paired devices
and opens a channel on one.

**The main thread's run loop has to be turning.** IOBluetooth delivers every
RFCOMM callback there, whichever thread opened the channel — found by running
against a real Linux peer, where a program doing its work on the main thread
opened the channel, sent everything, and never received a byte. An app has a
turning main run loop already; a command-line program has to provide one, as
the `rfcomm_macos` example now does. Without it the open fails with an error
that names the cause, instead of returning a channel that cannot receive.

Verified against real hardware: a signed bundle on a Mac to an Ubuntu machine
running this crate's Linux RFCOMM listener, ten echoed rounds per run, five
runs out of five.

Also tested against a mock of the far end: `rfcomm_mock.rs`
synthesises an `IOBluetoothDevice` and an `IOBluetoothRFCOMMChannel` the way the
delegate is synthesised, and they behave as IOBluetooth does — the open completes
later, from a run loop, and data arrives through the
real delegate. Everything from the device object inward is the shipping code:
the pump thread, its run loop, delegate dispatch, MTU-sized writes, local and
remote close, an open refused, failed, timed out or abandoned. The tests were
checked by breaking the pump six different ways; each break fails a test rather
than passing or hanging.
The mock also found a race of its own: a peer that opens, sends and hangs
up within one run-loop turn had its close seen before its open, so the open
failed and the data was lost. The open is now checked first, and a test
delivers all three at once.

### Linux RFCOMM could not open a connection

Found by running it, for the first time, between the two emulated controllers
of the QEMU VM (`scripts/qemu/vm.sh`). Three faults, each hiding the next:

* **`SockAddrRc` was `#[repr(C, packed)]`**, 9 bytes. The kernel's
  `struct sockaddr_rc` is not packed and is 10, and `rfcomm_sock_connect`
  refuses a shorter length with `EINVAL` — so every outbound RFCOMM connection
  failed before reaching the radio. `bind` tolerates a short address, which is
  why the listener worked and nothing looked wrong from that side. A test now
  pins the size and offsets.
* **`ClassicSecurity::None` asked for `BT_SECURITY_SDP`.** That level is meant
  for the SDP channel alone; on RFCOMM or a Classic L2CAP PSM it asks for a link
  with no SSP pairing at all, which an SSP peer refuses — `ECONNREFUSED` on a
  cold link, and success only when an earlier connection had left an
  authenticated link up, which is what made it look intermittent. It now maps
  to `BT_SECURITY_LOW`, the kernel's own default for these sockets.
* **A peer closing the channel read as an error.** Linux reports an orderly
  RFCOMM disconnect (the peer's DISC) to a reader as `ECONNRESET`, not end of
  file, so `on_close()` said `Error(..)` where it should say `ByPeer`.

Verified in the VM: an RFCOMM echo through `listen_rfcomm` and
`BluetoothDevice::open_rfcomm_channel`, on a cold link, three runs out of three.

### A notification stream did not end when its device disconnected

`start_notifications()` documents that "the stream ends when the device
disconnects". It did not, on any backend. Each one keeps its subscribers in a
map keyed by characteristic, and none removes them when the link drops — the
Apple backend's disconnect handler carried the comment "a notification stream
on a dead characteristic should end, not hang" above a line that only pruned
senders whose receiver had *already* been dropped. So a `while let Some(value)
= notifications.next().await` loop simply stopped producing values and waited
forever, on a characteristic that could never notify again.

The fix is in the facade, once, rather than in six backends: `Notifications`
now watches its device for a disconnect and ends when one arrives, after
delivering whatever was queued before it.

### Linux channels: `closed()` said nothing after the peer closed

RFCOMM, Classic L2CAP and LE L2CAP channels on Linux each read on a thread
that learned why the channel ended and told `on_close()`, but never recorded it
where `closed()` looks — so after the peer hung up, `on_close()` said `ByPeer`
and `closed()` said `None`. The reason is now shared between the two. Found on
the Linux end of the Mac-to-Ubuntu RFCOMM run; a test pins it with a Unix
socket pair standing in for the radio.

### `RemoteGattServer::disconnect_and_wait()`

`disconnect()` returns at once, as the specification's does, and the link goes
down some time later. A caller whose next step needs it down — here, a headset
that refuses its Classic link while the LE one is up — had to poll
`connected()`. This waits for the platform to report it.

### Two helpers every port wrote by hand

`RemoteGattServer::get_characteristics()` is every characteristic of every
service the grant reaches — the flat set `peripheral.characteristics()` was.
`LeScan::collect_for(duration)` scans for a while and returns one entry per
device, built on the new `Candidate::merge`, which folds a later sighting into
an earlier one so a name from a scan response ends up beside the services from
the advertisement.

### `linux-hci`: a scan before a connect broke the connect

Stopping a scan only set a flag. The scan thread was blocked in `read` on the
HCI socket and saw the flag at the next advertisement — with the controller
still scanning in the meantime — and a scan started in that window spawned a
second thread, because the first had already been forgotten. A connection made
next then had Scan Enable (off) land in the middle of it, at whatever moment the
stale threads woke, and failed with `ECONNREFUSED` or `EINPROGRESS`. Any
`request_le_scan` followed by a connect hit it; a lone `request_device` usually
did not, which is why the round-trip test never had.

The scan socket now has a receive timeout, so the thread checks its flag every
100 ms, and stopping joins it: when `set_radio_scanning(false)` returns, the
controller is no longer scanning. The `timeval` is two C `long`s, so a 32-bit Pi
gets an 8-byte one rather than a 16-byte one it would read as "no timeout".

Verified in the QEMU VM, interleaving the two builds in one boot: the fixed code
connected after a scan ten times out of ten, the original two out of ten. Not
quite every failure is gone — a connect made within seconds of another client
dropping its link to the same peer was still refused once in about twenty runs.

### `linux-hci`: a link `bluetoothd` already holds now says so

Linux gives an LE link one ATT channel. When `bluetoothd` brings a link up — as
it does by itself for a bonded device on its auto-connect list, the moment the
device advertises — it takes that channel, and this backend's connect then
fails with a bare `errno 16`. Before the link exists the same situation shows
up differently, and worse: the daemon attaches to the link this backend just
made, and notifications stop arriving although the subscription succeeded.

There is no sharing the channel, so this is not fixable from here; the message
now says what is holding it and what to do (`bluetoothctl disconnect`, take the
device off auto-connect, or stop the daemon). Reproduced in the VM by bonding
the two controllers and putting the peer on auto-connect.

The duplicated "could not connect to <address>:" prefix on every connect error
from this backend is gone as well.

### A dual-mode device bonded over BR/EDR now says why it will not connect

BlueZ's `Device1.Connect` chooses the bearer, and for a dual-mode device —
one whose LE side shares its BR/EDR address — it can choose BR/EDR, where there
is no GATT: always for a device bonded over BR/EDR only, such as headphones
paired for audio, and — seen between a real Ubuntu machine and a Mac, with no
pairing on either side — for a Mac advertising with its public address, which
fails with `br-connection-key-missing`. The error was
`br-connection-profile-unavailable`, which names neither. It now explains the
cause and points at `Device1.PreferredBearer`, which recent BlueZ exposes under
`bluetoothd -E`.

Setting that property from the backend was tried and taken out again. In the
VM, BlueZ 5.83 bonded the LE side as well and chose LE unaided, so the case the
property is for could not be reproduced — and with the property set, a BR/EDR
connection appeared that the unmodified code did not make. Not shipped on that
evidence.

### Feature combinations that did not compile

`rfcomm` on its own failed to build on Linux and on Android, and so did most
combinations with `classic-l2cap` and `profiles`:

* Linux, `rfcomm` without `classic-l2cap`: `classic_profile.rs` matched a
  `Transport::ClassicL2cap` variant that only exists with `classic-l2cap`.
* Linux, `classic-l2cap` without `rfcomm`: the same module imported the RFCOMM
  channel unconditionally, and `register_classic_profile` — which registers an
  RFCOMM profile — was gated on `classic` rather than `rfcomm`.
* Linux, `profiles`: `open_profile_rfcomm` and `open_profile_l2cap` called a
  `query_sdp` the BlueZ backend does not have; they now use
  `webbluetooth_linux::sdp::query`, as `Bluetooth::query_sdp` already did.
* Android, `rfcomm`: `listen_rfcomm_service` returned the backend's future
  instead of awaiting it. `classic-l2cap` had the same fault in
  `listen_classic_l2cap`, and the facade's `classic_l2cap` module was compiled
  for Linux only, so the Android half of it did not exist.

* Linux, `le-audio` without `raw-acl` or `raw-sco`: the raw HCI socket that
  carries ISO packets was compiled only for the other two, so `open_raw_iso`
  named a type that did not exist. And `raw-acl` or `raw-sco` on its own failed
  `-D warnings` on the other's unused packet-type constant.

Every combination of `rfcomm`, `classic-l2cap` and `profiles` now passes clippy
for Linux, all four Android ABIs, Windows, macOS, iOS and wasm, and the Linux
ones pass their tests in the Docker image against the mocked BlueZ.

### Rust 1.99 warned about the Linux socket declarations

`webbluetooth-linux` declared libc's `read` and `write` with `u8` pointers.
That is ABI-identical to libc's `void *`, but newer compilers check
declarations of those two symbols against the ones the standard library links,
and flag the mismatch in every crate that builds this one. They are now
declared with libc's own signatures behind byte-pointer wrappers, so no call
site changed. Found by building mw75 against this on a machine with Rust 1.99.

## 0.0.3 — 2026-09-28

### Notifications never reached a subscriber in the browser

`webbluetooth-wasm` decoded every notification the page delivered and then
dropped it. A build for `wasm32-unknown-unknown` could scan, connect, discover
services and characteristics, write to them, and see `startNotifications()`
succeed — and never receive a single value. Nothing reported an error, because
nothing had gone wrong on any path that returns a result.

The two halves disagreed about what names a characteristic. `backend.rs`
registers a subscriber under the handle key it was handed — the shim's
`"<device>/svc/N/chr/N"` path — and caches the value under that key as well:

```rust
self.notifications.lock().unwrap().entry(handle.key.clone())   // subscribe
self.values.lock().unwrap().get(&handle.key)                   // cached_value
```

The shim's `characteristicvaluechanged` listener reported `event.target.uuid`
instead. Every `subscribers.get_mut(&characteristic)` therefore looked a UUID
up in a map keyed by paths and missed, on every notification; `cached_value`
missed for the same reason. The fix is the listener sending `key`, which was
already in scope one line above, where it is used to recover the device id.

A test now holds the two halves together — `the_shim_reports_a_notification_by
_handle_key`, alongside the op-code parity tests, which is the same class of
failure one level down: both sides compile, both sides run, and the numbers or
the names quietly do not line up.

Found by a consumer that streams EEG over Web Bluetooth in a page, where the
symptom was a connected headset and an empty chart.

## 0.0.2 — 2026-09-28

### Bluetooth Classic, and the transports a browser has no word for

The native extension surface now also has opt-in `classic`, `rfcomm`, and
`classic-l2cap` features. The shared Classic layer validates Bluetooth
addresses, Class of Device values, RFCOMM channels, Classic L2CAP PSMs, and
security levels. Linux provides RFCOMM and Classic L2CAP byte streams; these
are deliberately outside the `public` Web Bluetooth subset. Android RFCOMM
service-record connections are also available through `BluetoothSocket`; the
Android Classic L2CAP and server paths remain future work. Windows RFCOMM is
available through the opt-in feature using WinSock `AF_BTH`.

The native transport coverage has since expanded: Linux, Android, and Windows
provide RFCOMM client/server listeners; Linux, Android, and Windows provide
Classic L2CAP client/server listeners; Linux and Android expose bonded Classic
device enumeration and service UUID lookup; and Linux exposes raw ACL and
SCO/eSCO HCI sockets behind separate features. SDP records can be parsed into
service UUIDs, RFCOMM channels, and L2CAP PSMs without coupling the parser to a
particular operating system.

Profile transport convenience APIs are now aligned where native support exists:
Android and Windows can open RFCOMM connections by shared SIG profile UUIDs,
and Linux can register RFCOMM or Classic L2CAP profile endpoints through BlueZ
`ProfileManager1`. These APIs expose transport lifecycle only; profile-specific
negotiation and codecs remain separate implementations.

The per-crate checks now reach all of this. 0.0.1 had no `profiles` feature, so
`--all-features` never turned one on and the SDP service-record builders behind
`ProfileManager1` had never been compiled at all — nor had a channel-pinned
RFCOMM profile's accept path, which dropped every incoming connection, nor the
Classic L2CAP listener, which named a constructor that does not exist. None of
that ever shipped. It is recorded here because the check that catches it is the
part worth having.

### The parity oracle had been missing the scanning half

`./scripts/update.sh surface` fetched `w3c/webref`'s `ed/idl/web-bluetooth.idl`,
which upstream has since split in two and renamed — confusingly, the halves
landed as `bluetooth-scanning.idl` (the core API) and `bluetooth.idl` (the
scanning extension). The old path now 404s, and the refresh is what surfaced
that the vendored snapshot had only ever held one of the two documents.

So the surface goes from 106 members to **130**. The 24 that were absent are the
whole LE Scanning API — `requestLEScan`, `BluetoothLEScan`, the constructible
`BluetoothLEScanFilter`, `BluetoothDataFilter`, and the scanning permission
descriptors — and the 0.0.1 claim that "every one of the 106 members its IDL
declares" was accounted for was therefore measuring against an oracle that did
not declare all of them.

Nothing was actually missing from the library: 20 of the 24 map onto
`Bluetooth::request_le_scan`, `LeScan` and `DeviceFilter`, which have been here
since 0.0.1, and the other four are the Permissions API, excluded like the rest
of it. The accounting now reads **101 implemented, 29 excluded**, and
`spec-surface.py` fetches both documents with a marker check per half — a rename
upstream is a 404 that stops the script, but a reshuffle would have been silent
and would simply have shrunk the surface again.

The other four oracles refreshed byte-identically: both CG blocklists, the three
assigned-number registries, MDN's status data, and the 697 WinRT interface IIDs.

### Grants, and an escape hatch from them

**An `unrestricted` feature, for the one caller an allowlist cannot serve.**
`Grant::all_services()` grants every service on a device rather than a named
set, and `Grant::unrestricted()` adds every company identifier's advertisement
data. Off by default, gated behind a feature, and documented as outside the Web
Bluetooth model.

The reason it has to exist: the allowlist is there to stop a *page* learning
more about a device than the user agreed to, and it works because a page knows
which services it came for. A general explorer does not — the interesting
services on a device are the vendor's own 128-bit ones, and reading their UUIDs
off the device is the whole point, so there is nothing to put in the list. With
only the allowlist, `get_primary_services(None)` correctly returns an empty set
and the device looks like it has nothing on it.

**Raw advertising bytes.** `Advertisement::raw` carries the packet as it came
off the air, where the transport hands it over — which only a raw HCI socket
does, so it is populated by the `linux-hci` backend and `None` everywhere else.
Those bytes contain every company's manufacturer data whole, which is exactly
what `restrict_to` exists to filter, so they are withheld from any grant that
does not already permit all manufacturer data.
`RequestDeviceOptions::accept_all_manufacturer_data` and the matching
`LeScanOptions` builder are how a scanner opts in, behind the same
`unrestricted` feature.

The blocklist is not part of this and still applies: `get_primary_service`
refuses a blocklisted UUID under this grant exactly as under any other, and
discovery still filters blocklisted attributes out rather than reporting them.
`allowed_services` now returns an `AllowedServices` rather than a `BTreeSet`,
because a set cannot express "all of them" and a caller handed one would
silently filter discovery down to nothing — the enum answers `contains` for
both shapes, so the filtering call sites cannot get it wrong. In the grant
store a wildcard is written `*`, which parses as neither a UUID nor a company
number, so a build without the feature reads such a record back as a grant of
nothing rather than inheriting a permission it cannot represent.

**Breaking:** `allowed_services` returns `AllowedServices` rather than
`BTreeSet<BluetoothUuid>`. A caller that matched on the set should call
`contains` on the enum instead.

### Two applications, in the repository

`apps/webbluetooth-explorer` is the first: a GATT explorer — scan, connect,
browse, read, write, subscribe — written against this API, running the same
source on macOS, Linux, Windows, iOS and Android. It exists partly to be useful
and partly because a library is only as good as the first real program written
against it; the `unrestricted` feature above is what that program turned up,
along with `Advertisement::raw` and `accept_all_manufacturer_data`.

Writing it against nRF Connect as a reference also produced the honest list of
what this API cannot yet do, which is short and specific: an `Advertising` that
carries only a local name and service UUIDs (no TX power, scan response,
interval or timeout), reliable write exposed as a property flag with no
operation behind it, and no way to refresh a GATT cache or delete a bond.

`apps/browser` is the second: a web browser whose pages get a real
`navigator.bluetooth`. WKWebView has no Web Bluetooth, so this wraps it in a
Tauri shell, injects a shim ahead of every page's own scripts, and implements
the API over this crate — macOS, iOS and Android. The device chooser, the
scanning prompt and the permissions view each keep their own window, on iOS as
well as on the desktop, so a page cannot paint over the thing that grants it
access or read the list of devices offered to it.

Neither app is published; both are `publish = false`.

## 0.0.1 — 2026-09-21

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


