# webbluetooth

The [Web Bluetooth API](https://webbluetoothcg.github.io/web-bluetooth/) in
Rust — on Apple platforms, Linux, Android, Windows and the browser.

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

```toml
[dependencies]
webbluetooth = "0.0.1"
```

That is the whole dependency list for the example above. `webbluetooth::prelude`
re-exports `StreamExt`, because notifications, advertisements and disconnects
are all streams and a stream has no `.next()` without it — so the example used
to need a `futures-util` dependency nobody had mentioned.

You do need an executor to run any of it. This crate brings none, so that it
works with whichever you already have; the examples use `futures_executor`,
which is the smallest thing that will do.

No Swift, no Objective-C, no Java, no `libdbus`, no code generation, no shim.
Each platform is reached through its own C ABI directly, and where one demands
a type that does not exist — a CoreBluetooth delegate, an Android callback
subclass — it is built at run time rather than shipped.

## Start here

```sh
cargo run --example doctor    # why Bluetooth is or isn't usable
cargo run --example scan      # list everything in range
cargo run --example battery   # read the nearest battery level
```

Run `doctor` first. Almost every "it finds nothing" report is an authorization
problem rather than anything to do with the radio — on Apple platforms a
process with no `NSBluetoothAlwaysUsageDescription` is reported
`Unauthorized` and is **never prompted**. The repository's README explains what
to do about that; it is the single most common way to lose an afternoon here.

## Platforms

| Platform | Central | Peripheral | L2CAP |
|---|:--:|:--:|:--:|
| macOS, iOS, Mac Catalyst | ✅ | ✅ | ✅ |
| tvOS, watchOS, visionOS | ✅ | ❌ | ✅ |
| Linux (BlueZ, or a raw socket with `linux-hci`) | ✅ | ✅ | ✅ |
| Android (API 26+) | ✅ | ✅ | ✅ (29+) |
| Windows 10 1709+ | ✅ | ✅ | ❌ |
| Browser (WebAssembly) | ✅ | ❌ | ❌ |
| Node.js / Deno | ✅ | ❌ | ❌ |

`PERIPHERAL_ROLE` and `L2CAP` are `const bool`s to branch on, and the modules
they describe do not exist where they are `false`.

## What is different from a browser

**The chooser is yours.** `requestDevice()` is defined around a user gesture
and a picker dialog. A library has no chrome to draw, so it is a trait:
`FirstMatch` (the default, and a deliberate removal of the consent step),
`StrongestSignal`, `TerminalChooser`, a predicate, or your own.

**Everything else about the security model is kept** — the GATT blocklist, the
manufacturer-data blocklist, and the rule that a device grants access only to
the services that were asked for.

## Features

| | |
|---|---|
| `linux-hci` | Talk to the controller directly instead of going through BlueZ. |
| `terminal-chooser` | A device chooser that prints numbered candidates and reads a line. |

## More

The [repository](https://github.com/eugenehp/webbluetooth) has the full guide:
authorization, the peripheral role, L2CAP channels, state restoration, how each
backend works, and what is tested where. `PLATFORMS.md` is the per-platform
reference.

## License

MIT. See `LICENSE`.
