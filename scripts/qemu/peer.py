#!/usr/bin/env python3
"""A BLE peripheral on BlueZ, publishing the same fixture as the iOS harness.

Linux has no peripheral role in this crate — BlueZ is the peer here, not the
code under test. What matters is that it publishes exactly what
`examples/ios-harness.rs` publishes, so `examples/roundtrip.rs` runs unchanged
against either and one test covers both platforms.

    sudo python3 peer.py --adapter hci1

Registers a GATT application and an advertisement with `bluetoothd` over D-Bus,
which is how a peripheral is meant to be built on BlueZ. Everything it serves is
answered live, so a read really does reach this process.
"""

import argparse
import dbus
import dbus.exceptions
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

BLUEZ = "org.bluez"
DBUS_OM = "org.freedesktop.DBus.ObjectManager"
DBUS_PROP = "org.freedesktop.DBus.Properties"
GATT_MANAGER = "org.bluez.GattManager1"
GATT_SERVICE = "org.bluez.GattService1"
GATT_CHRC = "org.bluez.GattCharacteristic1"
LE_ADV_MANAGER = "org.bluez.LEAdvertisingManager1"
LE_ADV = "org.bluez.LEAdvertisement1"

# The fixture, identical to the one the iOS harness publishes.
SERVICE = "6e400001-b5a3-f393-e0a9-e50e24dcca9e"
LEVEL = "6e400003-b5a3-f393-e0a9-e50e24dcca9e"
CONTROL = "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
# Device Information, so the blocklist has something real to refuse.
DEVICE_INFO = "0000180a-0000-1000-8000-00805f9b34fb"
SERIAL = "00002a25-0000-1000-8000-00805f9b34fb"
MANUFACTURER = "00002a29-0000-1000-8000-00805f9b34fb"


class InvalidArgs(dbus.exceptions.DBusException):
    _dbus_error_name = "org.freedesktop.DBus.Error.InvalidArgs"


class Application(dbus.service.Object):
    """The object-manager root BlueZ walks to find our services."""

    def __init__(self, bus):
        self.path = "/io/webbluetooth/peer"
        self.services = []
        super().__init__(bus, self.path)

    def get_path(self):
        return dbus.ObjectPath(self.path)

    def add_service(self, service):
        self.services.append(service)

    @dbus.service.method(DBUS_OM, out_signature="a{oa{sa{sv}}}")
    def GetManagedObjects(self):
        response = {}
        for service in self.services:
            response[service.get_path()] = service.properties()
            for chrc in service.characteristics:
                response[chrc.get_path()] = chrc.properties()
        return response


class Service(dbus.service.Object):
    def __init__(self, bus, index, uuid):
        self.path = f"/io/webbluetooth/peer/service{index}"
        self.uuid = uuid
        self.characteristics = []
        self.includes = []
        super().__init__(bus, self.path)

    def include(self, other):
        """Point an Include declaration at another service.

        BlueZ reads `Includes` off an exported service as an optional array of
        object paths, so a peer can offer a composite service — which is the
        only way to test `getIncludedServices` against something real.
        """
        self.includes.append(other)

    def get_path(self):
        return dbus.ObjectPath(self.path)

    def add_characteristic(self, chrc):
        self.characteristics.append(chrc)

    def properties(self):
        properties = {
            "UUID": self.uuid,
            "Primary": dbus.Boolean(True),
            "Characteristics": dbus.Array(
                [c.get_path() for c in self.characteristics], signature="o"
            ),
        }
        if self.includes:
            properties["Includes"] = dbus.Array(
                [s.get_path() for s in self.includes], signature="o"
            )
        return {GATT_SERVICE: properties}


class Characteristic(dbus.service.Object):
    def __init__(self, bus, index, uuid, flags, service, value=None):
        self.path = f"{service.path}/chrc{index}"
        self.uuid = uuid
        self.flags = flags
        self.service = service
        self.value = value if value is not None else []
        self.notifying = False
        super().__init__(bus, self.path)
        service.add_characteristic(self)

    def get_path(self):
        return dbus.ObjectPath(self.path)

    def properties(self):
        return {
            GATT_CHRC: {
                "Service": self.service.get_path(),
                "UUID": self.uuid,
                "Flags": dbus.Array(self.flags, signature="s"),
            }
        }

    @dbus.service.method(DBUS_PROP, in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        if interface != GATT_CHRC:
            raise InvalidArgs()
        return self.properties()[GATT_CHRC]

    @dbus.service.method(GATT_CHRC, in_signature="a{sv}", out_signature="ay")
    def ReadValue(self, options):
        print(f"  read  {self.uuid} -> {bytes(self.value).hex()}", flush=True)
        return dbus.Array(self.value, signature="y")

    @dbus.service.method(GATT_CHRC, in_signature="aya{sv}")
    def WriteValue(self, value, options):
        self.value = [dbus.Byte(b) for b in value]
        print(f"  write {self.uuid} <- {bytes(value).hex()}", flush=True)

    @dbus.service.method(GATT_CHRC)
    def StartNotify(self):
        self.notifying = True
        print(f"  subscribe {self.uuid}", flush=True)

    @dbus.service.method(GATT_CHRC)
    def StopNotify(self):
        self.notifying = False
        print(f"  unsubscribe {self.uuid}", flush=True)

    @dbus.service.signal(DBUS_PROP, signature="sa{sv}as")
    def PropertiesChanged(self, interface, changed, invalidated):
        pass

    def notify(self, value):
        """Push a new value to whoever subscribed."""
        self.value = [dbus.Byte(b) for b in value]
        if not self.notifying:
            return False
        self.PropertiesChanged(
            GATT_CHRC, {"Value": dbus.Array(self.value, signature="y")}, []
        )
        print(f"  notified {value[0]}", flush=True)
        return True


class Advertisement(dbus.service.Object):
    def __init__(self, bus, index, name):
        self.path = f"/io/webbluetooth/adv{index}"
        self.name = name
        super().__init__(bus, self.path)

    def get_path(self):
        return dbus.ObjectPath(self.path)

    @dbus.service.method(DBUS_PROP, in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        if interface != LE_ADV:
            raise InvalidArgs()
        return {
            "Type": "peripheral",
            "ServiceUUIDs": dbus.Array([SERVICE], signature="s"),
            "LocalName": dbus.String(self.name),
        }

    @dbus.service.method(LE_ADV)
    def Release(self):
        # Fires on every re-registration, so it is deliberately quiet.
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--adapter", default="hci1")
    parser.add_argument("--name", default="Rust Linux")
    args = parser.parse_args()

    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SystemBus()
    adapter = f"/org/bluez/{args.adapter}"

    app = Application(bus)

    # Device Information goes first, and not for tidiness: BlueZ builds its
    # database in the order `GetManagedObjects` lists the services, and
    # `gatt_db_service_add_included` returns NULL if the included service is
    # not in the database yet. Declaring the includer first fails with
    # "include service attributes failed" and no include is published.
    info = Service(bus, 0, DEVICE_INFO)
    app.add_service(info)

    fixture = Service(bus, 1, SERVICE)
    app.add_service(fixture)
    # A central then has an Include declaration to discover, not only primary
    # services.
    fixture.include(info)
    level = Characteristic(bus, 0, LEVEL, ["read", "notify"], fixture, [dbus.Byte(87)])
    Characteristic(bus, 1, CONTROL, ["write", "write-without-response"], fixture)
    Characteristic(
        bus, 0, SERIAL, ["read"], info, [dbus.Byte(b) for b in b"WBTEST-0001"]
    )
    Characteristic(
        bus, 1, MANUFACTURER, ["read"], info, [dbus.Byte(b) for b in b"webbluetooth"]
    )

    manager = dbus.Interface(bus.get_object(BLUEZ, adapter), GATT_MANAGER)
    manager.RegisterApplication(
        app.get_path(),
        {},
        reply_handler=lambda: print(f"published {SERVICE}", flush=True),
        error_handler=lambda e: (_ for _ in ()).throw(SystemExit(f"!! {e}")),
    )

    advert = Advertisement(bus, 0, args.name)
    ad_manager = dbus.Interface(bus.get_object(BLUEZ, adapter), LE_ADV_MANAGER)
    ad_manager.RegisterAdvertisement(
        advert.get_path(),
        {},
        reply_handler=lambda: print(
            f'PEER READY — advertising as "{args.name}" on {args.adapter}', flush=True
        ),
        error_handler=lambda e: (_ for _ in ()).throw(SystemExit(f"!! {e}")),
    )

    # Re-register the advertisement every couple of seconds.
    #
    # A real controller repeats its advertisement on an interval. `btvirt` does
    # not: it emits one burst of reports synchronously while it processes an
    # enable command, to whichever controllers are scanning at that instant.
    # When the *central* starts scanning, that burst lands about seven
    # microseconds after the Command Complete for Set Scan Enable and roughly
    # sixty before the kernel marks discovery active, so the kernel drops it and
    # nothing is ever sent again.
    #
    # Toggling the advertisement instead puts the burst at a moment of our
    # choosing — while the central is already discovering — which is what a
    # periodic advertiser would have done anyway.
    # Both calls have to be asynchronous. A blocking call from inside the main
    # loop deadlocks: BlueZ calls `Release` back into this process while we are
    # waiting for its reply, nothing services it, and the call times out with
    # NoReply — leaving the advertisement unregistered and the peer silent.
    def register_again():
        ad_manager.RegisterAdvertisement(
            advert.get_path(),
            {},
            reply_handler=lambda: None,
            error_handler=lambda e: print(f"  re-register failed: {e}", flush=True),
        )

    def readvertise():
        ad_manager.UnregisterAdvertisement(
            advert.get_path(),
            reply_handler=register_again,
            error_handler=lambda e: register_again(),
        )
        return True

    GLib.timeout_add_seconds(2, readvertise)

    # Drain a percent every few seconds, so a subscriber sees live traffic
    # rather than a replay of the last read.
    def tick():
        nxt = level.value[0] - 1
        level.notify([nxt if nxt > 0 else 100])
        return True

    GLib.timeout_add_seconds(5, tick)
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
