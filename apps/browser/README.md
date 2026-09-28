# WebBluetooth Browser

A web browser whose pages get a real `navigator.bluetooth`.

WKWebView has no Web Bluetooth. This wraps it in a Tauri shell, injects a shim
ahead of every page's own scripts, and implements the API over the
[`webbluetooth`](../../crates/webbluetooth) crate — so a site written against
the Web Bluetooth standard works here, unmodified, on macOS.

```
cargo run                                      # the browser, on macOS
WBB_SELFTEST=1 cargo run                       # check it, without a person watching
WBB_SELFTEST=1 WBB_SELFTEST_DEVICE='*' cargo run   # …including a real GATT round trip

cargo check --target aarch64-apple-ios         # the iOS build
cargo check --target aarch64-linux-android     # the Android build
tauri ios dev                                  # …on a device
tauri android dev
```

The mobile *specs* in `gen/` are committed, not generated on demand: the
Bluetooth usage string, the Android manifest and the permission request all live
there, and so does the fix for `tauri ios init` writing the Rust build phase as
`node tauri …` for a project with no Node frontend — which resolves to a file
that does not exist and dies with `MODULE_NOT_FOUND`. That fix is in
`gen/apple/project.yml` with the reason beside it; re-running `tauri ios init`
will undo it.

What is *not* committed is `gen/apple/*.xcodeproj`. xcodegen generates it from
`project.yml` — `tauri ios init` runs it for you, and `xcodegen generate` inside
`gen/apple` rebuilds it from the spec alone — and Xcode writes whoever is
building into it the first time the signing tab is opened.

### Signing

The team is the one setting here that belongs to the person building rather than
to the project, so `project.yml` reads it from the environment:

```sh
export APPLE_DEVELOPMENT_TEAM=ABCDE12345   # yours, not this
tauri ios dev
```

`security find-identity -v -p codesigning` shows it as the `OU` field of your
certificate's subject; `./scripts/ios.sh` in the repository root reads the same
value out of a provisioning profile instead. Left unset, xcodebuild fails with a
missing-team error — which is also what `tauri ios init` leaves you with, minus
the explanation.

## Platforms

| | |
|---|---|
| **macOS** | the browser: tabs, toolbar, menu, separate chooser window. |
| **iOS** | a browser with one webview: toolbar in the page, dialogs in their own windows. |
| **Android** | the same single-webview build, plus the runtime permissions Android needs. |

### Why iOS looks different

The desktop shell puts the toolbar and the page in two sibling webviews.
**wry has no child webviews on iOS** — `WebViewBuilder::build_as_child` is
documented "Android/iOS: Unsupported" — and Tauri gates `Window::add_child`
behind `desktop` for the same reason. Compiling the desktop shell for
`aarch64-apple-ios` fails on 19 distinct missing APIs: no `tauri::menu`, no
`add_child`, no `Webview::size/close/set_focus`.

So on mobile the toolbar is **injected into the page**
(`ui/mobile-toolbar.js`, behind a shadow root).

What does *not* move into the page is the part that matters. iOS does
support a second **window** — the self-test checks this on the device and
reports it — so the device chooser, the scanning prompt and the permissions
view keep their own, exactly as on a desktop. A site cannot paint over the
thing that grants it access, cannot read the list of devices it offers, and
cannot enumerate what other origins hold. `capabilities/mobile-dialogs.json`
is where that boundary is written down.

The cost is real and worth stating plainly: **a hostile page can draw a
convincing fake address bar**, because the real one is in its DOM. That is
phishing, not device access — and the chooser window displays the origin it
got from Rust, never from the page, so the pairing prompt still tells the
truth about who is asking. A native UIKit toolbar presented from a Tauri
mobile plugin is what would close that last gap.

Everything below the shell is shared and unchanged on iOS: the shim, the
bridge commands, the per-origin permission model, the persistent store and
the radio. `capabilities/mobile.json` grants them to the page's webview for
any `http(s)` origin, the same grant `content.json` makes on a desktop.

Two things the desktop does not need, both because `Window::set_theme` is
documented "iOS / Android: Unsupported": the theme arrives as a `data-theme`
attribute on the root element instead of through the window, and
`ui/home.css` keys its overrides on that as well as on
`prefers-color-scheme`. `app.exit()` is also avoided there — it walks into a
tao bug and aborts the process with a non-unwinding panic.

Android is the same shell — `src/shell_mobile.rs` is `#[cfg(mobile)]`, not
`#[cfg(target_os = "ios")]` — with two things iOS does not need:

* **A JavaVM and a Context.** Every other platform has an ambient way to
  reach its Bluetooth stack; Android does not, and `Runtime::init` has to be
  handed both or every call fails with "no Android Context". `ndk-context`
  is where the activity publishes them.
* **Runtime permissions.** Declaring `BLUETOOTH_SCAN` and
  `BLUETOOTH_CONNECT` in the manifest is not enough from API 23 on. The
  library deliberately does not ask — it has no Activity and no business
  choosing the moment — so `MainActivity.kt` does, once, at launch.
  `neverForLocation` on the scan permission is what lets the location
  permission be dropped on API 31 and later; it is a claim that scan results
  are not used to locate the user, and it is true here.

A real iOS browser would need the chooser and the toolbar to be **native**
UIKit presented from a Tauri mobile plugin in Swift, outside the web content
— that is the missing piece, not a small one.

## How it fits together

One window, one chrome webview, and one content webview per tab:

| | |
|---|---|
| `chrome` | the tab strip and toolbar, from `ui/`. Local assets only. |
| `content-1`, `content-2`, … | one per tab, with `shim/webbluetooth.js` injected. |

They are siblings, not nested — **the page is not in an iframe.** Sites that
refuse to be framed (`X-Frame-Options: deny`) load normally; the self-test
loads github.com to prove it. `target="_blank"` opens a tab rather than a
second window, which would have no toolbar and no way to see what had been
granted to whom.

Splitting them is what makes the security boundary expressible. Tauri checks
its ACL on every command from a remote origin, and the two capabilities say
who may call what:

- `capabilities/content.json` gives the `content-*` webviews the Bluetooth
  commands, for *any* `http(s)` origin. That is the feature.
- `capabilities/chrome.json` keeps navigation, the theme and the permission
  controls for the `chrome` webview. A page cannot reach them.

`permissions/browser.toml` lists which commands each of those means.

A call goes: page → shim → `__TAURI_INTERNALS__.invoke` → a command in
`src/bridge.rs` → `webbluetooth` → CoreBluetooth. Events come back the other
way through `Webview::eval`, which is not subject to the page's
Content-Security-Policy — a site with a strict `connect-src` would otherwise
be able to cut its own notifications.

## What a page is allowed to do

The same things a browser allows, enforced in `src/bridge.rs` and
`src/state.rs` rather than in the shim. **The shim is not a security control**
— it is ordinary page script and a site can replace it. Every check is on the
Rust side, and every command re-derives the caller's origin from the content
webview's current URL rather than trusting anything sent to it.

- **Secure context.** `https`, or a loopback host. On anything else
  `navigator.bluetooth` is absent, as in Chrome. `file:` is not trustworthy
  and is not exempted.
- **The chooser.** A device reaches an origin only by a person picking it out
  of the picker window (`src/chooser.rs`). There is no API that skips it, and
  `requestDevice` needs a real user gesture — `navigator.userActivation` where
  the webview has it, and a trusted-event fallback where it does not.
- **Scanning is asked separately** (`src/scanning.rs`). `requestLEScan()` is
  not the chooser with different words: it hands over every advertisement in
  range, continuously, from devices nobody pointed at. Chromium keeps a whole
  second controller for this and so does this. The answer is remembered per
  origin — including a refusal, so a site told no cannot re-ask on a loop.
- **Per-origin grants, per-tab lifetimes.** An origin may touch only the
  services its own request named. This matters more here than it looks: one radio session
  serves every site, so the library's own grant is the *union* over every
  origin that ever asked. `src/state.rs` holds the real table and the
  library's is a second fence behind it. Handles and connections are tracked
  per *tab* as well, because they have a different lifetime from a
  permission: a navigation in one tab must not tear down another's. A GATT
  link is shared, so the radio is only told to hang up when the last tab
  holding a device lets go — Chromium's `FrameConnectedBluetoothDevices`, one
  level up. The self-test holds a connection in one tab while another opens,
  navigates and closes, then proves the link is still live with a read.
- **Subframes get nothing.** Tauri injects both its IPC bootstrap and the
  shim into the main frame only, so a nested document has no
  `navigator.bluetooth` and no way to call a command. That is a property the
  self-test asserts rather than one left to chance: the obvious
  "improvement" — injecting into all frames, to make Web Bluetooth work in
  iframes — would be a privilege escalation, because the origin is derived
  from the *top-level* URL. Chromium solves this by keying permissions per
  frame and gating iframes behind Permissions-Policy; until this does the
  same, subframes stay out.
- **The GATT blocklist** the library vendors applies underneath all of it and
  cannot be turned off from here. The `unrestricted` feature is deliberately
  not enabled.

Handles given to a page are opaque and origin-checked on every use, so a
guessed handle belonging to another origin is refused.

## Permissions persist

Grants outlive the process, in `permissions.json` in the app config
directory, written `0600` through a temporary file and a rename. Chromium's
own `content/browser/bluetooth` notes describe exactly this move — from the
per-session `bluetooth_allowed_devices` to a persistent context "exposed in
the settings UI for users to manage" — and the second half of that sentence
is load-bearing: a permission nobody can find is a permission nobody can take
back, so the store and the manager window (`src/manager.rs`, the shield
button in the toolbar) ship together.

On startup, remembered identifiers become usable devices again through the
library's `adopt_device`, which needs neither a scan nor the device to be in
range. That happens in the background; `getDevices()` waits for it, so a page
asking the instant it loads does not see an empty list that fills in a moment
later. A device the platform can no longer resolve stays in the store — it
may just be a machine you have not seen since, and forgetting it would revoke
a permission nobody asked to revoke.

## The shim

`shim/webbluetooth.js` is the body of an IIFE; `src/shell.rs` wraps it with
the GATT name tables `build.rs` generates from
`crates/webbluetooth-core/spec/`, so `BluetoothUUID.getService('heart_rate')`
— which the specification defines as *synchronous* — can answer without a
round trip, from the same registry Rust resolves against.

It implements `Bluetooth`, `BluetoothDevice`, the four GATT interfaces,
`BluetoothCharacteristicProperties`, `BluetoothAdvertisingEvent`,
`BluetoothLEScan` and `BluetoothUUID`; object identity and caching as the spec describes it; events
that bubble up the GATT tree with `target` pinned to the characteristic; and
`DOMException` names that match, because sites branch on `err.name`.

## Keyboard and menu

The shortcuts live in the menu bar (`src/menu.rs`), not in a `keydown`
handler: the page has focus almost all the time, so a handler in the toolbar
would only fire when the toolbar happened to be focused — and ⌘L above all
has to work while reading a page. The Edit menu is load-bearing too; on macOS
it is what makes ⌘C and ⌘V work in a text field at all, and a window without
one has an address bar you cannot paste into.

⌘T new tab · ⌘W close tab · ⇧⌘W close window · ⌘L open location · ⌘R reload ·
⌘[ / ⌘] back and forward · ⇧⌘H start page · ⌥⌘I web inspector ·
⇧⌘B permissions.

## When a page will not load

Neither wry nor Tauri surfaces WKWebView's `didFailProvisionalNavigation`:
between them they offer navigation-started and page-load started/finished,
and nothing that says a host could not be reached. So a watchdog is the only
available signal — a load that has neither finished nor been replaced within
`NAVIGATION_TIMEOUT` gets `ui/error.html`, with the failed address carried in
the fragment so it never reaches a server or an asset request.

The consequence is worth knowing rather than hiding: a mistyped host sits
blank for those twelve seconds before saying anything, where a real browser
answers at once. The alternative — guessing from whether the URL reverted —
misfires on any slow page.

## Toolbar metrics

Taken from Chromium's `chrome/browser/ui/layout_constants.cc`, non-touch
desktop column, and noted in `ui/chrome.css`: `kLocationBarHeight` 34,
`kToolbarButtonHeight` 34, `kToolbarButtonIconSize` 20, `kLocationBarMargin`
9, `kToolbarElementPadding` 4, `TOOLBAR_INTERIOR_MARGIN` `VH(6, 6)` — so a
46px bar. The strip above it is `kTabHeight` (34 plus the one pixel it
overlaps the toolbar by) plus `kTabStripPadding` 6, so 41; the chrome webview
is 87. Copied rather than eyeballed: this is the one part of a browser people
have a trained expectation of, and the self-test measures the rendered result
against these numbers.

The window uses an overlay titlebar. It has to: with an ordinary one, macOS
clips a webview placed at the top of the window to whatever the titlebar does
not cover, which left the 46px toolbar rendering as an 8px sliver. The
toolbar's 78px left inset is the room the traffic lights need, and
`data-tauri-drag-region` on the bar gives the window back a way to be dragged.

## Theme

System, light or dark, defaulting to system, remembered in
`settings.json` in the app config directory. The whole mechanism is
`Window::set_theme`: that is what `prefers-color-scheme` reports inside the
window's webviews, so the toolbar, the picker, the start page *and* the loaded
site all follow, and every stylesheet here is a plain
`@media (prefers-color-scheme: dark)` with nothing app-specific in it.

## The self-test

`WBB_SELFTEST=1` turns the browser into a check of itself and exits with the
verdict. There is no other way to tell a shim that failed to install from one
that installed and refused everything — both look like a page that does
nothing.

1. The start page: the shim installed, UUID namespaces resolve correctly,
   commands round-trip, `requestLEScan` present and refusing an empty filter.
2. An arbitrary site (`WBB_SELFTEST_URL`, default `https://example.com/`, set
   empty to skip): the same, over Tauri's remote-origin ACL.
3. A subframe seeing neither the shim nor `__TAURI_INTERNALS__`.
4. The toolbar's rendered geometry, measured from the host and checked against
   the Chromium constants above.
5. The theme reaching both webviews in both directions.
6. A permission decision surviving the trip to disk and being cleanly
   removable, under an obviously fake origin that is cleared again.
7. A real scan, through the exact path `requestDevice()` takes, with a chooser
   that collects instead of choosing.
8. Tabs opening, switching and closing, with both webviews alive at once.
9. An unreachable host (`.invalid`, so it depends on nobody being down)
   landing on the error page.
10. **A real GATT round trip**, when `WBB_SELFTEST_DEVICE` is set: connect,
   discover, read, read a descriptor, write both ways, subscribe, take pushed
   values, disconnect — all from page JavaScript, so the conversions that only
   exist in the shim are exercised rather than assumed. Written against
   `crates/webbluetooth/examples/ios-harness.rs`; run
   `./scripts/ios.sh ios-harness peripheral` first.

11. **Cross-tab connection isolation**, also hardware-gated: one tab holds a
    connection while another tab opens, navigates and closes, and the first
    then *reads* from the device — a stale `connected` flag would pass a
    weaker check.

   `WBB_SELFTEST_DEVICE` turns off both the chooser *and* the user-gesture
   check, which is why it does nothing unless `WBB_SELFTEST` is also set and
   why the run prints a warning. `'*'` is usually the right value: a candidate
   only reaches a chooser after passing the probe's own service filter, and
   matching on the name is fragile — iOS drops an app's advertised local name
   when it is not in the foreground, so the fixture appears as "Rust iPad" or
   as plain "iPad" depending on the state of a screen nobody is looking at.

The user-gesture requirement is reported but *not* asserted: WebKit treats
host-evaluated script as user-initiated, so anything the probe measures there
would pass for the wrong reason.

## Known limits

- One tab, no popups. `target=_blank` navigates in place.
- No tabs, menu or error page on mobile; the toolbar is spoofable there, as
  above.
- Web Bluetooth is not offered inside iframes at all, deliberately — see
  above. Chrome offers it in same-origin frames and in cross-origin frames
  with a Permissions-Policy grant.
- macOS only in practice — the `Info.plist`, the titlebar handling and the
  backend are all Apple-specific, though nothing in the bridge is.
- `watchAdvertisements()` works; CoreBluetooth reports no `appearance` and no
  `txPower` in an advertisement, so those are `undefined`.
- Notification bursts are coalesced into one message, but the transport is
  still `eval` with values as JSON number arrays. Fine well past sensor
  rates; not a zero-copy path.
- No downloads, find-in-page or zoom. Tabs cannot be reordered or dragged
  out.

## Why it is its own workspace

The repository root workspace lists an app that is being written in parallel;
while that manifest has no targets, the whole workspace fails to load and
nothing that inherits from it can build. The `[workspace]` table in
`Cargo.toml` makes this crate a root of its own so it builds regardless. To
fold it back in: delete that table and add `apps/browser` to the root
`members`.
