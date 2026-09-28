//! A headless check that the whole path works.
//!
//! Run with `WBB_SELFTEST=1`: the browser starts, loads its own start page,
//! interrogates the shim from the host side and prints what it found, then
//! exits. There is no other way to tell a shim that failed to install from
//! one that installed and then refused everything — both look like a page
//! that does nothing — and the difference matters enough to be checkable
//! without a person watching a window.
//!
//! It asserts what can be asserted without hardware or a human: that
//! `navigator.bluetooth` exists and is the shim's, that names resolve to the
//! same UUIDs Rust would resolve them to, that commands round-trip, and that
//! `requestDevice` refuses to open a chooser with no user gesture behind it.
//! Whether a radio answers is a separate question, reported and not asserted.

use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use tauri::{AppHandle, Manager};
use webbluetooth::chooser::{Candidates, DeviceChooser};
use webbluetooth::RequestDeviceOptions;

use crate::settings::{Settings, ThemeChoice};
use crate::shell;
use crate::state::Browser;

/// The environment variable that turns this on.
pub const ENV: &str = "WBB_SELFTEST";

/// Names a device for the GATT phase to pick, by substring of its label, or
/// `*` for whatever the page's own filters matched. Setting it at all turns
/// two consent steps off: the chooser window, and the user-gesture
/// requirement in front of it.
///
/// `*` is usually the right value. A candidate only reaches a chooser after
/// passing the `filters` in the `requestDevice` call, so a probe that filters
/// on the fixture's service UUID has already excluded everything else in the
/// room — and matching on the name is fragile besides: iOS drops an app's
/// advertised local name when it is not in the foreground, so the harness
/// shows up as "Rust iPad" or as plain "iPad" depending on the state of a
/// screen nobody is looking at.
///
/// Both of those are the reason the API is safe to hand to a website, so the
/// switch is deliberately awkward — it does nothing unless [`ENV`] is *also*
/// set, the state it controls is not even registered otherwise, and the run
/// exits as soon as the checks finish. It exists because the one thing no
/// amount of reading can establish is whether a real characteristic can be
/// read over a real radio through this whole stack.
pub const DEVICE_ENV: &str = "WBB_SELFTEST_DEVICE";

/// The service the GATT phase looks for. Defaults to the fixture published by
/// `crates/webbluetooth/examples/ios-harness.rs`.
pub const SERVICE_ENV: &str = "WBB_SELFTEST_SERVICE";
const DEFAULT_SERVICE: &str = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";

/// Set only when both [`ENV`] and [`DEVICE_ENV`] are present.
pub struct AutoPick {
    /// Lowercased substring of the device label to accept.
    needle: String,
}

impl AutoPick {
    /// Read the gate. `None` in every ordinary run, including a self-test
    /// that was not explicitly pointed at a device.
    pub fn from_env() -> Option<Self> {
        std::env::var_os(ENV)?;
        std::env::var(DEVICE_ENV).ok().and_then(|needle| {
            let needle = needle.trim().to_lowercase();
            (!needle.is_empty()).then_some(Self { needle })
        })
    }

    /// A chooser that takes the first candidate whose label matches, or the
    /// first of any when the needle is `*`.
    pub fn chooser(&self) -> Picker {
        Picker {
            needle: self.needle.clone(),
        }
    }
}

/// Stands in for the picker window when [`AutoPick`] is on.
pub struct Picker {
    needle: String,
}

impl DeviceChooser for Picker {
    fn choose(&self, mut candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        let needle = self.needle.clone();
        Box::pin(async move {
            webbluetooth::timeout(Duration::from_secs(25), async move {
                while let Some(candidate) = candidates.next().await {
                    if needle == "*" || candidate.label().to_lowercase().contains(&needle) {
                        return Some(candidate.id.clone());
                    }
                }
                None
            })
            .await
            .ok()
            .flatten()
        })
    }
}

/// The site the second phase loads. The point of the browser is that an
/// arbitrary origin gets the API, and that is a different code path from the
/// start page: Tauri checks its ACL on every command from a remote origin,
/// and only `capabilities/content.json` opens it. Set to an empty string to
/// skip, which is the right thing on a machine with no network.
pub const URL_ENV: &str = "WBB_SELFTEST_URL";
const DEFAULT_URL: &str = "https://example.com/";

/// Long enough for a webview to load a local page on a busy machine.
const DEADLINE: Duration = Duration::from_secs(20);

const PROBE: &str = r#"
(() => {
  if (window.__WBB_SELFTEST_STARTED__) { return 'running'; }
  window.__WBB_SELFTEST_STARTED__ = true;
  (async () => {
    const report = {
      shimInstalled: !!window.__WEBBLUETOOTH_SHIM__,
      secureContext: window.isSecureContext,
      origin: location.origin,
      hasNavigatorBluetooth: 'bluetooth' in navigator,
      bluetoothTag: Object.prototype.toString.call(navigator.bluetooth),
      interfaces: [
        'BluetoothDevice', 'BluetoothRemoteGATTServer', 'BluetoothRemoteGATTService',
        'BluetoothRemoteGATTCharacteristic', 'BluetoothRemoteGATTDescriptor',
        'BluetoothCharacteristicProperties', 'BluetoothUUID',
      ].filter((name) => typeof window[name] !== 'undefined'),
    };
    try {
      report.uuidService = BluetoothUUID.getService('heart_rate');
      report.uuidCharacteristic = BluetoothUUID.getCharacteristic('gap.device_name');
      report.uuidDescriptor = BluetoothUUID.getDescriptor('gatt.client_characteristic_configuration');
      report.uuidAlias = BluetoothUUID.canonicalUUID(0x180d);
    } catch (e) { report.uuidError = String(e); }

    // Namespaced resolution: the same name is a different UUID depending on
    // which table is asked, and getting that wrong is silent.
    try {
      report.namespaced = [
        BluetoothUUID.getService('current_time'),
        BluetoothUUID.getCharacteristic('current_time'),
      ];
    } catch (e) { report.namespacedError = String(e); }

    try { report.badName = BluetoothUUID.getService('not_a_real_service'); }
    catch (e) { report.badNameThrew = e.constructor.name; }

    // Bounded. On iOS the first call blocks until somebody answers the
    // system's Bluetooth prompt, and an unbounded await there means the
    // whole report is never written — the one failure mode that looks
    // identical to a crash.
    const withTimeout = (promise, ms, label) =>
      Promise.race([
        promise,
        new Promise((_, reject) =>
          setTimeout(() => reject(new Error(`${label} did not answer in ${ms}ms`)), ms)
        ),
      ]);
    try {
      report.availability = await withTimeout(
        navigator.bluetooth.getAvailability(), 6000, 'getAvailability()'
      );
    } catch (e) { report.availabilityError = e.name + ': ' + e.message; }

    try {
      report.grantedDevices = (
        await withTimeout(navigator.bluetooth.getDevices(), 6000, 'getDevices()')
      ).length;
    } catch (e) { report.getDevicesError = e.name + ': ' + e.message; }

    // Reported, not asserted. WebKit treats script the host evaluated as
    // user-initiated, so `isActive` is true for everything this probe does
    // and a chooser really would open — which says nothing about whether a
    // page's own load-time script could open one. There is no way to ask
    // that question from out here; see the note in `judge`.
    report.nativeUserActivation = !!navigator.userActivation;
    report.userActivationActive = !!(
      navigator.userActivation && navigator.userActivation.isActive
    );

    // Argument validation belongs to the page, thrown synchronously.
    try {
      await navigator.bluetooth.requestDevice({ filters: [] });
      report.emptyFilters = 'NOT REJECTED';
    } catch (e) { report.emptyFilters = e.constructor.name; }

    // requestLEScan: present, and refusing a dictionary that restricts
    // nothing before any prompt can appear. The allow/refuse path itself
    // needs a person, so it is not driven from here.
    report.hasRequestLEScan = typeof navigator.bluetooth.requestLEScan === 'function';
    report.hasLEScanInterface = typeof window.BluetoothLEScan !== 'undefined';
    try {
      await navigator.bluetooth.requestLEScan({});
      report.emptyScan = 'NOT REJECTED';
    } catch (e) { report.emptyScan = e.constructor.name; }

    window.__WBB_SELFTEST__ = report;
  })();
  return 'started';
})()
"#;

const COLLECT: &str = "JSON.stringify(window.__WBB_SELFTEST__ ?? null)";

/// Point stdout and stderr at a file.
///
/// A phone has no console. An iOS application's file descriptors 1 and 2 go
/// nowhere — `devicectl --console` attaches and receives nothing — so a
/// self-test that reports by printing reports into a void. Redirecting the
/// descriptors themselves rather than changing every `println!` keeps one
/// implementation of the checks for all three platforms.
///
/// Retrieve it with:
///
/// ```text
/// xcrun devicectl device copy from --device <udid> \
///     --domain-type appDataContainer --domain-identifier rs.webbluetooth.browser \
///     --source Documents/selftest.log --destination .
/// ```
#[cfg(mobile)]
fn capture_output(app: &AppHandle) -> Option<std::path::PathBuf> {
    use std::os::unix::io::AsRawFd;

    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("selftest.log");
    let file = std::fs::File::create(&path).ok()?;

    // SAFETY: both descriptors are valid for the lifetime of the process —
    // `file` is deliberately leaked — and `dup2` on 1 and 2 is the standard
    // way to do this.
    unsafe {
        libc::dup2(file.as_raw_fd(), 1);
        libc::dup2(file.as_raw_fd(), 2);
    }
    std::mem::forget(file);
    Some(path)
}

/// Run both phases and exit the process with the verdict.
pub fn run(app: &AppHandle) {
    let app = app.clone();
    #[cfg(mobile)]
    let captured = capture_output(&app);

    std::thread::Builder::new()
        .name("selftest".into())
        .spawn(move || {
            #[cfg(mobile)]
            println!("(output captured to {captured:?})");
            let mut failures = Vec::new();

            println!("== phase 1: the browser's own start page ==");
            match probe(&app, "tauri://localhost") {
                Ok(report) => {
                    print(&report);
                    failures.extend(judge(&report));
                }
                Err(why) => failures.push(format!("start page: {why}")),
            }

            match remote_target() {
                None => println!("\n== phase 2: skipped ({URL_ENV} is empty) =="),
                Some(target) => {
                    println!("\n== phase 2: an arbitrary site, {target} ==");
                    match remote(&app, &target) {
                        Ok(report) => {
                            print(&report);
                            failures.extend(judge_remote(&report));
                        }
                        Err(why) => {
                            // Not a failure of the browser: without the page
                            // there is nothing to judge. Said loudly, because
                            // an unverified remote phase is the one that
                            // matters most.
                            println!("NOT VERIFIED — {why}");
                            println!("(no network? set {URL_ENV} to a reachable https URL)");
                        }
                    }
                }
            }

            println!("\n== phase 3: can a subframe reach the radio? ==");
            match frame_isolation(&app) {
                Ok((lines, bad)) => {
                    for line in &lines {
                        println!("{line}");
                    }
                    failures.extend(bad);
                }
                Err(why) => failures.push(format!("frame isolation: {why}")),
            }

            #[cfg(desktop)]
            {
            println!("\n== phase 4: is the toolbar laid out as intended? ==");
            match layout_check(&app) {
                Ok((lines, bad)) => {
                    for line in &lines {
                        println!("{line}");
                    }
                    failures.extend(bad);
                }
                Err(why) => failures.push(format!("toolbar layout: {why}")),
            }
            }

            #[cfg(mobile)]
            {
                println!("\n== phase 4b: can this platform host a second window? ==");
                for line in second_window(&app) {
                    println!("{line}");
                }
            }

            println!("\n== phase 5: does the theme reach every webview? ==");
            match theme_check(&app) {
                Ok(lines) => {
                    for line in &lines.0 {
                        println!("{line}");
                    }
                    failures.extend(lines.1);
                }
                Err(why) => failures.push(format!("theme: {why}")),
            }

            println!("\n== phase 6: do permissions reach the disk? ==");
            match persistence_check(&app) {
                Ok((lines, bad)) => {
                    for line in &lines {
                        println!("{line}");
                    }
                    failures.extend(bad);
                }
                Err(why) => failures.push(format!("persistence: {why}")),
            }

            println!("\n== phase 7: does the radio actually scan? ==");
            match scan(&app) {
                Ok(seen) if seen.is_empty() => {
                    // Not a failure. An empty room is an empty room, and this
                    // check cannot tell that from a broken one.
                    println!(
                        "no advertisements in {}s — nothing nearby, or the radio is idle",
                        SCAN_SECONDS
                    );
                }
                Ok(seen) => {
                    println!("{} device(s) the chooser would have listed:", seen.len());
                    for label in &seen {
                        println!("  {label}");
                    }
                }
                Err(why) => failures.push(format!("scanning: {why}")),
            }

            #[cfg(desktop)]
            {
            println!("\n== phase 8: do tabs open, switch and close? ==");
            match tab_lifecycle(&app) {
                Ok((lines, bad)) => {
                    for line in &lines {
                        println!("{line}");
                    }
                    failures.extend(bad);
                }
                Err(why) => failures.push(format!("tabs: {why}")),
            }
            }

            // Mobile serves only its own pages, so there is no unreachable
            // host to land on an error page.
            #[cfg(desktop)]
            {
            println!("\n== phase 9: does a failed navigation say so? ==");
            match error_page(&app) {
                Ok((line, bad)) => {
                    println!("{line}");
                    failures.extend(bad);
                }
                Err(why) => failures.push(format!("error page: {why}")),
            }
            }

            match AutoPick::from_env() {
                None => println!(
                    "\n== phase 10: skipped (set {DEVICE_ENV} to a device name to run the GATT round trip) =="
                ),
                Some(_) => {
                    println!("\n== phase 10: a real GATT round trip, through the shim ==");
                    match gatt(&app) {
                        Ok(report) => {
                            print(&report);
                            failures.extend(judge_gatt(&report));
                        }
                        Err(why) => failures.push(format!("GATT round trip: {why}")),
                    }

                    #[cfg(desktop)]
                    {
                        println!(
                            "\n== phase 11: does one tab's churn survive another's connection? =="
                        );
                        match connection_isolation(&app) {
                            Ok((lines, bad)) => {
                                for line in &lines {
                                    println!("{line}");
                                }
                                failures.extend(bad);
                            }
                            Err(why) => {
                                failures.push(format!("connection isolation: {why}"))
                            }
                        }
                    }
                }
            }

            if failures.is_empty() {
                println!("\nselftest: PASS");
            } else {
                for failure in &failures {
                    println!("selftest: FAIL — {failure}");
                }
            }
            finish(&app, failures.is_empty());
        })
        .ok();
}

/// Check that a nested browsing context gets nothing.
///
/// Today this holds by construction: Tauri injects both its IPC bootstrap and
/// our shim with `for_main_frame_only: true`, so a subframe has no
/// `__TAURI_INTERNALS__` to call through and no `navigator.bluetooth` to call
/// it with. That is worth pinning down rather than leaving as a property
/// nobody wrote down, because the obvious "improvement" —
/// `initialization_script_for_all_frames`, to make Web Bluetooth work in
/// iframes — would silently be a privilege escalation: `origin_of` derives
/// the origin from `Webview::url()`, which is the *top-level* document's, so
/// an embedded third-party frame would inherit the host page's grants.
/// Chromium avoids that by keying permissions per frame and gating iframes
/// behind Permissions-Policy; until this does the same, subframes get nothing
/// and this phase fails if that ever stops being true.
fn frame_isolation(app: &AppHandle) -> Result<(Vec<String>, Vec<String>), String> {
    const PROBE: &str = r#"
      (() => {
        const child = document.getElementById('child');
        if (!child || !child.contentWindow || !child.contentDocument) {
          return JSON.stringify({ ready: false });
        }
        if (child.contentDocument.readyState !== 'complete') {
          return JSON.stringify({ ready: false });
        }
        const w = child.contentWindow;
        return JSON.stringify({
          ready: true,
          topHasBluetooth: 'bluetooth' in navigator,
          childHasBluetooth: 'bluetooth' in w.navigator,
          childHasIpc: typeof w.__TAURI_INTERNALS__ !== 'undefined',
          childHasShim: typeof w.__WEBBLUETOOTH_SHIM__ !== 'undefined',
        });
      })()
    "#;

    let content = shell::active_content(app).ok_or("no content webview")?;
    let url: url::Url = "tauri://localhost/frames.html"
        .parse()
        .map_err(|e| format!("{e}"))?;
    content.navigate(url).map_err(|e| e.to_string())?;

    let started = Instant::now();
    let report = loop {
        if started.elapsed() > DEADLINE {
            return Err("the frame page never finished loading".into());
        }
        std::thread::sleep(Duration::from_millis(250));
        let Some(raw) = eval_sync(&content, PROBE) else {
            continue;
        };
        let once: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        let Ok(report) = serde_json::from_str::<serde_json::Value>(once.as_str().unwrap_or(&raw))
        else {
            continue;
        };
        if report["ready"] == true {
            break report;
        }
    };

    let lines = vec![
        format!(
            "  top frame    navigator.bluetooth: {}",
            report["topHasBluetooth"]
        ),
        format!(
            "  child frame  navigator.bluetooth: {}",
            report["childHasBluetooth"]
        ),
        format!(
            "  child frame  __TAURI_INTERNALS__: {}",
            report["childHasIpc"]
        ),
        format!(
            "  child frame  shim:                {}",
            report["childHasShim"]
        ),
    ];

    let mut bad = Vec::new();
    if report["topHasBluetooth"] != true {
        bad.push("the top frame lost navigator.bluetooth".to_string());
    }
    if report["childHasBluetooth"] != false {
        bad.push("a subframe has navigator.bluetooth — it would use the top page's grants".into());
    }
    if report["childHasIpc"] != false {
        bad.push("a subframe can reach __TAURI_INTERNALS__ and call commands directly".into());
    }
    if report["childHasShim"] != false {
        bad.push("the shim was injected into a subframe".into());
    }
    Ok((lines, bad))
}

#[cfg(desktop)]
/// Measure the toolbar as it actually rendered.
///
/// The toolbar is a fixed-height webview that *clips*: anything that does not
/// fit is not scrolled to, it is simply gone. Eyeballing the CSS does not
/// catch that, and neither does a build. So the geometry is read back out of
/// the live page and checked against what `CHROME_HEIGHT` promised.
fn layout_check(app: &AppHandle) -> Result<(Vec<String>, Vec<String>), String> {
    const MEASURE: &str = r#"
      (() => {
        const box = (sel) => {
          const el = document.querySelector(sel);
          if (!el) return null;
          const r = el.getBoundingClientRect();
          return { top: Math.round(r.top), height: Math.round(r.height), width: Math.round(r.width) };
        };
        const root = document.documentElement;
        return JSON.stringify({
          viewport: Math.round(window.innerHeight),
          viewportW: Math.round(window.innerWidth),
          dpr: window.devicePixelRatio,
          clientH: root.clientHeight,
          bar: box('.bar'),
          address: box('.address'),
          input: box('#url'),
          button: box('.bar > button'),
          clippedY: root.scrollHeight - root.clientHeight,
          clippedX: root.scrollWidth - root.clientWidth,
        });
      })()
    "#;

    let chrome = app.get_webview(shell::CHROME).ok_or("no chrome webview")?;
    let raw = eval_sync(&chrome, MEASURE).ok_or("the toolbar did not answer")?;
    // Double-encoded: the expression is itself a JSON string.
    let once: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let m: serde_json::Value =
        serde_json::from_str(once.as_str().unwrap_or(&raw)).map_err(|e| e.to_string())?;

    let num = |path: &[&str]| -> i64 {
        let mut node = &m;
        for key in path {
            node = &node[*key];
        }
        node.as_i64().unwrap_or(-1)
    };

    let viewport = num(&["viewport"]);
    let address_h = num(&["address", "height"]);
    let address_top = num(&["address", "top"]);
    let input_h = num(&["input", "height"]);
    let button_h = num(&["button", "height"]);
    let clipped_y = num(&["clippedY"]);

    let window = app.get_window(shell::WINDOW).ok_or("no window")?;
    let scale = window.scale_factor().unwrap_or(0.0);
    let inner = window.inner_size().map_err(|e| e.to_string())?;
    let chrome_size = chrome.size().map_err(|e| e.to_string())?;
    let content_size = shell::active_content(app).and_then(|c| c.size().ok());

    let lines = vec![
        format!(
            "  window             inner {}x{} physical, scale {scale}",
            inner.width, inner.height
        ),
        format!(
            "  chrome webview     {}x{} (as Rust sees it)",
            chrome_size.width, chrome_size.height
        ),
        format!("  content webview    {content_size:?}"),
        format!("  devicePixelRatio   {}", num(&["dpr"])),
        format!(
            "  toolbar viewport   {viewport}px (innerWidth {})",
            num(&["viewportW"])
        ),
        format!("  address bar        {address_h}px tall, {address_top}px from the top"),
        format!("  address input      {input_h}px"),
        format!("  buttons            {button_h}px"),
        format!("  clipped vertically {clipped_y}px"),
        format!("  documentElement    clientHeight {}", num(&["clientH"])),
    ];

    let mut lines = lines;
    let content_inner = shell::active_content(app)
        .and_then(|c| eval_sync(&c, "String(window.innerHeight)"))
        .unwrap_or_default();
    lines.push(format!("  content viewport   {content_inner}px tall"));
    let mut bad = Vec::new();
    // Chromium's numbers: a 41px tab strip over a 46px toolbar, and a 34px
    // omnibox centred in the latter with `TOOLBAR_INTERIOR_MARGIN` of 6 above
    // and below. See `ui/chrome.css`.
    const STRIP: i64 = 41;
    const TOOLBAR: i64 = 46;
    const OMNIBOX: i64 = 34;
    if viewport != STRIP + TOOLBAR {
        bad.push(format!(
            "the chrome webview is {viewport}px, not the {}px CHROME_HEIGHT sets — \
             on macOS this usually means the titlebar is covering it",
            STRIP + TOOLBAR
        ));
    }
    if address_h != OMNIBOX {
        bad.push(format!("the address bar is {address_h}px, not {OMNIBOX}px"));
    }
    if input_h > address_h {
        bad.push(format!(
            "the input is {input_h}px inside a {address_h}px field — it is setting the height, not taking it"
        ));
    }
    if clipped_y > 0 {
        bad.push(format!(
            "{clipped_y}px of the toolbar is clipped off the bottom"
        ));
    }
    // Centred *within the toolbar*, not within the whole webview: the strip
    // sits above it. `46 - 34 = 12`, so six above and six below. Getting this
    // wrong is exactly the bug the tab strip introduced — the toolbar kept
    // `height: 100%` and centred the field against all 87 pixels.
    let above = address_top - STRIP;
    let below = viewport - address_top - address_h;
    if (above - below).abs() > 1 {
        bad.push(format!(
            "the address bar is off-centre in the toolbar: {above}px above, {below}px below"
        ));
    }

    Ok((lines, bad))
}

/// Can a second window exist here?
///
/// This decides what an iOS build can be. wry has no child webviews on iOS,
/// so the chooser cannot sit beside the page — but if a second *window* can
/// be opened, it can sit in front of it, and then arbitrary sites could be
/// browsed with the consent step still somewhere the site cannot paint or
/// read. If it cannot, a site sharing a document with the chooser could both
/// enumerate every nearby device and pick one for itself, and the only
/// honest options are a native picker or no third-party browsing.
///
/// Reported, never failed: the answer is information, not a regression.
#[cfg(mobile)]
fn second_window(app: &AppHandle) -> Vec<String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let mut lines = Vec::new();
    let built = WebviewWindowBuilder::new(app, "probe", WebviewUrl::App("frame-child.html".into()))
        .title("probe")
        .build();

    match built {
        Ok(window) => {
            std::thread::sleep(Duration::from_millis(900));
            let visible = window
                .is_visible()
                .map(|v| v.to_string())
                .unwrap_or_else(|e| format!("unknown ({e})"));
            let inner = window
                .inner_size()
                .map(|s| format!("{}x{}", s.width, s.height))
                .unwrap_or_else(|e| format!("unknown ({e})"));
            // Did it get its own webview, or silently nothing?
            let alive = app.get_webview("probe").is_some();
            lines.push(format!("  a second window was created: yes"));
            lines.push(format!(
                "  visible: {visible}, inner size: {inner}, has a webview: {alive}"
            ));
            let _ = window.destroy();
        }
        Err(error) => {
            lines.push("  a second window was created: no".to_string());
            lines.push(format!("  reason: {error}"));
        }
    }
    lines
}

/// Check that choosing a theme reaches the pages.
///
/// The whole mechanism is one call: setting the *window's* theme, which is
/// what `prefers-color-scheme` reports inside its webviews. Every stylesheet
/// here is a plain `@media (prefers-color-scheme: dark)` with nothing
/// application-specific in it, so if that call does not land, the theme
/// control does nothing and every one of those stylesheets is a lie. Worth a
/// check rather than an assumption.
fn theme_check(app: &AppHandle) -> Result<(Vec<String>, Vec<String>), String> {
    // Two mechanisms, because there have to be. The desktop sets the
    // window's theme and `prefers-color-scheme` follows; `Window::set_theme`
    // is "iOS / Android: Unsupported", so a phone gets `data-theme` on the
    // root element and the stylesheets key their overrides on that.
    #[cfg(desktop)]
    const QUERY: &str = "matchMedia('(prefers-color-scheme: dark)').matches";
    #[cfg(mobile)]
    const QUERY: &str = "document.documentElement.dataset.theme === 'dark'";

    let settings = app.state::<Settings>();
    let restore = settings.theme();
    let mut lines = Vec::new();
    let mut failures = Vec::new();

    for (choice, want_dark) in [(ThemeChoice::Dark, true), (ThemeChoice::Light, false)] {
        settings.set_theme(app, choice);
        // The appearance change goes through the window server before the
        // webview re-evaluates the media query.
        std::thread::sleep(Duration::from_millis(500));

        // On mobile these are the same webview; naming both still reads
        // correctly and costs one extra query.
        let chrome = app.get_webview(shell::CHROME);
        let content = shell::active_content(app);
        for (label, webview) in [("chrome", chrome.clone()), ("content", content.clone())] {
            let Some(webview) = webview else {
                failures.push(format!("no {label} webview"));
                continue;
            };
            let answer = eval_sync(&webview, QUERY).unwrap_or_else(|| "?".into());
            let dark = answer.trim() == "true";
            lines.push(format!(
                "  theme={:<6} {label:<7} reads dark = {dark}",
                choice.as_str()
            ));
            if dark != want_dark {
                failures.push(format!(
                    "setting the {} theme did not reach the {label} webview",
                    choice.as_str()
                ));
            }
        }
    }

    settings.set_theme(app, restore);
    lines.push(format!("  restored theme={}", restore.as_str()));
    Ok((lines, failures))
}

#[cfg(desktop)]
/// Tabs open, switch and close, and each gets its own content webview.
fn tab_lifecycle(app: &AppHandle) -> Result<(Vec<String>, Vec<String>), String> {
    let tabs = app.state::<shell::Tabs>();
    let before = tabs.list();
    let first = tabs.active_label().ok_or("no tab is active")?;

    let opened = shell::open_tab(app, None).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(600));
    let with_two = tabs.list();
    let active_after_open = tabs.active_label();

    // Both webviews exist at once — the point of a tab is that the one
    // behind is still there, still loaded, still holding whatever it held.
    let both_exist = app.get_webview(&first).is_some() && app.get_webview(&opened).is_some();

    shell::activate(app, &first);
    std::thread::sleep(Duration::from_millis(300));
    let active_after_switch = tabs.active_label();

    shell::close_tab(app, &opened);
    std::thread::sleep(Duration::from_millis(400));
    let after = tabs.list();

    let mut lines = vec![
        format!("  started with {} tab(s)", before.len()),
        format!(
            "  opened {opened}: {} tab(s), active {:?}",
            with_two.len(),
            active_after_open
        ),
        format!("  both webviews live at once: {both_exist}"),
        format!("  switched back, active {active_after_switch:?}"),
        format!("  closed it: {} tab(s)", after.len()),
    ];

    let mut bad = Vec::new();
    if with_two.len() != before.len() + 1 {
        bad.push("opening a tab did not add one".to_string());
    }
    if active_after_open.as_deref() != Some(opened.as_str()) {
        bad.push("a newly opened tab did not come to the front".to_string());
    }
    if !both_exist {
        bad.push("the background tab's webview was not kept".to_string());
    }
    if active_after_switch.as_deref() != Some(first.as_str()) {
        bad.push("switching tabs did not change which is active".to_string());
    }
    if after.len() != before.len() {
        bad.push("closing a tab did not remove it".to_string());
    }
    if app.get_webview(&opened).is_some() {
        bad.push("a closed tab's webview outlived it".to_string());
    }
    lines.push(format!("  verdict: {} problem(s)", bad.len()));
    Ok((lines, bad))
}

#[cfg(desktop)]
/// A host that cannot be reached has to produce an explanation.
///
/// This is the phase most worth having, because the mechanism behind it is a
/// timeout rather than an event — nothing in wry reports a failed
/// navigation — and a timeout is exactly the kind of thing that quietly
/// stops working.
fn error_page(app: &AppHandle) -> Result<(String, Vec<String>), String> {
    // `.invalid` is reserved by RFC 2606 and can never resolve, so this
    // depends on no third party being down or up.
    const DEAD: &str = "https://nothing-here.invalid/";

    let content = shell::active_content(app).ok_or("no content webview")?;
    let dead: url::Url = DEAD.parse().map_err(|e| format!("{e}"))?;
    content.navigate(dead).map_err(|e| e.to_string())?;

    // A little past the watchdog, which is the thing being tested.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut landed = String::new();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        landed = content.url().map(|u| u.to_string()).unwrap_or_default();
        if landed.contains("error.html") {
            break;
        }
    }

    let ok = landed.contains("error.html") && landed.contains("nothing-here.invalid");
    let line = format!("  unreachable host landed on: {landed}");
    let bad = if ok {
        Vec::new()
    } else {
        vec![format!(
            "an unreachable host did not reach the error page — ended at {landed:?}"
        )]
    };
    Ok((line, bad))
}

/// The one thing no amount of reading establishes.
///
/// Everything else here tests the API's surroundings — that the shim is
/// installed, that a remote origin gets through the ACL, that a scan sees
/// something. This drives the part that is the actual point: connect,
/// discover, read a characteristic, read its descriptor, write, subscribe,
/// take a pushed value, disconnect. All of it from page JavaScript, so the
/// conversions that only exist in the shim — `Vec<u8>` to `DataView`, handle
/// rebinding across a re-fetch, event bubbling up the GATT tree, notification
/// delivery over `eval` — are exercised rather than assumed.
///
/// Needs a peripheral. `crates/webbluetooth/examples/ios-harness.rs` is the
/// fixture this is written against: `./scripts/ios.sh ios-harness peripheral`.
fn gatt(app: &AppHandle) -> Result<serde_json::Value, String> {
    // Long: the harness pushes a value every five seconds and the probe waits
    // for two of them.
    const GATT_DEADLINE: Duration = Duration::from_secs(120);

    let service = std::env::var(SERVICE_ENV).unwrap_or_else(|_| DEFAULT_SERVICE.to_string());
    let probe = GATT_PROBE.replace("__SERVICE__", &service);

    let content = shell::active_content(app).ok_or("no content webview")?;
    // Back to a page of ours, so the round trip is not at the mercy of
    // whatever phase 2 last loaded.
    let home: url::Url = "tauri://localhost/home.html"
        .parse()
        .map_err(|e| format!("{e}"))?;
    content.navigate(home).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(800));

    let started = Instant::now();
    loop {
        if started.elapsed() > GATT_DEADLINE {
            return Err("the round trip did not finish in time".into());
        }
        let _ = content.eval(probe.clone());
        std::thread::sleep(Duration::from_millis(500));
        let Some(raw) = eval_sync(&content, "JSON.stringify(window.__WBB_GATT__ ?? null)") else {
            continue;
        };
        let once: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        if let Ok(report) = serde_json::from_str::<serde_json::Value>(once.as_str().unwrap_or(&raw))
        {
            if !report.is_null() {
                return Ok(report);
            }
        }
    }
}

const GATT_PROBE: &str = r#"
(() => {
  if (window.__WBB_GATT_STARTED__) { return 'running'; }
  window.__WBB_GATT_STARTED__ = true;
  (async () => {
    const SERVICE = '__SERVICE__';
    const r = { service: SERVICE };
    const text = (view) => new TextDecoder().decode(new Uint8Array(view.buffer));
    try {
      const device = await navigator.bluetooth.requestDevice({
        filters: [{ services: [SERVICE] }],
      });
      r.deviceName = device.name || null;
      r.sameDeviceObject = (await navigator.bluetooth.getDevices())
        .some((d) => d === device);

      let disconnects = 0;
      device.addEventListener('gattserverdisconnected', () => { disconnects += 1; });

      const server = await device.gatt.connect();
      r.connected = server.connected;
      r.serverIsDeviceGatt = server === device.gatt;

      const svc = await server.getPrimaryService(SERVICE);
      r.serviceUuid = svc.uuid;
      // The specification caches these; a page that re-fetches and adds a
      // listener expects the first object's events.
      r.serviceIdentity = (await server.getPrimaryService(SERVICE)) === svc;
      r.serviceDevice = svc.device === device;

      const chars = await svc.getCharacteristics();
      r.characteristics = chars.map((c) => c.uuid).sort();
      const level = chars.find((c) => c.properties.read && c.properties.notify);
      const control = chars.find((c) => c.properties.write);
      if (!level) { throw new Error('the fixture has no read+notify characteristic'); }
      r.levelUuid = level.uuid;
      r.characteristicIdentity = (await svc.getCharacteristic(level.uuid)) === level;

      // --- read, and the event a read fires -----------------------------
      let onChar = 0;
      let onSvc = 0;
      let bubbledTargetWasCharacteristic = null;
      level.addEventListener('characteristicvaluechanged', () => { onChar += 1; });
      svc.addEventListener('characteristicvaluechanged', (e) => {
        onSvc += 1;
        bubbledTargetWasCharacteristic = e.target === level;
      });

      const value = await level.readValue();
      r.valueIsDataView = value instanceof DataView;
      r.readBytes = value.byteLength;
      r.readValue = value.byteLength ? value.getUint8(0) : null;
      r.valuePropertyMatches =
        level.value instanceof DataView &&
        level.value.byteLength === value.byteLength;
      r.readFiredOnCharacteristic = onChar;
      r.readBubbledToService = onSvc;
      r.bubbledTargetWasCharacteristic = bubbledTargetWasCharacteristic;

      // --- descriptors ---------------------------------------------------
      const descriptors = await level.getDescriptors();
      r.descriptors = descriptors.map((d) => d.uuid).sort();
      const userDescription = descriptors.find((d) =>
        d.uuid.startsWith('00002901'));
      if (userDescription) {
        r.userDescription = text(await userDescription.readValue());
        r.descriptorParent = userDescription.characteristic === level;
      }

      // --- writes ----------------------------------------------------------
      if (control) {
        r.controlUuid = control.uuid;
        await control.writeValueWithResponse(new Uint8Array([0x01]));
        r.wroteWithResponse = true;
        if (control.properties.writeWithoutResponse) {
          await control.writeValueWithoutResponse(new Uint8Array([0x02]));
          r.wroteWithoutResponse = true;
        }
      }

      // --- notifications ----------------------------------------------------
      let pushed = 0;
      let lastPushed = null;
      level.addEventListener('characteristicvaluechanged', (e) => {
        pushed += 1;
        lastPushed = e.target.value.getUint8(0);
      });
      const before = onChar;
      const returned = await level.startNotifications();
      r.startReturnsSelf = returned === level;
      r.isNotifying = true;
      // The fixture pushes every five seconds.
      await new Promise((done) => setTimeout(done, 13000));
      r.notifications = pushed;
      r.lastNotified = lastPushed;
      r.notificationsAlsoFiredEarlierListener = onChar > before;
      await level.stopNotifications();

      // --- teardown ---------------------------------------------------------
      device.gatt.disconnect();
      await new Promise((done) => setTimeout(done, 700));
      r.connectedAfterDisconnect = device.gatt.connected;
      r.disconnectEvents = disconnects;
      r.ok = true;
    } catch (error) {
      r.ok = false;
      r.error = `${error.name}: ${error.message}`;
    }
    window.__WBB_GATT__ = r;
  })();
  return 'started';
})()
"#;

#[cfg(desktop)]
/// A connection held by one tab has to survive everything another tab does.
///
/// This is what the per-tab bookkeeping in `state.rs` is for, and it is the
/// kind of thing that looks fine until someone opens a second tab. `gatt
/// .disconnect()` is a property of the *link*: before the refactor, any
/// navigation called `page_unloaded` and hung up every device in the
/// process, so a background tab reading a sensor went dead the moment the
/// foreground tab followed a link.
///
/// A stale `connected` flag would pass a naive check, so the proof is a
/// successful *read* after the churn, not the flag.
fn connection_isolation(app: &AppHandle) -> Result<(Vec<String>, Vec<String>), String> {
    let service = std::env::var(SERVICE_ENV).unwrap_or_else(|_| DEFAULT_SERVICE.to_string());
    let hold = HOLD_PROBE.replace("__SERVICE__", &service);

    let holder = app
        .state::<shell::Tabs>()
        .active_label()
        .ok_or("no active tab")?;
    let content = app.get_webview(&holder).ok_or("no content webview")?;

    // Connect, and stay connected.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > deadline {
            return Err("the holding tab never connected".into());
        }
        let _ = content.eval(hold.clone());
        std::thread::sleep(Duration::from_millis(500));
        let ready = eval_sync(&content, "String(window.__HELD_STATE__ ?? '')").unwrap_or_default();
        if ready.contains("ready") {
            break;
        }
        if ready.contains("failed") {
            let why =
                eval_sync(&content, "String(window.__HELD_ERROR__ ?? '')").unwrap_or_default();
            return Err(format!("the holding tab could not connect: {why}"));
        }
    }

    // Now churn: a second tab opens, goes somewhere, and closes.
    let other = shell::open_tab(app, None).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(500));
    if let Some(webview) = app.get_webview(&other) {
        if let Ok(url) = "tauri://localhost/frames.html".parse::<url::Url>() {
            let _ = webview.navigate(url);
        }
    }
    std::thread::sleep(Duration::from_millis(1200));
    shell::close_tab(app, &other);
    std::thread::sleep(Duration::from_millis(800));
    shell::activate(app, &holder);
    std::thread::sleep(Duration::from_millis(300));

    // A read is the proof. The flag alone could simply be stale.
    let _ = content.eval(VERIFY_PROBE.replace("__SERVICE__", &service));
    let verify_deadline = Instant::now() + Duration::from_secs(30);
    let report = loop {
        if Instant::now() > verify_deadline {
            return Err("the holding tab never answered after the churn".into());
        }
        std::thread::sleep(Duration::from_millis(400));
        let raw = eval_sync(&content, "JSON.stringify(window.__HELD_AFTER__ ?? null)")
            .unwrap_or_default();
        let once: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(once.as_str().unwrap_or(&raw))
        {
            if !value.is_null() {
                break value;
            }
        }
    };

    let lines = vec![
        format!("  held by {holder}, churned {other}"),
        format!("  still connected:     {}", report["connected"]),
        format!("  disconnect events:   {}", report["drops"]),
        format!("  read after the churn: {}", report["read"]),
    ];

    let mut bad = Vec::new();
    if report["connected"] != true {
        bad.push("another tab's churn dropped this tab's connection".to_string());
    }
    if report["drops"].as_u64().unwrap_or(1) != 0 {
        bad.push("the holding tab was told its device disconnected".to_string());
    }
    if !report["read"].is_number() {
        bad.push(format!(
            "the link was dead after the churn: {}",
            report["error"].as_str().unwrap_or("read failed")
        ));
    }
    Ok((lines, bad))
}

#[cfg(desktop)]
const HOLD_PROBE: &str = r#"
(() => {
  if (window.__HELD_STATE__) { return window.__HELD_STATE__; }
  window.__HELD_STATE__ = 'connecting';
  (async () => {
    try {
      const device = await navigator.bluetooth.requestDevice({
        filters: [{ services: ['__SERVICE__'] }],
      });
      window.__HELD_DROPS__ = 0;
      device.addEventListener('gattserverdisconnected', () => {
        window.__HELD_DROPS__ += 1;
      });
      await device.gatt.connect();
      window.__HELD__ = device;
      window.__HELD_STATE__ = 'ready';
    } catch (error) {
      window.__HELD_ERROR__ = `${error.name}: ${error.message}`;
      window.__HELD_STATE__ = 'failed';
    }
  })();
  return 'connecting';
})()
"#;

#[cfg(desktop)]
const VERIFY_PROBE: &str = r#"
(() => {
  if (window.__HELD_VERIFYING__) { return 'running'; }
  window.__HELD_VERIFYING__ = true;
  (async () => {
    const out = {
      connected: !!(window.__HELD__ && window.__HELD__.gatt.connected),
      drops: window.__HELD_DROPS__ ?? -1,
    };
    try {
      const service = await window.__HELD__.gatt.getPrimaryService('__SERVICE__');
      const chars = await service.getCharacteristics();
      const level = chars.find((c) => c.properties.read);
      const value = await level.readValue();
      out.read = value.getUint8(0);
      window.__HELD__.gatt.disconnect();
    } catch (error) {
      out.error = `${error.name}: ${error.message}`;
    }
    window.__HELD_AFTER__ = out;
  })();
  return 'started';
})()
"#;

/// What a real round trip has to have done.
fn judge_gatt(report: &serde_json::Value) -> Vec<String> {
    let mut failures = Vec::new();
    let mut require = |ok: bool, what: &str| {
        if !ok {
            failures.push(what.to_string());
        }
    };

    if report["ok"] != true {
        let error = report["error"].as_str().unwrap_or("no reason given");
        // The commonest cause by far is the fixture not being there, and
        // "no device was chosen" does not say so. The picker only ever
        // declines after scanning for its whole window.
        let hint = if error.contains("NotFound") {
            format!(
                " — nothing matching {:?} advertised the fixture's service within the \
                 scan window. Is the peripheral running? \
                 (./scripts/ios.sh ios-harness peripheral). Try {DEVICE_ENV}='*', which \
                 accepts whatever the probe's own service filter matched.",
                std::env::var(DEVICE_ENV).unwrap_or_default()
            )
        } else {
            String::new()
        };
        return vec![format!("the round trip failed: {error}{hint}")];
    }

    require(
        report["connected"] == true,
        "gatt.connect() did not report connected",
    );
    require(
        report["serverIsDeviceGatt"] == true,
        "device.gatt is not stable",
    );
    require(
        report["serviceIdentity"] == true,
        "getPrimaryService did not cache",
    );
    require(
        report["characteristicIdentity"] == true,
        "getCharacteristic did not cache",
    );
    require(report["serviceDevice"] == true, "service.device is wrong");
    require(
        report["valueIsDataView"] == true,
        "readValue did not give a DataView",
    );
    require(
        report["readBytes"].as_u64().unwrap_or(0) > 0,
        "readValue came back empty",
    );
    require(
        report["valuePropertyMatches"] == true,
        "characteristic.value was not updated by the read",
    );
    require(
        report["readFiredOnCharacteristic"].as_u64().unwrap_or(0) > 0,
        "a read did not fire characteristicvaluechanged",
    );
    require(
        report["readBubbledToService"].as_u64().unwrap_or(0) > 0,
        "characteristicvaluechanged did not bubble to the service",
    );
    require(
        report["bubbledTargetWasCharacteristic"] == true,
        "the bubbled event's target was not the characteristic",
    );
    require(
        report["descriptors"]
            .as_array()
            .is_some_and(|d| !d.is_empty()),
        "no descriptors were discovered",
    );
    require(
        report["wroteWithResponse"] == true,
        "writeValueWithResponse did not complete",
    );
    require(
        report["notifications"].as_u64().unwrap_or(0) > 0,
        "no notification arrived while subscribed",
    );
    require(
        report["startReturnsSelf"] == true,
        "startNotifications did not return the characteristic",
    );
    require(
        report["connectedAfterDisconnect"] == false,
        "gatt.connected stayed true after disconnect()",
    );

    failures
}

/// Check that a permission decision survives the trip to disk.
///
/// Uses the scanning answer rather than a device grant, because that is the
/// one permission that can be set without a radio or a person: the path it
/// takes — `Browser` to `PermissionStore` to the file and back — is the same
/// one a device grant takes.
///
/// The origin is obviously fake and is cleared again afterwards, so this does
/// not disturb real permissions. An interrupted run leaves one visible entry
/// in the manager and nothing else.
fn persistence_check(app: &AppHandle) -> Result<(Vec<String>, Vec<String>), String> {
    const ORIGIN: &str = "https://selftest.invalid";

    let state = app.state::<Browser>();
    let mut lines = Vec::new();
    let mut bad = Vec::new();

    state.set_scanning_decision(ORIGIN, false);

    let store = crate::permissions::PermissionStore::new(app);
    let written = store.load();
    let found = written
        .origins
        .get(ORIGIN)
        .and_then(|origin| origin.scanning);
    lines.push(format!("  wrote a refusal, read back: {found:?}"));
    if found != Some(false) {
        bad.push(format!(
            "a scanning decision did not survive the write — read back {found:?}"
        ));
    }

    // And that clearing it removes the record rather than leaving a stale one.
    state.clear_scanning_decision(ORIGIN);
    let after = store.load();
    let still_there = after.origins.contains_key(ORIGIN);
    lines.push(format!("  cleared it, still on disk: {still_there}"));
    if still_there {
        bad.push("clearing a decision left it in the file".to_string());
    }

    let remembered = state
        .everything()
        .iter()
        .filter(|summary| summary.origin != ORIGIN)
        .count();
    lines.push(format!(
        "  origins remembered from earlier runs: {remembered}"
    ));

    Ok((lines, bad))
}

/// How long the scan phase listens for.
const SCAN_SECONDS: u64 = 5;

/// Run the exact path `requestDevice()` takes, with a chooser that watches
/// instead of choosing.
///
/// This is the one part a page cannot be made to exercise from here — the
/// picker needs a human — so the picker is replaced and everything under it
/// is real: the same session, the same scan, the same candidate stream the
/// chooser window renders. If devices appear here, they appear there.
fn scan(app: &AppHandle) -> Result<Vec<String>, String> {
    let bluetooth = app.state::<Browser>().bluetooth.clone();
    let collector = Collector::default();
    let seen = collector.seen.clone();

    let outcome = tauri::async_runtime::block_on(async move {
        bluetooth
            .request_device_with(RequestDeviceOptions::new().accept_all_devices(), &collector)
            .await
    });

    // Declining is the expected end: the collector never picks anything.
    if let Err(error) = outcome {
        if !matches!(error, webbluetooth::Error::NotFound(_)) {
            return Err(error.to_string());
        }
    }
    let seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    Ok(seen.clone())
}

#[derive(Default)]
struct Collector {
    seen: Arc<Mutex<Vec<String>>>,
}

impl DeviceChooser for Collector {
    fn choose(&self, mut candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        let seen = self.seen.clone();
        Box::pin(async move {
            let deadline = Duration::from_secs(SCAN_SECONDS);
            let _ = webbluetooth::timeout(deadline, async {
                while let Some(candidate) = candidates.next().await {
                    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                    let label = match candidate.rssi() {
                        Some(dbm) => format!("{} ({dbm} dBm)", candidate.label()),
                        None => candidate.label().to_string(),
                    };
                    if !seen
                        .iter()
                        .any(|existing| existing.starts_with(candidate.label()))
                    {
                        seen.push(label);
                    }
                }
            })
            .await;
            None
        })
    }
}

/// End the run.
///
/// On a desktop the exit code is the verdict, which is what a CI job reads.
/// On iOS it cannot be: `app.exit()` there walks into a tao bug — "AppState
/// previously failed a state transition" — and aborts the process with a
/// non-unwinding panic *after* the results are written, which looks alarming
/// and is not the test failing. An iOS application has no business exiting
/// itself anyway; the verdict is in the log the run wrote.
#[cfg(desktop)]
fn finish(app: &AppHandle, passed: bool) {
    app.exit(if passed { 0 } else { 1 });
}

#[cfg(mobile)]
fn finish(_app: &AppHandle, _passed: bool) {
    println!("(the window stays open: exiting from here trips a tao bug on iOS)");
}

fn print(report: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(report).unwrap_or_default()
    );
}

fn remote_target() -> Option<String> {
    match std::env::var(URL_ENV) {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => Some(DEFAULT_URL.to_string()),
    }
}

/// Load a real site and probe there.
fn remote(app: &AppHandle, target: &str) -> Result<serde_json::Value, String> {
    let url: url::Url = target.parse().map_err(|e| format!("bad URL: {e}"))?;
    let expected = crate::origin::of(&url).ok_or("that URL has no origin")?;
    let content = shell::active_content(app).ok_or("no content webview")?;
    content.navigate(url).map_err(|e| e.to_string())?;
    probe(app, &expected)
}

/// Poll the page until it reports, ignoring anything from a document at a
/// different origin — after a navigation the old one answers for a moment.
fn probe(app: &AppHandle, expect_origin: &str) -> Result<serde_json::Value, String> {
    let started = Instant::now();
    loop {
        if started.elapsed() > DEADLINE {
            return Err(format!("timed out waiting for {expect_origin}"));
        }
        let Some(content) = shell::active_content(app) else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        // Kick the probe off; harmless to repeat, it guards itself.
        let _ = content.eval(PROBE);

        if let Some(raw) = eval_sync(&content, COLLECT) {
            // wry hands back the JSON encoding of the expression's value,
            // and the expression is itself a JSON string, so it arrives
            // double-encoded.
            let once: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
            let inner = once.as_str().unwrap_or(&raw);
            if let Ok(report) = serde_json::from_str::<serde_json::Value>(inner) {
                if !report.is_null() && report["origin"] == expect_origin {
                    return Ok(report);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn eval_sync(webview: &tauri::Webview, script: &str) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    webview
        .eval_with_callback(script, move |value| {
            let _ = tx.send(value);
        })
        .ok()?;
    rx.recv_timeout(Duration::from_secs(3)).ok()
}

/// What has to be true for this build to be worth shipping.
fn judge(report: &serde_json::Value) -> Vec<String> {
    let mut failures = Vec::new();
    let mut require = |ok: bool, what: &str| {
        if !ok {
            failures.push(what.to_string());
        }
    };

    require(report["shimInstalled"] == true, "the shim did not install");
    require(
        report["hasNavigatorBluetooth"] == true,
        "navigator.bluetooth is absent",
    );
    require(
        report["bluetoothTag"] == "[object Bluetooth]",
        "navigator.bluetooth is not a Bluetooth",
    );
    require(
        report["interfaces"].as_array().map_or(0, |a| a.len()) == 7,
        "an interface constructor is missing",
    );
    require(
        report["uuidService"] == "0000180d-0000-1000-8000-00805f9b34fb",
        "BluetoothUUID.getService('heart_rate') is wrong",
    );
    require(
        report["uuidCharacteristic"] == "00002a00-0000-1000-8000-00805f9b34fb",
        "BluetoothUUID.getCharacteristic('gap.device_name') is wrong",
    );
    require(
        report["uuidDescriptor"] == "00002902-0000-1000-8000-00805f9b34fb",
        "BluetoothUUID.getDescriptor(...) is wrong",
    );
    require(
        report["uuidAlias"] == "0000180d-0000-1000-8000-00805f9b34fb",
        "BluetoothUUID.canonicalUUID is wrong",
    );
    require(
        report["namespaced"][0] != report["namespaced"][1],
        "service and characteristic namespaces collapsed",
    );
    require(
        report["badNameThrew"] == "TypeError",
        "an unknown name did not throw TypeError",
    );
    require(
        report["availability"].is_boolean(),
        "getAvailability() did not round-trip",
    );
    require(
        report["grantedDevices"].is_number(),
        "getDevices() did not round-trip",
    );
    // The user-gesture requirement is deliberately not asserted. Everything
    // this probe runs was evaluated by the host, and WebKit marks that as
    // user-initiated, so `requestDevice` would legitimately be allowed
    // through and a pass here would mean nothing. `userActivationActive` in
    // the report is the evidence for why.
    require(
        report["emptyFilters"] == "TypeError",
        "an empty filters array was not rejected",
    );
    require(
        report["hasRequestLEScan"] == true,
        "navigator.bluetooth.requestLEScan is missing",
    );
    require(
        report["hasLEScanInterface"] == true,
        "the BluetoothLEScan interface is missing",
    );
    require(
        report["emptyScan"] == "TypeError",
        "requestLEScan accepted a dictionary that restricts nothing",
    );

    failures
}

/// What has to be true on a site this browser does not control.
///
/// Fewer assertions than the start page, and a different point: that Tauri's
/// ACL really does let a remote origin through to the Bluetooth commands.
/// `getAvailability()` returning a boolean is the proof — anything blocked by
/// a capability comes back as "not allowed by ACL" instead.
fn judge_remote(report: &serde_json::Value) -> Vec<String> {
    let mut failures = Vec::new();
    let mut require = |ok: bool, what: &str| {
        if !ok {
            failures.push(format!("on a remote origin, {what}"));
        }
    };

    require(report["shimInstalled"] == true, "the shim did not install");
    require(
        report["hasNavigatorBluetooth"] == true,
        "navigator.bluetooth is absent",
    );
    require(
        report["availability"].is_boolean(),
        format!(
            "getAvailability() did not round-trip ({})",
            report["availabilityError"]
        )
        .as_str(),
    );
    require(
        report["grantedDevices"].is_number(),
        "getDevices() did not round-trip",
    );
    failures
}
