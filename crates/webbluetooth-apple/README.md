# webbluetooth-apple

CoreBluetooth from pure Rust, and the adapter that turns it into
[`webbluetooth`](https://crates.io/crates/webbluetooth)'s portable model.

No Swift, no Objective-C source, no code generation. The delegate classes
CoreBluetooth requires do not exist in Rust, so they are built at run time —
`objc_allocateClassPair`, Rust `extern "C"` functions installed as method
implementations, `class_addProtocol` for `CBCentralManagerDelegate` and
`CBPeripheralDelegate`, `objc_registerClassPair`. Callbacks arrive on a private
`dispatch_queue`, so this works in a plain `fn main()` with no `NSRunLoop`.

`CoreBluetooth.framework` exports zero Swift-mangled symbols and ships no
`.swiftinterface`; a Swift bridge would add a `swiftc` step without removing a
single `objc_msgSend`. The Objective-C runtime is the whole of the dependency
list — it and libdispatch are both in libSystem.

**You want `webbluetooth` instead**, unless you are working on the backend.

## On the documentation here

Every type and module is documented. Individual fields, variants and constants
often are not: they mirror the platform API this wraps, name for name, and that
API's own reference is the better answer than a line repeating the name. The
adapter module — the half that presents `webbluetooth-core`'s model — is
`#[doc(hidden)]`, because it is public only so `webbluetooth` can reach it.

## License

MIT. See `LICENSE`.
