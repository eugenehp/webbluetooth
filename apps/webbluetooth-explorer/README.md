# WebBluetoothExplorer

A Bluetooth Low Energy explorer: scan, connect, walk the GATT tree, read and
write characteristics, subscribe to notifications.

A reimplementation of Joseph Ross's macOS
[Bluetility](https://github.com/jnross/Bluetility) on top of
[`webbluetooth`](../../crates/webbluetooth), so the same program runs on macOS,
Linux and Windows from one source tree.

```sh
cargo run -p webbluetooth-explorer --release
```

## The window

Three panes over a log:

| Pane | What it shows |
| --- | --- |
| Devices | Everything advertising, with a signal meter and live RSSI. Filter by name, signal floor, connectable, named-only. Hover for the whole advertising packet; right-click to copy any of it. Click to connect. |
| GATT | The whole device as a collapsing tree — services, with their characteristics nested under them and properties as `R W w N I`. A `◉` marks a live subscription. |
| Detail | The selected characteristic: properties, value as hex / ASCII / decimal / every numeric reading the length allows, Read, Write, Subscribe, and its descriptors. |

Bluetility put services and characteristics in two separate columns. This uses
one collapsing tree instead, which is how nRF Connect lays the same information
out: two columns cost about fifteen line-heights of width and showed one
service's characteristics at a time, and the pane that ran out of room was the
detail pane. The tree shows the shape of the whole device and leaves the width
where it is needed.

The **Link** menu in the device header carries what the platform will say and do
about the connection itself — ATT MTU, PHY, bonding, connection interval,
latency and supervision timeout — taken from nRF Connect's per-device menu,
because every one of them is a single call in `webbluetooth`. Not every platform
offers all of them: CoreBluetooth negotiates the MTU itself and exposes no PHY
API at all, so three of the four refuse on macOS and say so in the log.

Values longer than sixteen bytes get a hex dump instead of a single line. The
write field takes hex in whatever separators you paste (`01 A2 FF`, `a2:ff`,
`0xA2FF`) or raw text, and shows the bytes it is about to send before sending
them.

## What just changed

Anything that updates flashes red and fades out over about 700ms:

- a **device row**, each time that device is heard from;
- a **characteristic row**, when its value changes — in the list as well as the
  detail pane, because a subscription fires on characteristics that are not the
  one selected and the list is where that is visible;
- the **value block**, a completed **write**, and a freshly read **descriptor**.

A device in range advertises several times a second, so the device list is a busy
thing by design. ⚙ → *Flash on update* turns the whole effect off.

**Red means exactly one thing here: this changed just now.**

Getting there took three goes, all of which produced flicker:

1. The signal meter painted its weakest bar in alarm red, so a room with distant
   devices in it showed a column of small red rectangles — which reads as a
   fault rather than as "far away".
2. Raw RSSI moves several dB between consecutive packets from a device that is
   sitting perfectly still, so those rectangles changed bucket, and colour,
   several times a second. The list was sorted on the same raw figure, so it
   also reordered under the pointer.
3. Highlighting a device row on every advertisement — which is what "flash when
   updated" means taken literally — left every row permanently lit, because a
   device in range advertises several times a second.

So: the meter has one colour and the bar *count* carries the strength; the count
and the sort order both come from a smoothed reading with hysteresis, so a
stationary device picks an answer and keeps it; and a device row highlights only
when it appears or its name changes — things the row cannot show any other way.
Signal strength is not one of them, because the meter and the number beside it
are already saying it, continuously. The number shown is always the real last
reading.

`nothing_flickers_red_during_a_scan` renders twenty-four frames of a running
scan and fails if a single red pixel appears in any of them.

## Copying things out

An explorer's entire output is identifiers and bytes that have to end up
somewhere else, so everything on screen can leave it:

- **Every value, UUID, identifier and log line is selectable** with the mouse.

  This costs something, and the cost had to be paid rather than avoided: egui
  labels take the pointer for text selection, so a selectable label sitting
  inside a clickable row swallows the click and the row never sees it. Clicking
  a device's *name* — the obvious place to click — did nothing, while the signal
  meter and the margins around the text worked, which made it look intermittent
  rather than broken.
  
  The labels' responses are unioned into the row's instead of being discarded,
  so one gesture still does both: a drag selects the text, a click selects the
  device. Turning selection off would have been the easy fix and the wrong one.
  `clicking_a_device_name_connects_to_it` fails without the union.
- **Copy buttons say what they copy** — *Copy hex*, *Copy text*, *Copy dump*,
  *Copy tree*, *Copy all*. They are words rather than an icon on purpose: the
  obvious `⧉` is not in egui's fonts and renders as an empty rectangle, and the
  nearest glyph that *is* (`⎘`) looks like a redo arrow.
- **Copy all**, beside the characteristic heading, copies the entire detail pane
  as text — UUID, properties, value in every rendering, descriptors — so a paste
  into a bug report is not four separate copies stitched together by hand.
- **Copy tree**, beside the device heading, copies the whole GATT tree as an
  indented text report.
- **Right-click a device row** for its identifier, its name, or its whole
  advertising packet — a tooltip cannot be selected, so everything in it is
  reachable from the context menu instead. Right-click a tree row for its UUID.
- **Save…** in the log pane opens the platform's own save panel, defaulting to
  `webbluetooth-explorer-YYYYMMDD-HHMMSSZ.log` — UTC and sortable, so a directory
  of them lists in the order they were taken.

  Going through the system panel is not politeness, it is the thing that works.
  macOS mediates file access, and a location the *user* picked in the panel
  carries the permission to write there with it; a path this program invents for
  itself is one the system has every reason to refuse, and refuses quietly
  enough to look like a broken feature. A sandboxed build needs
  `com.apple.security.files.user-selected.read-write` in its entitlements and
  nothing else.

  On iOS the right affordance is not a save panel at all but a share sheet —
  `UIActivityViewController` — which is what puts a file into Files, iCloud Drive
  or AirDrop. This build targets macOS, Linux and Windows; iOS is not wired up,
  and the save panel is the desktop half of that story.

## Settings

The ⚙ menu in the toolbar:

| Setting | Default |
| --- | --- |
| Theme — System / Light / Dark | **System**, following the desktop, including when the desktop changes while the window is open |
| Size — −/+/Reset | 100%. Scales the whole window, text and spacing together |
| Flash on update | on |
| Log pane | on |
| Track signal while connected | off — it is traffic on the link |

Settings live for the run. Nothing is written to disk, because nothing else about
this program is either: it holds no grants between runs and keeps no device
history.

Every size in the interface is a multiple of the current line height rather than
a pixel count — the signal meter, text fields, spacing — and the side panels take
a *share of the window* rather than a fixed width, so shrinking the window
shrinks them instead of squeezing the detail pane. At the smallest size this
program allows, fixed panels left the detail pane about a hundred and seventy
points wide with the UUID and the Write button cut off the right-hand edge. The
toolbar wraps for the same reason: right-aligning the ⚙ button pushed it off the
edge entirely at that size, with no way to reach the settings at all.

## Resizing

Both splitters drag, and both had to be fixed before they did anything useful.

Capping each panel at a *fraction* of the window — which is how the narrow-window
clipping above got fixed the first time — left the GATT tree fifty points of
travel at the smallest window, with its minimum equal to its default: dragging
left did nothing at all, and the pane read as fixed. The cap is now expressed as
"leave the pane beside you this much room", which gives the whole remaining width
to drag through: about 470 points of travel at the default window size, up from
about 110.

egui's default grab radius for a panel edge is three points. Measured, a grab six
points off the splitter missed entirely — which is the difference between a
splitter that feels solid and one that feels broken. It is eight here.

`the_splitters_have_room_to_move_and_are_easy_to_grab` and
`the_splitter_can_be_grabbed_slightly_off_centre` drive real pointer events
through the window and assert on the resulting widths.

Narrowing was separately broken, and worse: the middle pane would follow the
pointer smoothly and then stop dead at about 183 points, well short of anything
`min_size` asked for. An egui panel is as wide as its widest row that **cannot
shrink**, and three widgets in here could not:

- `CollapsingHeader` lays its title out with `TextWrapMode::Extend` — it never
  wraps and never truncates — so one service called *Device Information* set the
  floor for the entire pane. The tree drives `CollapsingState` directly now,
  which makes the header an ordinary `Ui` holding an ordinary truncating label.
- `selectable_label` is a `Button`, and a button sizes to its content. The rows
  are `Button::selectable(…).truncate()`.
- The device rows' name, subtitle and *connected* badge, for the same reason.

Each was found by bisecting against a measurement, and each looked equally
plausible beforehand — the first two guesses (`min_size`, then the galley wrap
width) were both wrong. That is why
`a_pane_can_be_narrowed_to_the_minimum_it_declares` is a test and not a comment.

The result, at the default window size:

| Pane | Travel before | Travel now |
| --- | --- | --- |
| Devices | 166 pt | 808 pt |
| GATT tree | 193 pt | 550 pt |

and at the smallest window the program allows, the tree went from 50 points of
one-directional travel to 217.

## Seeing it

`screencapture` needs a Screen Recording grant that a build machine generally
does not have, which leaves every claim about how this *looks* resting on reading
the code. So the interface renders itself:

```sh
WEBBLUETOOTH_EXPLORER_SNAPSHOTS=1 cargo test -p webbluetooth-explorer render
```

writes `tests/snapshots/*.png` — idle, connected, narrow, smallest, and zoomed —
through `egui_kittest` and wgpu, with no window involved. The images are not
baselines: the log column carries a wall clock, so they are never byte-identical
between runs. They are there to be looked at.

Two tests do assert on what is rendered, and both were written because the thing
they check had already gone wrong:

- `the_interface_paints_no_id_clashes` reads egui's own paint list back and fails
  if it finds the 🔥 marker egui draws beside a duplicated widget id. An id clash
  is two widgets fighting over one piece of interaction state — the wrong row
  highlights, the wrong header opens — and it shows up as a red outline that
  flickers as the list re-sorts.
- `every_icon_has_a_glyph` checks every glyph the interface draws against the
  font actually loaded. A missing glyph is neither a compile error nor a runtime
  error; it is a button that looks broken. Every obvious choice failed this when
  it was written: `⧉`, `▶`, `◉`, `▾` and `⚠` are none of them in egui's default
  fonts.

## Why this needs a feature flag

Web Bluetooth grants access to a **named list** of services, because its job is
to stop a web page learning more about a device than the user agreed to. An
explorer is on the other side of that boundary: the interesting services on a
device are the vendor's own 128-bit ones, and their UUIDs cannot be named in
advance — reading them off the device is the entire point.

So this app is the one caller of `webbluetooth`'s `unrestricted` feature, which
adds `Grant::all_services()`. That is deliberately awkward to reach and is off
by default. The GATT blocklist is *not* part of it and still applies: HID
services, unsigned firmware update and the rest stay hidden here exactly as they
would in a browser.

## Platforms

Five targets compile: macOS, Linux, Windows, iOS and Android. Only macOS has
been *run* — everything else is verified by `cargo check` and by the tests,
which is a real check of the plumbing and not a substitute for a radio.

Two things the mobile targets needed, both of which the build found rather than
a person:

**No save panel.** `rfd` has no iOS or Android implementation and fails to
compile there, which is the build saying something true: a modal save panel is
not how either platform saves a file. iOS wants a share sheet
(`UIActivityViewController`) — which is what puts a file into Files, iCloud
Drive or AirDrop — and Android a Storage Access Framework intent. Until those
exist here, mobile writes to the app's own directory and reports the path. The
desktop save panel is unchanged.

**A different layout.** Three panes side by side need about forty line-heights
before any of them is readable, and a phone in portrait has half that. Below
that width the window shows one pane at a time — Devices, GATT, Detail — with a
selector, and picking a device or a characteristic moves you on rather than
making you find the selector. The auxiliary window buttons fold into the ⚙ menu,
because on a phone they are two lines of a four-line toolbar. It is the same
window rearranged, not a reduced one: nothing is unreachable on a phone that is
reachable on a laptop. The switch is decided by width, not by platform — a
narrow window on a desktop has the same problem, and a tablet in landscape has
the same room as a laptop.

Android also has to be told which activity kind hosts the app, and that choice
costs `accesskit`: `android-activity` owns the `android_main` entry point and
`accesskit` is only offered alongside `android-game-activity`, which wants a
Java subclass in the APK. This builds against `android-native-activity`, which
needs no Java side.

Settings go to the platform's own configuration directory. On iOS that is
`Documents` inside the app sandbox, which is correct and backed up. On Android
the right answer is the activity's `internal_data_path`, which only the Java
side knows and which this program has no route to yet — it falls back to a
directory it can write, which is worth knowing.

## macOS

Bluetooth is gated behind TCC. A process with no
`NSBluetoothAlwaysUsageDescription` is reported unauthorised and **is never
prompted** — it silently sees no adapter.

- `cargo run` works: `build.rs` links this crate's `Info.plist` into the binary's
  `__TEXT,__info_plist` section.
- For a real app, `./scripts/bundle-macos.sh` builds `target/WebBluetoothExplorer.app` and
  ad-hoc signs it, which gives TCC a stable identity to remember across
  rebuilds. `--install` copies it to `/Applications`.

The permission prompt is attributed to the process *responsible* for the launch,
not to this binary. Start it from Finder or Terminal.app the first time —
launched from an editor or a build agent it is denied with no dialog, and the
window then reports no adapter. `cargo run -p webbluetooth --example doctor` says
which of those has happened.

There is no application icon. The original's is its author's work, not something
to copy.

## Linux

Needs a running `bluetoothd`; the app talks to it over D-Bus and wants no special
privileges. With `webbluetooth`'s `linux-hci` feature the library can drive the
controller directly instead, which needs `CAP_NET_RAW` for scanning and cannot
pair — this app does not enable it.

## Windows

Nothing to set up. WinRT prompts on first use if the device is unpaired.

## How it is put together

| File | Responsibility |
| --- | --- |
| `engine.rs` | Every `webbluetooth` handle, on one thread running a `LocalPool`. Takes `Command`s, emits `Event`s. No UI types. |
| `model.rs` | What is on screen, and how an `Event` changes it. Where the rules live, and what the tests test. |
| `app.rs` | Drawing. Reads the model, sends commands, holds nothing else. |
| `names.rs` | Assigned-number lookup, UUID → readable name. |
| `format.rs` | Bytes to hex / ASCII / decimal / readings, and hex back to bytes. |
| `tests/radio.rs` | The one test that needs hardware. Everything else runs without a radio. |

egui is immediate mode, so the UI function runs every frame and must never
block. Nothing in `app.rs` awaits anything: the engine owns the radio, long
operations are separate tasks on its pool, and a `connect()` against a device
that has walked out of range cannot stall the window, the scan, or a read on
something else. The UI holds ids, UUIDs and bytes — never a GATT handle, whose
generation the backend can invalidate underneath it.

## Tests

```sh
cargo test -p webbluetooth-explorer
```

runs without a radio: the value formatter, the UUID tables, the fade curve, and
the model's rules — which device is selected after the list re-sorts, whether a
late answer from a superseded connection can land on the wrong row.

The radio is a separate, opt-in test, because it needs hardware, something in
range, and about fifteen seconds:

```sh
WEBBLUETOOTH_EXPLORER_RADIO_TEST=1 \
  cargo test -p webbluetooth-explorer --test radio -- --nocapture
```

It scans until some device reports *twice* — proving the signal column will
update rather than freeze at first sighting — then connects to the strongest
advertiser, asserts that discovery came back with something, and reads the first
readable characteristic it finds. A device that refuses to connect or refuses the
read is the peripheral's decision, and is skipped rather than failed; discovery
returning nothing on a device that *did* connect is a failure, because that is
exactly what the `unrestricted` grant exists to prevent.

## Beyond browsing

The list below was a gap analysis against nRF Connect for Mobile, read off its
own layouts and menus. It is now what this does.

**Name your own UUIDs.** The SIG has named about four hundred attributes and
none of them are the ones a device actually publishes — which is the one thing
the `unrestricted` grant exists to show you, and the one thing you then cannot
read. Right-click anything in the tree, or **⚙ → UUID names**. Names apply
everywhere, including in copied reports and macro steps, and outrank the SIG's
if you name something it has already named.

**Rename devices, and keep favourites.** Half a bench advertises as `n/a` or as
four copies of the same model name. Right-click a row to rename it or star it;
favourites sort to the top, and **⚙ → Autoconnect** reconnects to the strongest
one whenever nothing else is connected — for a device that sleeps and wakes.

**Macros.** Record a sequence of reads, writes and subscriptions, name it, file
it in a folder, and replay it against any device with the same services — once,
a fixed number of times, or looping until stopped. Steps can **wait for a
notification** rather than a fixed delay, optionally for one starting with
particular bytes, with a timeout: a device that answers in 20 ms on the bench
and 300 ms over a congested link needs a sequence that waits for the answer, not
one tuned to whichever it did last. Steps name characteristics
by UUID rather than by position, so a macro survives rediscovery and moves
between devices. A delay is inserted between recorded steps, because a sequence
replayed at full speed rarely works against hardware that was keeping up with a
human. A step that cannot be resolved stops the run rather than writing the rest
of the sequence to a device that did not accept the first part.

**A GATT server, and an advertiser.** Everything else here is the central role —
reading somebody else's device. **Server…** publishes services of your own and
advertises them, which is how you exercise the other side of what you are
building without a second piece of hardware. Templates for Battery, Device
Information, Nordic UART and Heart Rate, or **Clone this device** to stand up a
copy of whatever is in front — the fastest way to get a server a central already
knows how to talk to. Anything a central does to your server appears in the log.

**Known control-point values.** Most writes in a browser are to a control point,
whose encoding lives in a profile specification rather than in anything the
device advertises. Selecting one offers its defined values — Alert Level, Ringer
Control Point, Record Access Control Point, SC Control Point, the CCCD — each
with the bytes it sends and a note on what it means. Only encodings stated
plainly in their specification are included; a wrong guess writes wrong bytes to
somebody's hardware. Nordic's DFU control points are deliberately absent:
putting a device into bootloader mode from a write box is a destructive
operation dressed as a convenience.

**Recent writes.** The last dozen values written to each characteristic, newest
first, one click to put back in the field.

**Signal over time.** **Signal…** graphs every visible device's RSSI and exports
the readings as CSV. Click a name in the legend to pick its line out of the rest.
Beside each is a rough distance from signal strength, with an adjustable
one-metre reference — a log-distance model that assumes a clear path, so a wall
reads as several metres and it is shown as an approximation rather than a number
to act on.

**Scan filters worth the name.** By advertised service, by manufacturer company
identifier, by advertising-data content, and an exclusion list — because in a
building full of radios the useful question is usually "everything except that
lot". A company filter that does not parse matches nothing rather than quietly
matching everything.

**Log levels.** Debug is every read, write and notification; Info is what
happened; Warn is a refusal the program carried on through; Error is a failure.
The level and a text filter decide what is shown, and what is shown is what gets
copied or saved.

All of it — names, renames, favourites, macros, write history, window settings —
is remembered between runs in one tab-separated file under the platform's own
configuration directory. A record it cannot parse is skipped rather than fatal,
so a file written by a later version still gives up what this one understands.

### More than one device at a time

Each connection gets a tab, and each tab keeps its own tree, its own selection
and its own link state. A tab closes with the × beside its name, which
disconnects the device.

That × was fifteen points square, eight points from a tab that also takes
clicks. It worked perfectly and was, in practice, impossible to hit — with a
mouse, and hopelessly so with a thumb on the phone targets. It is now a real
square, larger again where the window is phone-shaped, with space before the
next tab so overshooting does not select a different device.
`a_tab_close_control_is_big_enough_to_hit` measures it, because "it works" and
"you can use it" turned out to be different claims. Comparing two units on a bench, or watching a central
and a peripheral at once, was the thing a single connection made impossible.

This is why a `CharRef` names a device as well as a position in the tree. With
one connection a pair of indices was unambiguous; with several, a value that
arrives after you have moved to another tab has to still land on the row it
belongs to. Routing by "whatever is in front" would put it on the wrong device,
silently, and only sometimes — `a_value_lands_on_its_own_device_not_the_active_one`
is the test for exactly that.

### Reading values, not bytes

A GATT value is bytes, and the hex/ASCII/decimal renderings are the honest thing
to show when nobody knows what they are. For the characteristics the SIG has
defined, somebody does: Battery Level reads as `87 %`, Heart Rate Measurement
reads its flags byte before its value — and gets the 8- versus 16-bit rate,
contact detection, energy and RR intervals right — Temperature Measurement reads
an IEEE-11073 medical float, which is not an IEEE-754 one and gives a
plausible-looking wrong number if treated as such.

**104 characteristics and 6 descriptors** are read this way — the Device
Information and Generic Access blocks, the whole time cluster, heart rate, blood
pressure, weight, cycling and running, glucose and continuous glucose,
environmental sensing, alerts, the User Data profile, Scan Parameters, the
Fitness Machine feature and range characteristics, and Apple's ANCS Notification
Source.

Only encodings stated plainly in their specification are decoded, and each
decoder refuses a value of the wrong length rather than guessing. A wrong
reading is worse than none: it looks like an answer — which is also why the
Fitness Machine *data* characteristics are absent: they are flag-driven with
field orders I could not state confidently.

Advertising data gets the same treatment where the raw packet is available —
split into its length-type-value records, each named from Assigned Numbers, with
Flags, TX power, intervals and manufacturer data read out.

### Beacons

Most of what a busy scan contains is beacons, and in a scanner that does not
know them they are an unlabelled run of manufacturer bytes. iBeacon and
AltBeacon are read out of manufacturer data; Eddystone UID, URL, TLM and EID out
of service data — including expanding a compressed Eddystone URL back into a
URL, and reading telemetry's two "not supported" sentinels as sentinels rather
than as a measurement.

One thing worth knowing: an iBeacon's proximity UUID is a location fix, and
`webbluetooth`'s manufacturer blocklist strips Apple's iBeacon frames before
they reach this program at all. So the iBeacon reader runs on frames from other
companies using the same layout, and on nothing else — the correct outcome, not
a gap.

### Timeline

**Timeline…** answers a different question from the signal graph: not how close
a device is but whether it was there, and when it stopped. One thin mark per
advertisement, so the density is the information and a device that sleeps shows
up as a gap.

### Suites

A macro is a recording replayed against whatever is in front. A **suite** is the
same sequence turned into a question with an answer: it names the devices it runs
against, asserts what it should read back, and leaves a result rather than a log
to read through. A macro that "worked" is one where nothing obviously broke; a
suite that passed is one where every expectation held.

Build one by creating a suite, choosing its targets, and adding a recorded macro
as a test — the macro supplies the steps, the suite adds the expectations.
Targets are named by device id, so a suite survives the tab order changing and
runs against several devices without anyone clicking between them. A failing test
stops that test and moves to the next: a suite is many independent questions and
the answers to the rest are still worth having, which is the opposite of a macro,
where half a sequence is worse than none.

### Still missing

| Theirs | Why not |
| --- | --- |
| Advertiser depth — TX power, scan response, interval, timeout, saved advertisement sets | `webbluetooth`'s `Advertising` carries a local name and service UUIDs and nothing else; this needs a library change across four peripheral backends |
| Fitness Machine data characteristics — Treadmill, Indoor Bike, Rower, Cross Trainer | flag-driven with field orders I could not state confidently; a wrong decoder here prints a plausible number, which is worse than hex |
| Nordic Thingy:52's ~20 vendor characteristics, Eddystone configuration | vendor UUIDs and layouts I have no hardware to check against |
| Descriptors on published characteristics | the library has the type; this app does not yet drive it |
| Definitions import/export as JSON | interoperates with nRF Connect for Desktop; ours are local only |
| Reliable write (begin / execute / abort) | `webbluetooth` exposes it as a property flag; there is no operation to call |
| Refresh or clone the GATT cache, delete bond information | CoreBluetooth has no API for either |
| Scanner mode and period, `autoConnect`, connect-with-PHY | Android-specific knobs the portable API does not expose |
| Eddystone, DFU and MCUmgr write assistants | Nordic bootloader and beacon protocols, not GATT browsing |
| The Desktop launcher's Programmer, Power Profiler, Cellular Monitor, Direct Test Mode | all need Nordic hardware |

### Raw advertising bytes

nRF Connect shows the unparsed packet. `webbluetooth`'s `Advertisement` carried
only parsed fields, so this needed a library change: `Advertisement::raw` now
carries the bytes where the transport hands them over.

Only a raw HCI socket does — CoreBluetooth reports a dictionary, BlueZ a set of
D-Bus properties, the browser an event object — so this is populated on Linux
with the `linux-hci` backend and `None` elsewhere, which the field says plainly
rather than pretending otherwise.

It also needed a rule. The raw packet contains every company's manufacturer data
whole, which is exactly what the grant's manufacturer filtering exists to
restrict; handing it to a caller granted one company identifier would hand over
the rest. So the bytes are withheld from any grant that does not already permit
all manufacturer data, and a scanner opts in with
`LeScanOptions::accept_all_manufacturer_data()` — behind the same `unrestricted`
feature, for the same reason.
