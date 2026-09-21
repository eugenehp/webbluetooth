#!/usr/bin/env python3
"""Turn windows-rs source into vendored IID constants.

Three things are extracted:

  interfaces  `define_interface!(IFoo, _, 0x…)` — an interface's own IID.
  classes     a runtime class's full name and default interface, which together
              make up its signature string.
  generics    the GUID of a parameterised type, taken from the `pinterface(...)`
              signature its bindings build.
"""
import re
import sys
from pathlib import Path


def screaming(name: str) -> str:
    """IGattCharacteristic3 -> IGATT_CHARACTERISTIC3"""
    out = re.sub(r'(?<=[a-z0-9])(?=[A-Z])', '_', name)
    out = re.sub(r'(?<=[A-Z])(?=[A-Z][a-z])', '_', out)
    return out.upper()


def hex_to_guid(h: str) -> str:
    """0xb5ee2f7b_4ad8_4642_ac48_80a0b500e887 -> canonical form"""
    return h.removeprefix('0x').replace('_', '-')


# The interfaces the backend actually calls. Generating slots for all 311 would
# be thousands of dead constants; this list is the contract, and a name missing
# from it fails the build rather than silently producing a wrong index.
WANTED_VTABLES = {
    "IBluetoothLEDevice", "IBluetoothLEDevice2", "IBluetoothLEDevice3",
    "IBluetoothLEDevice4", "IBluetoothLEDevice5", "IBluetoothLEDevice6",
    "IBluetoothLEDeviceStatics", "IBluetoothLEDeviceStatics2",
    # Connection parameters: reading what was agreed, and asking for something
    # else. `RequestPreferredConnectionParameters` lives on a later interface
    # than the one that carries the rest of the device.
    "IBluetoothLEConnectionParameters",
    "IBluetoothLEPreferredConnectionParameters",
    "IBluetoothLEPreferredConnectionParametersStatics",
    "IBluetoothLEPreferredConnectionParametersRequest",
    "IGattDeviceService", "IGattDeviceService3", "IGattDeviceServicesResult",
    "IGattCharacteristic", "IGattCharacteristic3", "IGattCharacteristicsResult",
    "IGattDescriptor", "IGattDescriptorsResult",
    "IGattReadResult", "IGattWriteResult", "IGattValueChangedEventArgs",
    "IGattSession", "IGattSessionStatics",
    "IGattServiceProvider", "IGattServiceProviderStatics",
    "IGattServiceProviderResult", "IGattServiceProviderAdvertisingParameters",
    "IGattLocalService", "IGattLocalCharacteristic", "IGattLocalCharacteristicResult",
    "IGattLocalCharacteristicParameters", "IGattLocalDescriptorParameters",
    "IGattReadRequest", "IGattReadRequestedEventArgs",
    "IGattWriteRequest", "IGattWriteRequestedEventArgs",
    "IGattSubscribedClient",
    "IBluetoothLEAdvertisementWatcher", "IBluetoothLEAdvertisementReceivedEventArgs",
    "IBluetoothLEAdvertisement", "IBluetoothLEManufacturerData",
    "IBluetoothLEAdvertisementPublisher", "IBluetoothLEAdvertisementPublisherStatusChangedEventArgs",
    # Buffers. GATT values are IBuffer, and DataReader/DataWriter convert them
    # without needing the IBufferByteAccess COM escape hatch.
    "IDataReader", "IDataWriter", "IDataReaderStatics", "IBuffer",
    # Discovery, for resolving a device that is already paired.
    "IDeviceInformation", "IDeviceInformationStatics",
    # The controllers themselves. A machine can have more than one radio, and
    # `GetDeviceSelector` plus `DeviceInformation` is how WinRT enumerates
    # them; `FromIdAsync` is how one is then opened by name.
    "IBluetoothAdapter", "IBluetoothAdapterStatics",
    # Pairing. `DeviceInformation.Pairing` is the handle; `PairAsync` runs the
    # ceremony and `IsPaired` says whether one is needed.
    "IDeviceInformation2",
    # The connection PHY: which of 1M, 2M or coded a link is running at.
    "IBluetoothLEConnectionPhy", "IBluetoothLEConnectionPhyInfo",
    "IDeviceInformationPairing", "IDeviceInformationPairingStatics",
    "IDeviceInformationPairing2", "IDevicePairingResult",
    # Async and collections. Parameterised, so their slot order comes from the
    # generic's own definition and is the same for every instantiation.
    "IAsyncOperation", "IAsyncInfo", "IVectorView", "IVector", "IIterable", "IIterator",
    # Activation, for the classes that have a constructor.
    "IActivationFactory", "IClosable",
    # A read or write request answered after the handler returns needs a
    # deferral held open across it.
    "IDeferral", "IGattClientNotificationResult",
}

# IUnknown is three slots and IInspectable three more, so an interface's own
# methods begin at six. `base__` in the generated vtable stands for all six.
FIRST_METHOD = 6


def vtable_slots(src: str) -> dict[str, list[str]]:
    """Method order for each `IFoo_Vtbl`, which is the slot order."""
    out: dict[str, list[str]] = {}
    # A parameterised vtable is generic and may carry a `where` clause across
    # several lines before the brace.
    pattern = r'pub struct (\w+)_Vtbl(?:<[^>]*>)?\s*(?:where\s+[^{]*?)?\{(.*?)\n\}'
    for m in re.finditer(pattern, src, re.S):
        name, body = m.group(1), m.group(2)
        if name not in WANTED_VTABLES:
            continue
        fields = re.findall(r'\n\s+pub (\w+):', body)
        # Drop `base__`; its six slots are accounted for by FIRST_METHOD.
        out[name] = [f for f in fields if f != "base__"]
    return out


def main() -> None:
    work = Path(sys.argv[1])

    interfaces: dict[str, str] = {}
    classes: dict[str, tuple[str, str]] = {}
    generics: dict[str, str] = {}
    vtables: dict[str, list[str]] = {}

    for path in sorted(work.glob('*.rs')):
        src = path.read_text()

        for m in re.finditer(r'define_interface!\((\w+),\s*\w+,\s*(0x[0-9a-f_]+)\)', src):
            interfaces[m.group(1)] = hex_to_guid(m.group(2))

        vtables.update(vtable_slots(src))

        # `impl RuntimeName for X { const NAME: &str = "Full.Name" }`
        names = dict(
            re.findall(r'RuntimeName for (\w+)\s*\{\s*const NAME: &\'static str = "([^"]+)"', src)
        )
        # `ConstBuffer::for_class::<Self, IFoo>()` inside `RuntimeType for X`
        for m in re.finditer(
            r'RuntimeType for (\w+)\s*\{[^}]*?for_class::<Self, (\w+)>', src, re.S
        ):
            cls, default = m.group(1), m.group(2)
            if cls in names:
                classes[cls] = (names[cls], default)

        # A parameterised type's own GUID, from the signature it builds.
        for m in re.finditer(r'pinterface\(\{([0-9a-f-]{36})\}', src):
            owner = None
            for im in re.finditer(r'\bfor\s+(\w+)\s*<', src[: m.start()]):
                owner = im.group(1)
            if owner:
                generics.setdefault(owner, m.group(1))

    out = [
        '//! WinRT interface identifiers, vendored from Windows metadata.',
        '//!',
        '//! **Generated — do not edit.** Run `scripts/update.sh winrt-iids`.',
        '//!',
        '//! An IID is not derivable from a name and not worth transcribing by hand, so',
        '//! these are extracted from `microsoft/windows-rs`, which is itself generated',
        '//! from the Windows SDK metadata. The parameterised types in [`generics`] carry',
        "//! the GUID of the *generic*, which [`crate::guid::Signature`] hashes together",
        '//! with its arguments to get the real IID.',
        '',
        'use crate::guid::Guid;',
        '',
        '/// Interfaces, by their metadata name.',
        'pub mod interfaces {',
        '    use super::Guid;',
    ]
    for name, guid in sorted(interfaces.items()):
        out.append(f'    pub const {screaming(name)}: Guid = Guid::parse("{guid}");')
    out.append('}')
    out.append('')

    out += [
        '/// Runtime classes: their full name, and the default interface a signature',
        '/// string names alongside it.',
        'pub mod classes {',
        '    use super::Guid;',
        '',
        '    /// `(full name, default interface)` — everything `rc(...)` needs.',
        '    pub type Class = (&\'static str, Guid);',
        '',
    ]
    for name, (full, default) in sorted(classes.items()):
        if default not in interfaces:
            continue
        out.append(
            f'    pub const {screaming(name)}: Class = '
            f'("{full}", Guid::parse("{interfaces[default]}"));'
        )
    out.append('}')
    out.append('')

    out += [
        '/// Vtable slot indices, in the order the metadata declares the methods.',
        '///',
        '/// A method called one slot out jumps into a different function with the',
        '/// wrong arguments, and nothing here would catch it — so these are',
        '/// generated from the same metadata as the IIDs rather than counted by',
        '/// hand.',
        'pub mod slots {',
    ]
    for name, methods in sorted(vtables.items()):
        module = re.sub(r'(?<=[a-z0-9])(?=[A-Z])', '_', name).lower()
        out.append(f'    /// `{name}`')
        out.append(f'    pub mod {module} {{')
        for i, method in enumerate(methods):
            out.append(f'        pub const {screaming(method)}: usize = {FIRST_METHOD + i};')
        out.append('    }')
    out.append('}')
    out.append('')

    out += [
        '/// Parameterised types, by the GUID of the generic itself.',
        'pub mod generics {',
        '    use super::Guid;',
    ]
    for name, guid in sorted(generics.items()):
        out.append(f'    pub const {screaming(name)}: Guid = Guid::parse("{guid}");')
    out.append('}')
    out.append('')

    print('\n'.join(out))


if __name__ == '__main__':
    main()
