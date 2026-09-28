//! Recorded sequences of operations.
//!
//! nRF Connect's flagship feature, and the one thing a GATT browser has that a
//! pile of one-off commands does not: bringing a device up is almost never a
//! single write. It is unlock, then configure, then wait, then read back — and
//! doing that by hand forty times while chasing a firmware bug is how the
//! fortieth one gets typed wrong.
//!
//! A macro is a list of [`Step`]s. Characteristics are named by UUID rather
//! than by position in the tree, so a macro recorded against one device runs
//! against another with the same services, and survives the device being
//! rediscovered.

use crate::format;
use webbluetooth::uuid::BluetoothUuid;

/// One operation in a sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Read a characteristic.
    Read(BluetoothUuid),
    /// Write to a characteristic.
    Write {
        /// Which characteristic.
        characteristic: BluetoothUuid,
        /// The bytes.
        value: Vec<u8>,
        /// `true` for a write-with-response.
        with_response: bool,
    },
    /// Start or stop notifications.
    Subscribe {
        /// Which characteristic.
        characteristic: BluetoothUuid,
        /// `true` to subscribe.
        on: bool,
    },
    /// Wait.
    ///
    /// The step that makes the rest work: a device that has just been written
    /// to is usually not ready to be read from.
    Delay(u64),
    /// Wait until a characteristic notifies, or give up.
    ///
    /// The step a fixed delay is a poor substitute for. A device that answers
    /// in 20 ms on the bench and 300 ms over a congested link needs a sequence
    /// that waits for the answer, not one tuned to whichever it did last.
    WaitFor {
        /// Which characteristic to listen to.
        characteristic: BluetoothUuid,
        /// Give up after this many milliseconds.
        timeout: u64,
        /// Wait for a value starting with these bytes, if given.
        ///
        /// Empty means any notification will do. A prefix rather than an exact
        /// match, because a status notification usually carries an opcode and
        /// then whatever it has to say.
        expect: Vec<u8>,
    },
}

impl Step {
    /// A one-line description, for the list.
    pub fn describe(&self, definitions: &crate::names::Definitions) -> String {
        let name = |uuid: &BluetoothUuid| {
            crate::names::label_with(definitions, uuid, crate::names::Kind::Characteristic)
        };
        match self {
            Self::Read(uuid) => format!("read {}", name(uuid)),
            Self::Write {
                characteristic,
                value,
                with_response,
            } => format!(
                "write {} {}{}",
                name(characteristic),
                format::hex(value),
                if *with_response { "" } else { " (no response)" }
            ),
            Self::Subscribe { characteristic, on } => format!(
                "{} {}",
                if *on { "subscribe" } else { "unsubscribe" },
                name(characteristic)
            ),
            Self::Delay(ms) => format!("wait {ms} ms"),
            Self::WaitFor {
                characteristic,
                timeout,
                expect,
            } => {
                if expect.is_empty() {
                    format!("wait for {} (up to {timeout} ms)", name(characteristic))
                } else {
                    format!(
                        "wait for {} starting {} (up to {timeout} ms)",
                        name(characteristic),
                        format::hex(expect)
                    )
                }
            }
        }
    }
}

/// A named sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    /// What it is called.
    pub name: String,
    /// Which folder it is filed under. Empty is the top level.
    ///
    /// nRF Connect ships its demos in folders — "micro:bit Demo", "Thingy
    /// Demo" — and a flat list stops being usable at about a dozen.
    pub group: String,
    /// How many times to run it. `0` means until stopped.
    pub repeat: u32,
    /// What it does, in order.
    pub steps: Vec<Step>,
}

impl Macro {
    /// A new, empty macro with the usual settings.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            group: String::new(),
            repeat: 1,
            steps: Vec::new(),
        }
    }

    /// How the repeat count reads in a list.
    pub fn repeat_label(&self) -> String {
        match self.repeat {
            0 => "loops".to_owned(),
            1 => String::new(),
            n => format!("×{n}"),
        }
    }
}

/// Write one step as a single tab-free field, for the store.
///
/// Same reasoning as the rest of the store: no serialisation dependency, and a
/// record a human can read and repair.
pub fn render_step(step: &Step) -> String {
    match step {
        Step::Read(uuid) => format!("read {}", uuid.as_str()),
        Step::Write {
            characteristic,
            value,
            with_response,
        } => format!(
            "write {} {} {}",
            characteristic.as_str(),
            if *with_response { "ack" } else { "noack" },
            value.iter().map(|b| format!("{b:02X}")).collect::<String>()
        ),
        Step::Subscribe { characteristic, on } => format!(
            "subscribe {} {}",
            characteristic.as_str(),
            if *on { "on" } else { "off" }
        ),
        Step::Delay(ms) => format!("delay {ms}"),
        Step::WaitFor {
            characteristic,
            timeout,
            expect,
        } => format!(
            "waitfor {} {timeout} {}",
            characteristic.as_str(),
            expect
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<String>()
        ),
    }
}

/// Read a step back. `None` for anything unrecognised, which is skipped
/// rather than fatal.
pub fn parse_step(text: &str) -> Option<Step> {
    let mut parts = text.split(' ');
    match parts.next()? {
        "read" => Some(Step::Read(BluetoothUuid::parse(parts.next()?).ok()?)),
        "write" => {
            let characteristic = BluetoothUuid::parse(parts.next()?).ok()?;
            let with_response = parts.next()? == "ack";
            // An empty payload is legal: some control points are written with
            // a zero-length value.
            let value = format::parse_hex(parts.next().unwrap_or("")).ok()?;
            Some(Step::Write {
                characteristic,
                value,
                with_response,
            })
        }
        "subscribe" => Some(Step::Subscribe {
            characteristic: BluetoothUuid::parse(parts.next()?).ok()?,
            on: parts.next()? == "on",
        }),
        "delay" => Some(Step::Delay(parts.next()?.parse().ok()?)),
        "waitfor" => Some(Step::WaitFor {
            characteristic: BluetoothUuid::parse(parts.next()?).ok()?,
            timeout: parts.next()?.parse().ok()?,
            // An absent prefix is "any notification", which is the common case.
            expect: format::parse_hex(parts.next().unwrap_or("")).ok()?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webbluetooth::uuid::characteristics;

    fn every_kind() -> Vec<Step> {
        vec![
            Step::Read(characteristics::BATTERY_LEVEL),
            Step::Write {
                characteristic: characteristics::HEART_RATE_CONTROL_POINT,
                value: vec![0x01],
                with_response: true,
            },
            Step::Write {
                characteristic: characteristics::HEART_RATE_CONTROL_POINT,
                value: Vec::new(),
                with_response: false,
            },
            Step::Subscribe {
                characteristic: characteristics::HEART_RATE_MEASUREMENT,
                on: true,
            },
            Step::Subscribe {
                characteristic: characteristics::HEART_RATE_MEASUREMENT,
                on: false,
            },
            Step::Delay(250),
        ]
    }

    #[test]
    fn a_conditional_wait_survives_a_round_trip() {
        for expect in [vec![], vec![0x01], vec![0x60, 0x0A, 0xFF]] {
            let step = Step::WaitFor {
                characteristic: characteristics::HEART_RATE_MEASUREMENT,
                timeout: 2500,
                expect,
            };
            let text = render_step(&step);
            assert!(!text.contains('\t'));
            assert_eq!(parse_step(&text).as_ref(), Some(&step), "{text:?}");
        }
    }

    /// The wait is the step a fixed delay is a poor substitute for, so it has
    /// to read clearly in the list.
    #[test]
    fn a_conditional_wait_describes_what_it_is_waiting_for() {
        let definitions = crate::names::Definitions::new();
        let any = Step::WaitFor {
            characteristic: characteristics::HEART_RATE_MEASUREMENT,
            timeout: 2000,
            expect: Vec::new(),
        };
        assert_eq!(
            any.describe(&definitions),
            "wait for Heart Rate Measurement (up to 2000 ms)"
        );

        let specific = Step::WaitFor {
            characteristic: characteristics::HEART_RATE_MEASUREMENT,
            timeout: 500,
            expect: vec![0x01, 0x02],
        };
        let described = specific.describe(&definitions);
        assert!(described.contains("starting 01 02"), "{described}");
    }

    #[test]
    fn a_macro_starts_as_a_single_pass_in_no_folder() {
        let fresh = Macro::new("bring-up");
        assert_eq!(fresh.repeat, 1);
        assert!(fresh.group.is_empty());
        assert!(fresh.steps.is_empty());
        // A single pass needs no label; anything else does.
        assert_eq!(fresh.repeat_label(), "");
        assert_eq!(
            Macro {
                repeat: 0,
                ..Macro::new("x")
            }
            .repeat_label(),
            "loops"
        );
        assert_eq!(
            Macro {
                repeat: 5,
                ..Macro::new("x")
            }
            .repeat_label(),
            "×5"
        );
    }

    #[test]
    fn every_step_survives_a_round_trip() {
        for step in every_kind() {
            let text = render_step(&step);
            assert!(!text.contains('\t'), "{text:?} would break the store");
            assert_eq!(parse_step(&text).as_ref(), Some(&step), "{text:?}");
        }
    }

    /// A macro recorded by a later version must not take the whole file with
    /// it.
    #[test]
    fn an_unreadable_step_is_skipped_not_fatal() {
        assert_eq!(parse_step("teleport 1234"), None);
        assert_eq!(parse_step(""), None);
        assert_eq!(parse_step("read not-a-uuid"), None);
        assert_eq!(parse_step("delay soon"), None);
        assert_eq!(parse_step("read"), None);
    }

    /// The write form has to keep the two kinds of write apart: sending a
    /// command where a request was recorded changes what the peer does.
    #[test]
    fn the_two_kinds_of_write_stay_distinct() {
        let ack = render_step(&Step::Write {
            characteristic: characteristics::BATTERY_LEVEL,
            value: vec![1],
            with_response: true,
        });
        let noack = render_step(&Step::Write {
            characteristic: characteristics::BATTERY_LEVEL,
            value: vec![1],
            with_response: false,
        });
        assert_ne!(ack, noack);
        assert!(matches!(
            parse_step(&ack),
            Some(Step::Write {
                with_response: true,
                ..
            })
        ));
        assert!(matches!(
            parse_step(&noack),
            Some(Step::Write {
                with_response: false,
                ..
            })
        ));
    }

    #[test]
    fn a_step_describes_itself_readably() {
        let definitions = crate::names::Definitions::new();
        let described: Vec<String> = every_kind()
            .iter()
            .map(|step| step.describe(&definitions))
            .collect();

        assert_eq!(described[0], "read Battery Level");
        assert!(described[1].starts_with("write Heart Rate Control Point 01"));
        assert!(described[2].ends_with("(no response)"));
        assert_eq!(described[3], "subscribe Heart Rate Measurement");
        assert_eq!(described[4], "unsubscribe Heart Rate Measurement");
        assert_eq!(described[5], "wait 250 ms");
    }

    /// A macro names characteristics by UUID so it survives rediscovery, and a
    /// name you gave one should show up in its steps.
    #[test]
    fn a_named_uuid_shows_its_name_in_a_step() {
        let mut definitions = crate::names::Definitions::new();
        let vendor = BluetoothUuid::parse("f000aa01-0451-4000-b000-000000000000").unwrap();
        definitions.insert(
            (crate::names::Kind::Characteristic, vendor.as_u128()),
            "IR temperature".to_owned(),
        );
        assert_eq!(
            Step::Read(vendor).describe(&definitions),
            "read IR temperature"
        );
    }
}
