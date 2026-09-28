//! Windows Bluetooth Classic L2CAP over Winsock `AF_BTH`.

#![cfg(windows)]

use futures_channel::mpsc;
use futures_core::Stream;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::windows::io::{FromRawSocket, RawSocket};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use webbluetooth_core::classic::ClassicPsm;
use webbluetooth_core::l2cap::{ChannelSink, Closed};

const AF_BTH: i32 = 32;
const BTPROTO_L2CAP: i32 = 0;

#[repr(C)]
struct SockAddrBth {
    address_family: u16,
    bt_addr: u64,
    service_class_id: windows_sys::core::GUID,
    port: u32,
}

/// An open Windows Bluetooth Classic L2CAP channel.
pub struct Channel {
    socket: Arc<std::net::TcpStream>,
    psm: ClassicPsm,
    peer: String,
    closed: Mutex<Option<Closed>>,
    stopped: Arc<AtomicBool>,
    writing: Mutex<()>,
}

impl Channel {
    /// Connect to a Bluetooth address and Classic PSM.
    pub fn connect(
        address: &str,
        psm: ClassicPsm,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        let bt_addr = parse_address(address)?;
        let raw = unsafe {
            windows_sys::Win32::Networking::WinSock::socket(
                AF_BTH,
                windows_sys::Win32::Networking::WinSock::SOCK_SEQPACKET,
                BTPROTO_L2CAP,
            )
        };
        if raw == windows_sys::Win32::Networking::WinSock::INVALID_SOCKET {
            return Err(format!(
                "could not create Classic L2CAP socket: {}",
                last_error()
            ));
        }
        let socket = unsafe { std::net::TcpStream::from_raw_socket(raw as RawSocket) };
        let address = SockAddrBth {
            address_family: AF_BTH as u16,
            bt_addr,
            service_class_id: unsafe { std::mem::zeroed() },
            port: psm.get() as u32,
        };
        let connected = unsafe {
            windows_sys::Win32::Networking::WinSock::connect(
                raw,
                (&raw const address).cast(),
                std::mem::size_of_val(&address) as i32,
            )
        };
        if connected == windows_sys::Win32::Networking::WinSock::SOCKET_ERROR {
            return Err(format!("could not connect Classic L2CAP: {}", last_error()));
        }
        let socket = Arc::new(socket);
        let stopped = Arc::new(AtomicBool::new(false));
        spawn_reader(socket.clone(), sink, stopped.clone(), psm);
        Ok(Self {
            socket,
            psm,
            peer: address_to_string(bt_addr),
            closed: Mutex::new(None),
            stopped,
            writing: Mutex::new(()),
        })
    }

    pub fn psm(&self) -> ClassicPsm {
        self.psm
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
    psm: ClassicPsm,
) {
    std::thread::Builder::new()
        .name(format!(
            "webbluetooth-windows-l2cap-classic-{:04x}",
            psm.get()
        ))
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
        .expect("could not start the Windows Classic L2CAP reader");
}

/// The portable async Windows Classic L2CAP channel.
pub type ClassicL2capChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

/// A Windows Classic L2CAP server listener.
pub struct ClassicL2capListener {
    socket: Arc<std::net::TcpStream>,
    incoming: mpsc::UnboundedReceiver<webbluetooth_core::Result<ClassicL2capChannel>>,
}

impl Stream for ClassicL2capListener {
    type Item = webbluetooth_core::Result<ClassicL2capChannel>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming).poll_next(cx)
    }
}

impl Drop for ClassicL2capListener {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

impl webbluetooth_core::l2cap::PlatformChannel for Channel {
    fn psm(&self) -> u16 {
        self.psm.get()
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

pub fn open(address: &str, psm: ClassicPsm) -> webbluetooth_core::Result<ClassicL2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel =
        Channel::connect(address, psm, channel_sink).map_err(webbluetooth_core::Error::Network)?;
    Ok(ClassicL2capChannel::new(channel, sink, incoming))
}

/// Listen for incoming Classic L2CAP connections on a PSM.
pub fn listen(psm: ClassicPsm) -> webbluetooth_core::Result<ClassicL2capListener> {
    let raw = unsafe {
        windows_sys::Win32::Networking::WinSock::socket(
            AF_BTH,
            windows_sys::Win32::Networking::WinSock::SOCK_SEQPACKET,
            BTPROTO_L2CAP,
        )
    };
    if raw == windows_sys::Win32::Networking::WinSock::INVALID_SOCKET {
        return Err(webbluetooth_core::Error::Network(format!(
            "could not create Classic L2CAP listener: {}",
            last_error()
        )));
    }
    let socket = unsafe { std::net::TcpStream::from_raw_socket(raw as RawSocket) };
    let address = SockAddrBth {
        address_family: AF_BTH as u16,
        bt_addr: 0,
        service_class_id: unsafe { std::mem::zeroed() },
        port: psm.get() as u32,
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
            "could not bind Classic L2CAP PSM 0x{:04x}: {}",
            psm.get(),
            last_error()
        )));
    }
    let socket = Arc::new(socket);
    let accept_socket = socket.clone();
    let (tx, rx) = mpsc::unbounded();
    std::thread::Builder::new()
        .name(format!(
            "webbluetooth-windows-l2cap-listener-{:04x}",
            psm.get()
        ))
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
            let socket =
                Arc::new(unsafe { std::net::TcpStream::from_raw_socket(accepted as RawSocket) });
            let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
            let stopped = Arc::new(AtomicBool::new(false));
            let channel = Channel {
                socket: socket.clone(),
                psm,
                peer: String::new(),
                closed: Mutex::new(None),
                stopped: stopped.clone(),
                writing: Mutex::new(()),
            };
            spawn_reader(socket, channel_sink, stopped, psm);
            if tx
                .unbounded_send(Ok(ClassicL2capChannel::new(channel, sink, incoming)))
                .is_err()
            {
                break;
            }
        })
        .map_err(|error| webbluetooth_core::Error::Network(error.to_string()))?;
    Ok(ClassicL2capListener {
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
