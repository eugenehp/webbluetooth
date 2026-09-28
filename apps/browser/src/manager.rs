//! The permissions manager.
//!
//! Persisting grants without somewhere to see them would be strictly worse
//! than forgetting them on quit: a permission nobody can find is a permission
//! nobody can take back. This is the window that makes the store legible —
//! every origin, every device it may reach, how many services, and whether it
//! has been allowed to scan.
//!
//! Its own window, and its own capability, so that nothing web content can
//! reach is able to enumerate what other sites have been granted.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::error::Result;
use crate::state::{Browser, OriginSummary};

/// The label of the manager window, and of its webview.
pub const WINDOW: &str = "permissions";

/// Every permission this browser is holding.
#[tauri::command]
pub async fn perm_list(state: tauri::State<'_, Browser>) -> Result<Vec<OriginSummary>> {
    // Remembered devices only get their names once they have been adopted.
    state.ready().await;
    Ok(state.everything())
}

/// Take one device away from one origin.
#[tauri::command]
pub fn perm_forget_device(
    state: tauri::State<'_, Browser>,
    origin: String,
    device_id: String,
) -> Result<()> {
    state.revoke(&origin, &device_id);
    Ok(())
}

/// Take everything away from one origin.
#[tauri::command]
pub fn perm_forget_origin(state: tauri::State<'_, Browser>, origin: String) -> Result<()> {
    state.revoke_origin(&origin);
    Ok(())
}

/// Forget the scanning answer, so the next `requestLEScan()` asks again.
#[tauri::command]
pub fn perm_clear_scanning(state: tauri::State<'_, Browser>, origin: String) -> Result<()> {
    state.clear_scanning_decision(&origin);
    Ok(())
}

/// Open the manager, or focus it if it is already up.
///
/// Its own window on every platform. This is the one surface that can
/// enumerate what *other* origins have been granted, so it must never share
/// a document with a page — which on iOS, where the toolbar does, means it
/// cannot be an overlay.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(WINDOW) {
        return existing.set_focus();
    }
    WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("permissions.html".into()))
        .title("Bluetooth permissions")
        .inner_size(560.0, 520.0)
        .min_inner_size(420.0, 320.0)
        .build()?;
    app.state::<crate::settings::Settings>().apply(app);
    Ok(())
}
