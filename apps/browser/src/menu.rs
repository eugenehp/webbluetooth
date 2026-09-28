//! The menu bar, which is also where the keyboard shortcuts live.
//!
//! Two reasons it is not a set of `keydown` handlers in the toolbar. The
//! page has focus almost all the time, so a shortcut handled in the chrome
//! webview would only work when the toolbar happened to be focused — ⌘L in
//! particular has to work while reading a page, which is the whole point of
//! it. And on macOS the Edit menu is what makes ⌘C and ⌘V work in a text
//! field at all; a window with no Edit menu has an address bar you cannot
//! paste into.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager};

use crate::shell;

/// Build the menu bar. Replaces Tauri's default, which has no browser in it.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let item = |id: &str, text: &str, accelerator: &str| {
        MenuItem::with_id(app, id, text, true, Some(accelerator))
    };

    // On macOS the first submenu is the application menu, whatever it is
    // called, and its name comes from the bundle rather than from here.
    let application = Submenu::with_items(
        app,
        "WebBluetooth Browser",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item("tab-new", "New Tab", "CmdOrCtrl+T")?,
            &item("open-location", "Open Location…", "CmdOrCtrl+L")?,
            &PredefinedMenuItem::separator(app)?,
            // ⌘W is the tab in a tabbed browser; the window moves to ⇧⌘W.
            &item("tab-close", "Close Tab", "CmdOrCtrl+W")?,
            &PredefinedMenuItem::close_window(app, Some("Close Window"))?,
        ],
    )?;

    // Entirely predefined, and entirely load-bearing: these are what give the
    // address bar working copy, paste and select-all.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item("reload", "Reload Page", "CmdOrCtrl+R")?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &item("devtools", "Web Inspector", "CmdOrCtrl+Alt+I")?,
        ],
    )?;

    let history = Submenu::with_items(
        app,
        "History",
        true,
        &[
            &item("back", "Back", "CmdOrCtrl+[")?,
            &item("forward", "Forward", "CmdOrCtrl+]")?,
            &PredefinedMenuItem::separator(app)?,
            &item("home", "Start Page", "CmdOrCtrl+Shift+H")?,
        ],
    )?;

    let bluetooth = Submenu::with_items(
        app,
        "Bluetooth",
        true,
        &[&item("permissions", "Permissions…", "CmdOrCtrl+Shift+B")?],
    )?;

    Menu::with_items(
        app,
        &[&application, &file, &edit, &view, &history, &bluetooth],
    )
}

/// Run what a menu item means. The toolbar buttons reach the same places.
pub fn handle(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "open-location" => {
            // Focus has to move to the toolbar's webview before the field in
            // it can take a caret.
            if let Some(chrome) = app.get_webview(shell::CHROME) {
                let _ = chrome.set_focus();
                let _ = chrome.eval("window.__CHROME__&&window.__CHROME__.focusAddress()");
            }
        }
        "reload" => shell::eval_in_content_public(app, "location.reload()"),
        "back" => shell::eval_in_content_public(app, "history.back()"),
        "forward" => shell::eval_in_content_public(app, "history.forward()"),
        "home" => {
            let _ = shell::go_home(app);
        }
        "devtools" => {
            if let Some(content) = shell::active_content(app) {
                content.open_devtools();
            }
        }
        "tab-new" => {
            let _ = shell::open_tab(app, None);
        }
        "tab-close" => {
            if let Some(label) = app.state::<shell::Tabs>().active_label() {
                shell::close_tab(app, &label);
            }
        }
        "permissions" => {
            let _ = crate::manager::open(app);
        }
        _ => {}
    }
}
