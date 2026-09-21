//! Grants that outlive the process.
//!
//! The specification models a permission store, not a per-run set: a
//! `BluetoothPermissionStorage` holds `allowedDevices`, `getDevices()` is
//! defined as reading it, and `requestDevice` is defined as *adding* to it.
//! That is what makes a page able to reconnect to something the user chose
//! last week without asking again.
//!
//! Nothing here runs by default. `Bluetooth::new` keeps its grants in
//! memory and loses them on exit, which is the safe thing for a program that
//! did not ask for anything else. A caller that wants the specification's
//! behaviour opts in with `Bluetooth::with_grants`.
//!
//! # What this file is
//!
//! The file *is* the permission. Anything that can write it can grant itself a
//! device and every service on it, without a chooser and without the user
//! being present — so it is created `0600`, and where you put it matters as
//! much as anything in this crate. A world-writable directory is a way to hand
//! out access to the user's heart-rate monitor.
//!
//! Losing it is harmless: an unreadable or corrupt store starts empty, which
//! costs a trip through the chooser and nothing else. That is why every parse
//! failure here is a skipped line rather than an error — a store that refuses
//! to load because one line is malformed would lock a caller out of devices it
//! legitimately owns.
//!
//! # Format
//!
//! One device per line, tab-separated, with a header naming the version:
//!
//! ```text
//! # webbluetooth grants v1
//! AA:BB:CC:DD:EE:FF<TAB>Heart Rate Belt<TAB>0000180d-…,0000180f-…<TAB>76
//! ```
//!
//! `<TAB>` is a literal tab. Shown this way because a tab in a doc
//! comment renders as ambiguous whitespace, and the separator being a
//! tab rather than a space is the whole reason a device name needs no
//! quoting.
//!
//! Plain text on purpose: a permission store you cannot read is one you cannot
//! audit, and the first question anyone asks of it is "what does this grant?".

use crate::registry::Grant;
use crate::uuid::BluetoothUuid;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The header, which is also the version marker.
const HEADER: &str = "# webbluetooth grants v1";

/// A grant as stored: what was allowed, and what the device was called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// The name the device had when it was granted.
    pub name: Option<String>,
    /// What it was granted access to.
    pub grant: Grant,
}

/// Grants kept in a file.
#[derive(Debug, Clone)]
pub struct GrantStore {
    path: PathBuf,
}

impl GrantStore {
    /// A store backed by `path`, which need not exist yet.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Where the grants are kept.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read every stored grant.
    ///
    /// An absent, unreadable or malformed store reads as empty rather than as
    /// an error: the worst that costs is a trip through the chooser, whereas
    /// refusing to start would lock a caller out of its own devices.
    pub fn load(&self) -> BTreeMap<String, Stored> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return BTreeMap::new();
        };
        parse(&text)
    }

    /// Write every grant, replacing what was there.
    ///
    /// Written to a temporary file and renamed, so an interrupted write leaves
    /// the previous store intact rather than a half-written one. A permission
    /// store truncated by a crash would silently revoke everything.
    pub fn save(&self, devices: &BTreeMap<String, Stored>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, render(devices))?;
        restrict(&temporary)?;
        std::fs::rename(&temporary, &self.path)
    }
}

/// `0600` — this file is a permission, and nobody else's business.
#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Windows inherits the directory's ACL, and there is no mode to set.
#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn render(devices: &BTreeMap<String, Stored>) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for (id, stored) in devices {
        let services: Vec<&str> = stored.grant.services.iter().map(|u| u.as_str()).collect();
        let companies: Vec<String> = stored
            .grant
            .manufacturer_data
            .iter()
            .map(|c| c.to_string())
            .collect();
        // A tab-separated record, so a name with spaces in it needs no
        // quoting; a name with a tab or newline in it is not representable and
        // is trimmed rather than allowed to forge a second record.
        let name = stored.name.as_deref().unwrap_or("");
        let name: String = name.chars().filter(|c| *c != '\t' && *c != '\n').collect();
        out.push_str(&format!(
            "{id}\t{name}\t{}\t{}\n",
            services.join(","),
            companies.join(",")
        ));
    }
    out
}

fn parse(text: &str) -> BTreeMap<String, Stored> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        // Only the line ending. `trim_end` would take the trailing tabs with
        // it, and a device granted no services has nothing but trailing tabs —
        // the record would lose its empty fields and stop parsing.
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(id), Some(name), Some(services)) = (fields.next(), fields.next(), fields.next())
        else {
            continue; // Not a record; skip it rather than fail the store.
        };
        let companies = fields.next().unwrap_or("");
        if id.is_empty() {
            continue;
        }
        out.insert(
            id.to_owned(),
            Stored {
                name: (!name.is_empty()).then(|| name.to_owned()),
                grant: Grant {
                    services: services
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| BluetoothUuid::parse(s).ok())
                        .collect(),
                    manufacturer_data: companies
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.parse().ok())
                        .collect(),
                },
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(services: &[u16], companies: &[u16], name: Option<&str>) -> Stored {
        Stored {
            name: name.map(str::to_owned),
            grant: Grant {
                services: services
                    .iter()
                    .map(|u| BluetoothUuid::from_u16(*u))
                    .collect(),
                manufacturer_data: companies.to_vec(),
            },
        }
    }

    /// A device granted nothing but the right to exist still round-trips.
    ///
    /// Its record is an id and three empty fields, so anything trimming
    /// trailing whitespace eats the structure and the grant disappears.
    #[test]
    fn a_grant_with_no_services_round_trips() {
        let mut devices = BTreeMap::new();
        devices.insert("11:22:33:44:55:66".to_owned(), stored(&[], &[], None));
        assert_eq!(parse(&render(&devices)), devices);
    }

    #[test]
    fn a_grant_survives_a_round_trip() {
        let mut devices = BTreeMap::new();
        devices.insert(
            "AA:BB:CC:DD:EE:FF".to_owned(),
            stored(&[0x180D, 0x180F], &[0x004C], Some("Heart Rate Belt")),
        );
        devices.insert("11:22:33:44:55:66".to_owned(), stored(&[], &[], None));

        let back = parse(&render(&devices));
        assert_eq!(back, devices);
    }

    /// A name is free text from a peer. It must not be able to forge a second
    /// record and grant itself a device.
    #[test]
    fn a_name_cannot_inject_another_record() {
        let mut devices = BTreeMap::new();
        devices.insert(
            "AA:BB:CC:DD:EE:FF".to_owned(),
            stored(
                &[0x180F],
                &[],
                Some("evil\tx\t0000180d-0000-1000-8000-00805f9b34fb\n99:99:99:99:99:99\thi\t"),
            ),
        );
        let back = parse(&render(&devices));
        assert_eq!(back.len(), 1, "one device in, one device out");
        assert!(
            !back.contains_key("99:99:99:99:99:99"),
            "the name must not have created a second grant"
        );
        // And the surviving grant is still only what it was given.
        let only = back.values().next().unwrap();
        assert_eq!(only.grant.services.len(), 1);
    }

    /// A store that will not parse must not lock the caller out.
    #[test]
    fn a_damaged_store_reads_as_empty_rather_than_failing() {
        assert!(parse("").is_empty());
        assert!(parse("not a record").is_empty());
        assert!(parse("# only a comment\n").is_empty());
        // A good line beside a bad one still loads.
        let mixed = format!(
            "{HEADER}\ngarbage\nAA:BB:CC:DD:EE:FF\t\t0000180f-0000-1000-8000-00805f9b34fb\t\n"
        );
        assert_eq!(parse(&mixed).len(), 1);
    }

    /// An unparseable UUID is dropped rather than widening the grant.
    #[test]
    fn a_malformed_uuid_is_dropped_not_guessed() {
        let line = format!("{HEADER}\ndev\t\tnot-a-uuid,0000180f-0000-1000-8000-00805f9b34fb\t\n");
        let back = parse(&line);
        let grant = &back.get("dev").unwrap().grant;
        assert_eq!(grant.services.len(), 1, "only the one that parsed");
    }

    /// Miri runs with filesystem isolation on, and this test is about the
    /// filesystem. Pre-existing: it has been failing `./scripts/check.sh miri`
    /// since it was written, because the crate it lived in was already in that
    /// step. The parsing either side of the file is covered above, on strings.
    #[test]
    #[cfg_attr(miri, ignore = "opens a real file; Miri isolates the filesystem")]
    fn saving_and_loading_uses_the_file() {
        let dir = std::env::temp_dir().join(format!("wbt-grants-{}", std::process::id()));
        let store = GrantStore::new(dir.join("grants.txt"));
        assert!(store.load().is_empty(), "nothing stored yet");

        let mut devices = BTreeMap::new();
        devices.insert(
            "AA:BB:CC:DD:EE:FF".to_owned(),
            stored(&[0x180F], &[], Some("Belt")),
        );
        store.save(&devices).expect("the store should be writable");

        assert_eq!(store.load(), devices);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "a permission store is not world-readable"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
