//! Linux Classic HID host transport over the HID control/interrupt PSMs.

use crate::l2cap_classic::ClassicL2capChannel;
use futures_core::Stream;
use std::sync::Mutex;
use webbluetooth_core::{
    ClassicPsm, HidError, HidPacket, InputValue, ReportDescriptor, Result, TransactionType,
};

const HID_CONTROL_PSM: u16 = 0x0011;
const HID_INTERRUPT_PSM: u16 = 0x0013;

/// A Linux Classic HID host connection.
pub struct HidHost {
    control: ClassicL2capChannel,
    interrupt: ClassicL2capChannel,
    descriptor: Mutex<Option<ReportDescriptor>>,
}

impl HidHost {
    /// Connect to a HID device's control and interrupt channels.
    pub async fn connect(address: &str) -> Result<Self> {
        let control = crate::l2cap_classic::open(
            address,
            ClassicPsm::new(HID_CONTROL_PSM).ok_or_else(|| {
                webbluetooth_core::Error::InvalidModification("invalid HID control PSM".into())
            })?,
        )?;
        let interrupt = crate::l2cap_classic::open(
            address,
            ClassicPsm::new(HID_INTERRUPT_PSM).ok_or_else(|| {
                webbluetooth_core::Error::InvalidModification("invalid HID interrupt PSM".into())
            })?,
        )?;
        Ok(Self {
            control,
            interrupt,
            descriptor: Mutex::new(None),
        })
    }

    /// Set the device's parsed report descriptor.
    pub fn set_report_descriptor(&self, bytes: &[u8]) -> std::result::Result<(), HidError> {
        let descriptor = ReportDescriptor::parse(bytes)?;
        *self.descriptor.lock().unwrap() = Some(descriptor);
        Ok(())
    }

    /// The parsed report descriptor, if one has been installed.
    pub fn report_descriptor(&self) -> Option<ReportDescriptor> {
        self.descriptor.lock().unwrap().clone()
    }

    /// Send a HID control-channel packet.
    pub fn send_control(&self, bytes: &[u8]) -> Result<()> {
        self.control.send(bytes)
    }

    /// Send an HID interrupt report.
    pub fn send_report(&self, bytes: &[u8]) -> Result<()> {
        self.interrupt.send(bytes)
    }

    /// Request an input, output, or feature report by report ID.
    pub fn get_report(&self, report_type: u8, report_id: u8) -> Result<()> {
        let packet = HidPacket {
            transaction: TransactionType::GetReport,
            parameter: report_type & 0x03,
            payload: vec![report_id],
        };
        self.control.send(
            &packet.encode().map_err(|error| {
                webbluetooth_core::Error::InvalidModification(error.to_string())
            })?,
        )
    }

    /// Send an output report through the HID control channel.
    pub fn set_report(&self, report: &[u8]) -> Result<()> {
        let packet = HidPacket {
            transaction: TransactionType::SetReport,
            parameter: 0x02,
            payload: report.to_vec(),
        };
        self.control.send(
            &packet.encode().map_err(|error| {
                webbluetooth_core::Error::InvalidModification(error.to_string())
            })?,
        )
    }

    /// Decode an incoming interrupt packet using the installed descriptor.
    pub fn decode_input_report(&self, packet: &[u8]) -> Result<Vec<InputValue>> {
        let descriptor = self.descriptor.lock().unwrap().clone().ok_or_else(|| {
            webbluetooth_core::Error::InvalidState("no HID report descriptor installed".into())
        })?;
        let packet = HidPacket::parse(packet)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        if packet.transaction != TransactionType::Input {
            return Err(webbluetooth_core::Error::InvalidModification(
                "HID packet is not an input report".into(),
            ));
        }
        descriptor
            .decode_input(&packet.payload)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))
    }

    /// Take the incoming HID interrupt report stream.
    pub fn take_reports(&self) -> Option<impl Stream<Item = Vec<u8>>> {
        self.interrupt.take_incoming()
    }

    /// Close both HID channels.
    pub fn close(&self) {
        self.control.close();
        self.interrupt.close();
    }
}
