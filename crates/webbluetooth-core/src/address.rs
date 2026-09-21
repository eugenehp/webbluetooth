//! Telling a Bluetooth address from a platform-minted device identifier.

/// Whether an identifier is a Bluetooth address rather than a platform UUID.
///
/// `AA:BB:CC:DD:EE:FF` — six hex pairs, colon-separated. Apple's identifiers
/// are UUIDs and fail this, which is exactly the distinction
/// `BluetoothDevice::address` needs.
pub fn is_bluetooth_address(id: &str) -> bool {
    let mut parts = 0;
    for part in id.split(':') {
        parts += 1;
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
    }
    parts == 6
}
