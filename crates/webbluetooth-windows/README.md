# webbluetooth-windows

WinRT from pure Rust, and the adapter that turns it into
[`webbluetooth`](https://crates.io/crates/webbluetooth)'s portable model.

No `windows` crate, no bindgen, no generated projection. COM is a vtable and a
calling convention, so the interfaces are declared by hand and called through
their own vtables; the IIDs and slot numbers come out of Microsoft's own
metadata rather than being transcribed (`scripts/update.sh winrt-iids`).

WinRT has no `Connect`: `BluetoothLEDevice.FromBluetoothAddressAsync` hands
back a device whether or not the radio has a link, and the link is established
by the first thing that needs it. The adapter treats service discovery as the
connection.

**You want `webbluetooth` instead**, unless you are working on the backend.

## On the documentation here

Every type and module is documented. Individual fields, variants and constants
often are not: they mirror the platform API this wraps, name for name, and that
API's own reference is the better answer than a line repeating the name. The
adapter module — the half that presents `webbluetooth-core`'s model — is
`#[doc(hidden)]`, because it is public only so `webbluetooth` can reach it.

## License

MIT. See `LICENSE`.
