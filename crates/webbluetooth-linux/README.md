# webbluetooth-linux

Linux Bluetooth from pure Rust, and the two adapters that turn it into
[`webbluetooth`](https://crates.io/crates/webbluetooth)'s portable model.

No `libdbus`, no bindgen. D-Bus is a wire protocol over a Unix socket, so it is
spoken directly: the codec, the object manager, and the BlueZ interfaces, all
hand-written. Descriptors arriving over `SCM_RIGHTS` need `recvmsg` and a
control buffer from the first byte, because a plain `read()` does not ignore
them, it destroys them.

Two adapters, both compiled on every Linux build so neither can drift:

* `backend` — BlueZ over D-Bus, and the peripheral role with it.
* `backend_hci` — no `bluetoothd` at all: ATT over an L2CAP socket for GATT, a
  raw HCI socket for scanning. Selected by `webbluetooth`'s `linux-hci`
  feature. Pairing and bonding live in the daemon, so encrypted
  characteristics are out of reach without it.

**You want `webbluetooth` instead**, unless you are working on the backend.

## On the documentation here

Every type and module is documented. Individual fields, variants and constants
often are not: they mirror the platform API this wraps, name for name, and that
API's own reference is the better answer than a line repeating the name. The
adapter module — the half that presents `webbluetooth-core`'s model — is
`#[doc(hidden)]`, because it is public only so `webbluetooth` can reach it.

## License

MIT. See `LICENSE`.
