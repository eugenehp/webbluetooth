#!/usr/bin/env bash
# Start a private bus, put a mocked BlueZ on it, then run the tests.
set -euo pipefail

echo "── environment ─────────────────────────────────────────"
rustc --version
python3 -c 'import dbusmock; print("python-dbusmock", dbusmock.__version__)'
echo

# A private session bus, so nothing here touches a real system bus.
eval "$(dbus-launch --sh-syntax)"
export DBUS_SESSION_BUS_ADDRESS
export WEBBLUETOOTH_DBUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"
echo "bus: $DBUS_SESSION_BUS_ADDRESS"

cleanup() {
    [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null || true
    [ -n "${DBUS_SESSION_BUS_PID:-}" ] && kill "$DBUS_SESSION_BUS_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Mock org.bluez on that bus, using python-dbusmock's BlueZ 5 template — which
# provides ObjectManager and adapter/device bookkeeping — then build a GATT tree
# on top of it.
python3 -m dbusmock --session --template bluez5 >/tmp/dbusmock.log 2>&1 &
MOCK_PID=$!

# Wait for it to claim the name rather than sleeping a fixed amount.
for _ in $(seq 1 50); do
    if dbus-send --session --dest=org.freedesktop.DBus --print-reply \
         /org/freedesktop/DBus org.freedesktop.DBus.ListNames 2>/dev/null \
         | grep -q '"org.bluez"'; then
        echo "org.bluez mock is up"
        break
    fi
    sleep 0.2
done

if ! python3 /work/docker/mock-bluez.py; then
    echo "!! could not build the mock GATT tree; dbusmock log follows" >&2
    cat /tmp/dbusmock.log >&2
    exit 1
fi

echo
echo "── tests ───────────────────────────────────────────────"
exec "$@"
