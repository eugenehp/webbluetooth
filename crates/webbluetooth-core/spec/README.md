# Vendored oracles

Nothing in here is written by hand. Each file is fetched from something
authoritative, vendored so the build is reproducible and offline, and checked
by a test that fails when it drifts.

| file | from | answers |
|---|---|---|
| `gatt-blocklist.txt` | the Web Bluetooth CG registry | which UUIDs must never be handed out |
| `manufacturer-data-blocklist.txt` | the Web Bluetooth CG registry | which advertised manufacturer data must never be reported |
| `web-bluetooth-surface.txt` | the specification's IDL, via `w3c/webref` | what the standard defines |
| `web-bluetooth-status.txt` | MDN's `browser-compat-data` | whether a member is deprecated or experimental, and whether anything ships it |
| `gatt-assigned-services.txt` | the Web Bluetooth CG registry | what `BluetoothUUID.getService("heart_rate")` resolves to |
| `gatt-assigned-characteristics.txt` | the Web Bluetooth CG registry | the same, for characteristics |
| `gatt-assigned-descriptors.txt` | the Web Bluetooth CG registry | the same, for descriptors |

Checked by `crates/webbluetooth/tests/web_bluetooth_surface.rs`,
`crates/webbluetooth-core/src/blocklist.rs` and
`crates/webbluetooth-core/src/uuid.rs`.

## A note on the assigned numbers

These three exist because the table in `uuid.rs` was written by hand, and hand
is exactly how the blocklist ended up with three wrong entries.

There are 268 name-to-UUID pairs in the registry. The crate had 79 of them, so
two thirds of the names the standard says resolve did not — and any one of the
79 could have been a digit out, which does not fail, it resolves to the wrong
attribute and reads the wrong characteristic.

So the constants stayed, because `services::HEART_RATE` is a `u16` you can
match on and a string lookup is not. But they are no longer the only thing that
resolves, and a test now checks every one of them against the vendored file. A
name only the registry knows resolves through the registry; a constant that
disagrees with it fails the build.

## A note on the manufacturer data blocklist

The specification prints a parsing algorithm for this file, and its regular
expression does not match the file:

```
manufacturer\ ([0-9a-f]+)\ ([0-9a-f]+\/[0-9a-f]+)     the spec's expression
manufacturer 4c advdata-02/ff                         the registry's only entry
```

The second group requires the prefix to start with hexadecimal digits, but
every entry writes it with a literal `advdata-` tag — which is the format the
registries repository documents, and which the step this algorithm then
delegates to ("parse an advertising data filter") expects already stripped.
`exec` returns `null`, and the algorithm returns an error.

That is not a harmless bug, because an unreadable list blocks *everything*: no
manufacturer data would reach any caller on any platform, and every
`manufacturerData` filter would throw. Since that is plainly not the intent,
the parser here follows the documented format — the `advdata-` tag, then the
spec's own `parse an advertising data filter` on what follows it.

The failure direction is kept either way: an entry that does not parse blocks
rather than opens, and a test pins that.

## Why two oracles for one API

They answer different questions and neither contains the other.

The **IDL** is the standard, so it is the measure of parity: 106 members,
including everything defined but not yet shipped by anyone. It carries no
notion of status — the word "deprecated" does not appear in it once.

**MDN** records what browsers have actually shipped: 49 members, and for each
one whether it is deprecated or experimental. It is much narrower, so it can
never establish completeness.

Replacing MDN with the IDL cost something that was not noticed until the gap
was pointed out: the exclusion of `writeValue` says the specification
deprecates it, and with only the IDL vendored there was nothing in the
repository that could have contradicted that claim. It happens to be true —
MDN marks it `deprecated` — but it had been written from memory, which is the
position the blocklist and the JNI slot table were in when they turned out to
be wrong.

The test now requires any exclusion claiming deprecation to be corroborated by
MDN, and requires the two oracles to agree wherever they overlap.

Refresh with `./scripts/update.sh <name>` and **read the diff**. Both of these
have been wrong before from being written out of memory: three blocklist
entries and, separately, half the assumptions about the API surface.

Two more oracles are not vendored here because they come with their tooling
rather than over the network — `jni.h` from whichever JDK is installed, which
`webbluetooth-android`'s `tests/slots.rs` parses, and Microsoft's metadata,
which `scripts/update.sh winrt-iids` turns into
`crates/webbluetooth-windows/src/iids.rs`.
