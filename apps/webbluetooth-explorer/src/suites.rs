//! Test suites: sequences with assertions, run against chosen targets.
//!
//! A macro is a recording replayed against whatever is in front. A suite is the
//! same sequence turned into a question with an answer: it names the devices it
//! runs against, it asserts what it expects to read back, and it produces a
//! result you can look at afterwards rather than a log you have to read.
//!
//! nRF Connect has this as a separate feature from macros for the same reason —
//! `TestSuite`, `Test`, `Operation`, `Result` — and the distinction is worth
//! keeping: a macro that "worked" is one where nothing obviously broke, and a
//! suite that passed is one where every expectation held.

use crate::macros::Step;
use webbluetooth::uuid::BluetoothUuid;

/// One thing a test does.
///
/// Every [`Step`] a macro can take, plus the one a macro cannot: checking that
/// what came back is what was expected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Do what a macro step does.
    Do(Step),
    /// Read a characteristic and require the value to match.
    Expect {
        /// Which characteristic.
        characteristic: BluetoothUuid,
        /// What it should start with. Empty means "any value will do", which
        /// still asserts that the read succeeded at all.
        prefix: Vec<u8>,
    },
}

impl Operation {
    /// A one-line description.
    pub fn describe(&self, definitions: &crate::names::Definitions) -> String {
        match self {
            Self::Do(step) => step.describe(definitions),
            Self::Expect {
                characteristic,
                prefix,
            } => {
                let name = crate::names::label_with(
                    definitions,
                    characteristic,
                    crate::names::Kind::Characteristic,
                );
                if prefix.is_empty() {
                    format!("expect {name} to read")
                } else {
                    format!("expect {name} to start {}", crate::format::hex(prefix))
                }
            }
        }
    }
}

/// A named test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Test {
    /// What it is called.
    pub name: String,
    /// What it does.
    pub operations: Vec<Operation>,
}

/// A named group of tests, and the devices to run them against.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Suite {
    /// What it is called.
    pub name: String,
    /// Which devices, by id. Empty means whichever tab is in front.
    ///
    /// Named rather than positional so a suite survives the tab order changing
    /// — and so a suite written for two devices runs against both without
    /// somebody having to click between them.
    pub targets: Vec<String>,
    /// The tests, in order.
    pub tests: Vec<Test>,
}

/// How one test came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Which device it ran against.
    pub device: String,
    /// Which test.
    pub test: String,
    /// `None` if it passed, else why it did not.
    pub failure: Option<String>,
    /// How long it took, in milliseconds.
    pub took: u64,
}

impl Outcome {
    /// Whether it passed.
    pub fn passed(&self) -> bool {
        self.failure.is_none()
    }
}

/// How a whole run came out.
#[derive(Debug, Clone, Default)]
pub struct Results {
    /// Which suite.
    pub suite: String,
    /// Every test on every target, in the order they ran.
    pub outcomes: Vec<Outcome>,
    /// Whether the run finished rather than being stopped.
    pub finished: bool,
}

impl Results {
    /// How many passed.
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.passed()).count()
    }

    /// How many did not.
    pub fn failed(&self) -> usize {
        self.outcomes.len() - self.passed()
    }

    /// A one-line summary.
    pub fn summary(&self) -> String {
        if self.outcomes.is_empty() {
            return "nothing ran".to_owned();
        }
        let failed = self.failed();
        if failed == 0 {
            format!("{} passed", self.passed())
        } else {
            format!("{} passed, {failed} failed", self.passed())
        }
    }

    /// The whole thing as pasteable text — which is the point of running one.
    pub fn report(&self, definitions: &crate::names::Definitions) -> String {
        let _ = definitions;
        let mut out = format!("{}\n{}\n", self.suite, self.summary());
        if !self.finished {
            out.push_str("stopped before the end\n");
        }
        for outcome in &self.outcomes {
            out.push_str(&format!(
                "{} {} on {} ({} ms){}\n",
                if outcome.passed() { "PASS" } else { "FAIL" },
                outcome.test,
                outcome.device,
                outcome.took,
                outcome
                    .failure
                    .as_ref()
                    .map(|why| format!(" — {why}"))
                    .unwrap_or_default()
            ));
        }
        out
    }
}

/// Write one operation as a single tab-free field, for the store.
pub fn render_operation(operation: &Operation) -> String {
    match operation {
        Operation::Do(step) => format!("do {}", crate::macros::render_step(step)),
        Operation::Expect {
            characteristic,
            prefix,
        } => format!(
            "expect {} {}",
            characteristic.as_str(),
            prefix
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<String>()
        ),
    }
}

/// Read one back. `None` for anything unrecognised.
pub fn parse_operation(text: &str) -> Option<Operation> {
    let (kind, rest) = text.split_once(' ')?;
    match kind {
        "do" => crate::macros::parse_step(rest).map(Operation::Do),
        "expect" => {
            let mut parts = rest.split(' ');
            Some(Operation::Expect {
                characteristic: BluetoothUuid::parse(parts.next()?).ok()?,
                prefix: crate::format::parse_hex(parts.next().unwrap_or("")).ok()?,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webbluetooth::uuid::characteristics;

    fn sample() -> Vec<Operation> {
        vec![
            Operation::Do(Step::Delay(100)),
            Operation::Do(Step::Read(characteristics::BATTERY_LEVEL)),
            Operation::Expect {
                characteristic: characteristics::BATTERY_LEVEL,
                prefix: vec![0x64],
            },
            Operation::Expect {
                characteristic: characteristics::DEVICE_NAME,
                prefix: Vec::new(),
            },
        ]
    }

    #[test]
    fn every_operation_survives_a_round_trip() {
        for operation in sample() {
            let text = render_operation(&operation);
            assert!(!text.contains('\t'), "{text:?} would break the store");
            assert_eq!(
                parse_operation(&text).as_ref(),
                Some(&operation),
                "{text:?}"
            );
        }
    }

    #[test]
    fn an_unreadable_operation_is_skipped_not_fatal() {
        assert_eq!(parse_operation("teleport 1234"), None);
        assert_eq!(parse_operation(""), None);
        assert_eq!(parse_operation("expect not-a-uuid"), None);
        assert_eq!(parse_operation("do teleport"), None);
    }

    /// The distinction that makes a suite different from a macro: an empty
    /// prefix still asserts the read *succeeded*, which is not nothing.
    #[test]
    fn an_empty_expectation_still_asserts_something() {
        let definitions = crate::names::Definitions::new();
        let any = Operation::Expect {
            characteristic: characteristics::DEVICE_NAME,
            prefix: Vec::new(),
        };
        assert_eq!(any.describe(&definitions), "expect Device Name to read");

        let specific = Operation::Expect {
            characteristic: characteristics::BATTERY_LEVEL,
            prefix: vec![0x64],
        };
        assert_eq!(
            specific.describe(&definitions),
            "expect Battery Level to start 64"
        );
    }

    fn outcome(device: &str, test: &str, failure: Option<&str>) -> Outcome {
        Outcome {
            device: device.to_owned(),
            test: test.to_owned(),
            failure: failure.map(str::to_owned),
            took: 12,
        }
    }

    #[test]
    fn results_count_and_summarise() {
        let mut results = Results {
            suite: "bring-up".to_owned(),
            finished: true,
            outcomes: vec![
                outcome("a", "reads battery", None),
                outcome("b", "reads battery", Some("got 00, expected 64")),
                outcome("a", "has a name", None),
            ],
        };
        assert_eq!(results.passed(), 2);
        assert_eq!(results.failed(), 1);
        assert_eq!(results.summary(), "2 passed, 1 failed");

        results.outcomes.retain(|o| o.passed());
        assert_eq!(results.summary(), "2 passed");
        assert_eq!(Results::default().summary(), "nothing ran");
    }

    /// A report is the reason to run a suite rather than a macro, so it has to
    /// carry the failure and which device produced it.
    #[test]
    fn a_report_names_the_device_that_failed() {
        let results = Results {
            suite: "bring-up".to_owned(),
            finished: true,
            outcomes: vec![
                outcome("AA:BB", "reads battery", None),
                outcome("CC:DD", "reads battery", Some("got 00, expected 64")),
            ],
        };
        let report = results.report(&crate::names::Definitions::new());
        assert!(report.contains("2 passed, 1 failed") || report.contains("1 passed, 1 failed"));
        assert!(report.contains("PASS reads battery on AA:BB"), "{report}");
        assert!(
            report.contains("FAIL reads battery on CC:DD") && report.contains("expected 64"),
            "{report}"
        );
    }

    #[test]
    fn a_run_that_was_stopped_says_so() {
        let results = Results {
            suite: "long one".to_owned(),
            finished: false,
            outcomes: vec![outcome("a", "first", None)],
        };
        assert!(results
            .report(&crate::names::Definitions::new())
            .contains("stopped before the end"));
    }
}
