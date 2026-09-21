//! A GATT client over an L2CAP socket, with no daemon in the way.
//!
//! Connecting an L2CAP socket to CID 4 — the fixed ATT channel — makes the
//! kernel establish the LE link and hands back a GATT connection. From there
//! everything is [`crate::att`] PDUs.
//!
//! # Serialising requests
//!
//! ATT allows exactly **one outstanding request per connection**: the peer will
//! not answer a second until the first is done. So a request takes a lock,
//! writes, and waits for its reply, while a reader thread separates replies
//! from unsolicited notifications — which may arrive at any time, including in
//! the middle of a request, and must not be mistaken for its answer.

use crate::att::{self, AttError, Characteristic, Descriptor, Response, Service, Uuid};
use crate::sys::{
    address_type, bind_le_source, close, connect, errno, parse_address, read, shutdown, socket,
    wait_ready, write, Ready, SockAddrL2, AF_BLUETOOTH, BTPROTO_L2CAP, EINTR, POLLIN, POLLOUT,
    SHUT_RDWR, SOCK_SEQPACKET,
};
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const ATT_CID: u16 = 4;
const DEFAULT_MTU: u16 = 517;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a GATT operation failed.
#[derive(Debug, Clone)]
pub enum Error {
    /// The socket could not be opened or connected.
    Io(String),
    /// The peer refused, with an ATT error code.
    Att(AttError),
    /// No reply within the deadline.
    Timeout,
    /// The link dropped.
    Disconnected,
    /// A reply that did not decode, or was not the one asked for.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "{m}"),
            Self::Att(e) => write!(f, "the peer refused: {e}"),
            Self::Timeout => f.write_str("the peer did not answer"),
            Self::Disconnected => f.write_str("the link dropped"),
            Self::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

/// Where unsolicited traffic goes.
pub trait NotificationSink: Send + Sync {
    /// A `Handle Value Notification` or `Indication` arrived.
    fn on_notification(&self, handle: u16, value: Vec<u8>);
    /// The link dropped.
    fn on_disconnected(&self);
}

struct Shared {
    fd: c_int,
    /// The reply to the request in flight, if it has arrived.
    reply: Mutex<Option<Response>>,
    arrived: Condvar,
    stopped: AtomicBool,
}

impl Shared {
    /// Write a PDU with a short deadline, discarding failures.
    ///
    /// Used for the replies this side owes the peer. There is nobody to report
    /// a failure to and nothing useful to do about one, but it still must not
    /// block the reader thread indefinitely.
    fn send(&self, pdu: &[u8]) {
        if !matches!(wait_ready(self.fd, POLLOUT, 1000), Ok(Ready::Yes)) {
            return;
        }
        unsafe { write(self.fd, pdu.as_ptr(), pdu.len()) };
    }

    /// Answer a request the peer made of us.
    ///
    /// This crate is a GATT client and has no attribute database to serve, but
    /// a client still shares the connection with a peer that may ask. BlueZ
    /// opens with `Exchange MTU Request`, and leaving it unanswered leaves the
    /// peer waiting on a reply that never comes. Everything else is declined
    /// properly, which is what `Error Response` is for.
    fn answer(&self, opcode: u8, body: &[u8]) {
        if opcode == att::opcode::EXCHANGE_MTU_REQUEST && body.len() >= 2 {
            self.send(&att::exchange_mtu_response(DEFAULT_MTU));
            return;
        }
        // The handle, where the request carries one, so the error names it.
        let handle = if body.len() >= 2 {
            u16::from_le_bytes([body[0], body[1]])
        } else {
            0
        };
        self.send(&att::error_response(
            opcode,
            handle,
            AttError(AttError::REQUEST_NOT_SUPPORTED),
        ));
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        unsafe { close(self.fd) };
    }
}

/// A GATT connection to one peer.
pub struct Connection {
    shared: Arc<Shared>,
    /// ATT allows one outstanding request; this enforces it.
    in_flight: Mutex<()>,
    mtu: Mutex<u16>,
    peer: String,
}

impl Connection {
    /// Open an ATT connection to `address`.
    ///
    /// `random_address` must match how the peer advertised — connecting to a
    /// random address as though it were public simply times out.
    pub fn open(
        address: &str,
        random_address: bool,
        sink: Arc<dyn NotificationSink>,
    ) -> Result<Self> {
        let bdaddr = parse_address(address)
            .ok_or_else(|| Error::Io(format!("{address:?} is not a Bluetooth address")))?;

        let fd = unsafe { socket(AF_BLUETOOTH, SOCK_SEQPACKET, BTPROTO_L2CAP) };
        if fd < 0 {
            return Err(Error::Io(format!(
                "could not create an L2CAP socket (errno {})",
                errno()
            )));
        }

        if let Err(e) = bind_le_source(fd, 0, ATT_CID) {
            unsafe { close(fd) };
            return Err(Error::Io(format!(
                "could not bind an LE ATT socket (errno {e})"
            )));
        }

        let addr = SockAddrL2 {
            family: AF_BLUETOOTH as u16,
            psm: 0,
            bdaddr,
            cid: ATT_CID.to_le(),
            bdaddr_type: address_type(random_address),
        };
        if unsafe { connect(fd, &addr, std::mem::size_of::<SockAddrL2>() as u32) } < 0 {
            let e = errno();
            unsafe { close(fd) };
            return Err(Error::Io(format!(
                "could not connect to {address} (errno {e})"
            )));
        }

        let shared = Arc::new(Shared {
            fd,
            reply: Mutex::new(None),
            arrived: Condvar::new(),
            stopped: AtomicBool::new(false),
        });
        spawn_reader(shared.clone(), sink);

        let connection = Self {
            shared,
            in_flight: Mutex::new(()),
            mtu: Mutex::new(23),
            peer: address.to_owned(),
        };

        // Negotiate up from the 23-byte default. A peer that refuses keeps 23,
        // which is correct rather than fatal.
        if let Ok(Response::Mtu(mtu)) = connection.request(&att::exchange_mtu_request(DEFAULT_MTU))
        {
            *connection.mtu.lock().unwrap() = mtu.clamp(23, DEFAULT_MTU);
        }
        Ok(connection)
    }

    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// The negotiated ATT MTU. A write carries `mtu - 3` bytes.
    pub fn mtu(&self) -> u16 {
        *self.mtu.lock().unwrap()
    }

    pub fn is_connected(&self) -> bool {
        !self.shared.stopped.load(Ordering::Acquire)
    }

    pub fn close(&self) {
        self.shared.stopped.store(true, Ordering::Release);
        unsafe { shutdown(self.shared.fd, SHUT_RDWR) };
    }

    /// Send one request and wait for its reply.
    fn request(&self, pdu: &[u8]) -> Result<Response> {
        let _one_at_a_time = self.in_flight.lock().map_err(|_| Error::Disconnected)?;
        if !self.is_connected() {
            return Err(Error::Disconnected);
        }
        *self.shared.reply.lock().unwrap() = None;

        let deadline = std::time::Instant::now() + REQUEST_TIMEOUT;
        self.send(pdu, deadline)?;

        let mut guard = self.shared.reply.lock().map_err(|_| Error::Disconnected)?;
        while guard.is_none() {
            if !self.is_connected() {
                return Err(Error::Disconnected);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            let (g, _) = self
                .shared
                .arrived
                .wait_timeout(guard, remaining)
                .map_err(|_| Error::Disconnected)?;
            guard = g;
        }
        guard.take().ok_or(Error::Disconnected)
    }

    /// Write one PDU, waiting for the socket rather than blocking in it.
    ///
    /// A bare `write` on an L2CAP socket has no deadline: if the channel is
    /// suspended — which the kernel does while security is pending — the call
    /// parks in `bt_sock_wait_ready` with `SO_SNDTIMEO` unset and never
    /// returns. That is a hang with no error and no timeout, below the reach of
    /// the deadline this request is already carrying. Polling first gives the
    /// deadline somewhere to apply.
    fn send(&self, pdu: &[u8], deadline: std::time::Instant) -> Result<()> {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        let ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        match wait_ready(self.shared.fd, POLLOUT, ms) {
            Ok(Ready::Yes) => {}
            Ok(Ready::TimedOut) => return Err(Error::Timeout),
            Ok(Ready::Closed) => return Err(Error::Disconnected),
            Err(e) => return Err(Error::Io(format!("poll failed (errno {e})"))),
        }
        let n = unsafe { write(self.shared.fd, pdu.as_ptr(), pdu.len()) };
        if n as usize != pdu.len() {
            return Err(Error::Io(format!("ATT write failed (errno {})", errno())));
        }
        Ok(())
    }

    /// Write a value in `Prepare Write` fragments, then apply them.
    ///
    /// One `Write Request` carries at most `MTU - 3` bytes, but a GATT
    /// attribute holds up to 512. Anything longer has to be queued in
    /// fragments and committed with `Execute Write`, or the peer rejects the
    /// oversized request.
    fn write_long(&self, handle: u16, value: &[u8]) -> Result<()> {
        // Prepare Write spends two more header bytes than Write Request on the
        // offset.
        let chunk = (*self.mtu.lock().unwrap() as usize)
            .saturating_sub(5)
            .max(1);
        for (i, part) in value.chunks(chunk).enumerate() {
            let offset = (i * chunk) as u16;
            match self.checked(&att::prepare_write_request(handle, offset, part)) {
                Ok(Response::PrepareWriteComplete) => {}
                Ok(other) => {
                    // Abandon the queue rather than leave a partial value to be
                    // committed by anyone else.
                    let _ = self.request(&att::execute_write_request(false));
                    return Err(Error::Protocol(format!(
                        "unexpected reply to a prepared write: {other:?}"
                    )));
                }
                Err(e) => {
                    let _ = self.request(&att::execute_write_request(false));
                    return Err(e);
                }
            }
        }
        match self.checked(&att::execute_write_request(true))? {
            Response::WriteComplete => Ok(()),
            other => Err(Error::Protocol(format!(
                "unexpected reply to an executed write: {other:?}"
            ))),
        }
    }

    /// Send a request, turning an ATT error reply into an `Err`.
    fn checked(&self, pdu: &[u8]) -> Result<Response> {
        match self.request(pdu)? {
            Response::Error { error, .. } => Err(Error::Att(error)),
            other => Ok(other),
        }
    }

    /// Every primary service on the peer.
    ///
    /// Repeats `Read By Group Type` from where the last reply stopped until the
    /// peer answers `ATTRIBUTE_NOT_FOUND`, which is how it says "no more".
    pub fn discover_services(&self) -> Result<Vec<Service>> {
        let mut found: Vec<Service> = Vec::new();
        let mut start = 0x0001u16;
        loop {
            let pdu =
                att::read_by_group_type_request(start, 0xFFFF, att::gatt_type::PRIMARY_SERVICE);
            match self.request(&pdu)? {
                Response::Services(batch) if !batch.is_empty() => {
                    let last = batch.last().map(|s| s.end_handle).unwrap_or(0xFFFF);
                    found.extend(batch);
                    // `last` is a u16, so this is the only way the range ends.
                    if last == 0xFFFF {
                        break;
                    }
                    start = last + 1;
                }
                Response::Error { error, .. } if error.is_end_of_discovery() => break,
                Response::Error { error, .. } => return Err(Error::Att(error)),
                other => {
                    return Err(Error::Protocol(format!(
                        "unexpected reply to discovery: {other:?}"
                    )))
                }
            }
        }
        Ok(found)
    }

    /// Every characteristic within a service's handle range.
    /// The services this one includes.
    ///
    /// An `Include` declaration whose included service has a 128-bit UUID does
    /// not carry it, so the UUID is read from the included service's own
    /// declaration — otherwise the caller would have a handle range it cannot
    /// name.
    pub fn discover_included_services(&self, service: &Service) -> Result<Vec<Service>> {
        let mut found: Vec<Service> = Vec::new();
        let mut start = service.start_handle;
        loop {
            if start > service.end_handle {
                break;
            }
            let pdu = att::read_by_type_request(start, service.end_handle, att::gatt_type::INCLUDE);
            match self.request(&pdu)? {
                Response::Includes(batch) if !batch.is_empty() => {
                    let last = batch.last().map(|i| i.handle).unwrap_or(service.end_handle);
                    for include in batch {
                        let uuid = match include.uuid {
                            Some(uuid) => uuid,
                            // A 128-bit included service: read its declaration.
                            None => match self.read(include.start_handle) {
                                Ok(bytes) => match att::Uuid::parse(&bytes) {
                                    Some(uuid) => uuid,
                                    None => continue,
                                },
                                Err(_) => continue,
                            },
                        };
                        found.push(Service {
                            start_handle: include.start_handle,
                            end_handle: include.end_handle,
                            uuid,
                        });
                    }
                    if last >= service.end_handle {
                        break;
                    }
                    start = last + 1;
                }
                // An empty batch or "attribute not found" both mean there are
                // no more; a service that includes nothing is the common case.
                Response::Includes(_) => break,
                Response::Error { error, .. } if error.is_end_of_discovery() => break,
                Response::Error { error, .. } => return Err(Error::Att(error)),
                other => {
                    return Err(Error::Protocol(format!(
                        "unexpected reply to include discovery: {other:?}"
                    )))
                }
            }
        }
        Ok(found)
    }

    pub fn discover_characteristics(&self, service: &Service) -> Result<Vec<Characteristic>> {
        let mut found: Vec<Characteristic> = Vec::new();
        let mut start = service.start_handle;
        loop {
            if start > service.end_handle {
                break;
            }
            let pdu = att::read_by_type_request(
                start,
                service.end_handle,
                att::gatt_type::CHARACTERISTIC,
            );
            match self.request(&pdu)? {
                Response::Characteristics(batch) if !batch.is_empty() => {
                    let last = batch.last().map(|c| c.handle).unwrap_or(service.end_handle);
                    found.extend(batch);
                    if last >= service.end_handle {
                        break;
                    }
                    start = last + 1;
                }
                Response::Error { error, .. } if error.is_end_of_discovery() => break,
                Response::Error { error, .. } => return Err(Error::Att(error)),
                other => {
                    return Err(Error::Protocol(format!(
                        "unexpected reply to discovery: {other:?}"
                    )))
                }
            }
        }
        Ok(found)
    }

    /// The descriptors of one characteristic — everything between its value
    /// handle and the next characteristic (or the end of the service).
    pub fn discover_descriptors(&self, start: u16, end: u16) -> Result<Vec<Descriptor>> {
        if start > end {
            return Ok(Vec::new());
        }
        let mut found: Vec<Descriptor> = Vec::new();
        let mut cursor = start;
        loop {
            if cursor > end {
                break;
            }
            match self.request(&att::find_information_request(cursor, end))? {
                Response::Descriptors(batch) if !batch.is_empty() => {
                    let last = batch.last().map(|d| d.handle).unwrap_or(end);
                    found.extend(batch);
                    if last >= end {
                        break;
                    }
                    cursor = last + 1;
                }
                Response::Error { error, .. } if error.is_end_of_discovery() => break,
                Response::Error { error, .. } => return Err(Error::Att(error)),
                other => {
                    return Err(Error::Protocol(format!(
                        "unexpected reply to discovery: {other:?}"
                    )))
                }
            }
        }
        Ok(found)
    }

    /// Read an attribute, following on with `Read Blob` if the value filled the
    /// MTU — which is the only signal that there is more of it.
    pub fn read(&self, handle: u16) -> Result<Vec<u8>> {
        let Response::Value(mut value) = self.checked(&att::read_request(handle))? else {
            return Err(Error::Protocol("expected a value".into()));
        };
        let first_chunk = self.mtu() as usize - 1;
        while value.len() == first_chunk + (value.len() - first_chunk) && value.len() >= first_chunk
        {
            let offset = value.len() as u16;
            match self.checked(&att::read_blob_request(handle, offset)) {
                Ok(Response::Value(more)) if !more.is_empty() => value.extend_from_slice(&more),
                // `ATTRIBUTE_NOT_LONG` and an empty blob both mean "that was all".
                _ => break,
            }
            if value.len() >= 512 {
                break;
            }
        }
        Ok(value)
    }

    /// Write and wait for the peer to acknowledge.
    pub fn write(&self, handle: u16, value: &[u8]) -> Result<()> {
        // One Write Request carries `MTU - 3` bytes. Longer values have to go
        // as prepared fragments; sending an oversized request instead gets it
        // refused, or silently truncated by a lenient peer.
        let single = (*self.mtu.lock().unwrap() as usize).saturating_sub(3);
        if value.len() > single {
            return self.write_long(handle, value);
        }
        match self.checked(&att::write_request(handle, value))? {
            Response::WriteComplete => Ok(()),
            other => Err(Error::Protocol(format!(
                "unexpected reply to a write: {other:?}"
            ))),
        }
    }

    /// Write without acknowledgement. Nothing comes back, so nothing is waited
    /// for and a dropped write is invisible.
    pub fn write_command(&self, handle: u16, value: &[u8]) -> Result<()> {
        // There is no unacknowledged form of a long write: `Prepare Write` is
        // a request and needs its response. Refusing beats sending a truncated
        // value that the caller believes was delivered whole.
        let single = (*self.mtu.lock().unwrap() as usize).saturating_sub(3);
        if value.len() > single {
            return Err(Error::Protocol(format!(
                "{} bytes does not fit one unacknowledged write (MTU allows {single}); \
                 use an acknowledged write",
                value.len()
            )));
        }
        let deadline = std::time::Instant::now() + REQUEST_TIMEOUT;
        self.send(&att::write_command(handle, value), deadline)
    }

    /// Subscribe by writing the Client Characteristic Configuration descriptor.
    ///
    /// There is no "subscribe" in ATT — `0x0001` asks for notifications,
    /// `0x0002` for indications, `0x0000` stops both.
    pub fn set_notify(&self, cccd_handle: u16, notify: bool, indicate: bool) -> Result<()> {
        let bits: u16 = u16::from(notify) | (u16::from(indicate) << 1);
        self.write(cccd_handle, &bits.to_le_bytes())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

fn spawn_reader(shared: Arc<Shared>, sink: Arc<dyn NotificationSink>) {
    std::thread::Builder::new()
        .name("webbluetooth-att".into())
        .spawn(move || {
            let mut buf = vec![0u8; 1024];
            while !shared.stopped.load(Ordering::Acquire) {
                // Poll rather than block in `read`, so `close()` is observed
                // instead of leaving this thread parked on a dead descriptor.
                match wait_ready(shared.fd, POLLIN, 250) {
                    Ok(Ready::Yes) => {}
                    Ok(Ready::TimedOut) => continue,
                    Ok(Ready::Closed) => break,
                    Err(e) if e == EINTR => continue,
                    Err(_) => break,
                }
                let n = unsafe { read(shared.fd, buf.as_mut_ptr(), buf.len()) };
                if n <= 0 {
                    if n < 0 && errno() == EINTR {
                        continue; // EINTR
                    }
                    break;
                }
                let pdu = &buf[..n as usize];
                let Some(response) = att::parse(pdu) else {
                    continue;
                };

                // Three kinds of inbound PDU, and only one of them is a reply.
                // Unsolicited values are delivered to the sink; the peer's own
                // requests are answered; everything else is the response to
                // whatever request is in flight.
                match response {
                    Response::Notification { handle, value } => sink.on_notification(handle, value),
                    Response::Indication { handle, value } => {
                        sink.on_notification(handle, value);
                        shared.send(&att::handle_value_confirmation());
                    }
                    Response::PeerRequest { opcode, body } => {
                        shared.answer(opcode, &body);
                    }
                    other => {
                        *shared.reply.lock().unwrap() = Some(other);
                        shared.arrived.notify_all();
                    }
                }
            }
            shared.stopped.store(true, Ordering::Release);
            // Wake anyone waiting on a reply that will never come.
            shared.arrived.notify_all();
            sink.on_disconnected();
        })
        .expect("could not start the ATT reader thread");
}

/// Find the Client Characteristic Configuration descriptor among a set.
pub fn find_cccd(descriptors: &[Descriptor]) -> Option<u16> {
    descriptors
        .iter()
        .find(|d| d.uuid == Uuid::Short(att::gatt_type::CLIENT_CHARACTERISTIC_CONFIGURATION))
        .map(|d| d.handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Address parsing and the socket layout are tested in `sys`.

    #[test]
    fn the_cccd_is_found_by_uuid() {
        let descriptors = vec![
            Descriptor {
                handle: 0x0010,
                uuid: Uuid::Short(0x2901),
            },
            Descriptor {
                handle: 0x0011,
                uuid: Uuid::Short(0x2902),
            },
        ];
        assert_eq!(find_cccd(&descriptors), Some(0x0011));
        assert_eq!(find_cccd(&[]), None);
    }
}
