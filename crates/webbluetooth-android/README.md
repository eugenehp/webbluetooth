# webbluetooth-android

Android Bluetooth from pure Rust, and the adapter that turns it into
[`webbluetooth`](https://crates.io/crates/webbluetooth)'s portable model.

No Java source, no Gradle step, no `.jar`. JNI is a C ABI, so it is called
directly — and the callback subclasses the Android framework requires cannot be
written in Rust, so their DEX is *generated at run time* and handed to
`InMemoryDexClassLoader`. The JNI function table's slot numbers are checked
against whichever JDK's `jni.h` is installed rather than being remembered.

Android has no ambient way to reach a `Context`, so one must be supplied before
any Bluetooth call — see `webbluetooth::android`.

**You want `webbluetooth` instead**, unless you are working on the backend.

## On the documentation here

Every type and module is documented. Individual fields, variants and constants
often are not: they mirror the platform API this wraps, name for name, and that
API's own reference is the better answer than a line repeating the name. The
adapter module — the half that presents `webbluetooth-core`'s model — is
`#[doc(hidden)]`, because it is public only so `webbluetooth` can reach it.

## License

MIT. See `LICENSE`.
