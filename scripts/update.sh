#!/usr/bin/env bash
# Refresh a vendored oracle, and read the diff.
#
#   ./scripts/update.sh blocklist     both CG blocklists, GATT and manufacturer
#   ./scripts/update.sh surface       the Web Bluetooth API surface, from MDN
#   ./scripts/update.sh winrt-iids    WinRT IIDs and vtable slots, from Microsoft
#   ./scripts/update.sh all
#
# Each of these replaces something that would otherwise be written from memory,
# which is how the blocklist ended up with three wrong entries and the JNI
# table with four wrong slots. See crates/webbluetooth-core/spec/README.md.
set -euo pipefail
cd "$(dirname "$0")/.."

say() { printf '\n\033[1m── %s ──\033[0m\n' "$*"; }

# ── The GATT blocklist ──────────────────────────────────────────────────────
#
# A moving target: UUIDs are added when a new class of device turns out to be
# exploitable. A snapshot, not a constant.
blocklist() {
    local url="https://raw.githubusercontent.com/WebBluetoothCG/registries/master/gatt_blocklist.txt"
    local dest="crates/webbluetooth-core/spec/gatt-blocklist.txt"
    curl -sSL --fail --max-time 30 "$url" -o "$dest.new"
    # A fetch that returned an error page would otherwise install silently and
    # quietly empty the blocklist, which fails open.
    if ! grep -q '^00001812-0000-1000-8000-00805f9b34fb$' "$dest.new"; then
        rm -f "$dest.new"
        echo "refusing to install: no HID service entry, the fetch looks wrong" >&2
        return 1
    fi
    mv "$dest.new" "$dest"
    echo "wrote $dest ($(grep -c . "$dest") lines)"

    # The second registry. Small today — one iBeacon entry — but it is the
    # only thing standing between a page and a device's proximity beacon, and
    # it is the list most likely to grow.
    url="https://raw.githubusercontent.com/WebBluetoothCG/registries/master/manufacturer_data_blocklist.txt"
    dest="crates/webbluetooth-core/spec/manufacturer-data-blocklist.txt"
    curl -sSL --fail --max-time 30 "$url" -o "$dest.new"
    if ! grep -q '^manufacturer ' "$dest.new"; then
        rm -f "$dest.new"
        echo "refusing to install: no manufacturer entry, the fetch looks wrong" >&2
        return 1
    fi
    mv "$dest.new" "$dest"
    echo "wrote $dest ($(grep -c . "$dest") lines)"

    # ── Assigned numbers ────────────────────────────────────────────────────
    #
    # The names `BluetoothUUID.getService("heart_rate")` resolves. Vendored for
    # the same reason as the blocklist: 268 name-to-UUID pairs is 268 chances
    # to transcribe one wrong, and a wrong one resolves silently to the wrong
    # attribute.
    for kind in services characteristics descriptors; do
        url="https://raw.githubusercontent.com/WebBluetoothCG/registries/master/gatt_assigned_$kind.txt"
        dest="crates/webbluetooth-core/spec/gatt-assigned-$kind.txt"
        curl -sSL --fail --max-time 30 "$url" -o "$dest.new"
        # Every line is `name UUID`; an error page has neither.
        if ! grep -qE '^[a-z0-9_.]+ [0-9a-fA-F-]{36}$' "$dest.new"; then
            rm -f "$dest.new"
            echo "refusing to install $dest: no name/UUID pairs, the fetch looks wrong" >&2
            return 1
        fi
        mv "$dest.new" "$dest"
        echo "wrote $dest ($(grep -cE '^[a-z0-9_.]+ ' "$dest") entries)"
    done
}

# ── The Web Bluetooth surface ───────────────────────────────────────────────
#
# Two oracles, because they answer two questions and neither subsumes the
# other:
#
#   the IDL  — what the standard defines. Complete, and the measure of parity.
#   MDN      — whether a member is deprecated or experimental, and whether
#              anything ships it. The IDL carries none of that; it does not
#              contain the word "deprecated" at all.
surface() {
    python3 scripts/spec-surface.py
    python3 scripts/mdn-status.py
}

# ── WinRT IIDs and vtable slots ─────────────────────────────────────────────
winrt_iids() {
    bash scripts/winrt-iids.sh
}

case "${1:-}" in
    blocklist)  say "CG blocklists";        blocklist ;;
    surface)    say "Web Bluetooth surface"; surface ;;
    winrt-iids) say "WinRT identifiers";     winrt_iids ;;
    all)        say "CG blocklists";        blocklist
                say "Web Bluetooth surface"; surface
                say "WinRT identifiers";     winrt_iids ;;
    *) echo "usage: $0 [blocklist|surface|winrt-iids|all]" >&2; exit 2 ;;
esac
