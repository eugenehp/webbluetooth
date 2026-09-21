# webbluetooth-node

[`webbluetooth`](https://crates.io/crates/webbluetooth) as a Node.js and Deno
addon.

No `napi-rs`, no bindgen. Node-API is a stable C ABI, so it is called directly.
The object graph JavaScript sees — `navigator.bluetooth`, devices, the GATT
tree — is built in Rust, and the crate's futures run on a worker thread so
Node's loop is never blocked. Which is why every public future in
`webbluetooth` has to be `Send`, and why a test asserts it.

It uses whichever backend the host platform has: CoreBluetooth on a Mac, BlueZ
on Linux, WinRT on Windows.

## License

MIT. See `LICENSE`.
