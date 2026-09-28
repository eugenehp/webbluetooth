//! The browser window: a chrome strip above, web content below.
//!
//! Two webviews in one window rather than one webview drawing its own
//! toolbar, because the toolbar has to be somewhere a page cannot reach. That
//! is also what lets the Tauri capability that carries the Web Bluetooth
//! commands name the *content* webview and nothing else: web content gets the
//! radio, and never gets navigation or the permission controls.

use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl, WindowEvent};

use crate::error::{JsError, Result};
use crate::origin;
use crate::settings::{Settings, ThemeChoice};
use crate::state::Browser;

/// The label of the main window.
pub const WINDOW: &str = "main";
/// The label of the toolbar webview.
pub const CHROME: &str = "chrome";
/// Labels of the webviews that render web content: one per tab, numbered.
///
/// A prefix rather than a fixed label because there are now several, and the
/// capability in `capabilities/content.json` matches them with `content-*`.
pub const CONTENT_PREFIX: &str = "content-";

/// Whether a webview label belongs to a tab.
pub fn is_content(label: &str) -> bool {
    label.starts_with(CONTENT_PREFIX)
}

/// Height of the toolbar, in logical pixels.
///
/// Chromium's own numbers, from `chrome/browser/ui/layout_constants.cc` for a
/// non-touch desktop: `kLocationBarHeight` and `kToolbarButtonHeight` are both
/// 34, and `TOOLBAR_INTERIOR_MARGIN` is `VH(6, 6)`. 6 + 34 + 6 = 46. The rest
/// of the toolbar's metrics are in `ui/chrome.css` and come from the same
/// table, so the thing reads as an address bar rather than as a dialog that
/// happens to contain a text field.
const TOOLBAR_HEIGHT: f64 = 46.0;

/// Height of the tab strip, from the same table: `kTabHeight` is
/// `34 + kTabstripToolbarOverlap(1)` and `kTabStripHeight` is that plus
/// `kTabStripPadding(6)`.
const TAB_STRIP_HEIGHT: f64 = 41.0;

/// The whole chrome webview: tab strip above, toolbar below.
const CHROME_HEIGHT: f64 = TAB_STRIP_HEIGHT + TOOLBAR_HEIGHT;

/// The shim, injected into every page the content webview loads.
///
/// Two pieces: the GATT name tables `build.rs` generates from the registry
/// `webbluetooth-core` vendors, and the implementation. They are wrapped in
/// one function so that neither the tables nor anything the shim declares
/// becomes a global the page can see or collide with — the only thing it
/// leaves behind is `navigator.bluetooth` and the interface constructors.
fn shim() -> String {
    // The only thing the host tells the shim about itself. False in every
    // ordinary run; see `selftest::DEVICE_ENV` for why a self-test pointed at
    // a named device turns the gesture requirement off, and why that is
    // gated on two environment variables rather than one.
    let ungated = crate::selftest::AutoPick::from_env().is_some();
    format!(
        "(function () {{\n'use strict';\nconst SELFTEST_UNGATED = {ungated};\n{tables}\n{body}\n}})();",
        tables = include_str!(concat!(env!("OUT_DIR"), "/assigned.js")),
        body = include_str!("../shim/webbluetooth.js"),
    )
}

/// The start page's path within the bundled assets.
const HOME_PATH: &str = "home.html";

/// How long a navigation may take before it is called a failure.
///
/// Neither Tauri nor wry surfaces WKWebView's `didFailProvisionalNavigation`,
/// so there is no event that says a page could not be reached — a bad host
/// just leaves the old document on screen with no explanation, which is the
/// single worst thing a browser can do silently. A watchdog is the available
/// signal: a load that has neither finished nor been replaced by this point
/// gets an error page. Generous, because the cost of being wrong is
/// interrupting a slow page that would have arrived.
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(12);

/// Where the content webview currently is, and where "home" is.
pub struct Navigation {
    home: std::sync::Mutex<Option<url::Url>>,
    /// Per tab: the load being waited on, and a sequence number so a
    /// watchdog can tell "still this one" from "replaced while I slept".
    pending: std::sync::Mutex<std::collections::HashMap<String, (u64, String)>>,
    next: std::sync::atomic::AtomicU64,
}

impl Navigation {
    fn new() -> Self {
        Self {
            home: std::sync::Mutex::new(None),
            pending: std::sync::Mutex::new(std::collections::HashMap::new()),
            next: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn home_url(&self) -> Option<url::Url> {
        self.home.lock().ok().and_then(|h| h.clone())
    }

    /// Note that a load has begun in a tab, and hand back its sequence
    /// number.
    fn begin(&self, tab: &str, url: &str) -> u64 {
        let seq = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(tab.to_string(), (seq, url.to_string()));
        }
        seq
    }

    fn settle(&self, tab: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(tab);
        }
    }

    /// The URL still waiting in this tab under this sequence number.
    fn still_pending(&self, tab: &str, seq: u64) -> Option<String> {
        self.pending
            .lock()
            .ok()
            .and_then(|p| p.get(tab).cloned())
            .filter(|(current, _)| *current == seq)
            .map(|(_, url)| url)
    }
}

/// One tab, as the strip draws it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    /// The content webview's label, which is also the tab's identity
    /// everywhere else — handles, grants and connections are all keyed on it.
    pub label: String,
    pub title: String,
    pub url: String,
    pub loading: bool,
}

/// Which tabs exist and which one is in front.
#[derive(Default)]
pub struct Tabs {
    inner: std::sync::Mutex<TabsInner>,
}

#[derive(Default)]
struct TabsInner {
    tabs: Vec<Tab>,
    active: usize,
    next: u64,
}

impl Tabs {
    fn lock(&self) -> std::sync::MutexGuard<'_, TabsInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn next_label(&self) -> String {
        let mut inner = self.lock();
        inner.next += 1;
        format!("{CONTENT_PREFIX}{}", inner.next)
    }

    /// The tab in front, if there is one.
    pub fn active_label(&self) -> Option<String> {
        let inner = self.lock();
        inner.tabs.get(inner.active).map(|t| t.label.clone())
    }

    /// Every tab, in strip order.
    pub fn list(&self) -> Vec<Tab> {
        self.lock().tabs.clone()
    }

    fn update(&self, label: &str, edit: impl FnOnce(&mut Tab)) {
        let mut inner = self.lock();
        if let Some(tab) = inner.tabs.iter_mut().find(|t| t.label == label) {
            edit(tab);
        }
    }
}

/// Open a tab, and bring it to the front.
pub fn open_tab(app: &AppHandle, url: Option<url::Url>) -> tauri::Result<String> {
    let Some(window) = app.get_window(WINDOW) else {
        return Err(tauri::Error::WebviewNotFound);
    };
    let label = app.state::<Tabs>().next_label();
    let target = match url {
        Some(url) => WebviewUrl::External(url),
        None => WebviewUrl::App(HOME_PATH.into()),
    };

    let (width, height) = window_size(app);
    window.add_child(
        content_builder(app, &label, target),
        LogicalPosition::new(0.0, CHROME_HEIGHT),
        LogicalSize::new(width, (height - CHROME_HEIGHT).max(1.0)),
    )?;

    {
        let tabs = app.state::<Tabs>();
        let mut inner = tabs.lock();
        inner.tabs.push(Tab {
            label: label.clone(),
            title: "New tab".into(),
            url: String::new(),
            loading: true,
        });
        inner.active = inner.tabs.len() - 1;
    }
    activate(app, &label);
    Ok(label)
}

/// Close a tab. The last one is replaced rather than removed, so the window
/// never ends up showing nothing at all.
pub fn close_tab(app: &AppHandle, label: &str) {
    let remaining = {
        let tabs = app.state::<Tabs>();
        let mut inner = tabs.lock();
        let Some(position) = inner.tabs.iter().position(|t| t.label == label) else {
            return;
        };
        inner.tabs.remove(position);
        if inner.active >= position && inner.active > 0 {
            inner.active -= 1;
        }
        inner.tabs.len()
    };

    // Everything the document held goes with it — the same teardown a
    // navigation does, because as far as the radio is concerned it is one.
    app.state::<Browser>().page_unloaded(label);
    if let Some(webview) = app.get_webview(label) {
        let _ = webview.close();
    }

    if remaining == 0 {
        let _ = open_tab(app, None);
    } else if let Some(next) = app.state::<Tabs>().active_label() {
        activate(app, &next);
    }
    refresh_chrome(app);
}

/// Bring one tab to the front.
pub fn activate(app: &AppHandle, label: &str) {
    {
        let tabs = app.state::<Tabs>();
        let mut inner = tabs.lock();
        if let Some(position) = inner.tabs.iter().position(|t| t.label == label) {
            inner.active = position;
        }
    }
    for tab in app.state::<Tabs>().list() {
        if let Some(webview) = app.get_webview(&tab.label) {
            if tab.label == label {
                let _ = webview.show();
                let _ = webview.set_focus();
            } else {
                let _ = webview.hide();
            }
        }
    }
    layout(app);
    refresh_chrome(app);
}

/// The webview of the tab in front.
pub fn active_content(app: &AppHandle) -> Option<Webview> {
    app.state::<Tabs>()
        .active_label()
        .and_then(|label| app.get_webview(&label))
}

fn window_size(app: &AppHandle) -> (f64, f64) {
    let Some(window) = app.get_window(WINDOW) else {
        return (0.0, 0.0);
    };
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return (0.0, 0.0);
    };
    (size.width as f64 / scale, size.height as f64 / scale)
}

/// Build the window, the toolbar, and a first tab.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    app.manage(Navigation::new());
    app.manage(Tabs::default());

    let window = tauri::window::WindowBuilder::new(app, WINDOW)
        .title("WebBluetooth Browser")
        .inner_size(1180.0, 820.0)
        .min_inner_size(520.0, 380.0)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .build()?;

    let (width, _) = window_size(app);
    // Not `auto_resize`: that fills the window, and these are tiled.
    // `layout` owns every rectangle so there is one description of where
    // things go rather than several that can disagree.
    window.add_child(
        tauri::webview::WebviewBuilder::new(CHROME, WebviewUrl::App("chrome.html".into())),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(width, CHROME_HEIGHT),
    )?;

    let first = open_tab(app, None)?;
    if let Some(content) = app.get_webview(&first) {
        if let Ok(url) = content.url() {
            if let Ok(mut home) = app.state::<Navigation>().home.lock() {
                *home = Some(url);
            }
        }
    }

    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Resized(_) = event {
            layout(&handle);
        }
    });

    app.state::<Settings>().apply(app);
    watch_adapter(app);
    Ok(())
}

/// A webview for one tab: the shim, and the hooks that keep the strip and
/// the permission state honest about which document is where.
fn content_builder(
    app: &AppHandle,
    label: &str,
    target: WebviewUrl,
) -> tauri::webview::WebviewBuilder<tauri::Wry> {
    let navigating = app.clone();
    let owner = label.to_string();
    let loading = app.clone();
    let loaded_label = label.to_string();
    let popup = app.clone();

    tauri::webview::WebviewBuilder::new(label, target)
        // This is the whole point of the application: every document a tab
        // loads gets `navigator.bluetooth` before its own scripts run. Tauri
        // injects its IPC bootstrap into the main frame of every page
        // regardless of origin, so the shim can rely on being able to call
        // in — and into the main frame *only*, which is why a subframe gets
        // neither. See the frame-isolation phase of the self-test.
        .initialization_script(shim())
        .on_navigation(move |url| {
            on_navigation(&navigating, &owner, url);
            true
        })
        .on_page_load({
            let app = loading.clone();
            let label = loaded_label.clone();
            move |webview, payload| match payload.event() {
                tauri::webview::PageLoadEvent::Started => {
                    set_loading(&app, &label, true);
                }
                tauri::webview::PageLoadEvent::Finished => {
                    app.state::<Navigation>().settle(&label);
                    set_loading(&app, &label, false);
                    read_title(&app, &label, &webview);
                    refresh_chrome(&app);
                }
            }
        })
        // A tabbed browser answers `target="_blank"` with a tab, not with a
        // second window that would have no toolbar, no tab strip and no way
        // to see which site had been granted what.
        .on_new_window(move |url, _features| {
            let _ = open_tab(&popup, Some(url));
            tauri::webview::NewWindowResponse::Deny
        })
}

/// Report the radio coming and going.
///
/// `navigator.bluetooth.onavailabilitychanged` is the only part of the API a
/// page cannot poll for itself, and the toolbar's indicator wants the same
/// signal. One watcher serves both, and every tab hears it.
fn watch_adapter(app: &AppHandle) {
    let bluetooth = app.state::<Browser>().bluetooth.clone();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut changes = bluetooth.watch_availability();
        while let Some(state) = changes.next().await {
            let payload = serde_json::json!({ "available": is_available(state) });
            for tab in handle.state::<Tabs>().list() {
                if let Some(content) = handle.get_webview(&tab.label) {
                    crate::bridge::push(&content, "availabilitychanged", payload.clone());
                }
            }
            refresh_chrome(&handle);
        }
    });
}

/// "Is there a radio this process could use" — the same question
/// `getAvailability()` answers, so the event and the getter agree.
fn is_available(state: std::result::Result<(), webbluetooth::Availability>) -> bool {
    !matches!(
        state,
        Err(webbluetooth::Availability::Unsupported)
            | Err(webbluetooth::Availability::Unauthorized)
    )
}

/// Ask the page what it is called, for the tab strip.
fn read_title(app: &AppHandle, label: &str, webview: &Webview) {
    let app = app.clone();
    let label = label.to_string();
    let _ = webview.eval_with_callback("document.title", move |title| {
        // Arrives JSON-encoded; an empty title is better shown as the host.
        let title = serde_json::from_str::<String>(&title).unwrap_or_default();
        app.state::<Tabs>().update(&label, |tab| {
            if !title.trim().is_empty() {
                tab.title = title;
            }
        });
        refresh_chrome(&app);
    });
}

/// Keep the two webviews tiled after a resize.
///
/// The toolbar auto-resizes its width but the content webview's *offset* is
/// ours to maintain, so both are set explicitly rather than half of it being
/// left to `auto_resize`.
fn layout(app: &AppHandle) {
    let (width, height) = window_size(app);
    if width <= 0.0 {
        return;
    }

    if let Some(chrome) = app.get_webview(CHROME) {
        let _ = chrome.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = chrome.set_size(LogicalSize::new(width, CHROME_HEIGHT));
    }
    // Every tab is sized, not only the visible one: a hidden webview that
    // kept a stale size would reflow the moment it was shown, and a page
    // that had laid itself out for the old width would jump.
    for tab in app.state::<Tabs>().list() {
        if let Some(content) = app.get_webview(&tab.label) {
            let _ = content.set_position(LogicalPosition::new(0.0, CHROME_HEIGHT));
            let _ = content.set_size(LogicalSize::new(width, (height - CHROME_HEIGHT).max(1.0)));
        }
    }
}

/// A document is going away.
///
/// Everything the old one held goes with it — GATT connections, notification
/// pumps, every handle — which is what a browser does across a navigation.
/// The *permissions* stay, keyed by origin, so a reload does not re-prompt.
/// Record that a tab is loading, and tell the toolbar if it is the visible
/// one. A background tab's spinner belongs on its own tab, not on the
/// address bar.
fn set_loading(app: &AppHandle, tab: &str, loading: bool) {
    app.state::<Tabs>().update(tab, |t| t.loading = loading);
    refresh_chrome(app);
}

fn on_navigation(app: &AppHandle, tab: &str, url: &url::Url) {
    // Only this tab's document is going away.
    app.state::<Browser>().page_unloaded(tab);
    app.state::<Tabs>().update(tab, |t| {
        t.loading = true;
        t.url = url.to_string();
        t.title = url.host_str().unwrap_or("New tab").to_string();
    });

    // Pages this browser serves are never unreachable, and watching them
    // would mean the error page could time itself out.
    if url.scheme() != "tauri" {
        let seq = app.state::<Navigation>().begin(tab, url.as_str());
        let handle = app.clone();
        let tab = tab.to_string();
        tauri::async_runtime::spawn(async move {
            tokio_sleep(NAVIGATION_TIMEOUT).await;
            if let Some(stalled) = handle.state::<Navigation>().still_pending(&tab, seq) {
                handle.state::<Navigation>().settle(&tab);
                show_error(&handle, &tab, &stalled, "took too long to respond");
            }
        });
    }

    // The toolbar reads the URL back from the webview, which has not been
    // updated yet at this point; ask it to refresh once this settles.
    let handle = app.clone();
    let shown = url.to_string();
    tauri::async_runtime::spawn(async move {
        if let Some(chrome) = handle.get_webview(CHROME) {
            let json = serde_json::to_string(&shown).unwrap_or_else(|_| "\"\"".into());
            let _ = chrome.eval(format!(
                "window.__CHROME__&&window.__CHROME__.navigated({json})"
            ));
        }
    });
}

/// Sleep without pulling in a runtime dependency of our own.
async fn tokio_sleep(duration: Duration) {
    // The library ships a runtime-agnostic timer; reuse it rather than
    // reaching for tokio directly, which is Tauri's to choose.
    webbluetooth::sleep(duration).await;
}

/// Replace the page with something that says what went wrong.
///
/// The failed URL travels in the fragment rather than the query so that it
/// never reaches a server and never appears in an asset request — there is no
/// server here, but a URL that was mistyped badly enough to fail is exactly
/// the kind of thing not to echo anywhere it could be logged.
fn show_error(app: &AppHandle, tab: &str, failed: &str, reason: &str) {
    let Some(content) = app.get_webview(tab) else {
        return;
    };
    let target = format!(
        "tauri://localhost/error.html#url={}&reason={}",
        form_urlencode(failed),
        form_urlencode(reason)
    );
    if let Ok(url) = url::Url::parse(&target) {
        let _ = content.navigate(url);
    }
    set_loading(app, tab, false);
}

/// Ask the toolbar to re-read the browser's state.
///
/// Pushing a redraw rather than the state itself keeps one description of
/// what the toolbar shows — `ui_state` — instead of two that can disagree.
pub fn refresh_chrome(app: &AppHandle) {
    if let Some(chrome) = app.get_webview(CHROME) {
        let _ = chrome.eval("window.__CHROME__&&window.__CHROME__.refresh()");
    }
}

// ---- what the toolbar shows ----------------------------------------------

/// A device one origin holds, for the permission popover.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantedDevice {
    pub id: String,
    pub name: Option<String>,
    pub connected: bool,
}

/// Everything the toolbar draws.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeState {
    pub url: String,
    pub origin: String,
    /// Whether this origin is one Web Bluetooth is offered on at all.
    pub secure: bool,
    /// `ready`, `off`, `unauthorized`, `unsupported` or `unknown`.
    pub adapter: &'static str,
    pub devices: Vec<GrantedDevice>,
    /// `system`, `light` or `dark`.
    pub theme: &'static str,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<String>,
}

/// Read the current state of the browser for the toolbar.
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
        })
        .collect();

    Ok(ChromeState {
        secure: parsed.as_ref().is_some_and(origin::is_secure),
        url,
        origin,
        adapter,
        devices,
        theme: app.state::<Settings>().theme().as_str(),
        tabs: app.state::<Tabs>().list(),
        active_tab: app.state::<Tabs>().active_label(),
    })
}

/// Open a tab.
#[tauri::command]
pub fn ui_tab_new(app: AppHandle) -> Result<()> {
    open_tab(&app, None).map_err(|e| JsError::invalid_state(e.to_string()))?;
    Ok(())
}

/// Close one.
#[tauri::command]
pub fn ui_tab_close(app: AppHandle, label: String) -> Result<()> {
    close_tab(&app, &label);
    Ok(())
}

/// Bring one to the front.
#[tauri::command]
pub fn ui_tab_select(app: AppHandle, label: String) -> Result<()> {
    activate(&app, &label);
    Ok(())
}

/// Follow the system, or pin light or dark.
#[tauri::command]
pub fn ui_set_theme(app: AppHandle, theme: String) -> Result<()> {
    app.state::<Settings>()
        .set_theme(&app, ThemeChoice::parse(&theme));
    Ok(())
}

// ---- navigation ----------------------------------------------------------

/// The page the user is looking at. Every toolbar action means "the tab in
/// front", never a particular webview.
fn content(app: &AppHandle) -> Option<Webview> {
    active_content(app)
}

/// Go to what the user typed.
#[tauri::command]
pub fn ui_navigate(app: AppHandle, input: String) -> Result<()> {
    let url = resolve(&input)?;
    content(&app)
        .ok_or_else(|| JsError::invalid_state("no content webview"))?
        .navigate(url)
        .map_err(|e| JsError::invalid_state(e.to_string()))
}

/// Turn address-bar input into a URL.
///
/// The rules a browser uses: something with a scheme is a URL, something that
/// looks like a host is one with `https://` in front, and anything else is a
/// search. Defaulting to `https` rather than `http` matters here beyond
/// habit — Web Bluetooth is only offered on a secure context, so guessing
/// `http` would silently produce a page that cannot use the radio.
fn resolve(input: &str) -> Result<url::Url> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(JsError::type_error("nothing to open"));
    }

    if let Ok(url) = url::Url::parse(trimmed) {
        if url.scheme() != "javascript" {
            return Ok(url);
        }
        // `javascript:` in an address bar is the classic self-XSS vector and
        // a browser has no reason to honour it.
        return Err(JsError::security("javascript: URLs are not accepted here"));
    }

    let looks_like_a_host = !trimmed.contains(char::is_whitespace)
        && trimmed.contains('.')
        && !trimmed.starts_with('.')
        && !trimmed.ends_with('.');
    if looks_like_a_host || trimmed.starts_with("localhost") {
        if let Ok(url) = url::Url::parse(&format!("https://{trimmed}")) {
            return Ok(url);
        }
    }

    let query: String = form_urlencode(trimmed);
    url::Url::parse(&format!("https://duckduckgo.com/?q={query}"))
        .map_err(|e| JsError::type_error(e.to_string()))
}

/// Percent-encode a search term. Small enough not to be worth a dependency.
fn form_urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Back, using the content webview's own session history.
#[tauri::command]
pub fn ui_back(app: AppHandle) -> Result<()> {
    eval_in_content(&app, "history.back()")
}

/// Forward.
#[tauri::command]
pub fn ui_forward(app: AppHandle) -> Result<()> {
    eval_in_content(&app, "history.forward()")
}

/// Reload.
#[tauri::command]
pub fn ui_reload(app: AppHandle) -> Result<()> {
    eval_in_content(&app, "location.reload()")
}

/// Back to the start page.
#[tauri::command]
pub fn ui_home(app: AppHandle) -> Result<()> {
    go_home(&app)
}

/// Run a script in the page, ignoring failure. For the menu, which has
/// nowhere to report one.
pub fn eval_in_content_public(app: &AppHandle, script: &str) {
    let _ = eval_in_content(app, script);
}

/// Navigate the page to the start page.
pub fn go_home(app: &AppHandle) -> Result<()> {
    let home = app
        .state::<Navigation>()
        .home_url()
        .ok_or_else(|| JsError::invalid_state("no start page"))?;
    content(app)
        .ok_or_else(|| JsError::invalid_state("no content webview"))?
        .navigate(home)
        .map_err(|e| JsError::invalid_state(e.to_string()))
}

fn eval_in_content(app: &AppHandle, script: &str) -> Result<()> {
    content(app)
        .ok_or_else(|| JsError::invalid_state("no content webview"))?
        .eval(script)
        .map_err(|e| JsError::invalid_state(e.to_string()))
}

/// Take a device away from the site currently loaded.
#[tauri::command]
pub fn ui_revoke_origin(
    app: AppHandle,
    state: tauri::State<'_, Browser>,
    device_id: String,
) -> Result<()> {
    let origin = content(&app)
        .and_then(|c| c.url().ok())
        .as_ref()
        .and_then(origin::of)
        .ok_or_else(|| JsError::invalid_state("no page is loaded"))?;
    state.revoke(&origin, &device_id);
    Ok(())
}

/// Open the permissions manager.
#[tauri::command]
pub fn ui_open_permissions(app: AppHandle) -> Result<()> {
    crate::manager::open(&app).map_err(|e| JsError::invalid_state(e.to_string()))
}

/// Open the web inspector on the page, which is the only way to debug a shim
/// that is not behaving. `tauri`'s `devtools` feature is on in this crate, so
/// it is there in a release build too — a browser whose inspector disappears
/// when you stop using a debug build is not much of a browser.
#[tauri::command]
pub fn ui_open_devtools(app: AppHandle) -> Result<()> {
    content(&app)
        .ok_or_else(|| JsError::invalid_state("no content webview"))?
        .open_devtools();
    Ok(())
}
