//! Bluetooth Mesh packet boundaries and addressing primitives.

/// A Bluetooth Mesh unicast/group/virtual address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshAddress(u16);

impl MeshAddress {
    /// Construct an address, rejecting the unassigned range.
    pub const fn new(value: u16) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Return the 16-bit wire value.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// A bounded Bluetooth Mesh network PDU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshNetworkPdu(Vec<u8>);

/// Parsed metadata from a Bluetooth Mesh network PDU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkHeader {
    /// IV index least-significant bit.
    pub ivi: bool,
    /// Whether the PDU is a control message.
    pub ctl: bool,
    /// Network TTL.
    pub ttl: u8,
    /// 24-bit sequence number.
    pub sequence: u32,
    /// Source unicast address.
    pub source: MeshAddress,
    /// Destination unicast/group address.
    pub destination: MeshAddress,
}

impl MeshNetworkPdu {
    /// The maximum network PDU size defined for a legacy Mesh bearer.
    pub const MAX_LEN: usize = 29;

    /// Validate and wrap a network PDU.
    pub fn new(bytes: Vec<u8>) -> Option<Self> {
        (!bytes.is_empty() && bytes.len() <= Self::MAX_LEN).then_some(Self(bytes))
    }

    /// Copy and validate a network PDU from a borrowed frame.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        Self::new(bytes.to_vec())
    }

    /// Borrow the encoded PDU.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Parse the unencrypted network-header fields.
    ///
    /// The encrypted network payload remains opaque; decryption requires the
    /// network-key/security layer supplied by a platform or application.
    pub fn header(&self) -> Option<NetworkHeader> {
        if self.0.len() < 10 {
            return None;
        }
        let ivi_ctl_ttl = self.0[0];
        let sequence = u32::from_be_bytes([0, self.0[1], self.0[2], self.0[3]]);
        let source = MeshAddress::new(u16::from_be_bytes([self.0[4], self.0[5]]))?;
        let destination = MeshAddress::new(u16::from_be_bytes([self.0[6], self.0[7]]))?;
        Some(NetworkHeader {
            ivi: ivi_ctl_ttl & 1 != 0,
            ctl: ivi_ctl_ttl & 2 != 0,
            ttl: (ivi_ctl_ttl >> 2) & 0x7f,
            sequence,
            source,
            destination,
        })
    }
}

impl NetworkHeader {
    /// Encode this header with an opaque encrypted network payload.
    pub fn encode(self, payload: &[u8]) -> Option<MeshNetworkPdu> {
        if self.ttl > 0x7f || self.sequence > 0x00ff_ffff || payload.is_empty() {
            return None;
        }
        let total_len = 8usize.checked_add(payload.len())?;
        if total_len > MeshNetworkPdu::MAX_LEN {
            return None;
        }
        let mut bytes = Vec::with_capacity(total_len);
        bytes.push((self.ivi as u8) | ((self.ctl as u8) << 1) | (self.ttl << 2));
        bytes.extend_from_slice(&self.sequence.to_be_bytes()[1..]);
        bytes.extend_from_slice(&self.source.get().to_be_bytes());
        bytes.extend_from_slice(&self.destination.get().to_be_bytes());
        bytes.extend_from_slice(payload);
        MeshNetworkPdu::new(bytes)
    }
}

/// Bluetooth Mesh provisioning state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProvisioningState {
    /// No provisioning transaction is active.
    #[default]
    Idle,
    /// Waiting for capabilities from the device.
    WaitingForCapabilities,
    /// Waiting for an authentication method.
    Authenticating,
    /// Distributing network and device keys.
    DistributingKeys,
    /// Provisioning completed.
    Complete,
    /// Provisioning failed.
    Failed,
}

/// Events emitted by a Mesh provisioning transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningEvent {
    /// The device reported its provisioning capabilities.
    Capabilities {
        /// Provisioning algorithms supported by the device.
        algorithms: Vec<u8>,
        /// Public-key capability/type reported by the device.
        public_key_type: u8,
    },
    /// Authentication is required using an application-selected method.
    AuthenticationRequired,
    /// Provisioning completed with the assigned unicast address.
    Complete {
        /// The first unicast address assigned to the device.
        address: MeshAddress,
    },
    /// Provisioning failed.
    Failed {
        /// The reason the transaction failed.
        reason: ProvisioningFailure,
    },
}

/// Provisioning failure reasons that can be surfaced without a platform stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningFailure {
    /// The peer rejected the provisioning request.
    Rejected,
    /// Authentication failed.
    Authentication,
    /// The transaction timed out.
    Timeout,
    /// The bearer disconnected.
    Disconnected,
}

/// Why a provisioning event could not be applied in the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningTransitionError {
    /// The event is not valid for this state.
    InvalidTransition {
        /// State in which the event was rejected.
        state: ProvisioningState,
    },
}

/// Small provisioning state machine driven by bearer events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProvisioningSession {
    state: ProvisioningState,
}

impl ProvisioningSession {
    /// Create an idle provisioning session.
    pub const fn new() -> Self {
        Self {
            state: ProvisioningState::Idle,
        }
    }

    /// Current provisioning state.
    pub const fn state(self) -> ProvisioningState {
        self.state
    }

    /// Start waiting for device capabilities.
    pub fn start(&mut self) -> bool {
        if self.state != ProvisioningState::Idle {
            return false;
        }
        self.state = ProvisioningState::WaitingForCapabilities;
        true
    }

    /// Apply a provisioning event and return the new state.
    pub fn apply(&mut self, event: &ProvisioningEvent) -> ProvisioningState {
        self.state = match event {
            ProvisioningEvent::Capabilities { .. } | ProvisioningEvent::AuthenticationRequired => {
                ProvisioningState::Authenticating
            }
            ProvisioningEvent::Complete { .. } => ProvisioningState::Complete,
            ProvisioningEvent::Failed { .. } => ProvisioningState::Failed,
        };
        self.state
    }

    /// Apply an event only when it is valid for the current state.
    pub fn try_apply(
        &mut self,
        event: &ProvisioningEvent,
    ) -> Result<ProvisioningState, ProvisioningTransitionError> {
        let next = match (self.state, event) {
            (
                ProvisioningState::WaitingForCapabilities,
                ProvisioningEvent::Capabilities { .. } | ProvisioningEvent::AuthenticationRequired,
            ) => ProvisioningState::Authenticating,
            (
                ProvisioningState::WaitingForCapabilities
                | ProvisioningState::Authenticating
                | ProvisioningState::DistributingKeys,
                ProvisioningEvent::Failed { .. },
            ) => ProvisioningState::Failed,
            (
                ProvisioningState::Authenticating | ProvisioningState::DistributingKeys,
                ProvisioningEvent::Complete { .. },
            ) => ProvisioningState::Complete,
            (state, _) => {
                return Err(ProvisioningTransitionError::InvalidTransition { state });
            }
        };
        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioning_session_transitions_and_rejects_restart() {
        let mut session = ProvisioningSession::new();
        assert!(session.start());
        assert!(!session.start());
        session.apply(&ProvisioningEvent::Capabilities {
            algorithms: vec![0],
            public_key_type: 0,
        });
        assert_eq!(session.state(), ProvisioningState::Authenticating);
        session.apply(&ProvisioningEvent::Complete {
            address: MeshAddress::new(1).unwrap(),
        });
        assert_eq!(session.state(), ProvisioningState::Complete);
    }

    #[test]
    fn checked_provisioning_transitions_reject_without_mutating() {
        let mut session = ProvisioningSession::new();
        assert_eq!(
            session.try_apply(&ProvisioningEvent::Complete {
                address: MeshAddress::new(1).unwrap(),
            }),
            Err(ProvisioningTransitionError::InvalidTransition {
                state: ProvisioningState::Idle
            })
        );
        assert!(session.start());
        assert!(session
            .try_apply(&ProvisioningEvent::Complete {
                address: MeshAddress::new(1).unwrap(),
            })
            .is_err());
        assert_eq!(session.state(), ProvisioningState::WaitingForCapabilities);
        assert_eq!(
            session
                .try_apply(&ProvisioningEvent::Capabilities {
                    algorithms: vec![0],
                    public_key_type: 0,
                })
                .unwrap(),
            ProvisioningState::Authenticating
        );
        assert_eq!(
            session
                .try_apply(&ProvisioningEvent::Complete {
                    address: MeshAddress::new(1).unwrap(),
                })
                .unwrap(),
            ProvisioningState::Complete
        );
        assert!(session
            .try_apply(&ProvisioningEvent::Failed {
                reason: ProvisioningFailure::Timeout,
            })
            .is_err());
    }

    #[test]
    fn checked_provisioning_accepts_failure_while_active() {
        for event in [
            ProvisioningEvent::Capabilities {
                algorithms: vec![],
                public_key_type: 0,
            },
            ProvisioningEvent::AuthenticationRequired,
        ] {
            let mut session = ProvisioningSession::new();
            assert!(session.start());
            let _ = session.try_apply(&event);
            assert_eq!(
                session
                    .try_apply(&ProvisioningEvent::Failed {
                        reason: ProvisioningFailure::Disconnected,
                    })
                    .unwrap(),
                ProvisioningState::Failed
            );
        }
    }

    #[test]
    fn parses_network_header_metadata() {
        let pdu = MeshNetworkPdu::new(vec![0x15, 0x01, 0x02, 0x03, 0x00, 0x01, 0xc0, 0x01, 0, 0])
            .unwrap();
        let header = pdu.header().unwrap();
        assert!(header.ivi);
        assert!(!header.ctl);
        assert_eq!(header.ttl, 5);
        assert_eq!(header.sequence, 0x010203);
        assert_eq!(header.source.get(), 1);
        assert_eq!(header.destination.get(), 0xc001);
    }

    #[test]
    fn encodes_and_decodes_network_header() {
        let header = NetworkHeader {
            ivi: true,
            ctl: false,
            ttl: 5,
            sequence: 0x010203,
            source: MeshAddress::new(1).unwrap(),
            destination: MeshAddress::new(0xc001).unwrap(),
        };
        let pdu = header.encode(&[0xaa, 0xbb]).unwrap();
        assert_eq!(
            pdu.as_bytes(),
            &[0x15, 0x01, 0x02, 0x03, 0, 1, 0xc0, 1, 0xaa, 0xbb]
        );
        assert_eq!(pdu.header(), Some(header));
        assert_eq!(MeshNetworkPdu::from_slice(pdu.as_bytes()), Some(pdu));
    }

    #[test]
    fn rejects_invalid_network_header_encodings() {
        let header = NetworkHeader {
            ivi: false,
            ctl: false,
            ttl: 0,
            sequence: 0,
            source: MeshAddress::new(1).unwrap(),
            destination: MeshAddress::new(2).unwrap(),
        };
        assert!(header.encode(&[]).is_none());
        assert!(NetworkHeader { ttl: 128, ..header }.encode(&[0]).is_none());
        assert!(NetworkHeader {
            sequence: 0x0100_0000,
            ..header
        }
        .encode(&[0])
        .is_none());
        assert!(header.encode(&[0; 22]).is_none());
    }
}
