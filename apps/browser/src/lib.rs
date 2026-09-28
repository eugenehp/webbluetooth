//! A browser whose pages get the real Web Bluetooth API.
//!
//! Every site loaded here finds a working `navigator.bluetooth`, implemented
//! over the `webbluetooth` crate rather than over whatever the host browser
//! engine does or does not provide — WKWebView provides nothing at all.
//!
//! # The shape of it
//!
//! One window with two webviews. The `chrome` one draws the toolbar from
//! local assets. The `content` one loads whatever the user asked for, with
//! [`shim/webbluetooth.js`](../shim/webbluetooth.js) injected ahead of the
//! page's own scripts. The shim turns each call into a Tauri command; the
//! commands in [`bridge`] enforce the permission model and drive the radio.
//!
//! Splitting the two is what makes the security boundary expressible: the
//! capability in `capabilities/content.json` names the `content` webview and
//! grants it the Bluetooth commands for *any* http(s) origin, and the one in
//! `capabilities/chrome.json` keeps navigation and the permission controls
//! for the toolbar. Tauri checks the ACL on every call from a remote origin,
//! so a page cannot reach anything the content capability does not list.
//!
//! # What a page is allowed to do
//!
//! The same thing a browser allows, enforced in three places that have to
//! agree:
//!
//! * **Secure context.** `https`, or a loopback host. Insecure origins get no
//!   `navigator.bluetooth` at all, as in Chrome.
//! * **The chooser.** A device reaches an origin only by a human picking it
//!   out of [`chooser`]'s window. There is no API that skips it.
//! * **The service allowlist.** An origin may touch only the services its
//!   request named, tracked per origin in [`state`] — the library's own grant
//!   is the union across every site and is a second fence, not the first.
//!
//! The GATT blocklist the library vendors applies underneath all of that, and
//! is not something this application can turn off.

#![warn(missing_docs)]

mod bridge;
mod chooser;
mod dto;
mod error;
mod manager;
mod origin;
mod permissions;
mod scanning;
mod selftest;
mod settings;
mod state;

// The shell is where the platforms part company. On a desktop the toolbar
// and the page are two sibling webviews, which is what keeps the chrome and
// the chooser somewhere a website cannot reach. wry does not offer child
// webviews on iOS — `build_as_child` is documented "Android/iOS:
// Unsupported" — so the mobile build is one webview showing this
// application's own pages, and is not a browser. See `shell_mobile.rs`.
#[cfg(desktop)]
mod menu;
#[cfg(desktop)]
mod shell;
#[cfg(mobile)]
#[path = "shell_mobile.rs"]
mod shell;

use tauri::Manager;

/// The iOS entry point.
///
/// An iOS application is started by `UIApplicationMain`, not by a Rust
/// `main`, so the generated Xcode project calls in here instead. Wrapped in
/// a module because the macro expands to an `extern "C"` function that this
/// crate's `missing_docs` has nothing to read a doc comment from, and
/// containing the exemption is better than turning the lint off everywhere.
#[cfg(mobile)]
#[allow(missing_docs)]
mod ios {
    #[tauri::mobile_entry_point]
    pub fn start() {
        super::run();
    }
}

/// Build and run the application.
///
/// Split out from `main` so that [`ios`] can call it too.
pub fn run() {
    let builder = tauri::Builder::default()
        .manage(chooser::ChooserState::default())
        .manage(scanning::ScanPromptState::default());

    // Two lists rather than one with holes in it: `generate_handler!` takes
    // a plain list of paths, and half of these do not exist off the desktop.
    #[cfg(desktop)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        // The Web Bluetooth surface, reachable from any site.
        bridge::wb_availability,
        bridge::wb_shim_hello,
        bridge::wb_request_device,
        bridge::wb_get_devices,
        bridge::wb_forget_device,
        bridge::wb_watch_advertisements,
        bridge::wb_unwatch_advertisements,
        bridge::wb_request_le_scan,
        bridge::wb_stop_le_scan,
        bridge::wb_gatt_connect,
        bridge::wb_gatt_disconnect,
        bridge::wb_gatt_connected,
        bridge::wb_primary_services,
        bridge::wb_included_services,
        bridge::wb_characteristics,
        bridge::wb_descriptors,
        bridge::wb_read_characteristic,
        bridge::wb_write_characteristic,
        bridge::wb_start_notifications,
        bridge::wb_stop_notifications,
        bridge::wb_read_descriptor,
        bridge::wb_write_descriptor,
        // The toolbar.
        shell::ui_state,
        shell::ui_navigate,
        shell::ui_back,
        shell::ui_forward,
        shell::ui_reload,
        shell::ui_home,
        shell::ui_revoke_origin,
        shell::ui_open_devtools,
        shell::ui_set_theme,
        shell::ui_open_permissions,
        shell::ui_tab_new,
        shell::ui_tab_close,
        shell::ui_tab_select,
        // The device picker.
        chooser::chooser_ready,
        chooser::chooser_pick,
        chooser::chooser_cancel,
        // The scanning prompt.
        scanning::scanprompt_ready,
        scanning::scanprompt_decide,
        // The permissions manager.
        manager::perm_list,
        manager::perm_forget_device,
        manager::perm_forget_origin,
        manager::perm_clear_scanning,
    ]);

    #[cfg(mobile)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        bridge::wb_availability,
        bridge::wb_shim_hello,
        bridge::wb_request_device,
        bridge::wb_get_devices,
        bridge::wb_forget_device,
        bridge::wb_watch_advertisements,
        bridge::wb_unwatch_advertisements,
        bridge::wb_request_le_scan,
        bridge::wb_stop_le_scan,
        bridge::wb_gatt_connect,
        bridge::wb_gatt_disconnect,
        bridge::wb_gatt_connected,
        bridge::wb_primary_services,
        bridge::wb_included_services,
        bridge::wb_characteristics,
        bridge::wb_descriptors,
        bridge::wb_read_characteristic,
        bridge::wb_write_characteristic,
        bridge::wb_start_notifications,
        bridge::wb_stop_notifications,
        bridge::wb_read_descriptor,
        bridge::wb_write_descriptor,
        shell::ui_state,
        shell::ui_set_theme,
        shell::ui_revoke_origin,
        shell::ui_open_permissions,
        shell::ui_navigate,
        shell::ui_back,
        shell::ui_forward,
        shell::ui_reload,
        shell::ui_home,
        chooser::chooser_ready,
        chooser::chooser_pick,
        chooser::chooser_cancel,
        scanning::scanprompt_ready,
        scanning::scanprompt_decide,
        manager::perm_list,
        manager::perm_forget_device,
        manager::perm_forget_origin,
        manager::perm_clear_scanning,
    ]);

    builder
        .setup(|app| {
            let handle = app.handle().clone();
            // Before any window exists, so the first one is already right
            // rather than flashing the wrong theme.
            handle.manage(settings::Settings::load(&handle));
            handle.manage(state::Browser::new(&handle));
            // Turn remembered identifiers back into usable devices once the
            // radio is up; `getDevices()` waits on it.
            state::Browser::spawn_restore(&handle);
            #[cfg(desktop)]
            {
                handle.set_menu(menu::build(&handle)?)?;
                let events = handle.clone();
                handle.on_menu_event(move |_, event| menu::handle(&events, event));
            }
            shell::build(&handle)?;
            // `WBB_SELFTEST=1` turns the browser into a check of itself; see
            // `selftest`. It exits when it is done.
            if std::env::var_os(selftest::ENV).is_some() {
                if let Some(auto) = selftest::AutoPick::from_env() {
                    eprintln!(
                        "!! {}: the device chooser and the user-gesture check are OFF \
                         for this run",
                        selftest::DEVICE_ENV
                    );
                    handle.manage(auto);
                }
                selftest::run(&handle);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the browser could not start");
}
