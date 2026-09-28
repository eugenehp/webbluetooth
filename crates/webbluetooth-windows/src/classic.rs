//! Windows Bluetooth Classic RFCOMM over Winsock `AF_BTH`.

#![cfg(windows)]

use futures_channel::mpsc;
use futures_core::Stream;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::windows::io::{FromRawSocket, RawSocket};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use webbluetooth_core::l2cap::{ChannelSink, Closed};

const AF_BTH: i32 = 32;
const BTPROTO_RFCOMM: i32 = 3;

#[repr(C)]
struct SockAddrBth {
    address_family: u16,
    bt_addr: u64,
    service_class_id: windows_sys::core::GUID,
    port: u32,
}

/// An open Windows RFCOMM byte stream.
pub struct Channel {
    socket: Arc<std::net::TcpStream>,
    channel: u8,
    peer: String,
    closed: Mutex<Option<Closed>>,
    stopped: Arc<AtomicBool>,
    writing: Mutex<()>,
}

impl Channel {
    /// Connect to a Bluetooth address and RFCOMM server channel.
    pub fn connect(address: &str, channel: u8, sink: Arc<dyn ChannelSink>) -> Result<Self, String> {
        Self::connect_inner(address, channel, unsafe { std::mem::zeroed() }, sink)
    }

    /// Connect to a Classic profile by its SDP service UUID.
    pub fn connect_service(
        address: &str,
        service_uuid: &str,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        Self::connect_inner(address, 0, parse_guid(service_uuid)?, sink)
    }

    fn connect_inner(
        address: &str,
        channel: u8,
        service_class_id: windows_sys::core::GUID,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        let bt_addr = parse_address(address)?;
        let has_service_uuid = service_class_id.data1 != 0
            || service_class_id.data2 != 0
            || service_class_id.data3 != 0
            || service_class_id.data4 != [0; 8];
        if !has_service_uuid && !(1..=30).contains(&channel) {
            return Err("RFCOMM channel must be between 1 and 30".into());
        }
        let raw = unsafe {
            windows_sys::Win32::Networking::WinSock::socket(
                AF_BTH,
                windows_sys::Win32::Networking::WinSock::SOCK_STREAM,
                BTPROTO_RFCOMM,
            )
        };
        if raw == windows_sys::Win32::Networking::WinSock::INVALID_SOCKET {
            return Err(format!("could not create RFCOMM socket: {}", last_error()));
        }
        let socket = unsafe { std::net::TcpStream::from_raw_socket(raw as RawSocket) };
        let address = SockAddrBth {
            address_family: AF_BTH as u16,
            bt_addr,
            service_class_id,
            port: channel as u32,
        };
        let connected = unsafe {
            windows_sys::Win32::Networking::WinSock::connect(
                raw,
                (&raw const address).cast(),
                std::mem::size_of_val(&address) as i32,
            )
        };
        if connected == windows_sys::Win32::Networking::WinSock::SOCKET_ERROR {
            return Err(format!("could not connect RFCOMM socket: {}", last_error()));
        }
        let socket = Arc::new(socket);
        let stopped = Arc::new(AtomicBool::new(false));
        spawn_reader(socket.clone(), sink, stopped.clone(), channel);
        Ok(Self {
            socket,
            channel,
            peer: address_to_string(bt_addr),
            closed: Mutex::new(None),
            stopped,
            writing: Mutex::new(()),
        })
    }

    pub fn channel(&self) -> u8 {
        self.channel
    }
    pub fn peer_id(&self) -> &str {
        &self.peer
    }
    pub fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        let _guard = self.writing.lock().unwrap();
        if let Some(reason) = self.closed() {
            return Err(reason);
        }
        let mut socket = &*self.socket;
        socket.write_all(bytes).map_err(|error| {
            let reason = Closed::Error(error.to_string());
            *self.closed.lock().unwrap() = Some(reason.clone());
            reason
        })
    }
    pub fn pending_bytes(&self) -> usize {
        0
    }
    pub fn closed(&self) -> Option<Closed> {
        self.closed.lock().unwrap().clone()
    }
    pub fn close(&self) {
        if self.closed().is_none() {
            *self.closed.lock().unwrap() = Some(Closed::Locally);
        }
        self.stopped.store(true, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.close();
    }
}

fn spawn_reader(
    socket: Arc<std::net::TcpStream>,
    sink: Arc<dyn ChannelSink>,
    stopped: Arc<AtomicBool>,
    channel: u8,
) {
    std::thread::Builder::new()
        .name(format!("webbluetooth-windows-rfcomm-{channel}"))
        .spawn(move || {
            let mut socket = &*socket;
            let mut bytes = [0; 4096];
            let reason = loop {
                if stopped.load(Ordering::Acquire) {
                    break Closed::Locally;
                }
                match socket.read(&mut bytes) {
                    Ok(0) => break Closed::ByPeer,
                    Ok(size) => sink.on_bytes(bytes[..size].to_vec()),
                    Err(error) => break Closed::Error(error.to_string()),
                }
            };
            sink.on_closed(reason);
        })
        .expect("could not start the Windows RFCOMM reader");
}

pub type RfcommChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

/// A Windows RFCOMM server listener.
pub struct RfcommListener {
    socket: Arc<std::net::TcpStream>,
    incoming: mpsc::UnboundedReceiver<webbluetooth_core::Result<RfcommChannel>>,
}

impl Stream for RfcommListener {
    type Item = webbluetooth_core::Result<RfcommChannel>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming).poll_next(cx)
    }
}

impl Drop for RfcommListener {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

impl webbluetooth_core::l2cap::PlatformChannel for Channel {
    fn psm(&self) -> u16 {
        self.channel as u16
    }
    fn peer_id(&self) -> &str {
        self.peer_id()
    }
    fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        Channel::send(self, bytes)
    }
    fn pending_bytes(&self) -> usize {
        self.pending_bytes()
    }
    fn closed(&self) -> Option<Closed> {
        self.closed()
    }
    fn close(&self) {
        self.close()
    }
}

pub fn open(address: &str, channel: u8) -> webbluetooth_core::Result<RfcommChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::connect(address, channel, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(RfcommChannel::new(channel, sink, incoming))
}

/// Open an RFCOMM profile by its service UUID.
pub fn open_service(address: &str, service_uuid: &str) -> webbluetooth_core::Result<RfcommChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::connect_service(address, service_uuid, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(RfcommChannel::new(channel, sink, incoming))
}

/// Listen for incoming RFCOMM connections on a server channel.
pub fn listen(channel: u8) -> webbluetooth_core::Result<RfcommListener> {
    if !(1..=30).contains(&channel) {
        return Err(webbluetooth_core::Error::InvalidModification(
            "RFCOMM channel must be between 1 and 30".into(),
        ));
    }
    let raw = unsafe {
        windows_sys::Win32::Networking::WinSock::socket(
            AF_BTH,
            windows_sys::Win32::Networking::WinSock::SOCK_STREAM,
            BTPROTO_RFCOMM,
        )
    };
    if raw == windows_sys::Win32::Networking::WinSock::INVALID_SOCKET {
        return Err(webbluetooth_core::Error::Network(format!(
            "could not create RFCOMM listener: {}",
            last_error()
        )));
    }
    let socket = unsafe { std::net::TcpStream::from_raw_socket(raw as RawSocket) };
    let address = SockAddrBth {
        address_family: AF_BTH as u16,
        bt_addr: 0,
        service_class_id: unsafe { std::mem::zeroed() },
        port: channel as u32,
    };
    let bind_result = unsafe {
        windows_sys::Win32::Networking::WinSock::bind(
            raw,
            (&raw const address).cast(),
            std::mem::size_of_val(&address) as i32,
        )
    };
    let listen_result = unsafe { windows_sys::Win32::Networking::WinSock::listen(raw, 8) };
    if bind_result == windows_sys::Win32::Networking::WinSock::SOCKET_ERROR
        || listen_result == windows_sys::Win32::Networking::WinSock::SOCKET_ERROR
    {
        return Err(webbluetooth_core::Error::Network(format!(
            "could not bind RFCOMM channel {channel}: {}",
            last_error()
        )));
    }
    let socket = Arc::new(socket);
    let (tx, rx) = mpsc::unbounded();
    let accept_socket = socket.clone();
    std::thread::Builder::new()
        .name(format!("webbluetooth-windows-rfcomm-listener-{channel}"))
        .spawn(move || loop {
            let accepted = unsafe {
                windows_sys::Win32::Networking::WinSock::accept(
                    std::os::windows::io::AsRawSocket::as_raw_socket(&*accept_socket) as _,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if accepted == windows_sys::Win32::Networking::WinSock::INVALID_SOCKET {
                break;
            }
            let accepted = unsafe { std::net::TcpStream::from_raw_socket(accepted as RawSocket) };
            let accepted = Arc::new(accepted);
            let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
            let connected = Channel {
                socket: accepted,
                channel,
                peer: String::new(),
                closed: Mutex::new(None),
                stopped: Arc::new(AtomicBool::new(false)),
                writing: Mutex::new(()),
            };
            spawn_reader(
                connected.socket.clone(),
                channel_sink,
                connected.stopped.clone(),
                channel,
            );
            if tx
                .unbounded_send(Ok(RfcommChannel::new(connected, sink, incoming)))
                .is_err()
            {
                break;
            }
        })
        .map_err(|error| webbluetooth_core::Error::Network(error.to_string()))?;
    Ok(RfcommListener {
        socket,
        incoming: rx,
    })
}

fn parse_address(value: &str) -> Result<u64, String> {
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() != 6 {
        return Err("Bluetooth address needs six octets".into());
    }
    let mut address = 0u64;
    for part in parts {
        let octet = u8::from_str_radix(part, 16).map_err(|_| "invalid Bluetooth address")?;
        address = (address << 8) | u64::from(octet);
    }
    Ok(address)
}

fn address_to_string(address: u64) -> String {
    (0..6)
        .rev()
        .map(|shift| format!("{:02X}", (address >> (shift * 8)) & 0xff))
        .collect::<Vec<_>>()
        .join(":")
}

fn last_error() -> i32 {
    unsafe { windows_sys::Win32::Networking::WinSock::WSAGetLastError() }
}

fn parse_guid(value: &str) -> Result<windows_sys::core::GUID, String> {
    let bytes = value.as_bytes();
    if bytes.len() != 36
        || bytes[8] != b'-'
        || bytes[13] != b'-'
        || bytes[18] != b'-'
        || bytes[23] != b'-'
    {
        return Err("service UUID must be a canonical GUID".into());
    }
    fn hex(slice: &[u8]) -> Result<u32, String> {
        u32::from_str_radix(std::str::from_utf8(slice).map_err(|_| "invalid UUID")?, 16)
            .map_err(|_| "invalid UUID".into())
    }
    let mut data4 = [0u8; 8];
    for (index, slot) in data4.iter_mut().enumerate() {
        *slot = u8::from_str_radix(
            std::str::from_utf8(&bytes[19 + index * 2..21 + index * 2])
                .map_err(|_| "invalid UUID")?,
            16,
        )
        .map_err(|_| "invalid UUID".to_string())?;
    }
    Ok(windows_sys::core::GUID {
        data1: hex(&bytes[0..8])?,
        data2: hex(&bytes[9..13])? as u16,
        data3: hex(&bytes[14..18])? as u16,
        data4,
    })
}
