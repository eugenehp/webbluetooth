# Platform support

Every Apple platform builds and is checked by `./scripts/check.sh apple`.

| Platform | Target(s) | Central | Peripheral | L2CAP | Restoration |
|---|---|:--:|:--:|:--:|:--:|
| macOS | `aarch64-apple-darwin`, `x86_64-apple-darwin` | ✅ | ✅ | ✅ | ⚠️ inert |
| iOS | `aarch64-apple-ios` | ✅ | ✅ | ✅ | ✅ |
| iOS Simulator | `aarch64-apple-ios-sim` | ⚠️ no radio | ⚠️ no radio | — | — |
| Mac Catalyst | `aarch64-apple-ios-macabi` | ✅ | ✅ | ✅ | ⚠️ inert |
| tvOS | `aarch64-apple-tvos`, `-sim` | ✅ | ❌ | ✅ | ⚠️ inert |
| watchOS | `aarch64-apple-watchos`, `-sim`, `arm64_32-apple-watchos` | ✅ | ❌ | ✅ | ⚠️ inert |
| visionOS | `aarch64-apple-visionos`, `-sim` | ✅ | ❌ | ✅ | ⚠️ inert |
| Linux (BlueZ) | `*-unknown-linux-gnu` | ✅ | ✅ | ✅ | ❌ n/a |
| Linux (`linux-hci`) | `*-unknown-linux-gnu` | ✅ | ⚠️ via BlueZ | ✅ | ❌ n/a |
| Android (API 26+) | `aarch64-linux-android`, `armv7-`, `x86_64-`, `i686-` | ✅ | ✅ | ✅ (29+) | ❌ n/a |
| Raspberry Pi | see below | ✅ | ✅ | ✅ | ❌ n/a |
| Windows 10 1709+ | `x86_64-pc-windows-msvc`, `aarch64-`, `i686-`, `-gnu` | ✅ | ✅ | ❌ | ❌ n/a |
| Browser (WebAssembly) | `wasm32-unknown-unknown` | ✅ | ❌ | ❌ | ❌ n/a |
| Node.js / Deno | native addon, host platform's backend | ✅ | ❌ | ❌ | ❌ n/a |

The last two are not radios. **WebAssembly** runs against the browser's own
`navigator.bluetooth` — a client of the standard rather than an implementation
of it — so the chooser, the blocklist and the permission prompt are the
browser's. **Node and Deno** are the opposite: neither has
`navigator.bluetooth`, so the addon *provides* it, backed by whichever native
backend the host platform uses. One `.node` file serves both, because Deno
implements Node-API.

tvOS, watchOS and visionOS are Rust tier-3 targets: no prebuilt `std`, so they
need nightly and `-Z build-std`. The script handles that and skips them with a
note if nightly or `rust-src` is missing.

```sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
./scripts/check.sh apple
./scripts/check.sh apple
```

## ⚠️ `try_notify_centrals` is not on Linux

Every other peripheral method is on all four platforms — a test in
`webbluetooth-core` reads the four adapters and fails if that stops being true.
This one is the exception, and it is BlueZ's rather than a gap:

| | how a value reaches a subscriber |
|---|---|
| Apple | `updateValue:forCharacteristic:onSubscribedCentrals:` — the list is an argument |
| Android | `notifyCharacteristicChanged(device, …)` — one call per device |
| Windows | `NotifyValueAsync(subscribedClient)` |
| Linux (BlueZ) | `PropertiesChanged` on the characteristic's own D-Bus object |

The D-Bus object *is* the characteristic, not a characteristic-and-central
pair, so every subscriber is watching the one signal. There is no argument to
narrow it with. Notifying everybody instead would be wider than the caller
asked for, quietly, so the method is absent and the compiler says so.

`try_notify` — which means *all* subscribers — is on all four.

## ❌ Peripheral role: not on watchOS, tvOS or visionOS

Not a decision of this crate — CoreBluetooth's own annotations. The classes are
present in every SDK's `.tbd`, so `objc_getClass("CBPeripheralManager")`
succeeds even on a Watch, but the **initialisers** are unavailable:

```objc
CBPeripheralManager     -initWithDelegate:queue:[options:]
CBMutableService        -initWithType:primary:
CBMutableCharacteristic -initWithType:properties:value:permissions:
CBMutableDescriptor     -initWithType:value:
        API_AVAILABLE(ios(6.0), macos(10.9))
        API_UNAVAILABLE(watchos, tvos)
        API_UNAVAILABLE(visionos)
```

visionOS is easy to miss — it is a separate annotation on the following line,
and it excludes the peripheral role even though visionOS is otherwise
iOS-derived.

Rather than let a call fail at runtime, `build.rs` sets a `peripheral_role` cfg
on macOS, iOS and Android only, and the `peripheral` module does not exist
elsewhere. No Linux backend implements it either: BlueZ would mean exporting a
GATT application over D-Bus, and the daemon-free backend would mean writing an
ATT *server*.
Branch on the public constant:

```rust
if webbluetooth::PERIPHERAL_ROLE {
    // `webbluetooth::peripheral` exists here
}
```

## ⚠️ Restoration is an iOS feature in practice

`CBCentralManagerOptionRestoreIdentifierKey` is declared on every platform and
passing it is harmless, but **only iOS relaunches a terminated process to finish
Bluetooth work**, and only one that declares the `bluetooth-central` (or
`bluetooth-peripheral`) background mode. Elsewhere `willRestoreState:` never
arrives and `Bluetooth::restored_session()` stays `None` — the code is correct,
the platform simply never triggers it.

## ⚠️ Simulators have no Bluetooth

Every simulator builds and runs, and the adapter reports
`Availability::Unsupported`. There is no host-radio passthrough. Test on device.

## One radio, many watchers

Every platform scans globally, so `requestDevice`, `requestLEScan` and
`watchAdvertisements` cannot each own a scan. `crates/webbluetooth-core/src/scan.rs`
reference-counts one radio scan and fans its sightings out.

Per platform, what "restart the radio" costs when a new watcher widens the
filter:

| | radio scan | filter | widening costs |
|---|---|---|---|
| Apple | `scanForPeripheralsWithServices:` | service UUIDs | nothing — a second call replaces the filter |
| Linux (BlueZ) | `StartDiscovery` | `SetDiscoveryFilter` | a stop and a start: BlueZ only accepts a filter change while stopped |
| Android | `BluetoothLeScanner.startScan` | `ScanFilter` list | a stop and a start: the filter is fixed when the scan starts |
| Windows | `BluetoothLEAdvertisementWatcher` | none used | nothing — the watcher is unfiltered |
| `linux-hci` | raw HCI socket | none possible | nothing — a raw scan reports whatever the radio hears |

Two platform details the hub has to respect:

* **`keepRepeatedDevices` is not just a filter.** Apple, BlueZ and Android all
  suppress duplicate advertisements unless told otherwise, so a chooser that
  wants live RSSI needs them on. One watcher asking for repeats turns them on
  for the radio, and the watchers that did not ask still get one report per
  device — the suppression moves into the hub.
* **Only some platforms report a failed scan.** Android has `onScanFailed`, and
  a raw HCI socket can fail to open; CoreBluetooth has no such callback at all.
  Where the signal exists, every watcher's stream is ended, so a chooser sees
  the scan finish rather than waiting out its whole window for traffic that is
  not coming.

## Included services

An `Include` declaration lets a composite service point at another rather than
restate it, and every platform surfaces them differently:

| | how they arrive | round trip needed |
|---|---|---|
| Apple | `discoverIncludedServices:forService:`, then `service.includedServices` | yes — CoreBluetooth will not populate it unasked |
| Linux (BlueZ) | the `Includes` property on `org.bluez.GattService1` | no — read from the cached tree |
| Android | `BluetoothGattService.getIncludedServices()` | no — filled in during service discovery |
| Windows | `IGattDeviceService3.GetIncludedServicesAsync` | yes |
| `linux-hci` | `Read By Type` for `0x2802`, by hand | yes |

Two things about this are easy to get wrong.

### An ATT response does not say what type it answered

`Read By Type Response` carries the entries and an entry length, and nothing
about the attribute type that was asked for — so a characteristic declaration
and an include declaration arrive on the same opcode. The only discriminator is
the length, and GATT makes the two sets disjoint:

```text
characteristic: handle + properties + value handle + UUID   = 7 or 21
include:        handle + start + end [+ 16-bit UUID]        = 6 or 8
```

The 6-byte form exists because a 128-bit UUID does not fit the declaration. Its
UUID has to be read from the included service's own declaration, or the caller
is left with a handle range it cannot name.

Before this, the parser required an entry length of at least 7 and returned
`None` below it — so include declarations were silently invisible on that
backend, and an 8-byte one decoded as a characteristic whose UUID failed to
parse.

### Publishing one through BlueZ is order-dependent

BlueZ builds its database in the order `GetManagedObjects` lists the services,
and `gatt_db_service_add_included` returns `NULL` when the included service is
not in the database yet. Declaring the includer first fails with

```text
src/gatt-database.c:include_services() include service attributes failed
```

which is logged by `bluetoothd` and reported to the application as success —
`RegisterApplication` still returns, just without the include. `peer.py`
therefore lists Device Information before the service that includes it.

## Connection parameters

Not part of Web Bluetooth, and only two platforms can even ask:

| | how | what it reports back |
|---|---|---|
| Android | `BluetoothGatt.requestConnectionPriority` | whether the request was sent |
| Windows | `RequestPreferredConnectionParameters`, Windows 10 2004+ | a request object with a status |
| Apple | — | CoreBluetooth does not expose connection parameters at all |
| Linux (BlueZ) | — | no D-Bus API; they live behind the kernel management socket, which `bluetoothd` owns |
| `linux-hci` | — | the kernel owns the ACL link, so an LE Connection Update sent behind its back would desynchronise it |

Both are requests. The peripheral decides the interval and neither platform
reports what was agreed, so a success means the ask was made and nothing more —
which is why the API takes a [`ConnectionPriority`] rather than microseconds.

Windows expresses this as three presets (`Balanced`, `ThroughputOptimized`,
`PowerOptimized`) which line up exactly with the three priorities, so nothing
has to be stated in connection intervals. `IBluetoothLEDevice6` carries the
call; an older build fails the `QueryInterface` and is reported as unsupported
rather than as an error.

### RSSI, and what it means per platform

`device.rssi()` is a live read on Apple and Android only:

| | source | freshness |
|---|---|---|
| Apple | `readRSSI` | measured on request |
| Android | `readRemoteRssi` | measured on request |
| Linux (BlueZ) | the cached `RSSI` property | last advertisement, and dropped once stale |
| Windows | the last advertisement this crate saw | capped at 30 seconds, then reported unsupported |
| `linux-hci` | — | the scan reports it; there is no connected read without `bluetoothd` |

WinRT has no equivalent of `readRSSI` — signal strength arrives only on an
advertisement — so Windows answers from the last sighting rather than not at
all, with an age bound so a reading from some earlier minute is not passed off
as describing the link now. `127` is the "no reading" sentinel everywhere and is
never returned as a measurement.

## Miri, and what it can reach

`./scripts/check.sh miri` runs the unit tests under Miri. Undefined behaviour in
the unsafe glue does not show up as a test failure — a wrong pointer, a bad
transmute or a data race can all pass quietly — and most of this workspace is
unsafe glue.

Miri cannot call foreign functions, so anything that reaches the Objective-C
runtime is marked `#[cfg_attr(miri, ignore)]`: 8 tests in `webbluetooth` and 7
in `webbluetooth-apple`, which is most of that crate. What *is* covered is every
byte-level layer — the D-Bus codec, ATT and HCI parsing, DEX generation, the
WinRT IID arithmetic, and the COM delegate called through its own hand-built
vtable — plus the parser fuzzers, with a small corpus, because there the point
is that Miri is watching rather than the coverage.

### It found a reference leak

In `com::tests::delegates_with_different_iids_do_not_answer_for_each_other`,
which exists specifically to check reference counting:

```rust
assert_eq!(query(&a, TEST_IID).0, S_OK);   // succeeds, so AddRefs…
…
release(query(&a, TEST_IID).1);            // …and this queries a third time
```

A successful `QueryInterface` returns a `+1` reference. The assertion threw its
pointer away, so the delegate never reached zero and its 48 bytes leaked. The
production refcounting was correct; the test was not. A test that leaks
references cannot notice if the code under test starts leaking them too.

### Leak checking is on everywhere except the top crate

`webbluetooth` owns a singleton timer thread by design — one background thread
with a deadline heap, rather than a thread per sleep — and Miri counts a thread
still running at exit as a leak. `-Zmiri-ignore-leaks` also switches off
*memory* leak detection, which is why it is confined to that one crate rather
than passed globally.

## Which events a platform actually reports

The specification has events for things not every platform will tell you about.
Where a signal exists it is used; where it does not, the stream exists and
stays silent rather than the method being absent — a caller should not have to
`cfg` around an API to write portable code.

| | Apple | Linux (BlueZ) | Android | Windows | `linux-hci` | Browser |
|---|:--:|:--:|:--:|:--:|:--:|:--:|
| `gattserverdisconnected` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `characteristicvaluechanged` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `advertisementreceived` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `serviceadded`/`changed`/`removed` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `availabilitychanged` | polled | polled | polled | polled | polled | ✅ pushed |

Each platform reports a service change in its own way, and one of them is this
crate's own work:

| | how |
|---|---|
| Apple | `peripheral:didModifyServices:` |
| Android | `BluetoothGattCallback.onServiceChanged` |
| BlueZ | `GattService1` objects appearing and vanishing, plus a second `ServicesResolved` |
| Windows | `BluetoothLEDevice.GattServicesChanged` |
| Browser | `serviceadded`/`servicechanged`/`serviceremoved` — specified, not yet shipped by any browser; the listener is installed and starts working the day one ships |
| `linux-hci` | an indication on the peer's own **Service Changed** characteristic |

The last is the only backend with no stack to inherit it from. The
specification is explicit — *"the UA MUST subscribe to Indications from the
Service Changed characteristic, if it exists"* — so this one discovers
`0x2A05` in the Generic Attribute service, writes its CCCD for indications,
and treats an indication on that handle as "every cached handle is now stale"
rather than as a value for anyone to read.

One deliberate reduction, and one exception:

**Three service events become one stream.** The platforms report *that* the
service set changed and that existing handles are stale — not which service it
was. Splitting that into `serviceadded`, `servicechanged` and `serviceremoved`
would mean inventing a distinction nobody has.

**Availability is polled, not pushed** — except in a browser, which has an
`availabilitychanged` event and gets it. CoreBluetooth and BlueZ do report
adapter state changes, a raw HCI socket does not, and Android would need a
`BroadcastReceiver` this crate does not install. Rather than fire promptly on
two platforms and never on three, `Bluetooth::watch_availability` asks on an
interval. The interval is the latency, not the resolution: a change is reported
once, when it is noticed.

## The browser, and Node and Deno

Two targets that are not radios, for opposite reasons.

### WebAssembly uses the browser's Web Bluetooth

A wasm module cannot call `navigator.bluetooth`: it has linear memory and
numbers, no DOM, and no way to obtain one. Every call out is an import the page
fills in. That is true of `wasm-bindgen` too — the only question is whether the
glue is generated or written down, and here it is written down, in
`crates/webbluetooth-wasm/js/webbluetooth.js`.

The boundary is one import, `wbt_call(op, ptr, len) -> token`, rather than
forty: the shape of a call lives in one codec, and adding an operation does not
change the ABI. Op codes, event codes and error codes must match on both sides,
and a Rust test asserts each one appears in the shim with the same number —
because the failure otherwise is silent. A call succeeds, the wrong thing
happens, nothing reports an error.

Three things belong to the browser on this target, and the code says so rather
than faking them:

- **The chooser.** `requestDevice` opens the browser's picker; a supplied
  `DeviceChooser` is not consulted. A page choosing its own device would defeat
  the permission prompt entirely.
- **The blocklist.** This crate's is applied on top; the browser ships its own.
- **RSSI, connection parameters, L2CAP.** Never exposed to a page.

A browser cannot block, either — blocking the only thread stops the event loop,
so the callback that would complete the future never runs. Blocking there does
not wait, it deadlocks. Hence a single-threaded executor rather than a
`block_on`.

### Node and Deno provide it

Neither has `navigator.bluetooth` — it is a browser API, and a server-side
runtime has no picker and no prompt to build one on. So the addon exposes the
native backends instead, and one `.node` file serves both: Deno implements
Node-API.

Node-API is declared rather than wrapped. It is a C ABI with an explicit
stability guarantee, which is the whole reason it exists; declaring it is no
more exotic than the JNI, COM and Objective-C-runtime declarations elsewhere
here, and it takes no binding crate.

The symbols come from the host process, so the linker has to be told that the
undefined ones are deliberate — `-undefined dynamic_lookup` on Mach-O,
`--unresolved-symbols=ignore-all` for ELF executables. Both go through the
*unqualified* `cargo::rustc-link-arg`: the `-bins` and `-tests` variants name
target kinds this package does not have, and Cargo rejects a link argument
aimed at an absent kind — which fails the entire workspace build, not just the
one package.

### The object graph is built in Rust

`requestDevice` returns a `BluetoothDevice` with a `gatt`; the server hands out
services, a service hands out characteristics. That shape is the API, and it is
built here rather than reassembled by a JavaScript wrapper.

A wrapper works, and this had one. The reason it is gone: it puts the shape of
the API in two places and only one of them is checked by a compiler. Rename a
method on the Rust side and the mistake surfaces as `undefined is not a
function` — at the call site, at runtime, in somebody else's program.

Each object owns its Rust handle through `napi_wrap`, freed by a finalizer when
the object is collected. `gatt.connected` is a getter rather than a stored
field, because it changes under the caller and a value copied in at
construction would be a lie the moment the link dropped.

Notifications go back through the *same* threadsafe function as promise
results. Two queues would let a notification overtake the disconnection that
invalidated it.

Two things only *running* it revealed:

**Exports must be enumerable.** `napi_default` is not writable, enumerable or
configurable. The module loads, the methods work, and `Object.keys(module)` is
empty — so `const { requestDevice } = require(...)` silently yields
`undefined`.

**Something must hold the event loop open.** The threadsafe function is
unref'd when idle, so a loaded addon is never the reason a process will not
exit. But an operation in flight has to re-ref it, or a script that does
nothing but `await` one call **exits before the answer arrives, prints
nothing, and reports success** — the worst shape a failure can take. The count,
not a flag: several operations can be outstanding and the loop must stay open
until the last lands.

The addon builds for Windows too, and needs nothing extra to do it. The usual
answer there is an import library extracted from `node.exe` — a build step, and
a copy of Node on the build machine. `raw-dylib` removes both: the compiler
synthesises the import stubs from the declarations and names `node.exe` as the
module they come from, so the addon cross-compiles from anywhere. macOS and
Linux need only `-undefined dynamic_lookup` and ELF's default tolerance.

### The `!Send` bug this turned up

The addon runs the crate's futures on a worker thread so Node's loop is never
blocked, which requires them to be `Send`. On Apple they were not: three
discovery functions built a `Vec<*mut c_void>` of CBUUIDs for one synchronous
call and left it in scope across the `await` after it. Nothing in the crate
noticed, because nothing in the crate needed `Send`.

Scoping the pointers fixed it, and `crates/webbluetooth/tests/futures_are_send.rs`
pins every public future and handle type so it cannot return. It matters well
beyond Node — a `!Send` future rules out every multi-threaded executor.

## Unacknowledged writes, per platform

`writeValueWithoutResponse` has no acknowledgement. It does have flow control,
and conflating the two is how data goes missing silently. Each platform limits
it differently, and only one of them fails quietly:

| | what happens when the stack cannot take another write |
|---|---|
| **CoreBluetooth** | buffers, then **discards** — no error, no callback |
| **BlueZ** | the D-Bus reply carries the error |
| **Android** | `writeCharacteristic` returns `false` |
| **Windows** | the operation reports a `GattCommunicationStatus` |
| `linux-hci` | `poll(POLLOUT)` blocks until the socket has room |

Apple's is the one that needs care, and the framework provides for it:
`canSendWriteWithoutResponse` says whether there is room and
`peripheralIsReadyToSendWriteWithoutResponse:` says when there is again. A
write that ignores them does not fail — it corrupts, which is worse. Interest
in the callback is registered *before* the check, because it can land on the
dispatch queue between the two.

### BlueZ: `AcquireWrite`, and why it needs a transport change

`WriteValue` is a D-Bus method call, so an unacknowledged write costs a round
trip into `bluetoothd` and back **per packet**. `AcquireWrite` hands over a
`SOCK_SEQPACKET` descriptor instead: one write is one ATT PDU, with the
kernel's send buffer for flow control and no IPC in the path.

Getting that descriptor is not a decoding change. A D-Bus value of type `h` is
an *index*; the descriptor itself travels in `SCM_RIGHTS` ancillary data beside
the message. A reader using `read()` does not ignore it — the kernel has
nowhere to put ancillary data without a control buffer, so it **discards it and
closes the descriptor**, with no error and nothing in the body to suggest
anything went missing. So the socket is read with `recvmsg` from the first
byte, whether or not anything is expecting a descriptor yet.

Matching them to messages: a stream socket's read boundaries are not message
boundaries, so descriptors queue in arrival order and each message takes
exactly the count its `UNIX_FDS` header declares. A message declaring none
takes none, so it cannot steal one meant for the message behind it.

Three ways the fast path declines rather than misbehaves — no socket offered
(older BlueZ, or another client holds the characteristic), a payload larger
than the socket's MTU (a seqpacket write cannot be split, and truncating would
be worse), or a revoked socket. Each falls back to `WriteValue`.

## Backpressure, where there is none to apply

A notification arrives on a platform callback thread — CoreBluetooth's dispatch
queue, the D-Bus reader, the ATT reader, a JNI callback. Each must return
promptly; blocking one stalls the stack, and blocking CoreBluetooth's dispatch
queue can deadlock the framework.

So there is nobody to push back on. The peer has already sent the value. The
only real choices are an unbounded queue — which turns a stalled consumer into
an out-of-memory crash — or a bounded one that loses something.

`crate::backlog` bounds it, keeps the newest, and counts what it dropped.
`Notifications::lost()` is cumulative rather than reset-on-read so that two
readings can be compared: if it moved between one value and the next, the
difference is exactly the size of the gap. A consumer reassembling a firmware
image can notice a hole and start again rather than stitch corrupt bytes.
`Overflow::KeepOldest` is available per subscription for exactly that case.

## Pairing, and why it is not in the standard

Web Bluetooth has no pairing API, and that is right for a browser: the user
agent pairs on the user's behalf when a peer demands it, and a page is never
told. A library's caller *is* the application, so it gets to ask.

The omission matters more than it sounds. A large class of real devices —
heart-rate straps, glucose meters, anything with a privacy requirement — marks
its interesting characteristics as requiring encryption. Reading one without a
bond fails with `insufficient authentication`, and no amount of retrying helps.

| platform | `pair()` |
|---|---|
| BlueZ | `org.bluez.Device1.Pair`; the daemon runs the ceremony and asks the registered agent |
| Android | `createBond`, then polls `getBondState` until it settles |
| Windows | `DeviceInformationPairing.PairAsync`; Windows picks the ceremony and prompts |
| Apple | **nothing** — `Pairing::Implicit` |
| Browser | **nothing** — `Pairing::Implicit`, the user agent's job |
| `linux-hci` | **fails** — there is no Security Manager without a daemon |

Apple returns a third outcome rather than success or failure, because neither
is true. CoreBluetooth exposes no pairing API on any of its platforms; the
system runs the ceremony itself the first time an encrypted attribute is
touched. Reporting success would claim a bond that does not exist, and
reporting failure would send a caller looking for a problem that is not there —
when the correct next step is simply to read the characteristic they wanted.

Android polls rather than listening. The outcome arrives in an
`ACTION_BOND_STATE_CHANGED` broadcast, which needs a `BroadcastReceiver`
registered against a `Context` this crate does not assume it has. A pairing
ceremony is a human-scale event, so a quarter-second granularity is invisible;
the timeout is a minute, because the real ceiling is a person reading a dialog.

`linux-hci` cannot do this at all, and the reason is structural rather than
unfinished work: pairing is the Security Manager Protocol, SMP lives in the
kernel driven by `bluetoothd`, and the bond store outlives the process. A
backend whose purpose is to avoid that daemon cannot borrow its Security
Manager. A device requiring encryption needs the BlueZ backend.

## Permissions per platform

| | Required |
|---|---|
| **macOS** app | `NSBluetoothAlwaysUsageDescription` in `Contents/Info.plist`; sandboxed apps also need the `com.apple.security.device.bluetooth` entitlement |
| **macOS** CLI | Same key, linked into `__TEXT,__info_plist` by a build script — see `crates/webbluetooth/build.rs` |
| **iOS / visionOS / watchOS / tvOS** | `NSBluetoothAlwaysUsageDescription`. Also `NSBluetoothPeripheralUsageDescription` if you deploy to iOS 12 or earlier |
| **Background use (iOS)** | `UIBackgroundModes` containing `bluetooth-central` and/or `bluetooth-peripheral` |

On macOS the TCC prompt is attributed to the **responsible process** — the
terminal or IDE that launched yours. Under a wrapper with no Bluetooth grant of
its own, the request is denied without a dialog.

## Android

### JNI is reached by slot index, so the indices are checked against `jni.h`

Android hands the crate a struct of function pointers and no symbols to link
against, so every JNI call is `table[N]`. An index that is off by one does not
fail to compile and usually does not crash — it calls the function next to the
one you meant, with your arguments.

`REGISTER_NATIVES` was 216, which is `UnregisterNatives`. That takes a class,
ignores the rest, and returns `JNI_OK`, so registration appeared to succeed
while no callback was ever bound. Three more were wrong the same way:
`GetByteArrayElements`, `ReleaseByteArrayElements` and `NewDirectByteBuffer`.

### What an emulator adds over `dexdump`

`./scripts/android/art.sh` boots an AVD and runs the generated classes there.
That covers three things nothing else does:

* The superclass is the real `android.bluetooth.BluetoothGattCallback`, so the
  class genuinely **links** rather than merely parsing.
* The constructor is **invoked**. It is the only method in these classes with a
  `code_item` — four units written by hand — and until this ran, nothing had
  ever executed it.
* `examples/art_probe.rs` is a `cdylib` that ART `System.load`s, so the crate's
  own path runs on the device: DEX into a direct `ByteBuffer`, through
  `InMemoryDexClassLoader`, `RegisterNatives` to bind a method, instantiate.

The emulator has a working Bluetooth stack (`state: ON`, both
`android.hardware.bluetooth*` features), so a full BLE round-trip is possible in
principle. It is **not** built: that needs an APK with a `Context` and runtime
`BLUETOOTH_SCAN`/`CONNECT`/`ADVERTISE` grants. The probe runs under `dalvikvm`,
which has no `Context` at all, so a plain `java.lang.Object` stands in for one —
fine because nothing it touches needs the system services.

`tests/slots.rs` parses `struct JNINativeInterface_` and
`struct JNIInvokeInterface_` out of the JDK's own `jni.h` and compares every
constant in the crate against the declaration order, plus both table lengths.
It needs no JVM, only the header, and it covers the whole table rather than the
slots a behavioural test happens to exercise. `tests/jvm.rs` still runs the
used slots against a real VM through the Invocation API; the two together are
what `./scripts/check.sh android` reports.

Note that `tests/jvm.rs` must share one VM across its tests — a process may
create exactly one. Without that, the second `JNI_CreateJavaVM` fails and its
test skips silently, which is how the wrong indices survived being "verified".

## Android in an app process

`./scripts/android/apk.sh <role>` builds an APK — without Gradle: `aapt2` links
the manifest, `d8` makes the dex, the `.so` is added to the archive, `apksigner`
signs it — installs it on an emulator, grants the runtime permissions and runs
`examples/android-harness.rs`.

The APK exists because `scripts/android/art.sh` cannot go further. That runs
under `dalvikvm`, which has no `Context`, so it never reaches
`BluetoothManager` and never moves a packet. An `Activity` is the only thing
that can hold a `Context` and the `BLUETOOTH_SCAN`/`CONNECT`/`ADVERTISE`
grants.

It found two bugs immediately, and neither would have failed a test.

### Every callback event was being dropped

The sink registry was a `HashMap` keyed by the callback object's **global**
reference, looked up with the **local** reference JNI hands a native method:

```rust
pub fn bind_sink(callback: &Ref, sink: Arc<dyn EventSink>) {
    sinks().lock().unwrap().insert(callback.key(), sink);   // global ref
}
fn emit(this: JObject, event: Event) {
    …m.get(&(this as usize))…                               // local ref
}
```

A JNI reference is a handle, not an address. The two denote the same object
through different pointers, so the lookup never matched and **every** event was
discarded — no scan results, no connection changes, no notifications, no
`onServiceAdded`. Nothing errored; callers simply waited forever. `publish()`
hung with the platform log cheerfully showing `onServiceAdded() status=0`.

`IsSameObject` is the only way to ask whether two references mean the same
object, so the registry is now a short list that gets walked.
`tests/jvm.rs::a_local_and_a_global_reference_differ_in_value` pins this down
against a real JVM.

### A transient failure at startup was permanent

The backend resolved the adapter once, in its constructor, and cached the
verdict. Android brings the Bluetooth stack up asynchronously, so a process
that starts early sees no adapter for a second or two — and was then
`Unsupported` for the life of the process, with no retry. The adapter is opened
on demand now; it costs a handful of JNI calls against a BLE round trip that
takes milliseconds.

The same code collapsed *every* failure into `Availability::Unsupported`, which
surfaces as "this system has no Bluetooth LE support" — so a missing method or
a pending exception was reported as missing hardware.

### Advertising cannot carry a name on Android

`AdvertiseData` has no API for a custom local name; `Advertising::local_name`
can only become `setIncludeDeviceName(true)`, which uses the *adapter's* name.
An emulator is called `sdk_gphone64_arm64`: eighteen characters, so twenty
bytes, which with a 128-bit service UUID (eighteen more) and the flags exceeds
the 31 an advertisement holds. Android then refuses the whole advertisement
rather than truncating, and the crate reports it faithfully:

```
!! NetworkError: could not start advertising: the advertising payload is larger than 31 bytes
```

Changing the name means changing `BluetoothAdapter.setName`, which is global to
the device. The harness advertises the service UUID alone.

### What the emulator could not do

A round trip needs two devices, and two emulators cannot be made to hear each
other here. Both attach to a single `netsimd`, which models position as well as
radio — but this SDK package ships only the daemon, with no `netsim` CLI and no
reachable control API, so the devices cannot be listed or moved into range. The
central registers its scan (`onScannerRegistered status=0`) and hears nothing.

So on Android the peripheral half is exercised for real and the central half
only up to starting a scan. Two callbacks — `onServiceAdded` and
`onAdvertisingStarted` — did travel the fixed `emit` path on-device, which is
what confirms the reference-identity fix rather than merely compiling it.

## iOS on a real device

`./scripts/ios.sh <example> [args]` builds for `aarch64-apple-ios`, wraps the
binary in an `.app`, finds a connected device, a provisioning profile covering
its UDID and a signing identity in that profile's team, installs it and runs it
with the console attached. Four things about the platform are worth knowing
before reading a failure as a bug in the crate.

### The Bluetooth prompt needs a foreground app

iOS presents the prompt to the app that is active and on screen. A bundle with
no `UIApplication` never becomes active, is never asked, and its manager sits in
`Unknown` forever — the `doctor` example reports `permission not determined` and
`adapter state never reported`, which reads like a broken backend and is not
one. `examples/ios-harness.rs` therefore boots a real `UIApplication` with a
window, whose delegate is synthesised at runtime like every other delegate here.

The grant is per bundle identifier and persists, so only the first run of a
given bundle needs someone to tap **Allow**.

### `availability()` settles for five seconds

It waits that long for the manager to report a state and then returns what it
has. That is right for an app whose grant already exists, and wrong for the very
first run, where the answer depends on a human. On a first run expect
`Availability::Unknown` rather than a denial — the harness polls
`authorization()` separately instead of reading anything into it.

### An app's services join the *device's* GATT database

`addService:` does not create a server for your app. It adds to the one server
the device publishes, next to the services iOS provides itself. Publishing the
assigned Battery Service from an app therefore puts two `0x180F` services on one
device, and a central asking for "the" battery service gets whichever is
returned first — in practice iOS's own, whose level requires an encrypted link
and fails with `CBError 15, Encryption is insufficient`.

Use a private UUID for anything a test needs to find unambiguously. The harness
publishes `6e400001-b5a3-f393-e0a9-e50e24dcca9e` for exactly this reason.

### A device has two names, and they disagree

The advertised Local Name and the GAP Device Name are different fields. An iPad
advertising `Rust iPad` reports a device name of `iPad` as soon as anything has
connected to it once. A `name` filter matches against **either**, because
preferring one silently hides devices that are still advertising the other.

## Windows

WinRT (`Windows.Devices.Bluetooth`), reached through hand-written COM. Needs
Windows 10 1709 or newer for `GattServiceProvider`; the central role works
further back.

**L2CAP is unavailable.** Neither WinRT nor Win32 exposes an LE
connection-oriented channel, so the `l2cap` module does not exist on Windows and
`webbluetooth::L2CAP` is `false`. Every other platform has one.

Two Windows behaviours differ from the other peripheral backends:

* **Advertising belongs to the service.** `GattServiceProvider` both publishes
  and advertises, so `Advertising::local_name` has no effect — Windows
  advertises the machine's Bluetooth name. Publish before advertising.
* **A request must be held open with a deferral.** The event handler returns
  long before a caller has decided what to answer; without a deferral the
  request window closes and the central times out. The engine takes one for
  every read and write.

Like CoreBluetooth and unlike Android, Windows manages the Client Characteristic
Configuration descriptor itself and reports subscriptions directly, so
`Request::Subscribed` comes from `SubscribedClientsChanged` rather than from
watching descriptor writes.

### Regenerating the identifiers

```sh
./scripts/update.sh winrt-iids
```

Reads Microsoft's generated bindings and rewrites
`crates/webbluetooth-windows/src/iids.rs`: interface IIDs, runtime class names
with their default interfaces, the GUIDs of parameterised types, and **vtable
slot indices**. Re-run after a Windows SDK bump and read the diff. IIDs do not
change and vtables are an ABI Microsoft does not break, but a few slots are
pinned by a test so a reorder would fail loudly rather than silently.

## The Linux peripheral role

BlueZ owns the controller, so a peripheral is not built by talking to hardware
but by *becoming a D-Bus service* and handing `bluetoothd` the tree it should
publish — services, characteristics and descriptors as exported objects, pointed
at with `GattManager1.RegisterApplication`. Advertising is a second object
registered with `LEAdvertisingManager1`.

Four things follow from that, and all four shaped the implementation.

**Every read is a blocking D-Bus call.** BlueZ expects `ReadValue` to *return*
the bytes, while this crate hands the application a `ReadRequest` to answer
whenever it likes. The handler therefore parks on a channel until the
application responds, with a deadline shorter than BlueZ's own so the central
sees a proper ATT error rather than a stalled link.

**Which meant inbound calls could no longer run on the D-Bus reader thread.**
That thread is the only one reading the socket, so a parked handler would stall
every inbound message behind it — including the replies to our own calls. The
deadlock is not hypothetical: `RegisterApplication` does not return until BlueZ
has walked the tree, so we must be able to answer `GetManagedObjects` while
blocked waiting for it. Method calls now each get their own thread.

**Notifications are property changes.** There is no notify call. A subscribed
central is sent a value by emitting `PropertiesChanged` for the characteristic's
`Value`, and BlueZ decides from the flags whether that leaves as a notification
or an indication.

**BlueZ does not say who subscribed.** `StartNotify` carries no device and no
MTU, so a `RemoteCentral` from a subscription stands for the subscription rather
than identifying a peer — unlike a read or write, where the options dictionary
does carry the device path.

Two limits worth stating: a service cannot be withdrawn from a registered
application, so `unpublish` re-registers without it; and `publish_l2cap_channel`
returns `NotSupported`, because BlueZ has no D-Bus API for it — a peripheral
would have to listen on an L2CAP socket itself.

### Choosing an adapter

`WEBBLUETOOTH_ADAPTER=hci1`, or `Bluez::select_adapter("hci1")`, picks a
controller; otherwise the first is used, which sorts to `hci0`. Re-reading the
object tree keeps whatever was chosen, so a session is never silently moved to
another controller. This is also how the two-adapter test puts each end of a
conversation on its own radio.

## Linux under QEMU

`./scripts/qemu/vm.sh` boots a Debian VM with a real Bluetooth stack, two
emulated controllers and a BlueZ peer, so both Linux backends can be run against
something rather than only compiled. `scripts/qemu/peer.py` publishes the same
fixture as `examples/ios-harness.rs`, which is why `examples/roundtrip.rs` runs
unchanged on either.

### Why a VM and not a container

Docker Desktop's linuxkit kernel is built with `# CONFIG_BT is not set`. There
is no Bluetooth subsystem, `AF_BLUETOOTH` fails with `EAFNOSUPPORT`, and no
amount of `--privileged` changes it. The D-Bus mock in `docker/` is the ceiling
a container can reach; a VM brings its own kernel.

Getting a virtual controller took three steps, each because the obvious thing
is missing:

* Debian's **cloud** kernel ships no Bluetooth modules at all.
* Debian's **generic** arm64 kernel ships Bluetooth but is built with
  `# CONFIG_BT_HCIVHCI is not set`, so `/dev/vhci` does not exist. The driver is
  one self-contained file and builds out of tree against the headers.
* `hci_vhci` only creates the device node. Something has to emulate a controller
  behind it, and Debian packages no `btvirt` — it comes from a BlueZ source
  build with `--enable-testing`.

`btvirt -l2` then gives two linked controllers on a shared virtual air.

### The emulated radio does not advertise on an interval

A real controller repeats its advertisement. `btvirt` emits one burst of
reports synchronously while it processes an *enable* command, to whichever
controllers are scanning at that instant. When the central starts scanning, that
burst lands about seven microseconds after the Command Complete for Set Scan
Enable and roughly sixty before the kernel marks discovery active — so the
kernel drops it, and nothing is ever sent again. Discovery finds nothing while
the trace clearly shows reports arriving.

`peer.py` therefore re-registers its advertisement every two seconds, which puts
the burst at a moment when the central is genuinely discovering. That is what a
periodic advertiser would have done anyway.

### Legacy and extended advertising data are separate buffers

`bluetoothd` configures advertising through the *extended* commands when the
controller claims 5.0 support, even when the PDUs it asks for are legacy
`ADV_IND`. `btvirt` keeps that data in the extended set and leaves the legacy
advertising buffer empty — then emits *both* report types regardless of which
scan mode is enabled.

The `linux-hci` backend scans with `LE Set Scan Enable`, which is correct, and
so receives the legacy report with `Data length: 0`: the peer's address with no
name and no service UUIDs, so a service filter can never match. Populating the
legacy buffer directly makes the fixture visible to it:

```sh
# flags + complete 128-bit service UUID + short name, 30 of 31 bytes
sudo hcitool -i hci1 cmd 0x08 0x0008 1e \
  02 01 06 \
  11 07 9e ca dc 24 0e e5 a9 e0 93 f3 a3 b5 01 00 40 6e \
  08 09 52 75 73 74 48 43 49 00
sudo hcitool -i hci1 cmd 0x08 0x000a 01
```

On a real controller a legacy scan sees legacy PDUs with their data, so this is
a fidelity gap in the emulator rather than something the backend should work
around.

### What the emulator found in `linux-hci`

That backend had never been run before this, and four bugs came out of it. All
are fixed; they are recorded because each would have failed on real hardware in
a way that looks like something else.

**The ATT socket was never bound.** The kernel left the channel's source type
at `BDADDR_BREDR` and `l2cap_chan_connect` refuses a BR/EDR source with an LE
destination, so every connect returned `EINVAL`. Binding to `BDADDR_ANY` with
`BDADDR_LE_PUBLIC` is what tells the socket it is an LE one; BlueZ's own tools
do the same.

**The peer's requests were being read as replies.** BlueZ opens a connection
with its own `Exchange MTU Request`. The reader treated everything that was not
a notification as the answer to whatever request was in flight, so the peer's
question was consumed as our answer — and then never answered, leaving BlueZ
waiting. This crate has no attribute database to serve, but a client still
shares the connection with a peer that may ask, so requests are now either
answered (`Exchange MTU`) or declined with `Error Response`.

**A blocking write could hang with no deadline.** With the peer waiting, our
next write parked in `bt_sock_wait_ready` — which loops on `schedule_timeout`
with `SO_SNDTIMEO` unset, and shows up as `wchan=__lock_sock`. `request()`
already carried a timeout, but the syscall never returned to it, so the hang was
below the reach of the deadline. Every send now polls first, which gives the
deadline somewhere to apply.

**Long writes did not exist.** One `Write Request` carries `MTU - 3` bytes and a
GATT attribute holds 512, but only `Write Request` and `Write Command` were
implemented. Anything longer is now sent as `Prepare Write` fragments and
committed with `Execute Write`; an unacknowledged write that does not fit is
refused rather than truncated.

### Scan responses have to be merged by hand

A device does not necessarily say everything in one packet. `bluetoothd` puts
the service UUIDs in the advertisement and the name in the scan response, which
arrives as a *separate* report. Judged on its own the advertisement looks
nameless and the scan response fails a service filter, so the device is found
without a name — or missed entirely.

Every other transport merges these before we see them. On a raw HCI socket it is
ours to do, so reports now accumulate per address and the filter is applied to
the merged view. `Advertisement::merge` is careful about two things: a scan
response must not retract connectability, and UUID sets accumulate rather than
replace.

## Raspberry Pi

A Pi is Linux with BlueZ, so it uses the Linux backend unchanged — there is no
separate Pi build. What differs is the architecture, and Pi OS ships in three:

| Model | Target |
|---|---|
| Pi 5, 4, 3, Zero 2 W — 64-bit Pi OS | `aarch64-unknown-linux-gnu` |
| Pi 4, 3, Zero 2 W — 32-bit Pi OS | `armv7-unknown-linux-gnueabihf` |
| Pi Zero, Zero W, 1, CM1 (ARMv6) | `arm-unknown-linux-gnueabihf` |

`musl` variants of the first two are checked as well, for static binaries.
`./scripts/check.sh pi` builds and lints all five, plus `linux-hci` on ARM.

### Why a Pi has no `hci0`

Unlike every other Linux machine, the Pi's controller is attached over a **UART**
rather than USB, by a systemd unit. That gives it three failure modes worth
knowing before blaming the code, and the `doctor` example checks all three:

1. **`dtoverlay=disable-bt` in `config.txt`** removes the controller outright.
   Delete the line and reboot.
2. **The attach service is not running** — `hciuart`, or `bluetooth-dev` on a
   Pi 5. `sudo systemctl enable --now hciuart`.
3. **On a Pi 3 or Zero W the controller shares the PL011 UART with the serial
   console.** Enabling that console on `/dev/ttyAMA0` takes the UART away from
   Bluetooth. Turn the console off in `raspi-config`, or accept the lower speed
   of the `miniuart-bt` overlay.

`doctor` reads `/proc/device-tree/model`, so it knows it is on a Pi and says
these things only there.

### Which backend

The default BlueZ backend is right for Raspberry Pi OS, which runs `bluetoothd`
and puts the default user in the `bluetooth` group. Reach for `linux-hci` on a
stripped image with no daemon — it needs `CAP_NET_RAW` to scan and nothing at
all for GATT, at the cost of pairing.

## Linux: two backends

| | **BlueZ** (default) | **`linux-hci`** |
|---|---|---|
| needs `bluetoothd` | yes | no |
| GATT | D-Bus calls to `org.bluez` | ATT over an L2CAP socket (CID 4) |
| scanning | `Adapter1.StartDiscovery` | raw HCI socket, `LE Set Scan Enable` |
| privileges | D-Bus policy | none for GATT, `CAP_NET_RAW` to scan |
| pairing / bonding | `Device1.Pair`, the daemon's Security Manager | **absent** |
| RSSI while connected | yes | scan-time only |

`linux-hci` exists for systems with no daemon. Its limitation is real and worth
restating: `bluetoothd` owns SMP, so without it a characteristic requiring an
encrypted link answers `INSUFFICIENT_AUTHENTICATION` and there is nothing to
satisfy it with. Unencrypted GATT — most of it — works.

One API difference on both Linux backends: **`BluetoothDevice::id` is the
peer's Bluetooth address.** BlueZ exposes it and keys its object paths on it,
and HCI reports it directly. Apple never does. Code that treats the id as opaque
works everywhere, but a Linux id identifies a person in a way a macOS one does
not.

```sh
docker run --rm -v "$PWD":/work:ro webbluetooth-test cargo test --workspace
./scripts/check.sh linux
```

Running against real hardware needs the container to see a controller:

```sh
docker run --rm --privileged --net=host \
    -v /var/run/dbus:/var/run/dbus \
    -v "$PWD":/work:ro webbluetooth-test cargo test --workspace
```

Docker Desktop on macOS or Windows cannot do this — its VM has no Bluetooth
hardware to pass through.

## Architecture notes

Nothing here is platform-specific by construction. The Objective-C runtime,
libdispatch, `NSStream` and `CFRunLoop` are identical across all of them, and
there is no Swift, no `.m`, and no compiler step to configure per target — the
delegate classes are synthesised at runtime, so the same code path runs
everywhere. The only `cfg` in the crate is the peripheral-role gate above.
