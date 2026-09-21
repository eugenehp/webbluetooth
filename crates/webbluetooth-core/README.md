# webbluetooth-core

The portable half of [`webbluetooth`](https://crates.io/crates/webbluetooth),
and the vocabulary its six backends share.

Everything here is platform-independent: the error type the specification
defines, the UUID registry, the scan filters, both of the Web Bluetooth CG's
blocklists, the grant store, the shared scan hub, the L2CAP channel wrapper.
None of it knows whether it is talking to CoreBluetooth, BlueZ, WinRT, the
Android framework, a raw HCI socket or the browser's own Web Bluetooth.

It exists as a crate rather than a module so that a backend in
`webbluetooth-apple` and a backend in `webbluetooth-windows` can share a
vocabulary without either being able to reach the public API above them. That
direction matters: three backends used to reach upwards, and anything two
backends both needed had nowhere to live.

**You almost certainly want `webbluetooth` instead.** Everything here is public
because a backend in another crate has to name it, not because it is an API to
build on.

## The vendored oracles

`spec/` holds files fetched from something authoritative and checked by tests
that fail when they drift: the two CG blocklists, the assigned-number
registries, the specification's own IDL, and MDN's compatibility data. Refresh
them with `./scripts/update.sh` from the repository and **read the diff** —
this table has been wrong before from being written out of memory.

## License

MIT. See `LICENSE`.
