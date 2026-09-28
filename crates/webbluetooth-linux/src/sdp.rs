//! Bluetooth Classic SDP queries over the BR/EDR SDP L2CAP PSM.

use crate::sys::{
    close, connect, errno, parse_address, read, socket, SockAddrL2, AF_BLUETOOTH, BTPROTO_L2CAP,
    SOCK_SEQPACKET,
};
use webbluetooth_core::{BluetoothUuid, SdpServiceRecord};

const SDP_PSM: u16 = 0x0001;
const SERVICE_SEARCH_ATTRIBUTE_REQUEST: u8 = 0x06;
const SERVICE_SEARCH_ATTRIBUTE_RESPONSE: u8 = 0x07;

/// Query SDP service records from a Classic peer by service UUID.
///
/// This is Linux-only and requires a Classic address. The query uses SDP's
/// standard ServiceSearchAttribute transaction and follows continuation state
/// responses until the peer completes the result.
pub fn query(address: &str, service: BluetoothUuid) -> Result<Vec<SdpServiceRecord>, String> {
    let peer =
        parse_address(address).ok_or_else(|| format!("{address:?} is not a Bluetooth address"))?;
    let fd = unsafe { socket(AF_BLUETOOTH, SOCK_SEQPACKET, BTPROTO_L2CAP) };
    if fd < 0 {
        return Err(format!("could not create SDP socket (errno {})", errno()));
    }
    let target = SockAddrL2 {
        family: AF_BLUETOOTH as u16,
        psm: SDP_PSM.to_le(),
        bdaddr: peer,
        cid: 0,
        bdaddr_type: 0,
    };
    if unsafe { connect(fd, &target, std::mem::size_of::<SockAddrL2>() as u32) } < 0 {
        unsafe { close(fd) };
        return Err(format!(
            "could not connect to SDP on {address} (errno {})",
            errno()
        ));
    }
    let result = query_connected(fd, service);
    unsafe { close(fd) };
    result
}

fn query_connected(fd: i32, service: BluetoothUuid) -> Result<Vec<SdpServiceRecord>, String> {
    let mut transaction = 1u16;
    let mut continuation = Vec::new();
    let mut records = Vec::new();
    loop {
        let request = request(transaction, service, &continuation);
        let written = unsafe { crate::sys::write(fd, request.as_ptr(), request.len()) };
        if written != request.len() as isize {
            return Err(format!("SDP request failed (errno {})", errno()));
        }
        let mut buffer = vec![0u8; 65535];
        let length = unsafe { read(fd, buffer.as_mut_ptr(), buffer.len()) };
        if length < 0 {
            return Err(format!("SDP response failed (errno {})", errno()));
        }
        let response = &buffer[..length as usize];
        let (payload, next) = response_payload(response, transaction)?;
        records.extend(parse_records(payload)?);
        if next.is_empty() {
            return Ok(records);
        }
        continuation = next;
        transaction = transaction.wrapping_add(1);
    }
}

fn request(transaction: u16, service: BluetoothUuid, continuation: &[u8]) -> Vec<u8> {
    let uuid = uuid_element(service);
    let mut search_pattern = vec![0x35, uuid.len() as u8];
    search_pattern.extend_from_slice(&uuid);
    let attribute_range = [0x35, 0x05, 0x0a, 0x00, 0x00, 0xff, 0xff];
    let mut parameters = search_pattern;
    parameters.extend_from_slice(&attribute_range);
    parameters.push(continuation.len() as u8);
    parameters.extend_from_slice(continuation);
    let mut packet = vec![SERVICE_SEARCH_ATTRIBUTE_REQUEST];
    packet.extend_from_slice(&transaction.to_be_bytes());
    packet.extend_from_slice(&(parameters.len() as u16).to_be_bytes());
    packet.extend_from_slice(&parameters);
    packet
}

fn uuid_element(service: BluetoothUuid) -> Vec<u8> {
    if let Some(value) = service.as_u16() {
        vec![0x19, (value >> 8) as u8, value as u8]
    } else {
        let hex = service.as_str().replace('-', "");
        let mut bytes = Vec::with_capacity(17);
        bytes.push(0x1c);
        for index in (0..32).step_by(2) {
            bytes.push(u8::from_str_radix(&hex[index..index + 2], 16).unwrap_or(0));
        }
        bytes
    }
}

fn response_payload(response: &[u8], transaction: u16) -> Result<(&[u8], Vec<u8>), String> {
    if response.len() < 5 || response[0] != SERVICE_SEARCH_ATTRIBUTE_RESPONSE {
        return Err("invalid SDP response PDU".into());
    }
    if u16::from_be_bytes([response[1], response[2]]) != transaction {
        return Err("unexpected SDP transaction ID".into());
    }
    let parameters = u16::from_be_bytes([response[3], response[4]]) as usize;
    if response.len() < 5 + parameters || parameters < 3 {
        return Err("truncated SDP response".into());
    }
    let byte_count = u16::from_be_bytes([response[5], response[6]]) as usize;
    if parameters < 2 + byte_count + 1 || response.len() < 7 + byte_count {
        return Err("invalid SDP attribute byte count".into());
    }
    let continuation_offset = 7 + byte_count;
    let continuation_length = response[continuation_offset] as usize;
    if response.len() < continuation_offset + 1 + continuation_length {
        return Err("truncated SDP continuation state".into());
    }
    Ok((
        &response[7..7 + byte_count],
        response[continuation_offset + 1..continuation_offset + 1 + continuation_length].to_vec(),
    ))
}

fn parse_records(mut payload: &[u8]) -> Result<Vec<SdpServiceRecord>, String> {
    let mut records = Vec::new();
    while !payload.is_empty() {
        if payload.len() < 2 || payload[0] != 0x35 {
            return Err("invalid SDP record sequence".into());
        }
        let length = match payload[1] {
            n @ 0..=4 => 1usize << n,
            5 => payload.get(2).copied().ok_or("truncated SDP record")? as usize,
            _ => return Err("unsupported SDP record length".into()),
        };
        let header = if payload[1] <= 4 { 2 } else { 3 };
        if payload.len() < header + length {
            return Err("truncated SDP record data".into());
        }
        records.push(
            SdpServiceRecord::parse_attribute_list(&payload[header..header + length])
                .map_err(|e| e.to_string())?,
        );
        payload = &payload[header + length..];
    }
    Ok(records)
}
