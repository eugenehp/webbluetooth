#!/usr/bin/env bash
# Regenerate crates/webbluetooth-windows/src/iids.rs from Microsoft's metadata.
#
# WinRT interface IIDs are not guessable and not worth transcribing by hand, so
# they are extracted from microsoft/windows-rs — which is itself generated from
# the Windows metadata — and vendored. Re-run after a Windows SDK bump and read
# the diff; IIDs do not change, but new interfaces appear.
set -euo pipefail
cd "$(dirname "$0")/.."

BASE="https://raw.githubusercontent.com/microsoft/windows-rs/master/crates/libs"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fetch() {
    local path="$1" out="$2"
    if ! curl -sSL --fail --max-time 60 "$BASE/$path" -o "$WORK/$out"; then
        echo "could not fetch $path" >&2
        exit 1
    fi
}

fetch "windows/src/Windows/Devices/Bluetooth/mod.rs"                          bluetooth.rs
fetch "windows/src/Windows/Devices/Bluetooth/GenericAttributeProfile/mod.rs"  gatt.rs
fetch "windows/src/Windows/Devices/Bluetooth/Advertisement/mod.rs"            advertisement.rs
fetch "windows/src/Windows/Devices/Enumeration/mod.rs"                        enumeration.rs
fetch "windows/src/Windows/Storage/Streams/mod.rs"                            streams.rs
fetch "windows/src/Windows/Foundation/mod.rs"                                 foundation.rs
fetch "future/src/bindings.rs"                                                future.rs
fetch "collections/src/bindings.rs"                                           collections.rs

python3 scripts/winrt-iids.py "$WORK" > crates/webbluetooth-windows/src/iids.rs
echo "wrote crates/webbluetooth-windows/src/iids.rs"
echo "  interfaces: $(grep -c 'pub const' crates/webbluetooth-windows/src/iids.rs)"
cargo fmt -p webbluetooth-windows 2>/dev/null || true
