//! The scanning prompt.
//!
//! `requestLEScan()` is not the chooser with different words. The chooser
//! hands over one device a person pointed at; a scan hands over *every*
//! advertisement in the room, continuously, including from devices the person
//! has never heard of and cannot see. Chromium keeps a whole second
//! controller for this — `bluetooth_device_scanning_prompt_controller` — for
//! the same reason this is a separate window: granting it through the device
//! picker would be asking one question and answering another.
//!
//! The answer is remembered per origin, so a site polling advertisements does
//! not re-prompt on every call.

use std::sync::Mutex;

use futures_channel::oneshot;
use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::error::{JsError, Result};

/// The label of the prompt window, and of its webview.
pub const WINDOW: &str = "scanprompt";

/// What the prompt tells the user it is being asked for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanAsk {
    /// The site asking, as `https://example.com`.
    pub origin: String,
    /// The scan's filters, rendered for a human.
    pub wants: Vec<String>,
    /// `acceptAllAdvertisements` — everything in range, unfiltered.
    pub accept_all: bool,
}

struct Active {
    ask: ScanAsk,
    ready: Option<oneshot::Sender<()>>,
    answer: Option<oneshot::Sender<bool>>,
}

/// The one scanning prompt that can be open at a time.
#[derive(Default)]
pub struct ScanPromptState {
    active: Mutex<Option<Active>>,
}

impl ScanPromptState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn settle(&self, allowed: bool) {
        if let Some(active) = self.lock().as_mut() {
            if let Some(tx) = active.answer.take() {
                let _ = tx.send(allowed);
            }
        }
    }
}

/// The prompt's script is up and wants to know what it is asking about.
#[tauri::command]
pub fn scanprompt_ready(state: tauri::State<'_, ScanPromptState>) -> Result<ScanAsk> {
    let mut slot = state.lock();
    let active = slot
        .as_mut()
        .ok_or_else(|| JsError::invalid_state("no scan request is open"))?;
    if let Some(tx) = active.ready.take() {
        let _ = tx.send(());
    }
    Ok(active.ask.clone())
}

/// The user allowed or refused the scan.
#[tauri::command]
pub fn scanprompt_decide(allow: bool, state: tauri::State<'_, ScanPromptState>) {
    state.settle(allow);
}

/// Put the question to the user. `false` for anything that is not a clear yes.
pub async fn ask(app: &AppHandle, ask: ScanAsk) -> bool {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (answer_tx, answer_rx) = oneshot::channel();

    {
        let state = app.state::<ScanPromptState>();
        let mut slot = state.lock();
        if slot.is_some() {
            // A second prompt while one is open. Refuse rather than stack
            // dialogs, as with the chooser.
            return false;
        }
        *slot = Some(Active {
            ask,
            ready: Some(ready_tx),
            answer: Some(answer_tx),
        });
    }

    let window = match build_window(app) {
        Ok(window) => {
            app.state::<crate::settings::Settings>().apply(app);
            window
        }
        Err(error) => {
            eprintln!("could not open the scanning prompt: {error}");
            app.state::<ScanPromptState>().lock().take();
            return false;
        }
    };

    // Closing the window is a refusal, not a question left open.
    {
        let app = app.clone();
        window.on_window_event(move |event| {
            if matches!(
                event,
                WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
            ) {
                app.state::<ScanPromptState>().settle(false);
            }
        });
    }

    // Nothing is pumped into this window, but waiting for it to say it is
    // ready keeps the prompt from being answered before it has drawn.
    let _ = ready_rx.await;
    let allowed = answer_rx.await.unwrap_or(false);

    dismiss(&window);
    app.state::<ScanPromptState>().lock().take();
    allowed
}

/// Where the prompt is drawn: its own window, on every platform, for the
/// same reason as the chooser's.
type Surface = tauri::WebviewWindow;

fn build_window(app: &AppHandle) -> tauri::Result<Surface> {
    if let Some(existing) = app.get_webview_window(WINDOW) {
        let _ = existing.destroy();
    }
    let builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("scanprompt.html".into()))
        .title("Scan for devices")
        .inner_size(420.0, 300.0)
        .resizable(false);

    #[cfg(desktop)]
    let builder = builder.minimizable(false).always_on_top(true).center();

    builder.build()
}

fn dismiss(surface: &Surface) {
    let _ = surface.destroy();
}
