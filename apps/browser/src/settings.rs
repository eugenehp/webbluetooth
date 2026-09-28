//! Preferences that outlive a run.
//!
//! One so far: the theme. Kept next to the application's other state rather
//! than in the webview's `localStorage`, because the toolbar, the chooser and
//! the page are three separate browsing contexts and only the host sees all
//! of them.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Theme};

/// Light, dark, or whatever the system says.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follow the operating system, and keep following it when it changes.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl ThemeChoice {
    /// Parse what the toolbar sent, falling back to following the system
    /// rather than rejecting — an unknown value is not worth an error dialog.
    pub fn parse(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    /// The name the toolbar uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// `None` means "stop overriding", which is how a window goes back to
    /// following the system.
    fn as_window_theme(self) -> Option<Theme> {
        match self {
            Self::System => None,
            Self::Light => Some(Theme::Light),
            Self::Dark => Some(Theme::Dark),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    theme: ThemeChoice,
}

/// The preferences, and where they are written back to.
pub struct Settings {
    theme: Mutex<ThemeChoice>,
    path: Option<PathBuf>,
}

impl Settings {
    /// Read what the last run left, if anything.
    pub fn load(app: &AppHandle) -> Self {
        let path = app
            .path()
            .app_config_dir()
            .ok()
            .map(|dir| dir.join("settings.json"));

        let stored = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<Stored>(&text).ok())
            .unwrap_or_default();

        Self {
            theme: Mutex::new(stored.theme),
            path,
        }
    }

    /// The current choice.
    pub fn theme(&self) -> ThemeChoice {
        *self.theme.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Change it, apply it, and remember it.
    pub fn set_theme(&self, app: &AppHandle, choice: ThemeChoice) {
        *self.theme.lock().unwrap_or_else(|e| e.into_inner()) = choice;
        self.apply(app);
        self.save();
    }

    /// Push the choice at the windows.
    ///
    /// This is the whole mechanism: setting a window's theme flips
    /// `prefers-color-scheme` inside its webviews, so the toolbar, the
    /// chooser, the start page *and* whatever site is loaded all follow — the
    /// CSS for each of them is plain `@media (prefers-color-scheme: dark)`
    /// with nothing application-specific in it. On macOS the setting is
    /// app-wide; every window is set anyway, for the platforms where it is
    /// not.
    pub fn apply(&self, app: &AppHandle) {
        let theme = self.theme().as_window_theme();
        for window in app.windows().values() {
            let _ = window.set_theme(theme);
        }

        // `Window::set_theme` is documented "iOS / Android: Unsupported", so
        // on a phone the line above changes nothing and the choice has to
        // reach the stylesheets another way. `prefers-color-scheme` cannot
        // be overridden from a page — it reports the system — so the
        // attribute is what `ui/home.css` keys its overrides on.
        #[cfg(mobile)]
        {
            let name = self.theme().as_str();
            let script = if name == "system" {
                "delete document.documentElement.dataset.theme".to_string()
            } else {
                format!("document.documentElement.dataset.theme = {name:?}")
            };
            for webview in app.webviews().values() {
                let _ = webview.eval(&script);
            }
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let stored = Stored {
            theme: self.theme(),
        };
        let Ok(text) = serde_json::to_string_pretty(&stored) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Losing a theme preference is not worth interrupting anyone over.
        let _ = std::fs::write(path, text);
    }
}
