//! Permissions that outlive the process.
//!
//! Chromium's own notes on `content/browser/bluetooth` describe moving from
//! the per-session `bluetooth_allowed_devices` to a persistent chooser
//! context "exposed in the settings UI for users to manage more easily". This
//! is that, for this browser: the grants a person made through the chooser
//! survive a restart, and there is a window listing every one of them with a
//! way to take it back.
//!
//! Two consequences worth being deliberate about, because they are the reason
//! a browser has a permissions screen at all:
//!
//! * **The file is the permission.** Anything that can read it can work out
//!   which devices this machine has been asked to trust, and a later run
//!   reaches them with nobody present. It is written `0600`.
//! * **A grant nobody can see is a grant nobody can revoke.** Persisting
//!   without the manager window would be strictly worse than forgetting on
//!   quit, so the two ship together.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// One device, as remembered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredDevice {
    /// What it was called when it was granted, so the manager can show
    /// something better than an opaque identifier for a device that is out of
    /// range.
    #[serde(default)]
    pub name: Option<String>,
    /// The canonical service UUIDs this origin may reach on it.
    #[serde(default)]
    pub services: Vec<String>,
}

/// What one origin holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredOrigin {
    #[serde(default)]
    pub devices: BTreeMap<String, StoredDevice>,
    /// `Some(false)` is a remembered refusal, and is not the same as `None`:
    /// a site told no must not be able to re-ask on a loop.
    #[serde(default)]
    pub scanning: Option<bool>,
}

/// The whole file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stored {
    #[serde(default)]
    pub origins: BTreeMap<String, StoredOrigin>,
}

/// Where permissions are kept.
pub struct PermissionStore {
    path: Option<PathBuf>,
}

impl PermissionStore {
    /// `permissions.json` beside the other application state.
    pub fn new(app: &AppHandle) -> Self {
        Self::at(
            app.path()
                .app_config_dir()
                .ok()
                .map(|dir| dir.join("permissions.json")),
        )
    }

    /// A store at an explicit path, or nowhere at all.
    pub fn at(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    /// Read what earlier runs left.
    ///
    /// An absent, unreadable or malformed file reads as empty rather than as
    /// an error. The worst that costs is a trip back through the chooser,
    /// whereas refusing to start would lock someone out of their own browser.
    pub fn load(&self) -> Stored {
        self.path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Write every permission, replacing what was there.
    ///
    /// Through a temporary file and a rename, so an interrupted write leaves
    /// the previous file intact: a permission store truncated by a crash
    /// would silently revoke everything.
    pub fn save(&self, stored: &Stored) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(stored) else {
            return;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let temporary = path.with_extension("tmp");
        if std::fs::write(&temporary, text).is_err() {
            return;
        }
        restrict(&temporary);
        let _ = std::fs::rename(&temporary, path);
    }
}

/// Owner-only. This file names the devices a machine will reach without
/// asking again.
#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "wbb-permissions-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        path
    }

    #[test]
    fn round_trips() {
        let path = temp();
        let store = PermissionStore::at(Some(path.clone()));

        let mut stored = Stored::default();
        let mut origin = StoredOrigin {
            scanning: Some(true),
            ..StoredOrigin::default()
        };
        origin.devices.insert(
            "AABBCC".into(),
            StoredDevice {
                name: Some("Polar H10".into()),
                services: vec!["0000180d-0000-1000-8000-00805f9b34fb".into()],
            },
        );
        stored.origins.insert("https://example.com".into(), origin);

        store.save(&stored);
        let read = store.load();

        let back = &read.origins["https://example.com"];
        assert_eq!(back.scanning, Some(true));
        assert_eq!(back.devices["AABBCC"].name.as_deref(), Some("Polar H10"));
        assert_eq!(back.devices["AABBCC"].services.len(), 1);

        // The file names devices this machine will reach without asking
        // again, so it must not be world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "permissions.json must be 0600");
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_reads_as_empty() {
        // Rather than as an error: the worst an empty read costs is a trip
        // back through the chooser.
        let store = PermissionStore::at(Some(temp()));
        assert!(store.load().origins.is_empty());
    }

    #[test]
    fn a_corrupt_file_reads_as_empty() {
        let path = temp();
        std::fs::write(&path, "{ not json").unwrap();
        let store = PermissionStore::at(Some(path.clone()));
        assert!(store.load().origins.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
