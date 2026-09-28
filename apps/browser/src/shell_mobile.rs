//! The iOS shell.
//!
//! A browser with one webview instead of two.
//!
//! The desktop shell puts the toolbar and the page in two sibling webviews.
//! wry has no child webviews on iOS — `build_as_child` is documented
//! "Android/iOS: Unsupported" — so there is one, and the toolbar has to live
//! *inside* the page, injected ahead of it.
//!
//! What does **not** move into the page is the part that matters. iOS does
//! support a second *window*, so the device chooser, the scanning prompt and
//! the permissions view keep their own, exactly as on a desktop: a site
//! cannot paint over the thing that grants it access, cannot read the list
//! of devices it offers, and cannot enumerate what other origins hold.
//!
//! The cost is honest and worth stating: a hostile page can draw a
//! convincing fake address bar, because the real one is in its DOM. That is
//! phishing, not device access — and the chooser window shows the origin it
//! got from *here*, not from the page, so the pairing prompt still tells the
//! truth about who is asking.

use serde::Serialize;
use tauri::{AppHandle, Manager, Webview, WebviewUrl, WebviewWindowBuilder};

use crate::error::{JsError, Result};
use crate::origin;
use crate::settings::{Settings, ThemeChoice};
use crate::state::{Browser, GrantedDevice};

/// The single webview. It is both the "chrome" and the "content".
pub const CHROME: &str = "main";

/// The start page.
const HOME_PATH: &str = "home.html";

/// The same page, as something `navigate` can take. Platform-dependent
/// because the asset scheme is.
#[cfg(target_os = "ios")]
const HOME_URL: &str = "tauri://localhost/home.html";
#[cfg(target_os = "android")]
const HOME_URL: &str = "http://tauri.localhost/home.html";

/// Whether a webview label may call the Web Bluetooth commands.
pub fn is_content(label: &str) -> bool {
    label == CHROME
}

/// The page. There is only ever one.
pub fn active_content(app: &AppHandle) -> Option<Webview> {
    app.get_webview(CHROME)
}

/// The shim, wrapped with the GATT name tables `build.rs` generates. Same
/// text as the desktop build: nothing in it is platform-specific.
fn shim() -> String {
    let ungated = crate::selftest::AutoPick::from_env().is_some();
    format!(
        "(function () {{\n'use strict';\nconst SELFTEST_UNGATED = {ungated};\n{tables}\n{body}\n}})();",
        tables = include_str!(concat!(env!("OUT_DIR"), "/assigned.js")),
        body = include_str!("../shim/webbluetooth.js"),
    )
}

/// Whether the single webview may go somewhere.
///
/// Anywhere the web is, plus the schemes Tauri serves this application's own
/// assets on — which differ by platform: iOS gets `tauri://localhost`,
/// Android `http://tauri.localhost`. `about:blank` is allowed because a
/// webview passes through it on the way to the first real page, and refusing
/// that would leave a blank screen that looks exactly like a crash.
fn allow_navigation(url: &url::Url) -> bool {
    matches!(url.scheme(), "tauri" | "asset" | "http" | "https") || url.as_str() == "about:blank"
}

/// The toolbar, injected into every page.
///
/// It has to be in the page: there is no second webview to put it in. A site
/// can therefore tamper with it, which is why nothing security-relevant is
/// decided here — the chooser is a separate window and gets its origin from
/// Rust. A shadow root keeps the page's stylesheet from reaching in by
/// accident, which is the common case; a page that goes looking can still
/// remove it.
fn toolbar() -> String {
    include_str!("../ui/mobile-toolbar.js").to_string()
}

/// Hand the Android backend a JavaVM and a Context.
///
/// Every other platform has an ambient way to reach its Bluetooth stack.
/// Android does not: `Runtime::init` has to be given both, and until it is,
/// every call fails with "no Android Context". `ndk-context` is where the
/// activity publishes them, and tao fills it in before any of this runs.
///
/// Failing here is reported and not fatal. A device with Bluetooth turned
/// off at the OS level is already a case the API has an answer for
/// (`getAvailability()` is false), and refusing to start the window would
/// turn a recoverable condition into a blank screen.
#[cfg(target_os = "android")]
fn start_android_backend() {
    use webbluetooth::android::{JObject, Vm};

    let context = ndk_context::android_context();
    let vm = Vm(context.vm() as *mut _);
    match webbluetooth::android::init(vm, context.context() as JObject) {
        Ok(()) => {}
        Err(error) => eprintln!("the Android Bluetooth backend did not start: {error}"),
    }
}

/// Create the window and its webview.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    #[cfg(target_os = "android")]
    start_android_backend();

    let handle = app.clone();
    WebviewWindowBuilder::new(app, CHROME, WebviewUrl::App(HOME_PATH.into()))
        .initialization_script(shim())
        .initialization_script(toolbar())
        .on_navigation(move |url| {
            let allowed = allow_navigation(url);
            if !allowed {
                eprintln!("refused navigation to {url}: this build serves its own pages only");
                return false;
            }
            // Same rule as the desktop build: the document going away takes
            // its handles, its notification pumps and its GATT connections
            // with it. A reload must not leave a device connected to a page
            // that no longer exists.
            handle.state::<Browser>().page_unloaded(CHROME);
            true
        })
        .build()?;

    app.state::<Settings>().apply(app);
    watch_adapter(app);
    Ok(())
}

/// Report the radio coming and going, for `onavailabilitychanged`.
fn watch_adapter(app: &AppHandle) {
    use futures_util::StreamExt;

    let bluetooth = app.state::<Browser>().bluetooth.clone();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut changes = bluetooth.watch_availability();
        while let Some(state) = changes.next().await {
            if let Some(content) = active_content(&handle) {
                let available = !matches!(
                    state,
                    Err(webbluetooth::Availability::Unsupported)
                        | Err(webbluetooth::Availability::Unauthorized)
                );
                crate::bridge::push(
                    &content,
                    "availabilitychanged",
                    serde_json::json!({ "available": available }),
                );
            }
            refresh_chrome(&handle);
        }
    });
}

/// Ask the page to re-read what the browser knows.
pub fn refresh_chrome(app: &AppHandle) {
    if let Some(content) = active_content(app) {
        let _ = content.eval("window.__CHROME__&&window.__CHROME__.refresh()");
    }
}

// ---- what the page shows --------------------------------------------------

/// The same shape the desktop toolbar reads, minus everything that needs a
/// second webview. Keeping one type means `ui/home.js` does not have to know
/// which platform it is on.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeState {
    pub url: String,
    pub origin: String,
    pub secure: bool,
    pub adapter: &'static str,
    pub devices: Vec<GrantedDevice>,
    pub theme: &'static str,
    /// Always empty: there are no tabs here.
    pub tabs: Vec<()>,
    pub active_tab: Option<String>,
}

/// Read the current state, for the page.
#[tauri::command]
pub async fn ui_state(app: AppHandle, state: tauri::State<'_, Browser>) -> Result<ChromeState> {
    let url = active_content(&app)
        .and_then(|c| c.url().ok())
        .map(|u| u.to_string())
        .unwrap_or_default();
    let parsed = url::Url::parse(&url).ok();
    let origin = parsed.as_ref().and_then(origin::of).unwrap_or_default();

    let adapter = match state.bluetooth.availability().await {
        Ok(()) => "ready",
        Err(webbluetooth::Availability::PoweredOff) => "off",
        Err(webbluetooth::Availability::Unauthorized) => "unauthorized",
        Err(webbluetooth::Availability::Unsupported) => "unsupported",
        Err(_) => "unknown",
    };

    let devices = state
        .origin_devices(&origin)
        .into_iter()
        .map(|(id, name, connected)| GrantedDevice {
            id,
            name,
            connected,
            services: 0,
        })
        .collect();

    Ok(ChromeState {
        secure: parsed.as_ref().is_some_and(origin::is_secure),
        url,
        origin,
        adapter,
        devices,
        theme: app.state::<Settings>().theme().as_str(),
        tabs: Vec::new(),
        active_tab: None,
    })
}

/// Follow the system, or pin light or dark.
#[tauri::command]
pub fn ui_set_theme(app: AppHandle, theme: String) -> Result<()> {
    app.state::<Settings>()
        .set_theme(&app, ThemeChoice::parse(&theme));
    Ok(())
}

/// Show the permissions overlay.
#[tauri::command]
pub fn ui_open_permissions(app: AppHandle) -> Result<()> {
    crate::manager::open(&app).map_err(|e| JsError::invalid_state(e.to_string()))
}

/// Go to what was typed in the in-page toolbar.
#[tauri::command]
pub fn ui_navigate(app: AppHandle, input: String) -> Result<()> {
    let url = resolve(&input)?;
    active_content(&app)
        .ok_or_else(|| JsError::invalid_state("no webview"))?
        .navigate(url)
        .map_err(|e| JsError::invalid_state(e.to_string()))
}

/// Turn address-bar input into a URL: a scheme means a URL, something
/// host-shaped gets `https://`, anything else is a search. Defaulting to
/// `https` is not habit — Web Bluetooth is only offered on a secure context,
/// so guessing `http` would quietly produce a page that cannot use the radio.
fn resolve(input: &str) -> Result<url::Url> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(JsError::type_error("nothing to open"));
    }
    if let Ok(url) = url::Url::parse(trimmed) {
        if url.scheme() == "javascript" {
            return Err(JsError::security("javascript: URLs are not accepted here"));
        }
        return Ok(url);
    }
    let host_shaped = !trimmed.contains(char::is_whitespace)
        && trimmed.contains('.')
        && !trimmed.starts_with('.')
        && !trimmed.ends_with('.');
    if host_shaped || trimmed.starts_with("localhost") {
        if let Ok(url) = url::Url::parse(&format!("https://{trimmed}")) {
            return Ok(url);
        }
    }
    let query: String = trimmed
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect();
    url::Url::parse(&format!("https://duckduckgo.com/?q={query}"))
        .map_err(|e| JsError::type_error(e.to_string()))
}

fn eval_in_page(app: &AppHandle, script: &str) -> Result<()> {
    active_content(app)
        .ok_or_else(|| JsError::invalid_state("no webview"))?
        .eval(script)
        .map_err(|e| JsError::invalid_state(e.to_string()))
}

/// Back, using the webview's own session history.
#[tauri::command]
pub fn ui_back(app: AppHandle) -> Result<()> {
    eval_in_page(&app, "history.back()")
}

/// Forward.
#[tauri::command]
pub fn ui_forward(app: AppHandle) -> Result<()> {
    eval_in_page(&app, "history.forward()")
}

/// Reload.
#[tauri::command]
pub fn ui_reload(app: AppHandle) -> Result<()> {
    eval_in_page(&app, "location.reload()")
}

/// Back to the start page.
#[tauri::command]
pub fn ui_home(app: AppHandle) -> Result<()> {
    let home: url::Url = HOME_URL
        .parse()
        .map_err(|_| JsError::invalid_state("no start page"))?;
    active_content(&app)
        .ok_or_else(|| JsError::invalid_state("no webview"))?
        .navigate(home)
        .map_err(|e| JsError::invalid_state(e.to_string()))
}

/// Take a device away from the page's origin.
#[tauri::command]
pub fn ui_revoke_origin(
    app: AppHandle,
    state: tauri::State<'_, Browser>,
    device_id: String,
) -> Result<()> {
    let origin = active_content(&app)
        .and_then(|c| c.url().ok())
        .as_ref()
        .and_then(origin::of)
        .ok_or_else(|| JsError::invalid_state("no page is loaded"))?;
    state.revoke(&origin, &device_id);
    Ok(())
}
