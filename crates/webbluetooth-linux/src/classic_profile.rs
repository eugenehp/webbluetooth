//! BlueZ Classic `Profile1` registration and RFCOMM connection delivery.

use crate::bluez::{interfaces, Bluez};
use crate::dbus::Connection;
use crate::dbus::{Message, Value};
#[cfg(feature = "classic-l2cap")]
use crate::l2cap_classic::{Channel as ClassicL2capChannelImpl, ClassicL2capChannel};
use crate::rfcomm::{Channel, RfcommChannel};
use futures_channel::mpsc;
use futures_core::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use webbluetooth_core::{BluetoothUuid, ClassicSecurity, Error, Result};

static NEXT_PROFILE: AtomicUsize = AtomicUsize::new(0);

/// A channel accepted by a registered BlueZ Classic profile.
pub enum ProfileChannel {
    /// RFCOMM profile connection.
    Rfcomm(RfcommChannel),
    /// Classic L2CAP profile connection.
    #[cfg(feature = "classic-l2cap")]
    ClassicL2cap(ClassicL2capChannel),
}

/// A Classic profile registered with BlueZ.
pub struct ProfileRegistration {
    connection: Arc<Connection>,
    path: String,
    incoming: mpsc::UnboundedReceiver<Result<ProfileChannel>>,
}

impl Stream for ProfileRegistration {
    type Item = Result<ProfileChannel>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming).poll_next(cx)
    }
}

impl ProfileRegistration {
    /// Register an RFCOMM profile with BlueZ.
    pub fn register(
        bluez: &Bluez,
        service: BluetoothUuid,
        security: ClassicSecurity,
    ) -> Result<Self> {
        Self::register_transport(bluez, service, security, Transport::Rfcomm)
    }

    /// Register an RFCOMM profile with BlueZ.
    pub fn register_rfcomm(
        bluez: &Bluez,
        service: BluetoothUuid,
        security: ClassicSecurity,
    ) -> Result<Self> {
        Self::register_transport(bluez, service, security, Transport::Rfcomm)
    }

    /// Register RFCOMM on a specific server channel.
    pub fn register_rfcomm_channel(
        bluez: &Bluez,
        service: BluetoothUuid,
        channel: u8,
        security: ClassicSecurity,
    ) -> Result<Self> {
        if !(1..=30).contains(&channel) {
            return Err(Error::InvalidModification(
                "RFCOMM channel must be between 1 and 30".into(),
            ));
        }
        Self::register_transport(bluez, service, security, Transport::RfcommChannel(channel))
    }

    /// Register a Classic L2CAP profile with BlueZ.
    #[cfg(feature = "classic-l2cap")]
    pub fn register_l2cap(
        bluez: &Bluez,
        service: BluetoothUuid,
        psm: webbluetooth_core::ClassicPsm,
        security: ClassicSecurity,
    ) -> Result<Self> {
        Self::register_transport(bluez, service, security, Transport::ClassicL2cap(psm))
    }

    fn register_transport(
        bluez: &Bluez,
        service: BluetoothUuid,
        security: ClassicSecurity,
        transport: Transport,
    ) -> Result<Self> {
        let path = format!(
            "/org/webbluetooth/profile{}",
            NEXT_PROFILE.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = mpsc::unbounded();
        let handler = Arc::new(ProfileHandler {
            tx,
            security,
            transport,
        });
        bluez.export(&path, handler);
        let options = Value::dict([
            ("Name".into(), Value::Str("webbluetooth".into())),
            ("Role".into(), Value::Str("server".into())),
            (
                "RequireAuthentication".into(),
                Value::Bool(matches!(
                    security,
                    ClassicSecurity::Authentication
                        | ClassicSecurity::Encryption
                        | ClassicSecurity::SecureEncryption
                )),
            ),
            ("RequireAuthorization".into(), Value::Bool(false)),
            (
                "ServiceRecord".into(),
                Value::bytes(&service_record(service, transport)),
            ),
            (
                "Channel".into(),
                Value::Uint16(transport.channel().unwrap_or(0) as u16),
            ),
        ]);
        bluez
            .call(
                "/org/bluez",
                interfaces::PROFILE_MANAGER,
                "RegisterProfile",
                vec![
                    Value::ObjectPath(path.clone()),
                    Value::Str(service.as_str().into()),
                    options,
                ],
            )
            .map_err(|error| Error::Network(format!("RegisterProfile failed: {error}")))?;
        Ok(Self {
            connection: bluez.connection().clone(),
            path,
            incoming: rx,
        })
    }
}

// The SDP service record builders. A registered profile is only discoverable
// if BlueZ is handed a record describing it, and BlueZ takes that record as raw
// SDP bytes rather than building one itself.
fn uuid_element(uuid: BluetoothUuid) -> Vec<u8> {
    if let Some(value) = uuid.as_u16() {
        vec![0x19, (value >> 8) as u8, value as u8]
    } else {
        let hex = uuid.as_str().replace('-', "");
        let mut bytes = vec![0x1c];
        for index in (0..32).step_by(2) {
            bytes.push(u8::from_str_radix(&hex[index..index + 2], 16).unwrap_or(0));
        }
        bytes
    }
}

fn sequence(body: Vec<u8>) -> Vec<u8> {
    let mut out = vec![0x35, body.len() as u8];
    out.extend(body);
    out
}

fn uint16(value: u16) -> Vec<u8> {
    vec![0x09, (value >> 8) as u8, value as u8]
}

fn uint8(value: u8) -> Vec<u8> {
    vec![0x08, value]
}

fn attribute(id: u16, value: Vec<u8>) -> Vec<u8> {
    let mut out = vec![0x09, (id >> 8) as u8, id as u8];
    out.extend(value);
    out
}

fn service_record(service: BluetoothUuid, transport: Transport) -> Vec<u8> {
    let service_class = sequence(uuid_element(service));
    let mut protocol = Vec::new();
    let l2cap = sequence({
        let mut body = uuid_element(BluetoothUuid::from_u16(0x0100));
        if let Transport::ClassicL2cap(psm) = transport {
            body.extend(uint16(psm.get()));
        }
        body
    });
    protocol.extend(l2cap);
    let rfcomm_channel = match transport {
        Transport::Rfcomm => Some(0),
        Transport::RfcommChannel(channel) => Some(channel),
        #[cfg(feature = "classic-l2cap")]
        Transport::ClassicL2cap(_) => None,
    };
    if let Some(channel) = rfcomm_channel {
        protocol.extend(sequence({
            let mut body = uuid_element(BluetoothUuid::from_u16(0x0003));
            body.extend(uint8(channel));
            body
        }));
    }
    let protocol = sequence(protocol);
    let mut record = attribute(0x0001, service_class);
    record.extend(attribute(0x0004, protocol));
    record.extend(attribute(
        0x0009,
        sequence(sequence({
            let mut body = uuid_element(service);
            body.extend(uint16(0x0100));
            body
        })),
    ));
    record
}

impl ProfileRegistration {
    /// Unregister this profile from BlueZ.
    pub fn unregister(&mut self) -> Result<()> {
        self.connection
            .call_blocking(
                Message::call(
                    "org.bluez",
                    "/org/bluez",
                    interfaces::PROFILE_MANAGER,
                    "UnregisterProfile",
                )
                .with_body(vec![Value::ObjectPath(self.path.clone())]),
                std::time::Duration::from_secs(30),
            )
            .map_err(|error| Error::Network(format!("UnregisterProfile failed: {error}")))?;
        Ok(())
    }
}

impl Drop for ProfileRegistration {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}

struct ProfileHandler {
    tx: mpsc::UnboundedSender<Result<ProfileChannel>>,
    security: ClassicSecurity,
    transport: Transport,
}

#[derive(Clone, Copy)]
enum Transport {
    Rfcomm,
    RfcommChannel(u8),
    #[cfg(feature = "classic-l2cap")]
    ClassicL2cap(webbluetooth_core::ClassicPsm),
}

impl Transport {
    fn channel(self) -> Option<u8> {
        match self {
            Self::RfcommChannel(channel) => Some(channel),
            _ => None,
        }
    }
}

impl crate::dbus::connection::ObjectHandler for ProfileHandler {
    fn handle(&self, call: &Message) -> std::result::Result<Vec<Value>, (String, String)> {
        if call.interface.as_deref() != Some(interfaces::PROFILE) {
            return Err((
                "org.freedesktop.DBus.Error.UnknownMethod".into(),
                "not Profile1".into(),
            ));
        }
        match call.member.as_deref() {
            Some("Release") => Ok(Vec::new()),
            Some("RequestDisconnection") => Ok(Vec::new()),
            Some("NewConnection") => self.new_connection(call),
            _ => Err((
                "org.freedesktop.DBus.Error.UnknownMethod".into(),
                "unknown Profile1 method".into(),
            )),
        }
    }
}

impl ProfileHandler {
    fn new_connection(&self, call: &Message) -> std::result::Result<Vec<Value>, (String, String)> {
        let fd_index = call.body.get(1).and_then(Value::as_u64).ok_or_else(|| {
            (
                "org.bluez.Error.Rejected".into(),
                "NewConnection did not carry an FD".into(),
            )
        })? as usize;
        let original_fd = *call.received_fds.get(fd_index).ok_or_else(|| {
            (
                "org.bluez.Error.Rejected".into(),
                "NewConnection FD index was invalid".into(),
            )
        })?;
        let fd = crate::dbus::fds::duplicate(original_fd).map_err(|error| {
            (
                "org.bluez.Error.Rejected".into(),
                format!("could not duplicate NewConnection FD: {error}"),
            )
        })?;
        let (sink, wrapper, incoming) = webbluetooth_core::l2cap::sink();
        let _ = self.security;
        let result = match self.transport {
            // Both RFCOMM forms yield the same channel; they differ only in
            // whether BlueZ picked the channel number or we pinned it, and an
            // adopted FD reports whichever one was used.
            Transport::Rfcomm | Transport::RfcommChannel(_) => {
                let number = self.transport.channel().unwrap_or(0);
                let channel = Channel::adopt_fd(fd, number, String::new(), sink);
                Ok(ProfileChannel::Rfcomm(RfcommChannel::new(
                    channel, wrapper, incoming,
                )))
            }
            #[cfg(feature = "classic-l2cap")]
            Transport::ClassicL2cap(psm) => {
                let channel = ClassicL2capChannelImpl::adopt_fd(fd, psm, String::new(), sink);
                Ok(ProfileChannel::ClassicL2cap(ClassicL2capChannel::new(
                    channel, wrapper, incoming,
                )))
            }
        };
        self.tx.unbounded_send(result).map_err(|_| {
            (
                "org.bluez.Error.Rejected".into(),
                "profile listener is closed".into(),
            )
        })?;
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_registration_path_is_unique() {
        assert_ne!(
            format!(
                "/org/webbluetooth/profile{}",
                NEXT_PROFILE.fetch_add(1, Ordering::Relaxed)
            ),
            format!(
                "/org/webbluetooth/profile{}",
                NEXT_PROFILE.fetch_add(1, Ordering::Relaxed)
            )
        );
    }

    #[test]
    fn rfcomm_record_contains_service_and_protocol_attributes() {
        let bytes = service_record(BluetoothUuid::from_u16(0x1101), Transport::RfcommChannel(3));
        assert!(bytes.windows(2).any(|pair| pair == [0x00, 0x01]));
        assert!(bytes.contains(&0x03));
    }
}
