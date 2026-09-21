//! Choosing a device — the part a browser draws a dialog for.
//!
//! `requestDevice()` is defined around a user gesture and a chooser UI: the
//! page proposes filters, the *browser* scans, and a human picks one device.
//! That consent step is the whole reason the API is safe to expose, so it is
//! kept here as a real extension point rather than quietly dropped. A library
//! has no chrome to draw, so the decision is a trait: implement it against
//! whatever UI the application has, or use one of the built-ins.
//!
//! The candidate stream may report the same device more than once — every
//! advertisement is forwarded, so RSSI keeps updating while a chooser
//! deliberates. Choosers that present a list should deduplicate by
//! [`Candidate::id`].

use crate::filter::Advertisement;
use crate::timer;
use futures_core::Stream;
// Re-exported rather than merely imported: `DeviceChooser::choose` returns
// one, so implementing the trait means naming the type — and without this,
// that meant adding `futures-util` to your own Cargo.toml to implement a trait
// from this one. Both of this crate's chooser examples had to. It is the same
// gap the prelude was built to close for `StreamExt`, left open one level
// down.
pub use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

/// A device seen during a scan that matched the request's filters.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The per-host identifier — `BluetoothDevice.id`. Stable for this machine
    /// and this peripheral; not the Bluetooth address, which Apple never
    /// exposes.
    pub id: String,
    /// The device's name, from the GAP name or the advertised local name.
    pub name: Option<String>,
    /// The advertisement that matched.
    pub advertisement: Advertisement,
}

impl Candidate {
    /// Signal strength in dBm, or `None` if the radio did not report one.
    pub fn rssi(&self) -> Option<i32> {
        (self.advertisement.rssi != crate::filter::UNAVAILABLE_RSSI)
            .then_some(self.advertisement.rssi)
    }

    /// Something printable: the name if there is one, else the identifier.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// A live stream of matching devices, ending when the scan stops.
pub struct Candidates {
    /// The sightings, as they arrive. Bounded: a chooser that stops reading
    /// loses the oldest rather than stalling the radio.
    pub inner: crate::backlog::Receiver<Candidate>,
}

impl Stream for Candidates {
    type Item = Candidate;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Candidate>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

/// Decides which device `request_device` returns.
///
/// Return the chosen [`Candidate::id`], or `None` to refuse — which surfaces to
/// the caller as `NotFoundError`, the same thing a browser reports when a user
/// dismisses the picker.
pub trait DeviceChooser: Send + Sync + 'static {
    /// Pick a device, or `None` to cancel the request.
    ///
    /// Returns the chosen device's id. The stream ends when the scan does, so
    /// a chooser that reads to the end has seen everything there was.
    fn choose(&self, candidates: Candidates) -> BoxFuture<'static, Option<String>>;
}

/// Take the first device that matches.
///
/// The default. Convenient and fully automatic — and worth being deliberate
/// about, because it removes the consent step the Web Bluetooth security model
/// is built on. Prefer a chooser that involves the operator for anything
/// user-facing.
#[derive(Debug, Clone)]
pub struct FirstMatch {
    timeout: Option<Duration>,
}

impl FirstMatch {
    /// Wait indefinitely for a match.
    pub fn new() -> Self {
        Self { timeout: None }
    }

    /// Give up after `timeout` with `NotFoundError`.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
        }
    }
}

impl Default for FirstMatch {
    fn default() -> Self {
        Self::with_timeout(Duration::from_secs(10))
    }
}

impl DeviceChooser for FirstMatch {
    fn choose(&self, mut candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        let timeout = self.timeout;
        Box::pin(async move {
            let first = async move { candidates.next().await.map(|c| c.id) };
            match timeout {
                None => first.await,
                Some(d) => timer::timeout(d, first).await.ok().flatten(),
            }
        })
    }
}

/// Collect for a window, then take the strongest signal.
///
/// Useful when several identical devices are in range and the nearest one is
/// meant — a rack of sensors, say. Costs the full window in latency even if the
/// right device answers immediately.
#[derive(Debug, Clone)]
pub struct StrongestSignal {
    window: Duration,
}

impl StrongestSignal {
    /// Collect for `window`, then take the strongest signal seen.
    pub fn new(window: Duration) -> Self {
        Self { window }
    }
}

impl Default for StrongestSignal {
    fn default() -> Self {
        Self::new(Duration::from_secs(4))
    }
}

impl DeviceChooser for StrongestSignal {
    fn choose(&self, candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        let window = self.window;
        Box::pin(async move {
            let seen = collect_for(candidates, window).await;
            seen.into_iter()
                .max_by_key(|c| c.rssi().unwrap_or(i32::MIN))
                .map(|c| c.id)
        })
    }
}

/// Run a closure over each candidate; the first `true` wins.
///
/// The escape hatch for application-specific logic that does not warrant its
/// own type — matching a serial number out of the manufacturer data, say.
pub struct Predicate<F>(std::sync::Arc<F>);

impl<F> Predicate<F>
where
    F: Fn(&Candidate) -> bool + Send + Sync + 'static,
{
    /// Take the first candidate the predicate accepts.
    pub fn new(predicate: F) -> Self {
        Self(std::sync::Arc::new(predicate))
    }
}

impl<F> DeviceChooser for Predicate<F>
where
    F: Fn(&Candidate) -> bool + Send + Sync + 'static,
{
    fn choose(&self, mut candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        let predicate = self.0.clone();
        Box::pin(async move {
            while let Some(c) = candidates.next().await {
                if predicate(&c) {
                    return Some(c.id);
                }
            }
            None
        })
    }
}

/// Gather every candidate seen during `window`, deduplicated by id, keeping the
/// most recent advertisement for each.
async fn collect_for(mut candidates: Candidates, window: Duration) -> Vec<Candidate> {
    let mut seen: HashMap<String, Candidate> = HashMap::new();
    let gather = async {
        while let Some(c) = candidates.next().await {
            seen.insert(c.id.clone(), c);
        }
    };
    let _ = timer::timeout(window, gather).await;
    seen.into_values().collect()
}

/// A numbered list on stdout and a line read from stdin.
///
/// The closest thing a terminal program has to the browser's picker, and the
/// only built-in chooser that preserves the consent step. Behind the
/// `terminal-chooser` feature.
#[cfg(feature = "terminal-chooser")]
#[derive(Debug, Clone)]
pub struct TerminalChooser {
    window: Duration,
    prompt: String,
}

#[cfg(feature = "terminal-chooser")]
impl TerminalChooser {
    /// Scan for `window`, then prompt.
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            prompt: "Select a device".into(),
        }
    }

    /// Replace the prompt text.
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }
}

#[cfg(feature = "terminal-chooser")]
impl Default for TerminalChooser {
    fn default() -> Self {
        Self::new(Duration::from_secs(5))
    }
}

#[cfg(feature = "terminal-chooser")]
impl DeviceChooser for TerminalChooser {
    fn choose(&self, candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        use std::io::Write;
        let (window, prompt) = (self.window, self.prompt.clone());
        Box::pin(async move {
            eprintln!("Scanning for {:.0}s…", window.as_secs_f32());
            let mut found = collect_for(candidates, window).await;
            if found.is_empty() {
                eprintln!("No matching devices found.");
                return None;
            }
            found.sort_by_key(|c| std::cmp::Reverse(c.rssi().unwrap_or(i32::MIN)));

            eprintln!();
            for (i, c) in found.iter().enumerate() {
                match c.rssi() {
                    Some(db) => {
                        eprintln!("  [{}] {:<28} {:>4} dBm  {}", i + 1, c.label(), db, c.id)
                    }
                    None => eprintln!("  [{}] {:<28} {:>8}  {}", i + 1, c.label(), "—", c.id),
                }
            }
            eprint!("\n{prompt} [1-{}, or Enter to cancel]: ", found.len());
            let _ = std::io::stderr().flush();

            // stdin is blocking; read it off the executor.
            let (tx, rx) = futures_channel::oneshot::channel();
            std::thread::spawn(move || {
                let mut line = String::new();
                let ok = std::io::stdin().read_line(&mut line).is_ok();
                let _ = tx.send(ok.then_some(line));
            });
            let line = rx.await.ok().flatten()?;

            let n: usize = line.trim().parse().ok()?;
            found.get(n.checked_sub(1)?).map(|c| c.id.clone())
        })
    }
}
