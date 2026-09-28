//! The device picker.
//!
//! `requestDevice()` is defined around a user gesture and a chooser dialog,
//! and that consent step is the reason the API is safe to hand to an arbitrary
//! website at all. The library makes it a trait precisely because a library
//! has no chrome to draw it in; a browser does, so this is the implementation
//! that was always missing.
//!
//! It is its own window, not a panel inside the page's webview. A page must
//! not be able to draw over the picker, read which devices are nearby, or see
//! anything but the one identifier the user chose.

use std::collections::HashMap;
use std::sync::Mutex;

use futures_channel::oneshot;
use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use webbluetooth::chooser::{Candidates, DeviceChooser};

use crate::dto::AdvertisingEvent;
use crate::error::{JsError, Result};

/// The label of the picker window, and of its webview.
pub const WINDOW: &str = "chooser";

/// What the picker tells the user it is picking for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    /// The site asking, as `https://example.com`.
    pub origin: String,
    /// The request's filters, rendered for a human: "name starts with Polar".
    pub wants: Vec<String>,
    /// `acceptAllDevices` — every device nearby is offered.
    pub accept_all: bool,
}

struct Active {
    prompt: Prompt,
    /// Fired once the picker's own script is up. Evaluating into the window
    /// before that would land on a page that has no handler yet, and the
    /// candidate would be dropped silently.
    ready: Option<oneshot::Sender<()>>,
    answer: Option<oneshot::Sender<Option<String>>>,
}

/// The one picker that can be open at a time.
#[derive(Default)]
pub struct ChooserState {
    active: Mutex<Option<Active>>,
}

impl ChooserState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Settle the open request, if there is one. Later calls do nothing.
    fn answer(&self, chosen: Option<String>) {
        if let Some(active) = self.lock().as_mut() {
            if let Some(tx) = active.answer.take() {
                let _ = tx.send(chosen);
            }
        }
    }
}

/// The picker's script is up and wants to know what it is asking for.
#[tauri::command]
pub fn chooser_ready(state: tauri::State<'_, ChooserState>) -> Result<Prompt> {
    let mut slot = state.lock();
    let active = slot
        .as_mut()
        .ok_or_else(|| JsError::invalid_state("no device request is open"))?;
    if let Some(tx) = active.ready.take() {
        let _ = tx.send(());
    }
    Ok(active.prompt.clone())
}

/// The user chose a device.
#[tauri::command]
pub fn chooser_pick(id: String, state: tauri::State<'_, ChooserState>) {
    state.answer(Some(id));
}

/// The user dismissed the picker. `requestDevice()` rejects with
/// `NotFoundError`, which is what a browser reports for a cancelled chooser.
#[tauri::command]
pub fn chooser_cancel(state: tauri::State<'_, ChooserState>) {
    state.answer(None);
}

/// Drives the picker window for one `requestDevice()` call.
pub struct ChooserUi {
    app: AppHandle,
    prompt: Prompt,
}

impl ChooserUi {
    /// Build a chooser for one request from one origin.
    pub fn new(app: AppHandle, prompt: Prompt) -> Self {
        Self { app, prompt }
    }
}

impl DeviceChooser for ChooserUi {
    fn choose(&self, candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        let app = self.app.clone();
        let prompt = self.prompt.clone();
        Box::pin(async move { run(app, prompt, candidates).await })
    }
}

async fn run(app: AppHandle, prompt: Prompt, mut candidates: Candidates) -> Option<String> {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (answer_tx, answer_rx) = oneshot::channel();

    {
        let state = app.state::<ChooserState>();
        let mut slot = state.lock();
        if slot.is_some() {
            // A second picker while one is open. A browser ignores the later
            // request rather than stacking dialogs; `None` surfaces to the
            // page as the NotFoundError a dismissed chooser gives.
            return None;
        }
        *slot = Some(Active {
            prompt: prompt.clone(),
            ready: Some(ready_tx),
            answer: Some(answer_tx),
        });
    }

    let window = match build_window(&app) {
        Ok(window) => {
            // A new window starts on the system theme; re-apply the choice so
            // the picker does not flash the wrong one on a platform where the
            // setting is per-window.
            app.state::<crate::settings::Settings>().apply(&app);
            window
        }
        Err(error) => {
            eprintln!("could not open the device chooser: {error}");
            app.state::<ChooserState>().lock().take();
            return None;
        }
    };

    // Closing the window is a refusal. Without this the request would hang
    // on a channel nothing can ever fire.
    {
        let app = app.clone();
        window.on_window_event(move |event| {
            if matches!(
                event,
                WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
            ) {
                app.state::<ChooserState>().answer(None);
            }
        });
    }

    let pump = tauri::async_runtime::spawn({
        let window = window.clone();
        async move {
            if ready_rx.await.is_err() {
                return;
            }
            // One row per device, updated in place. A scan reports the same
            // peripheral over and over, and a list that grew a row each time
            // would be unusable.
            let mut seen: HashMap<String, AdvertisingEvent> = HashMap::new();
            while let Some(candidate) = candidates.next().await {
                let event = AdvertisingEvent::new(
                    &candidate.id,
                    candidate.name.clone(),
                    &candidate.advertisement,
                );
                let previous = seen.insert(candidate.id.clone(), event.clone());
                // Redraw only when something a user can see has changed.
                if previous.as_ref().is_some_and(|p| {
                    p.name == event.name && p.rssi == event.rssi && p.uuids == event.uuids
                }) {
                    continue;
                }
                let Ok(json) = serde_json::to_string(&event) else {
                    continue;
                };
                let _ = window.eval(format!(
                    "window.__CHOOSER__ && window.__CHOOSER__.candidate({json})"
                ));
            }
        }
    });

    let chosen = answer_rx.await.ok().flatten();

    pump.abort();
    dismiss(&window);
    app.state::<ChooserState>().lock().take();
    chosen
}

/// Where the picker is drawn: its own window, on every platform.
///
/// This is the boundary the whole consent model rests on — a page must not
/// be able to paint over the thing that grants it access, or read the list
/// of devices it offers. iOS has no *child* webviews, which is why the
/// toolbar has to live inside the page there, but it does support a second
/// window, so the picker keeps its own.
type Surface = tauri::WebviewWindow;

fn build_window(app: &AppHandle) -> tauri::Result<Surface> {
    // A stale window from an aborted request would make `build` fail on a
    // duplicate label.
    if let Some(existing) = app.get_webview_window(WINDOW) {
        let _ = existing.destroy();
    }
    let builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("chooser.html".into()))
        .title("Connect to a device")
        .inner_size(420.0, 440.0)
        .min_inner_size(340.0, 280.0)
        .resizable(true);

    // Not on mobile: there is no window furniture to configure there, and
    // the builder does not carry these.
    #[cfg(desktop)]
    let builder = builder.minimizable(false).always_on_top(true).center();

    builder.build()
}

fn dismiss(surface: &Surface) {
    let _ = surface.destroy();
}
