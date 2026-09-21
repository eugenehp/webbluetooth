# Coming from btleplug

[btleplug](https://github.com/deviceplug/btleplug) is where most Rust Bluetooth
programs start, and this is the mapping between the calls one actually makes
and their counterparts here. It is drawn from a real port rather than from
reading two API docs side by side: [muse-rs](https://github.com/eugenehp/muse-rs),
an EEG headset client that streams from eight characteristics at once, moved
across, and every row below is a call that port had to replace.

The short version: the GATT half is a near-mechanical translation, and two
things are genuinely different — **you must name the services you intend to
use**, and **notifications are per characteristic** rather than one stream for
the whole peripheral.

## The mapping

| btleplug | webbluetooth |
|---|---|
| `Manager::new()`, `manager.adapters()` | `Bluetooth::shared()` |
| polling `adapter.adapter_state()` for `PoweredOn` | `bluetooth.availability().await` |
| `adapter.start_scan(ScanFilter)` + `adapter.peripherals()` | `bluetooth.request_device(options)` with a `DeviceChooser` |
| …or scanning to enumerate several | `bluetooth.request_le_scan(LeScanOptions)`, then `adopt_candidate` |
| `peripheral.properties()` → `local_name` | `device.name()` |
| `peripheral.id()` | `device.id()` |
| `peripheral.connect()` | `device.gatt().connect()` |
| `peripheral.discover_services()` | implicit in `get_primary_service` |
| `peripheral.characteristics()` (one flat set) | `service.get_characteristics(None)`, per service |
| `peripheral.subscribe(&char)` | `characteristic.start_notifications()` → a `Stream` |
| `peripheral.notifications()` (one stream, each item tagged) | `.tagged()` on each, merged with `stream::select_all` |
| `peripheral.write(&char, data, WriteType::WithoutResponse)` | `characteristic.write_value_without_response(data)` |
| `peripheral.write(&char, data, WriteType::WithResponse)` | `characteristic.write_value_with_response(data)` |
| `peripheral.read(&char)` | `characteristic.read_value()` |
| `peripheral.is_connected()` | `gatt.connected()` (not `async`) |
| `peripheral.disconnect()` | `gatt.disconnect()` (not `async`) |
| `adapter.events()` filtered for `CentralEvent::DeviceDisconnected` | `device.watch_disconnect()` |
| `uuid::Uuid` constants | `BluetoothUuid` — or keep yours, with `features = ["uuid"]` |

## What is actually different

**You name the services you intend to reach.** btleplug hands you every
characteristic the peripheral has. Web Bluetooth grants access to exactly the
services a request listed in `filters` or `optional_services`, and
`get_primary_service` refuses anything else with a `SecurityError`. This is the
specification's security model rather than an omission here, and it is the
first thing a port trips over:

```rust
let device = bluetooth.request_device(
    RequestDeviceOptions::new()
        .filter(DeviceFilter::new().name_prefix("Muse"))
        // Without this line the service below is a SecurityError.
        .optional_service(0xfe8du16)?,
).await?;
```

**Notifications are per characteristic.** btleplug gives one stream for the
whole peripheral and puts the UUID in each item, so a dispatch loop matches on
`notification.uuid`. Here each subscription is its own stream of values. To get
the same shape back, tag and merge:

```rust
use webbluetooth::stream::{select_all, StreamExt};

let mut subscriptions = Vec::new();
for characteristic in characteristics {
    subscriptions.push(characteristic.start_notifications().await?.tagged());
}

let mut values = select_all(subscriptions);
while let Some((uuid, value)) = values.next().await {
    // The same `match uuid` dispatch a btleplug program already has.
}
```

It is worth the step: `lost()` is per subscription, so a merged set can say
which characteristic dropped packets rather than only that some did. See
`examples/sensors.rs`.

**One session per process.** Each `Bluetooth::new()` opens a separate adapter
session with its own grants and connections, and a `BluetoothDevice` only works
through the session that found it — so a device discovered by one and connected
by another will not work. `Bluetooth::shared()` is the process-wide one; prefer
it. btleplug's `Manager` has no equivalent hazard, which is exactly why it is
worth knowing about.

**Handles go stale across a disconnect.** Services and characteristics belong
to the connection that produced them. After a reconnect, rediscover rather than
reusing what you held: the old handles fail with `InvalidStateError` instead of
messaging a freed object.

**`connect()` has no timeout**, the way btleplug's does not either — it waits
for the peripheral to come into range for as long as that takes. Wrap it in
`webbluetooth::timeout` if your program has a person watching it.

## What you get for the move

Web Bluetooth's own security model, enforced: the per-device service allowlist
above, and the [GATT blocklist](https://github.com/WebBluetoothCG/registries)
vendored from the Community Group registry, so a characteristic the standard
forbids is filtered out of discovery rather than returned and refused.

Beyond the standard, because the platforms offer them: the peripheral role,
L2CAP channels, connection parameters and PHY, adapter selection, and iOS state
preservation. See the README.

## What you give up

btleplug supports Bluetooth Classic on some platforms; this is LE only, as Web
Bluetooth is. And if your program genuinely wants every characteristic of an
unknown device without declaring anything up front, the allowlist is in your
way by design — `sensors.rs` shows the shape that replaces it, which is to take
the services as configuration.
