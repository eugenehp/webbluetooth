# webbluetooth-wasm

The browser's own Web Bluetooth, reached from WebAssembly, and the adapter that
turns it into [`webbluetooth`](https://crates.io/crates/webbluetooth)'s
portable model.

No `wasm-bindgen`. The boundary is a hand-written ABI — a small byte codec and
a handful of imported functions — with a JavaScript shim on the other side
(`js/webbluetooth.js`). A test checks that the two agree about the encoding.

The chooser is the browser's, because `requestDevice` there opens the real
picker; and the blocklist is Chrome's, with this crate's applied on top.

**You want `webbluetooth` instead**, unless you are working on the backend.

## On the documentation here

Every type and module is documented. Individual fields, variants and constants
often are not: they mirror the platform API this wraps, name for name, and that
API's own reference is the better answer than a line repeating the name. The
adapter module — the half that presents `webbluetooth-core`'s model — is
`#[doc(hidden)]`, because it is public only so `webbluetooth` can reach it.

## License

MIT. See `LICENSE`.
