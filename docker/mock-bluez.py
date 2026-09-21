#!/usr/bin/env python3
"""Build a fake BlueZ device with a real GATT tree, using python-dbusmock.

No container has a Bluetooth controller, so the Linux backend is exercised
against the exact D-Bus interfaces bluetoothd would publish. The mock serves
org.bluez; everything the crate does — GetManagedObjects, discovery filters,
Connect, ReadValue, WriteValue, StartNotify, PropertiesChanged — is real
traffic over a real bus.

Publishes a Heart Rate device:

    /org/bluez/hci0                                     Adapter1
    /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF               Device1
      …/service0010                  180d                GattService1
        …/char0011                   2a37 notify,read    GattCharacteristic1
          …/desc0012                 2902                GattDescriptor1
        …/char0013                   2a38 read           GattCharacteristic1
        …/char0014                   2a39 write          GattCharacteristic1
"""
import sys
import dbus

BLUEZ = "org.bluez"
# Generic object manipulation lives on the standard mock interface…
MOCK = "org.freedesktop.DBus.Mock"
# …while the bluez5 template's own helpers live on its own.
TEMPLATE = "org.bluez.Mock"
ADAPTER = "/org/bluez/hci0"
DEVICE = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
SERVICE = DEVICE + "/service0010"
MEASUREMENT = SERVICE + "/char0011"
CCCD = MEASUREMENT + "/desc0012"
LOCATION = SERVICE + "/char0013"
CONTROL = SERVICE + "/char0014"

HEART_RATE = "0000180d-0000-1000-8000-00805f9b34fb"
BATTERY = "0000180f-0000-1000-8000-00805f9b34fb"


def main():
    bus = dbus.SessionBus()
    root = dbus.Interface(bus.get_object(BLUEZ, "/"), MOCK)
    template = dbus.Interface(bus.get_object(BLUEZ, "/"), TEMPLATE)

    # The template gives us ObjectManager plus adapter/device bookkeeping.
    template.AddAdapter("hci0", "testhost")
    template.AddDevice("hci0", "AA:BB:CC:DD:EE:FF", "Mock Heart Rate")

    device = dbus.Interface(bus.get_object(BLUEZ, DEVICE), MOCK)
    device_props = dbus.Interface(bus.get_object(BLUEZ, DEVICE), dbus.PROPERTIES_IFACE)

    # Properties a scan needs to match on.
    #
    # ManufacturerData and ServiceData are deliberately left alone: the bluez5
    # template already creates them as empty arrays, dbusmock has no way to
    # replace an existing property, and `UpdateProperties` cannot carry a
    # `dbus.Dictionary` anyway — it is in dbusmock's scalar type list and gets
    # `.conjugate()` called on it. Filter matching over both is unit-tested in
    # `filter.rs` instead, where it needs no bus at all.
    device.UpdateProperties(
        "org.bluez.Device1",
        {
            "UUIDs": dbus.Array([HEART_RATE, BATTERY], signature="s"),
            "RSSI": dbus.Int16(-61),
            "TxPower": dbus.Int16(4),
            "AddressType": "public",
            "ServicesResolved": dbus.Boolean(False),
            "Connected": dbus.Boolean(False),
        },
    )

    # Connecting must flip Connected *and* ServicesResolved, and signal both —
    # the crate waits for the second before walking the GATT tree.
    device.AddMethod(
        "org.bluez.Device1",
        "Connect",
        "",
        "",
        'self.UpdateProperties("org.bluez.Device1", {'
        '  "Connected": dbus.Boolean(True),'
        '  "ServicesResolved": dbus.Boolean(True) })',
    )
    device.AddMethod(
        "org.bluez.Device1",
        "Disconnect",
        "",
        "",
        'self.UpdateProperties("org.bluez.Device1", {'
        '  "Connected": dbus.Boolean(False),'
        '  "ServicesResolved": dbus.Boolean(False) })',
    )

    # ── GATT tree ───────────────────────────────────────────────────────────
    root.AddObject(
        SERVICE,
        "org.bluez.GattService1",
        {"UUID": HEART_RATE, "Device": dbus.ObjectPath(DEVICE), "Primary": dbus.Boolean(True)},
        [],
    )

    # Heart Rate Measurement: notify + read, so both paths are covered.
    root.AddObject(
        MEASUREMENT,
        "org.bluez.GattCharacteristic1",
        {
            "UUID": "00002a37-0000-1000-8000-00805f9b34fb",
            "Service": dbus.ObjectPath(SERVICE),
            "Flags": dbus.Array(["read", "notify"], signature="s", variant_level=1),
            "Notifying": dbus.Boolean(False),
            "MTU": dbus.UInt16(185),
            "Value": dbus.Array([dbus.Byte(0x00), dbus.Byte(72)], signature="y", variant_level=1),
        },
        [],
    )
    measurement = dbus.Interface(bus.get_object(BLUEZ, MEASUREMENT), MOCK)
    measurement.AddMethod(
        "org.bluez.GattCharacteristic1", "ReadValue", "a{sv}", "ay", "ret = [0x00, 72]"
    )
    # StartNotify flips Notifying and pushes one value, so a subscriber sees
    # traffic without needing a timer in the mock.
    measurement.AddMethod(
        "org.bluez.GattCharacteristic1",
        "StartNotify",
        "",
        "",
        'self.UpdateProperties("org.bluez.GattCharacteristic1", {'
        '  "Notifying": dbus.Boolean(True),'
        '  "Value": dbus.Array([dbus.Byte(0x00), dbus.Byte(77)], signature="y") })',
    )
    measurement.AddMethod(
        "org.bluez.GattCharacteristic1",
        "StopNotify",
        "",
        "",
        'self.UpdateProperties("org.bluez.GattCharacteristic1", {'
        '  "Notifying": dbus.Boolean(False) })',
    )

    root.AddObject(
        CCCD,
        "org.bluez.GattDescriptor1",
        {
            "UUID": "00002902-0000-1000-8000-00805f9b34fb",
            "Characteristic": dbus.ObjectPath(MEASUREMENT),
            "Value": dbus.Array([dbus.Byte(0), dbus.Byte(0)], signature="y", variant_level=1),
        },
        [],
    )
    cccd = dbus.Interface(bus.get_object(BLUEZ, CCCD), MOCK)
    cccd.AddMethod("org.bluez.GattDescriptor1", "ReadValue", "a{sv}", "ay", "ret = [0x00, 0x00]")

    # Body Sensor Location: read-only, for a second characteristic.
    root.AddObject(
        LOCATION,
        "org.bluez.GattCharacteristic1",
        {
            "UUID": "00002a38-0000-1000-8000-00805f9b34fb",
            "Service": dbus.ObjectPath(SERVICE),
            "Flags": dbus.Array(["read"], signature="s", variant_level=1),
            "Value": dbus.Array([dbus.Byte(0x02)], signature="y", variant_level=1),
        },
        [],
    )
    dbus.Interface(bus.get_object(BLUEZ, LOCATION), MOCK).AddMethod(
        "org.bluez.GattCharacteristic1", "ReadValue", "a{sv}", "ay", "ret = [0x02]"
    )

    # Heart Rate Control Point: write-only, to prove writes reach the peer and
    # that reading one refuses.
    root.AddObject(
        CONTROL,
        "org.bluez.GattCharacteristic1",
        {
            "UUID": "00002a39-0000-1000-8000-00805f9b34fb",
            "Service": dbus.ObjectPath(SERVICE),
            "Flags": dbus.Array(["write", "write-without-response"], signature="s", variant_level=1),
            "Value": dbus.Array([], signature="y", variant_level=1),
        },
        [],
    )
    dbus.Interface(bus.get_object(BLUEZ, CONTROL), MOCK).AddMethod(
        "org.bluez.GattCharacteristic1",
        "WriteValue",
        "aya{sv}",
        "",
        'self.UpdateProperties("org.bluez.GattCharacteristic1", {'
        '  "Value": dbus.Array(args[0], signature="y") })',
    )

    print("mock BlueZ ready: adapter hci0, device AA:BB:CC:DD:EE:FF, 3 characteristics")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # noqa: BLE001 — the harness wants the reason, not a trace
        print(f"mock setup failed: {e}", file=sys.stderr)
        sys.exit(1)
