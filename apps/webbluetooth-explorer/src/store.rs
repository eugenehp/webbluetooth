//! What survives the process exiting.
//!
//! Five of this program's features are only useful if they are remembered: the
//! names you give vendor UUIDs, the name you give a device whose own name is
//! `n/a`, which devices are favourites, what you last wrote to a
//! characteristic, and how the window is set up. None of that is worth a
//! database and none of it is worth a serialisation dependency.
//!
//! So: one tab-separated file, one record per line, a leading field naming the
//! kind. Same shape as `webbluetooth`'s own grant store, for the same reasons —
//! a name with spaces needs no quoting, an unknown record kind is skipped
//! rather than fatal, and the whole thing can be read and fixed in a text
//! editor when something goes wrong.

use crate::names::Kind;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Everything remembered between runs.
#[derive(Debug, Clone, Default)]
pub struct Remembered {
    /// Names given to UUIDs the Bluetooth SIG has not assigned one.
    pub definitions: BTreeMap<(Kind, u128), String>,
    /// Names given to devices, overriding whatever they call themselves.
    pub renames: BTreeMap<String, String>,
    /// Devices marked as worth keeping at the top of the list.
    pub favourites: BTreeSet<String>,
    /// Recent write payloads, newest first, by characteristic UUID.
    pub writes: BTreeMap<u128, Vec<Vec<u8>>>,
    /// Everything in the settings menu.
    pub settings: BTreeMap<String, String>,
    /// Recorded sequences of operations.
    pub macros: Vec<crate::macros::Macro>,
    /// Test suites.
    pub suites: Vec<crate::suites::Suite>,
}

/// How many past writes to keep for one characteristic.
///
/// Enough to cover a bring-up session's worth of poking at one control point,
/// few enough that the list stays scannable.
pub const WRITE_HISTORY: usize = 12;

impl Remembered {
    /// Record a write, newest first, without duplicates.
    pub fn remember_write(&mut self, uuid: u128, value: Vec<u8>) {
        if value.is_empty() {
            return;
        }
        let history = self.writes.entry(uuid).or_default();
        // Writing the same bytes twice should move them to the top, not fill
        // the list with copies of themselves.
        history.retain(|existing| existing != &value);
        history.insert(0, value);
        history.truncate(WRITE_HISTORY);
    }

    /// A setting, or `default` if it was never written.
    pub fn setting<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.settings.get(key).map_or(default, String::as_str)
    }

    /// A boolean setting.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        match self.settings.get(key).map(String::as_str) {
            Some("1") => true,
            Some("0") => false,
            _ => default,
        }
    }

    /// Record a boolean setting.
    pub fn set_flag(&mut self, key: &str, value: bool) {
        self.settings
            .insert(key.to_owned(), if value { "1" } else { "0" }.to_owned());
    }
}

/// Where the file lives.
///
/// The platform's own configuration directory, so it sits where a user (or a
/// backup) would expect rather than next to the binary.
pub fn path() -> PathBuf {
    directory().join("state.tsv")
}

fn directory() -> PathBuf {
    #[allow(deprecated)] // `home_dir` is un-deprecated as of Rust 1.85.
    let home = std::env::home_dir();

    #[cfg(target_os = "macos")]
    let base = home.map(|home| home.join("Library/Application Support"));

    // On iOS `home_dir` is the app's sandbox root, and `Documents` under it is
    // the writable, backed-up place a file belongs. Nothing outside the sandbox
    // is reachable, so there is no other answer to look for.
    #[cfg(target_os = "ios")]
    let base = home.map(|home| home.join("Documents"));

    // Android has no home directory and no `XDG_CONFIG_HOME`. The correct path
    // is the activity's `internal_data_path`, which only the Java side knows
    // and which `android-activity` hands over at startup — this program has no
    // route to it yet, so it falls back to the cache directory the runtime
    // does expose. Settings written there survive a restart but not
    // necessarily an aggressive cleanup, which is worth knowing and is better
    // than writing somewhere unreadable.
    #[cfg(target_os = "android")]
    let base = std::env::var_os("ANDROID_DATA")
        .map(|data| PathBuf::from(data).join("data"))
        .or(home);

    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join("AppData/Roaming")));

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".config")));

    base.unwrap_or_else(std::env::temp_dir)
        .join("WebBluetoothExplorer")
}

/// Read what was remembered. A missing or unreadable file is an empty memory,
/// never an error: losing your UUID names is bad, failing to start is worse.
pub fn load() -> Remembered {
    std::fs::read_to_string(path())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Write it back.
pub fn save(remembered: &Remembered) -> std::io::Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Written beside and renamed, so an interrupted write cannot leave a
    // half-file where the real one was.
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, render(remembered))?;
    std::fs::rename(&temporary, &path)
}

const HEADER: &str = "# WebBluetoothExplorer state v1";

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Service => "service",
        Kind::Characteristic => "characteristic",
        Kind::Descriptor => "descriptor",
    }
}

fn kind_from(name: &str) -> Option<Kind> {
    match name {
        "service" => Some(Kind::Service),
        "characteristic" => Some(Kind::Characteristic),
        "descriptor" => Some(Kind::Descriptor),
        _ => None,
    }
}

/// A field that cannot contain a tab or a newline, because the format is made
/// of them. Trimmed rather than escaped: a name with a tab in it is not worth
/// an escaping scheme, and silently forging a second record would be worse.
fn field(text: &str) -> String {
    text.chars().filter(|c| *c != '\t' && *c != '\n').collect()
}

fn render(remembered: &Remembered) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for ((kind, uuid), name) in &remembered.definitions {
        out.push_str(&format!(
            "definition\t{}\t{uuid:032x}\t{}\n",
            kind_name(*kind),
            field(name)
        ));
    }
    for (id, name) in &remembered.renames {
        out.push_str(&format!("rename\t{}\t{}\n", field(id), field(name)));
    }
    for id in &remembered.favourites {
        out.push_str(&format!("favourite\t{}\n", field(id)));
    }
    for (uuid, history) in &remembered.writes {
        for value in history {
            out.push_str(&format!(
                "write\t{uuid:032x}\t{}\n",
                value
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join("")
            ));
        }
    }
    for (key, value) in &remembered.settings {
        out.push_str(&format!("setting\t{}\t{}\n", field(key), field(value)));
    }
    for suite in &remembered.suites {
        out.push_str(&format!(
            "suite\t{}\t{}\n",
            field(&suite.name),
            suite
                .targets
                .iter()
                .map(|t| field(t))
                .collect::<Vec<_>>()
                .join(",")
        ));
        for test in &suite.tests {
            out.push_str(&format!(
                "test\t{}\t{}\n",
                field(&suite.name),
                field(&test.name)
            ));
            for operation in &test.operations {
                out.push_str(&format!(
                    "op\t{}\t{}\t{}\n",
                    field(&suite.name),
                    field(&test.name),
                    field(&crate::suites::render_operation(operation))
                ));
            }
        }
    }
    for recorded in &remembered.macros {
        // One record per step, keyed by the macro's name, so a step nobody can
        // parse costs one step rather than the whole macro.
        out.push_str(&format!(
            "macro\t{}\t{}\t{}\n",
            field(&recorded.name),
            field(&recorded.group),
            recorded.repeat
        ));
        for step in &recorded.steps {
            out.push_str(&format!(
                "step\t{}\t{}\n",
                field(&recorded.name),
                field(&crate::macros::render_step(step))
            ));
        }
    }
    out
}

fn parse(text: &str) -> Remembered {
    let mut out = Remembered::default();
    for line in text.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        // An unrecognised record is skipped, not fatal: a file written by a
        // later version should still give up what this one understands.
        match fields.next() {
            Some("definition") => {
                let (Some(kind), Some(uuid), Some(name)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                let (Some(kind), Ok(uuid)) = (kind_from(kind), u128::from_str_radix(uuid, 16))
                else {
                    continue;
                };
                if !name.is_empty() {
                    out.definitions.insert((kind, uuid), name.to_owned());
                }
            }
            Some("rename") => {
                let (Some(id), Some(name)) = (fields.next(), fields.next()) else {
                    continue;
                };
                if !id.is_empty() && !name.is_empty() {
                    out.renames.insert(id.to_owned(), name.to_owned());
                }
            }
            Some("favourite") => {
                if let Some(id) = fields.next().filter(|id| !id.is_empty()) {
                    out.favourites.insert(id.to_owned());
                }
            }
            Some("write") => {
                let (Some(uuid), Some(hex)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let (Ok(uuid), Ok(value)) = (
                    u128::from_str_radix(uuid, 16),
                    crate::format::parse_hex(hex),
                ) else {
                    continue;
                };
                let history = out.writes.entry(uuid).or_default();
                if history.len() < WRITE_HISTORY && !value.is_empty() {
                    history.push(value);
                }
            }
            Some("macro") => {
                let Some(name) = fields.next().filter(|n| !n.is_empty()) else {
                    continue;
                };
                if !out.macros.iter().any(|m| m.name == name) {
                    let mut recorded = crate::macros::Macro::new(name);
                    // Both optional, so a file written before they existed
                    // still reads.
                    recorded.group = fields.next().unwrap_or("").to_owned();
                    recorded.repeat = fields.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                    out.macros.push(recorded);
                }
            }
            Some("step") => {
                let (Some(name), Some(text)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let Some(step) = crate::macros::parse_step(text) else {
                    continue;
                };
                if let Some(recorded) = out.macros.iter_mut().find(|m| m.name == name) {
                    recorded.steps.push(step);
                }
            }
            Some("suite") => {
                let Some(name) = fields.next().filter(|n| !n.is_empty()) else {
                    continue;
                };
                if !out.suites.iter().any(|s| s.name == name) {
                    out.suites.push(crate::suites::Suite {
                        name: name.to_owned(),
                        targets: fields
                            .next()
                            .unwrap_or("")
                            .split(',')
                            .filter(|t| !t.is_empty())
                            .map(str::to_owned)
                            .collect(),
                        tests: Vec::new(),
                    });
                }
            }
            Some("test") => {
                let (Some(suite), Some(name)) = (fields.next(), fields.next()) else {
                    continue;
                };
                if let Some(suite) = out.suites.iter_mut().find(|s| s.name == suite) {
                    if !suite.tests.iter().any(|t| t.name == name) {
                        suite.tests.push(crate::suites::Test {
                            name: name.to_owned(),
                            operations: Vec::new(),
                        });
                    }
                }
            }
            Some("op") => {
                let (Some(suite), Some(test), Some(text)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                let Some(operation) = crate::suites::parse_operation(text) else {
                    continue;
                };
                if let Some(test) = out
                    .suites
                    .iter_mut()
                    .find(|s| s.name == suite)
                    .and_then(|s| s.tests.iter_mut().find(|t| t.name == test))
                {
                    test.operations.push(operation);
                }
            }
            Some("setting") => {
                let (Some(key), Some(value)) = (fields.next(), fields.next()) else {
                    continue;
                };
                if !key.is_empty() {
                    out.settings.insert(key.to_owned(), value.to_owned());
                }
            }
            _ => {}
        }
    }
    out
}

/// Write to an exact path, for tests.
#[cfg(test)]
fn save_to(path: &std::path::Path, remembered: &Remembered) -> std::io::Result<()> {
    std::fs::write(path, render(remembered))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Remembered {
        let mut r = Remembered::default();
        r.definitions.insert(
            (Kind::Service, 0xf000_aa00_0451_4000_b000_0000_0000_0000),
            "TI SensorTag IR".to_owned(),
        );
        r.renames
            .insert("AA:BB:CC".to_owned(), "the one on my desk".to_owned());
        r.favourites.insert("AA:BB:CC".to_owned());
        r.remember_write(0x2A19, vec![0x01, 0xA2]);
        r.set_flag("flash", false);
        r.settings.insert("theme".to_owned(), "dark".to_owned());
        r
    }

    #[test]
    fn suites_survive_a_round_trip() {
        use crate::macros::Step;
        use crate::suites::{Operation, Suite, Test};

        let mut r = Remembered::default();
        r.suites.push(Suite {
            name: "bring-up".to_owned(),
            targets: vec!["AA:BB".to_owned(), "CC:DD".to_owned()],
            tests: vec![
                Test {
                    name: "reads battery".to_owned(),
                    operations: vec![
                        Operation::Do(Step::Delay(50)),
                        Operation::Expect {
                            characteristic: webbluetooth::uuid::characteristics::BATTERY_LEVEL,
                            prefix: vec![0x64],
                        },
                    ],
                },
                Test {
                    name: "has a name".to_owned(),
                    operations: vec![Operation::Expect {
                        characteristic: webbluetooth::uuid::characteristics::DEVICE_NAME,
                        prefix: Vec::new(),
                    }],
                },
            ],
        });
        assert_eq!(parse(&render(&r)).suites, r.suites);
    }

    /// An operation belonging to a test that was never declared has nowhere to
    /// go, and must not invent one.
    #[test]
    fn an_orphan_operation_is_dropped() {
        let text = format!("{HEADER}\nop\tno-such-suite\tno-such-test\tdo delay 10\n");
        assert!(parse(&text).suites.is_empty());
    }

    #[test]
    fn macros_survive_a_round_trip() {
        use crate::macros::{Macro, Step};
        let mut r = Remembered::default();
        r.macros.push(Macro {
            name: "unlock and read".to_owned(),
            group: "bring-up".to_owned(),
            repeat: 3,
            steps: vec![
                Step::Write {
                    characteristic: webbluetooth::uuid::characteristics::BATTERY_LEVEL,
                    value: vec![0xAA, 0x55],
                    with_response: true,
                },
                Step::Delay(200),
                Step::Read(webbluetooth::uuid::characteristics::BATTERY_LEVEL),
            ],
        });
        let back = parse(&render(&r));
        assert_eq!(back.macros, r.macros);
    }

    /// A step this version cannot read should cost that step, not the macro.
    #[test]
    fn an_unreadable_step_does_not_take_the_macro_with_it() {
        let text = format!(
            "{HEADER}\n\
             macro\tbring-up\n\
             step\tbring-up\tdelay 100\n\
             step\tbring-up\tteleport somewhere\n\
             step\tbring-up\tdelay 200\n"
        );
        let back = parse(&text);
        assert_eq!(back.macros.len(), 1);
        assert_eq!(back.macros[0].steps.len(), 2);
    }

    /// A step belonging to a macro that was never declared has nowhere to go
    /// and must not invent one.
    #[test]
    fn an_orphan_step_is_dropped() {
        let text = format!("{HEADER}\nstep\tnever-declared\tdelay 100\n");
        assert!(parse(&text).macros.is_empty());
    }

    #[test]
    fn everything_survives_a_round_trip() {
        let back = parse(&render(&sample()));
        let original = sample();

        assert_eq!(back.definitions, original.definitions);
        assert_eq!(back.renames, original.renames);
        assert_eq!(back.favourites, original.favourites);
        assert_eq!(back.writes, original.writes);
        assert_eq!(back.settings, original.settings);
        assert!(!back.flag("flash", true));
        assert_eq!(back.setting("theme", "system"), "dark");
    }

    /// A file written by a later version must still give up what this one
    /// understands, rather than being thrown away whole.
    #[test]
    fn an_unknown_record_is_skipped_not_fatal() {
        let text = format!(
            "{HEADER}\n\
             something-from-the-future\ta\tb\tc\n\
             favourite\tAA:BB:CC\n\
             definition\tnot-a-kind\t00\tx\n\
             definition\tservice\tnot-hex\tx\n\
             rename\tDD:EE:FF\tLock\n"
        );
        let back = parse(&text);
        assert_eq!(back.favourites.len(), 1);
        assert_eq!(
            back.renames.get("DD:EE:FF").map(String::as_str),
            Some("Lock")
        );
        assert!(back.definitions.is_empty(), "both bad records dropped");
    }

    #[test]
    fn a_tab_in_a_name_cannot_forge_a_second_record() {
        let mut r = Remembered::default();
        r.renames.insert(
            "AA:BB:CC".to_owned(),
            "evil\tfavourite\tDD:EE:FF".to_owned(),
        );
        let back = parse(&render(&r));

        assert!(back.favourites.is_empty(), "no record was forged");
        assert_eq!(
            back.renames.get("AA:BB:CC").map(String::as_str),
            Some("evilfavouriteDD:EE:FF")
        );
    }

    #[test]
    fn write_history_is_newest_first_deduplicated_and_bounded() {
        let mut r = Remembered::default();
        for i in 0..(WRITE_HISTORY as u8 + 5) {
            r.remember_write(0x2A19, vec![i]);
        }
        let history = &r.writes[&0x2A19];
        assert_eq!(history.len(), WRITE_HISTORY);
        assert_eq!(history[0], vec![WRITE_HISTORY as u8 + 4], "newest first");

        // Re-writing an old value moves it up rather than duplicating it.
        r.remember_write(0x2A19, vec![WRITE_HISTORY as u8]);
        let history = &r.writes[&0x2A19];
        assert_eq!(history[0], vec![WRITE_HISTORY as u8]);
        assert_eq!(
            history
                .iter()
                .filter(|v| *v == &vec![WRITE_HISTORY as u8])
                .count(),
            1
        );

        // An empty write is not worth remembering.
        r.remember_write(0x2A19, Vec::new());
        assert_eq!(r.writes[&0x2A19].len(), WRITE_HISTORY);
    }

    #[test]
    fn a_missing_file_is_an_empty_memory_not_an_error() {
        assert_eq!(parse("").definitions.len(), 0);
        assert!(parse("garbage without tabs").favourites.is_empty());
    }

    /// Whatever the platform, the answer has to be an absolute path this
    /// program can actually create — a relative one, or an empty one, means the
    /// settings silently go nowhere.
    #[test]
    fn the_directory_is_somewhere_real() {
        let directory = directory();
        assert!(directory.is_absolute(), "{}", directory.display());
        assert!(directory.ends_with("WebBluetoothExplorer"));
        assert!(path().ends_with("state.tsv"));
        assert!(path().starts_with(&directory));
    }

    #[test]
    fn it_writes_where_it_says_it_does() {
        let directory = std::env::temp_dir().join(format!("wbe-store-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let file = directory.join("state.tsv");

        save_to(&file, &sample()).expect("written");
        let back = parse(&std::fs::read_to_string(&file).expect("readable"));
        assert_eq!(back.favourites, sample().favourites);

        let _ = std::fs::remove_dir_all(&directory);
    }
}
