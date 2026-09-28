//! The window.
//!
//! Four panes, as the original had them: devices, services, characteristics,
//! and a detail view for the selected characteristic, over a log. Nothing here
//! touches Bluetooth — it reads [`Model`] and sends [`Command`]s, which is what
//! keeps the frame rate independent of the radio.

use crate::engine::{CharRef, Command, Handle, Level};
use crate::format;
use crate::model::{descriptor_ref, CharacteristicView, Model, Sort};
use crate::names::{self, Kind};
use egui::{Color32, RichText};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use webbluetooth::CharacteristicProperties;
use webbluetooth::{ConnectionPriority, Phy};

/// The glyphs the interface draws.
///
/// Collected here because egui ships Ubuntu-Light plus a small icon font, and
/// most of the obvious choices are in neither: `⧉` for copy, `▶` for play, `◉`
/// for a live subscription and `⚠` for a warning all render as an empty box.
/// A missing glyph is not a compile error and not a runtime error — it is a
/// rectangle on screen that looks like a broken feature — so
/// `every_icon_has_a_glyph` checks the whole set against the actual font.
mod icon {
    /// Start scanning.
    pub const SCAN: &str = "⏵";
    /// Stop scanning.
    pub const STOP: &str = "⏹";
    /// This characteristic has a live subscription.
    pub const SUBSCRIBED: &str = "⏺";
    /// Settings.
    pub const SETTINGS: &str = "⚙";
    /// Make the interface smaller. A real minus sign, not a hyphen.
    pub const SMALLER: &str = "−";
    /// Make it larger.
    pub const LARGER: &str = "+";
    /// At least, for the signal floor.
    pub const AT_LEAST: &str = "≥";
    /// Close a tab.
    pub const CLOSE: &str = "×";

    /// Everything above, for the test that checks them.
    #[cfg(test)]
    pub const ALL: &[(&str, &str)] = &[
        ("SCAN", SCAN),
        ("STOP", STOP),
        ("SUBSCRIBED", SUBSCRIBED),
        ("SETTINGS", SETTINGS),
        ("SMALLER", SMALLER),
        ("LARGER", LARGER),
        ("AT_LEAST", AT_LEAST),
        ("CLOSE", CLOSE),
    ];
}

/// How the write field is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteAs {
    /// Hex digits, however they are separated.
    Hex,
    /// The text itself, as UTF-8.
    Text,
}

/// The application.
pub struct App {
    model: Model,
    engine: Handle,
    write_text: String,
    write_as: WriteAs,
    write_with_response: bool,
    write_error: Option<String>,
    /// Hex being typed into a descriptor row, by descriptor index. Cleared
    /// whenever the selection moves, for the same reason the main write field
    /// is.
    descriptor_writes: std::collections::HashMap<usize, String>,
    /// Poll the connected device's RSSI on a timer, as the original's device
    /// pane did. Off by default: it is traffic on the link.
    track_rssi: bool,
    last_rssi_poll: Instant,
    /// What the MTU box in the link menu is asking for.
    wanted_mtu: u16,
    /// Something in [`crate::store::Remembered`] changed and is not on disk.
    dirty: bool,
    /// When it was last written, so a busy session does not write constantly.
    last_saved: Instant,
    /// Which auxiliary windows are open.
    show_filters: bool,
    show_definitions: bool,
    show_graph: bool,
    show_timeline: bool,
    /// Which device's line is picked out of the graph, if any.
    graph_highlight: Option<String>,
    /// Measured signal at one metre, for the distance estimate.
    rssi_at_one_metre: i32,
    show_server: bool,
    show_macros: bool,
    show_suites: bool,
    /// The name being typed for a suite about to be created.
    suite_name: String,
    /// Steps captured since recording started, if it is on.
    ///
    /// `Some(vec![])` is "recording, nothing yet", which is a different state
    /// from `None` — the button has to be able to tell them apart.
    recording: Option<Vec<crate::macros::Step>>,
    /// The name being typed for the macro about to be saved.
    macro_name: String,
    /// The folder it will be filed under.
    macro_group: String,
    /// The server being composed, whether or not it is published.
    server: crate::engine::ServerSetup,
    /// Free text for the characteristic being added.
    new_service: String,
    new_characteristic: String,
    /// Connect to a favourite as soon as it is seen.
    autoconnect: bool,
    /// The device being renamed, and the name being typed.
    renaming: Option<(String, String)>,
    /// A UUID being named, its kind, and the name being typed.
    defining: Option<(webbluetooth::uuid::BluetoothUuid, Kind, String)>,
    show_log: bool,
    /// Whether the device list is on screen. It can be put away to give a
    /// small window entirely to the device being worked on.
    show_devices: bool,
    /// Which pane a narrow window is showing. Ignored when there is room for
    /// all of them.
    pane: Pane,
    /// Light, dark, or whatever the desktop is set to. `System` by default,
    /// which is also egui's own default — set explicitly anyway, so the
    /// setting and the state cannot disagree at startup.
    theme: egui::ThemePreference,
    /// Whether a value that just changed is highlighted.
    flash_updates: bool,
}

impl App {
    /// Build the window and start the engine thread.
    pub fn new(ctx: &egui::Context) -> Self {
        let engine = crate::engine::start(ctx.clone());
        // Nothing to look at until the radio is listening, and the first thing
        // anybody does with this program is scan.
        engine.send(Command::SetScanning(true));

        let mut app = Self::with_engine(engine);
        app.model.remembered = crate::store::load();
        app.restore_settings(ctx);
        app
    }

    /// Put the window back the way it was left.
    fn restore_settings(&mut self, ctx: &egui::Context) {
        let remembered = &self.model.remembered;
        self.theme = match remembered.setting("theme", "system") {
            "light" => egui::ThemePreference::Light,
            "dark" => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        };
        ctx.set_theme(self.theme);
        ctx.set_zoom_factor(
            remembered
                .setting("zoom", "1")
                .parse::<f32>()
                .unwrap_or(1.0)
                .clamp(0.5, 3.0),
        );
        self.flash_updates = remembered.flag("flash", true);
        self.show_log = remembered.flag("log_pane", true);
        self.show_devices = remembered.flag("device_list", true);
        self.track_rssi = remembered.flag("track_rssi", false);
        self.autoconnect = remembered.flag("autoconnect", false);
        self.model.favourites_first = remembered.flag("favourites_first", true);
        self.model.named_only = remembered.flag("named_only", false);
        self.model.connectable_only = remembered.flag("connectable_only", false);
        self.model.sort = match remembered.setting("sort", "signal") {
            "name" => Sort::Name,
            "recent" => Sort::LastSeen,
            _ => Sort::Rssi,
        };
        self.model.log_level = crate::engine::Level::ALL
            .into_iter()
            .find(|level| level.name() == remembered.setting("log_level", "Info"))
            .unwrap_or(crate::engine::Level::Info);
    }

    /// Copy the current settings back into what will be written.
    fn capture_settings(&mut self, ctx: &egui::Context) {
        let zoom = ctx.zoom_factor();
        let remembered = &mut self.model.remembered;
        remembered.settings.insert(
            "theme".into(),
            match self.theme {
                egui::ThemePreference::Light => "light",
                egui::ThemePreference::Dark => "dark",
                egui::ThemePreference::System => "system",
            }
            .into(),
        );
        remembered
            .settings
            .insert("zoom".into(), format!("{zoom:.2}"));
        remembered.set_flag("flash", self.flash_updates);
        remembered.set_flag("log_pane", self.show_log);
        remembered.set_flag("device_list", self.show_devices);
        remembered.set_flag("track_rssi", self.track_rssi);
        remembered.set_flag("autoconnect", self.autoconnect);
        remembered.set_flag("favourites_first", self.model.favourites_first);
        remembered.set_flag("named_only", self.model.named_only);
        remembered.set_flag("connectable_only", self.model.connectable_only);
        remembered.settings.insert(
            "sort".into(),
            match self.model.sort {
                Sort::Rssi => "signal",
                Sort::Name => "name",
                Sort::LastSeen => "recent",
            }
            .into(),
        );
        remembered
            .settings
            .insert("log_level".into(), self.model.log_level.name().into());
    }

    /// Note that something worth remembering changed.
    fn remember(&mut self) {
        self.dirty = true;
    }

    /// Write it out, at most once a second.
    ///
    /// Batched rather than written on every change: toggling a checkbox should
    /// not touch the disk, and losing at most a second of settings if the
    /// process is killed costs nothing.
    fn flush(&mut self, ctx: &egui::Context) {
        if !self.dirty || self.last_saved.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.capture_settings(ctx);
        self.dirty = false;
        self.last_saved = Instant::now();
        if let Err(why) = crate::store::save(&self.model.remembered) {
            self.model.apply(crate::engine::Event::Log(
                Level::Warn,
                format!("could not save settings: {why}"),
            ));
        }
    }

    /// The same window, with no radio behind it. For rendering in a test.
    #[cfg(test)]
    fn detached() -> Self {
        Self::with_engine(crate::engine::Handle::detached())
    }

    fn with_engine(engine: Handle) -> Self {
        Self {
            model: Model::default(),
            engine,
            write_text: String::new(),
            write_as: WriteAs::Hex,
            write_with_response: true,
            write_error: None,
            descriptor_writes: std::collections::HashMap::new(),
            track_rssi: false,
            last_rssi_poll: Instant::now(),
            // 247 is what a 251-byte LL payload leaves for ATT, and what most
            // peripherals that negotiate at all settle on.
            wanted_mtu: 247,
            dirty: false,
            last_saved: Instant::now(),
            show_filters: false,
            show_definitions: false,
            show_graph: false,
            show_timeline: false,
            graph_highlight: None,
            // The middle of the range nRF Connect suggests.
            rssi_at_one_metre: -66,
            show_server: false,
            show_macros: false,
            show_suites: false,
            suite_name: String::new(),
            recording: None,
            macro_name: String::new(),
            macro_group: String::new(),
            server: crate::engine::ServerSetup {
                local_name: "WebBluetoothExplorer".into(),
                services: Vec::new(),
            },
            new_service: String::new(),
            new_characteristic: String::new(),
            autoconnect: false,
            renaming: None,
            defining: None,
            show_log: true,
            show_devices: true,
            pane: Pane::Devices,
            theme: egui::ThemePreference::System,
            flash_updates: true,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// Draw one frame.
    ///
    /// Separate from the `eframe::App` impl so that a test can run the whole
    /// interface headlessly: constructing an `eframe::Frame` needs a window,
    /// and an `egui::Ui` does not. That is what makes the id-clash check below
    /// possible at all.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        // egui's default grab radius for a panel edge is three points, which is
        // a target you have to aim at: measured, a grab six points off the
        // splitter missed entirely. This window is mostly panels, and their
        // widths are the main thing worth adjusting, so the splitters are given
        // a hit area you can find without looking.
        //
        // Set every frame rather than once, because it has to survive a theme
        // change — the light and dark styles are separate objects.
        ui.ctx().all_styles_mut(|style| {
            style.interaction.resize_grab_radius_side = SPLITTER_GRAB;
        });

        for event in self.engine.drain() {
            self.model.apply(event);
        }

        if self.track_rssi && self.last_rssi_poll.elapsed() >= Duration::from_secs(1) {
            self.last_rssi_poll = Instant::now();
            // Every open link, not only the one in front: a tab in the
            // background is still connected, and its graph line should not stop.
            let open: Vec<String> = self
                .model
                .sessions
                .iter()
                .filter(|session| session.connected)
                .map(|session| session.id.clone())
                .collect();
            for id in open {
                self.engine.send(Command::ReadRssi(id));
            }
        }

        // While scanning, "last seen" ages every frame whether or not an event
        // arrived, so the list has to redraw on a clock as well as on events.
        if self.model.scanning {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }

        // A fade is animation: it has to redraw on a clock, not only when the
        // next event arrives, or it freezes part-way through.
        if self.flash_updates && self.anything_fading() {
            ui.ctx().request_repaint_after(Duration::from_millis(32));
        }

        // A phone-sized window shows one pane at a time. Decided from the
        // width rather than from the platform, because a narrow window on a
        // desktop has exactly the same problem and a tablet in landscape has
        // exactly the same room as a laptop.
        let compact = ui.available_width() < em(ui) * COMPACT_BELOW;

        // Order is the layout: outer panels claim their edge first, and the
        // central pane gets whatever is left.
        self.toolbar(ui, compact);
        self.autoconnect_if_asked();
        self.filters_window(ui.ctx());
        self.graph_window(ui.ctx());
        self.timeline_window(ui.ctx());
        self.server_window(ui.ctx());
        self.macros_window(ui.ctx());
        self.suites_window(ui.ctx());
        self.definitions_window(ui.ctx());
        self.rename_window(ui.ctx());
        if self.show_log {
            self.log_pane(ui);
        }
        if compact {
            self.compact_panes(ui);
        } else {
            if self.show_devices {
                self.device_pane(ui);
            }
            self.gatt_panes(ui);
        }
        self.flush(ui.ctx());
    }
}

impl App {
    fn toolbar(&mut self, ui: &mut egui::Ui, compact: bool) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            // Wrapped, and with the settings button at the *start*. It used to
            // be right-aligned at the end, which meant that at the smallest
            // window this program allows it was pushed off the edge entirely
            // and there was no way to reach the settings at all. A row that
            // wraps loses nothing.
            ui.horizontal_wrapped(|ui| {
                self.settings_menu(ui, compact);
                ui.separator();

                let label = if self.model.scanning {
                    format!("{}  Stop", icon::STOP)
                } else {
                    format!("{}  Scan", icon::SCAN)
                };
                if ui.button(label).clicked() {
                    self.engine.send(Command::SetScanning(!self.model.scanning));
                }
                if ui.button("Clear").clicked() {
                    // Keep whatever is connected: clearing the list is about
                    // the room being noisy, not about dropping the link.
                    // Keep whatever is open: clearing the list is about the
                    // room being noisy, not about dropping links.
                    let keep: Vec<String> = self
                        .model
                        .sessions
                        .iter()
                        .map(|session| session.id.clone())
                        .collect();
                    self.model.devices.retain(|id, _| keep.contains(id));
                }

                ui.separator();
                ui.label(format!("{} devices", self.model.devices.len()));

                ui.separator();
                ui.label("Sort");
                ui.selectable_value(&mut self.model.sort, Sort::Rssi, "Signal");
                ui.selectable_value(&mut self.model.sort, Sort::Name, "Name");
                ui.selectable_value(&mut self.model.sort, Sort::LastSeen, "Recent");

                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut self.model.filter)
                        .hint_text("filter name or id")
                        .desired_width(em(ui) * 10.0),
                );
                // On a phone these six buttons are two whole lines of a
                // four-line toolbar, on a screen that has none to spare. They
                // move into the settings menu instead, which is one line.
                if !compact {
                    self.window_buttons(ui);
                }
                ui.checkbox(&mut self.model.named_only, "Named");
                ui.checkbox(&mut self.model.connectable_only, "Connectable")
                    .on_hover_text(
                        "Hide devices that said they do not accept connections. A \
                         device that did not say either way stays.",
                    );
                let mut floor = self.model.rssi_floor.unwrap_or(-100);
                let mut limited = self.model.rssi_floor.is_some();
                if ui
                    .checkbox(&mut limited, icon::AT_LEAST)
                    .on_hover_text("Hide anything weaker than the signal floor")
                    .changed()
                {
                    self.model.rssi_floor = limited.then_some(floor);
                }
                if limited
                    && ui
                        .add(
                            egui::DragValue::new(&mut floor)
                                .range(-100..=-30)
                                .suffix(" dBm"),
                        )
                        .changed()
                {
                    self.model.rssi_floor = Some(floor);
                }

                if let Some(why) = &self.model.unavailable {
                    ui.separator();
                    ui.label(
                        RichText::new(format!("Bluetooth unavailable — {why}"))
                            .color(ui.visuals().warn_fg_color)
                            .strong(),
                    );
                }
            });
            ui.add_space(3.0);
        });
    }

    /// The buttons that open the auxiliary windows.
    fn window_buttons(&mut self, ui: &mut egui::Ui) {
        // A filter still narrowing the list when its window is shut is
        // indistinguishable from a room that has gone quiet, so the button
        // stays lit.
        let narrowed = self.model.filters_are_narrowing();
        if ui
            .selectable_label(self.show_suites, "Suites…")
            .on_hover_text("Sequences with assertions, run against chosen devices")
            .clicked()
        {
            self.show_suites = !self.show_suites;
        }
        if ui
            .selectable_label(self.show_macros || self.recording.is_some(), "Macros…")
            .on_hover_text("Record a sequence of operations and replay it")
            .clicked()
        {
            self.show_macros = !self.show_macros;
        }
        if ui
            .selectable_label(self.show_server || self.model.server_running, "Server…")
            .on_hover_text("Publish services of your own and advertise them")
            .clicked()
        {
            self.show_server = !self.show_server;
        }
        if ui
            .selectable_label(self.show_timeline, "Timeline…")
            .on_hover_text("When each device was heard from")
            .clicked()
        {
            self.show_timeline = !self.show_timeline;
        }
        if ui
            .selectable_label(self.show_graph, "Signal…")
            .on_hover_text("Signal strength over time, and CSV export")
            .clicked()
        {
            self.show_graph = !self.show_graph;
        }
        if ui
            .selectable_label(self.show_filters || narrowed, "Filters…")
            .on_hover_text("Service, company, advertising data, exclusions, signal floor")
            .clicked()
        {
            self.show_filters = !self.show_filters;
        }
    }

    /// Connect to a favourite the moment it turns up.
    ///
    /// nRF Connect calls this autoconnect. Useful for a device that sleeps and
    /// wakes: leave the scan running and it reconnects itself rather than
    /// needing to be caught in the list. Deliberately only when nothing else is
    /// connected — a surprise disconnection to chase something else would be
    /// worse than not doing it.
    fn autoconnect_if_asked(&mut self) {
        // Only when nothing is open at all. With tabs it would be possible to
        // keep opening them, which is a way to end up connected to a room.
        if !self.autoconnect || !self.model.sessions.is_empty() {
            return;
        }
        // The strongest favourite in range, so a shelf of them does not race.
        let best = self
            .model
            .visible_devices()
            .into_iter()
            .filter(|row| self.model.is_favourite(&row.id))
            .filter(|row| row.adv.connectable != Some(false))
            .max_by(|a, b| {
                a.rssi_smoothed
                    .unwrap_or(f32::NEG_INFINITY)
                    .total_cmp(&b.rssi_smoothed.unwrap_or(f32::NEG_INFINITY))
            })
            .map(|row| row.id.clone());

        if let Some(id) = best {
            self.model.apply(crate::engine::Event::Log(
                Level::Info,
                format!("autoconnecting to favourite {id}"),
            ));
            self.engine.send(Command::Connect(id));
        }
    }

    /// Signal strength over time, for every device the filters leave visible.
    ///
    /// Painted rather than plotted with a charting crate: it is a handful of
    /// polylines on a grid, and a dependency for that would be larger than the
    /// rest of this program.
    fn graph_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_graph;
        let mut export = false;
        let mut pick: Option<Option<String>> = None;
        egui::Window::new("Signal")
            .open(&mut open)
            .resizable(true)
            .default_size([em_of(ctx) * 34.0, em_of(ctx) * 16.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Last few minutes, strongest at the top")
                            .small()
                            .weak(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("Export CSV…")
                            .on_hover_text("Every visible device's readings")
                            .clicked()
                        {
                            export = true;
                        }
                    });
                });

                let rows: Vec<_> = self
                    .model
                    .visible_devices()
                    .into_iter()
                    .filter(|row| !row.history.is_empty())
                    .cloned()
                    .collect();
                if rows.is_empty() {
                    ui.label(RichText::new("Nothing heard yet.").weak());
                    return;
                }
                // The window is a scale in dBm; the axis is fixed rather than
                // fitted so that a device getting weaker visibly gets weaker
                // instead of the graph rescaling under it.
                const TOP: f32 = -30.0;
                const BOTTOM: f32 = -100.0;
                let span = self
                    .model
                    .devices
                    .values()
                    .flat_map(|row| row.history.front())
                    .map(|(at, _)| at.elapsed().as_secs_f32())
                    .fold(1.0_f32, f32::max);

                let height = (ui.available_height() - em(ui)).max(em(ui) * 6.0);
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);
                let grid = ui.visuals().weak_text_color().gamma_multiply(0.25);
                for dbm in [-40, -55, -70, -85, -100] {
                    let y = rect.top()
                        + rect.height() * ((TOP - dbm as f32) / (TOP - BOTTOM)).clamp(0.0, 1.0);
                    painter.line_segment(
                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                        egui::Stroke::new(1.0, grid),
                    );
                    painter.text(
                        egui::pos2(rect.left() + 2.0, y),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{dbm}"),
                        egui::TextStyle::Small.resolve(ui.style()),
                        ui.visuals().weak_text_color(),
                    );
                }

                let highlighted = self.graph_highlight.clone();
                for (index, row) in rows.iter().enumerate() {
                    let dimmed = highlighted.as_ref().is_some_and(|wanted| wanted != &row.id);
                    let colour = if dimmed {
                        series_colour(index).gamma_multiply(0.25)
                    } else {
                        series_colour(index)
                    };
                    let points: Vec<egui::Pos2> = row
                        .history
                        .iter()
                        .map(|(at, dbm)| {
                            let age = at.elapsed().as_secs_f32();
                            let x = rect.right() - rect.width() * (age / span).clamp(0.0, 1.0);
                            let y = rect.top()
                                + rect.height()
                                    * ((TOP - *dbm as f32) / (TOP - BOTTOM)).clamp(0.0, 1.0);
                            egui::pos2(x, y)
                        })
                        .collect();
                    if points.len() > 1 {
                        painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, colour)));
                    }
                }

                ui.horizontal_wrapped(|ui| {
                    for (index, row) in rows.iter().enumerate() {
                        // Click a name to pick its line out of the rest, which
                        // is the only way to read a graph with a dozen devices
                        // on it. nRF Connect long-presses for the same reason.
                        let picked = highlighted.as_deref() == Some(row.id.as_str());
                        if ui
                            .selectable_label(
                                picked,
                                RichText::new(format!(
                                    "— {}{}",
                                    self.model.display_name(row),
                                    row.rssi_smoothed
                                        .map(|dbm| format!(
                                            "  ~{}",
                                            distance(dbm, self.rssi_at_one_metre)
                                        ))
                                        .unwrap_or_default()
                                ))
                                .small()
                                .color(series_colour(index)),
                            )
                            .on_hover_text(
                                "Click to pick this line out. Distance is a rough estimate \
                                 from signal strength — it assumes a clear path, and walls \
                                 make it wrong.",
                            )
                            .clicked()
                        {
                            pick = Some(if picked { None } else { Some(row.id.clone()) });
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("1 m reference").small().weak());
                    ui.add(
                        egui::DragValue::new(&mut self.rssi_at_one_metre)
                            .range(-90..=-40)
                            .suffix(" dBm"),
                    )
                    .on_hover_text(
                        "Measured signal at one metre, used for the distance estimate. \
                         Typical values are -73 to -60 dBm.",
                    );
                });
                // A live graph has to redraw whether or not an event arrived.
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            });
        self.show_graph = open;
        if let Some(choice) = pick {
            self.graph_highlight = choice;
        }

        if export {
            match save_csv_interactively(&self.model.signal_csv()) {
                None => {}
                Some(Ok(path)) => self.model.apply(crate::engine::Event::Log(
                    Level::Info,
                    format!("signal readings written to {}", path.display()),
                )),
                Some(Err(why)) => self.model.apply(crate::engine::Event::Log(
                    Level::Error,
                    format!("could not write the readings: {why}"),
                )),
            }
        }
    }

    /// Suites: sequences with assertions, run against chosen targets.
    ///
    /// A macro is a recording replayed against whatever is in front. A suite is
    /// the same sequence turned into a question with an answer — it names its
    /// targets, asserts what it expects, and leaves a result rather than a log
    /// to read back. A macro that "worked" is one where nothing obviously
    /// broke; a suite that passed is one where every expectation held.
    fn suites_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_suites;
        let mut run: Option<crate::suites::Suite> = None;
        let mut remove: Option<usize> = None;
        let mut toggle_target: Option<(usize, String)> = None;
        let mut promote: Option<(usize, String, Vec<crate::suites::Operation>)> = None;
        let mut changed = false;
        let running = self.model.macro_running;

        egui::Window::new("Suites")
            .open(&mut open)
            .resizable(true)
            .default_size([em_of(ctx) * 32.0, em_of(ctx) * 22.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.suite_name)
                            .hint_text("new suite")
                            .desired_width(em(ui) * 10.0),
                    );
                    if ui.button("Create").clicked() && !self.suite_name.trim().is_empty() {
                        let name = self.suite_name.trim().to_owned();
                        if !self.model.remembered.suites.iter().any(|s| s.name == name) {
                            self.model.remembered.suites.push(crate::suites::Suite {
                                name,
                                ..crate::suites::Suite::default()
                            });
                            changed = true;
                        }
                        self.suite_name.clear();
                    }
                });

                if let Some(results) = &self.model.suite_results {
                    ui.separator();
                    ui.horizontal(|ui| {
                        let failed = results.failed() > 0;
                        ui.label(
                            RichText::new(format!("{}: {}", results.suite, results.summary()))
                                .strong()
                                .color(if failed {
                                    ui.visuals().error_fg_color
                                } else {
                                    ui.visuals().hyperlink_color
                                }),
                        );
                        let report = results.report(&self.model.remembered.definitions);
                        copy_button(ui, "Copy", "the whole report", || report);
                    });
                    for outcome in &results.outcomes {
                        ui.label(
                            RichText::new(format!(
                                "  {} {} on {} ({} ms){}",
                                if outcome.passed() { "PASS" } else { "FAIL" },
                                outcome.test,
                                outcome.device,
                                outcome.took,
                                outcome
                                    .failure
                                    .as_deref()
                                    .map(|why| format!(" — {why}"))
                                    .unwrap_or_default()
                            ))
                            .small()
                            .monospace()
                            .color(if outcome.passed() {
                                ui.visuals().weak_text_color()
                            } else {
                                ui.visuals().error_fg_color
                            }),
                        );
                    }
                }

                ui.separator();
                if self.model.remembered.suites.is_empty() {
                    ui.label(RichText::new("No suites yet.").weak());
                    ui.label(
                        RichText::new(
                            "Create one, then add a recorded macro to it as a test — the \
                             macro supplies the steps, the suite adds the expectations.",
                        )
                        .small()
                        .weak(),
                    );
                    return;
                }

                let open_devices: Vec<String> = self
                    .model
                    .sessions
                    .iter()
                    .map(|session| session.id.clone())
                    .collect();
                let macros = self.model.remembered.macros.clone();

                egui::ScrollArea::vertical()
                    .id_salt("suites-scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for (index, suite) in
                            self.model.remembered.suites.clone().iter().enumerate()
                        {
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .add_enabled(!running, egui::Button::new("Run"))
                                    .on_disabled_hover_text("Something is already running")
                                    .clicked()
                                {
                                    run = Some(suite.clone());
                                }
                                ui.label(RichText::new(&suite.name).strong());
                                ui.label(
                                    RichText::new(format!("{} tests", suite.tests.len()))
                                        .small()
                                        .weak(),
                                );
                                if ui.small_button("Delete").clicked() {
                                    remove = Some(index);
                                }
                            });

                            // Targets: which devices this runs against. Named,
                            // so the suite survives the tab order changing.
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("targets").small().weak());
                                if open_devices.is_empty() {
                                    ui.label(
                                        RichText::new("connect a device to choose").small().weak(),
                                    );
                                }
                                for id in &open_devices {
                                    let chosen = suite.targets.contains(id);
                                    let name = self.model.devices.get(id).map_or_else(
                                        || id.clone(),
                                        |row| self.model.display_name(row).to_owned(),
                                    );
                                    if ui.selectable_label(chosen, name).clicked() {
                                        toggle_target = Some((index, id.clone()));
                                    }
                                }
                                if suite.targets.is_empty() {
                                    ui.label(
                                        RichText::new("(none chosen — runs on all open)")
                                            .small()
                                            .weak(),
                                    );
                                }
                            });

                            for test in &suite.tests {
                                ui.label(
                                    RichText::new(format!(
                                        "  {} — {} operations",
                                        test.name,
                                        test.operations.len()
                                    ))
                                    .small(),
                                );
                            }

                            // A macro becomes a test: the steps come across and
                            // expectations are added to it afterwards.
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("add test from macro").small().weak());
                                for recorded in &macros {
                                    if ui.small_button(&recorded.name).clicked() {
                                        promote = Some((
                                            index,
                                            recorded.name.clone(),
                                            recorded
                                                .steps
                                                .iter()
                                                .cloned()
                                                .map(crate::suites::Operation::Do)
                                                .collect(),
                                        ));
                                    }
                                }
                                if macros.is_empty() {
                                    ui.label(
                                        RichText::new("record one in Macros first").small().weak(),
                                    );
                                }
                            });
                            ui.separator();
                        }
                    });
            });

        self.show_suites = open;
        if let Some(index) = remove {
            self.model.remembered.suites.remove(index);
            changed = true;
        }
        if let Some((index, id)) = toggle_target {
            if let Some(suite) = self.model.remembered.suites.get_mut(index) {
                if let Some(at) = suite.targets.iter().position(|t| t == &id) {
                    suite.targets.remove(at);
                } else {
                    suite.targets.push(id);
                }
                changed = true;
            }
        }
        if let Some((index, name, operations)) = promote {
            if let Some(suite) = self.model.remembered.suites.get_mut(index) {
                suite.tests.retain(|t| t.name != name);
                suite.tests.push(crate::suites::Test { name, operations });
                changed = true;
            }
        }
        if let Some(suite) = run {
            self.engine.send(Command::RunSuite(suite));
        }
        if changed {
            self.remember();
        }
    }

    /// Recorded sequences, and the recorder.
    ///
    /// Bringing a device up is almost never one write — it is unlock, then
    /// configure, then wait, then read back. Doing that by hand for the
    /// fortieth time while chasing a firmware bug is how the fortieth one gets
    /// typed wrong.
    fn macros_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_macros;
        let mut run: Option<(String, Vec<crate::macros::Step>, u32)> = None;
        let mut remove: Option<usize> = None;
        let mut edit: Option<(usize, u32)> = None;
        let mut stop = false;
        let mut changed = false;
        let running = self.model.macro_running;

        egui::Window::new("Macros")
            .open(&mut open)
            .resizable(true)
            .default_size([em_of(ctx) * 30.0, em_of(ctx) * 20.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // The count is read before the buttons, so toggling the
                    // recorder does not happen while it is borrowed.
                    let recorded = self.recording.as_ref().map(Vec::len);
                    match recorded {
                        None => {
                            if ui
                                .button("Record")
                                .on_hover_text(
                                    "Every read, write and subscription you make from now \
                                     on is added to the sequence.",
                                )
                                .clicked()
                            {
                                self.recording = Some(Vec::new());
                            }
                        }
                        Some(count) => {
                            if ui.button("Stop").clicked() {
                                self.recording = None;
                            }
                            ui.label(
                                RichText::new(format!(
                                    "recording — {count} step{}",
                                    if count == 1 { "" } else { "s" }
                                ))
                                .small()
                                .color(ui.visuals().warn_fg_color),
                            );
                        }
                    }

                    if let Some(steps) = self.recording.clone().filter(|s| !s.is_empty()) {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.macro_name)
                                .hint_text("name it")
                                .desired_width(em(ui) * 10.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.macro_group)
                                .hint_text("folder")
                                .desired_width(em(ui) * 7.0),
                        );
                        if ui.button("Save").clicked() && !self.macro_name.trim().is_empty() {
                            let name = self.macro_name.trim().to_owned();
                            // Re-recording under an existing name replaces it,
                            // which is what "save" means when the name is
                            // already taken.
                            self.model.remembered.macros.retain(|m| m.name != name);
                            let mut recorded = crate::macros::Macro::new(name);
                            recorded.group = self.macro_group.trim().to_owned();
                            recorded.steps = steps;
                            self.model.remembered.macros.push(recorded);
                            self.macro_name.clear();
                            self.recording = None;
                            changed = true;
                        }
                    }
                });

                if let Some(steps) = &self.recording {
                    if !steps.is_empty() {
                        ui.separator();
                        for step in steps {
                            ui.label(
                                RichText::new(format!(
                                    "  {}",
                                    step.describe(&self.model.remembered.definitions)
                                ))
                                .small()
                                .weak(),
                            );
                        }
                    }
                }

                ui.separator();
                if self.model.remembered.macros.is_empty() {
                    ui.label(RichText::new("Nothing recorded yet.").weak());
                    return;
                }

                let connected = self.model.session().is_some_and(|s| s.connected);
                egui::ScrollArea::vertical()
                    .id_salt("macros-scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        // Grouped into folders, because a flat list stops
                        // being usable at about a dozen.
                        let all = self.model.remembered.macros.clone();
                        let mut groups: Vec<&str> = all.iter().map(|m| m.group.as_str()).collect();
                        groups.sort_unstable();
                        groups.dedup();

                        for group in groups {
                            if !group.is_empty() {
                                ui.label(RichText::new(group).small().weak().strong());
                            }
                            for (index, recorded) in
                                all.iter().enumerate().filter(|(_, m)| m.group == group)
                            {
                                ui.horizontal_wrapped(|ui| {
                                    if running {
                                        if ui.button("Stop").clicked() {
                                            stop = true;
                                        }
                                    } else if ui
                                        .add_enabled(connected, egui::Button::new("Run"))
                                        .on_disabled_hover_text("Connect to a device first")
                                        .clicked()
                                    {
                                        run = Some((
                                            recorded.name.clone(),
                                            recorded.steps.clone(),
                                            recorded.repeat,
                                        ));
                                    }
                                    ui.label(RichText::new(&recorded.name).strong());
                                    ui.label(
                                        RichText::new(format!(
                                            "{} steps {}",
                                            recorded.steps.len(),
                                            recorded.repeat_label()
                                        ))
                                        .small()
                                        .weak(),
                                    );

                                    let mut repeat = recorded.repeat;
                                    let mut looping = repeat == 0;
                                    if ui
                                        .checkbox(&mut looping, "Loop")
                                        .on_hover_text("Run until stopped")
                                        .changed()
                                    {
                                        edit = Some((index, if looping { 0 } else { 1 }));
                                    }
                                    if !looping
                                        && ui
                                            .add(
                                                egui::DragValue::new(&mut repeat)
                                                    .range(1..=1000)
                                                    .prefix("×"),
                                            )
                                            .changed()
                                    {
                                        edit = Some((index, repeat));
                                    }
                                    if ui.small_button("Delete").clicked() {
                                        remove = Some(index);
                                    }
                                });
                                for step in &recorded.steps {
                                    ui.label(
                                        RichText::new(format!(
                                            "    {}",
                                            step.describe(&self.model.remembered.definitions)
                                        ))
                                        .small()
                                        .weak()
                                        .monospace(),
                                    );
                                }
                                ui.separator();
                            }
                        }
                    });
            });

        self.show_macros = open;
        if let Some(index) = remove {
            self.model.remembered.macros.remove(index);
            changed = true;
        }
        if let Some((index, repeat)) = edit {
            if let Some(recorded) = self.model.remembered.macros.get_mut(index) {
                recorded.repeat = repeat;
                changed = true;
            }
        }
        if stop {
            self.engine.send(Command::StopMacro);
        }
        if let Some((name, steps, repeat)) = run {
            // Against whichever device is in front, which is what "run" means
            // when several are open.
            if let Some(device) = self.model.active.clone() {
                self.engine.send(Command::RunMacro {
                    device,
                    name,
                    steps,
                    repeat,
                });
            }
        }
        if changed {
            self.remember();
        }
    }

    /// The active session's services, or nothing when no tab is in front.
    fn session_services(&self) -> &[crate::model::ServiceView] {
        self.model
            .session()
            .map(|session| session.services.as_slice())
            .unwrap_or_default()
    }

    /// The active session's selected service index.
    fn selected_service(&self) -> Option<usize> {
        self.model.session()?.selected_service
    }

    /// The active session's selected characteristic index.
    fn selected_characteristic(&self) -> Option<usize> {
        self.model.session()?.selected_characteristic
    }

    /// Move the selection in the active session.
    fn select(&mut self, service: Option<usize>, characteristic: Option<usize>) {
        if let Some(session) = self.model.session_mut() {
            session.selected_service = service;
            session.selected_characteristic = characteristic;
        }
    }

    /// Note an operation, if the recorder is running.
    fn record(&mut self, step: crate::macros::Step) {
        if let Some(steps) = &mut self.recording {
            // A delay is inserted between operations automatically, because a
            // sequence replayed at full speed almost never works against
            // hardware that was keeping up with a human.
            if !steps.is_empty() {
                steps.push(crate::macros::Step::Delay(RECORDED_GAP));
            }
            steps.push(step);
        }
    }

    /// The other half of the radio: publishing services of your own.
    ///
    /// Everything else in this window is the central role — reading somebody
    /// else's device. This publishes a GATT server and advertises it, which is
    /// how you exercise the other side of whatever you are building without a
    /// second piece of hardware. nRF Connect has the same pair of tabs for the
    /// same reason.
    fn server_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_server;
        let mut publish = false;
        let mut withdraw = false;
        let mut remove_service: Option<usize> = None;
        let mut clone = false;
        let mut notify: Option<(webbluetooth::uuid::BluetoothUuid, Vec<u8>)> = None;

        egui::Window::new("Server")
            .open(&mut open)
            .resizable(true)
            .default_size([em_of(ctx) * 34.0, em_of(ctx) * 24.0])
            .show(ctx, |ui| {
                if !webbluetooth::PERIPHERAL_ROLE {
                    ui.label(
                        RichText::new(
                            "This platform has no peripheral role, so nothing can be \
                             published from here.",
                        )
                        .color(ui.visuals().warn_fg_color),
                    );
                    return;
                }

                ui.horizontal(|ui| {
                    ui.label("Advertise as");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.server.local_name)
                            .hint_text("a name centrals will show")
                            .desired_width(em(ui) * 14.0),
                    );
                });

                ui.horizontal(|ui| {
                    if self.model.server_running {
                        if ui.button("Withdraw").clicked() {
                            withdraw = true;
                        }
                        ui.label(
                            RichText::new("published and advertising")
                                .small()
                                .color(ui.visuals().hyperlink_color),
                        );
                    } else {
                        let ready = !self.server.services.is_empty();
                        if ui
                            .add_enabled(ready, egui::Button::new("Publish"))
                            .on_disabled_hover_text("Add a service first")
                            .clicked()
                        {
                            publish = true;
                        }
                        if let Some(why) = &self.model.server_error {
                            ui.label(
                                RichText::new(why)
                                    .small()
                                    .color(ui.visuals().error_fg_color),
                            );
                        }
                    }
                });

                ui.separator();
                ui.label(RichText::new("TEMPLATES").small().weak().strong());
                ui.horizontal_wrapped(|ui| {
                    // Copy the tree off whatever is in front. nRF Connect calls
                    // this "Clone device's services", and it is the fastest way
                    // to stand up something a central already knows how to
                    // talk to: stand up a copy of the real thing.
                    let cloneable = self
                        .model
                        .session()
                        .is_some_and(|session| !session.services.is_empty());
                    if ui
                        .add_enabled(cloneable, egui::Button::new("Clone this device"))
                        .on_disabled_hover_text("Connect to a device and discover it first")
                        .clicked()
                    {
                        clone = true;
                    }
                    for template in crate::server_templates::ALL {
                        if ui
                            .small_button(template.label)
                            .on_hover_text(template.note)
                            .clicked()
                        {
                            self.server.services.push((template.build)());
                        }
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Add service");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_service)
                            .hint_text("180F or a full UUID")
                            .desired_width(em(ui) * 12.0),
                    );
                    if ui.small_button("Add").clicked() {
                        match parse_uuid(&self.new_service) {
                            Ok(uuid) => {
                                self.server.services.push(crate::engine::ServiceSetup {
                                    uuid,
                                    advertise: true,
                                    characteristics: Vec::new(),
                                });
                                self.new_service.clear();
                            }
                            Err(why) => self
                                .model
                                .apply(crate::engine::Event::Log(Level::Warn, why)),
                        }
                    }
                });

                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("server-scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        if self.server.services.is_empty() {
                            ui.label(
                                RichText::new(
                                    "Nothing to publish yet. A template is the quickest \
                                     start.",
                                )
                                .weak(),
                            );
                        }
                        for si in 0..self.server.services.len() {
                            let uuid = self.server.services[si].uuid;
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(names::label_with(
                                        &self.model.remembered.definitions,
                                        &uuid,
                                        Kind::Service,
                                    ))
                                    .strong(),
                                );
                                ui.label(
                                    RichText::new(names::short(&uuid))
                                        .small()
                                        .weak()
                                        .monospace(),
                                );
                                ui.checkbox(&mut self.server.services[si].advertise, "advertise")
                                    .on_hover_text(
                                        "Name it in the advertising packet, so a central \
                                     filtering for it finds this device.",
                                    );
                                if ui.small_button("Remove").clicked() {
                                    remove_service = Some(si);
                                }
                            });

                            for ci in 0..self.server.services[si].characteristics.len() {
                                let characteristic =
                                    self.server.services[si].characteristics[ci].clone();
                                ui.horizontal(|ui| {
                                    ui.add_space(em(ui));
                                    ui.label(names::label_with(
                                        &self.model.remembered.definitions,
                                        &characteristic.uuid,
                                        Kind::Characteristic,
                                    ));
                                    ui.label(
                                        RichText::new(property_summary(characteristic.properties))
                                            .small()
                                            .weak(),
                                    );
                                    let mut hex = format::hex(&characteristic.value);
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut hex)
                                                .desired_width(em(ui) * 8.0)
                                                .font(egui::TextStyle::Monospace),
                                        )
                                        .changed()
                                    {
                                        if let Ok(value) = format::parse_hex(&hex) {
                                            self.server.services[si].characteristics[ci].value =
                                                value;
                                        }
                                    }
                                    let can_notify = characteristic.properties.notify()
                                        || characteristic.properties.indicate();
                                    if can_notify
                                        && self.model.server_running
                                        && ui
                                            .small_button("Notify")
                                            .on_hover_text(
                                                "Send this value to whoever is subscribed",
                                            )
                                            .clicked()
                                    {
                                        notify = Some((
                                            characteristic.uuid,
                                            self.server.services[si].characteristics[ci]
                                                .value
                                                .clone(),
                                        ));
                                    }
                                });
                            }

                            ui.horizontal(|ui| {
                                ui.add_space(em(ui));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_characteristic)
                                        .hint_text("characteristic UUID")
                                        .desired_width(em(ui) * 10.0),
                                );
                                if ui.small_button("Add here").clicked() {
                                    match parse_uuid(&self.new_characteristic) {
                                        Ok(uuid) => {
                                            self.server.services[si].characteristics.push(
                                                crate::engine::CharacteristicSetup {
                                                    uuid,
                                                    properties: CharacteristicProperties(
                                                        CharacteristicProperties::READ
                                                            | CharacteristicProperties::WRITE
                                                            | CharacteristicProperties::NOTIFY,
                                                    ),
                                                    value: Vec::new(),
                                                },
                                            );
                                            self.new_characteristic.clear();
                                        }
                                        Err(why) => self
                                            .model
                                            .apply(crate::engine::Event::Log(Level::Warn, why)),
                                    }
                                }
                            });
                            ui.separator();
                        }
                    });
            });

        self.show_server = open;
        if clone {
            if let Some(session) = self.model.session() {
                for service in &session.services {
                    self.server.services.push(crate::engine::ServiceSetup {
                        uuid: service.uuid,
                        advertise: false,
                        characteristics: service
                            .characteristics
                            .iter()
                            .map(|view| crate::engine::CharacteristicSetup {
                                uuid: view.info.uuid,
                                properties: view.info.properties,
                                // Whatever was read from the real device, so a
                                // clone answers the same way it did.
                                value: view.value.clone().unwrap_or_default(),
                            })
                            .collect(),
                    });
                }
            }
        }
        if let Some(index) = remove_service {
            self.server.services.remove(index);
        }
        if publish {
            self.engine.send(Command::StartServer(self.server.clone()));
        }
        if withdraw {
            self.engine.send(Command::StopServer);
        }
        if let Some((characteristic, value)) = notify {
            self.engine.send(Command::NotifySubscribers {
                characteristic,
                value,
            });
        }
    }

    /// When each device was present, rather than how strong it was.
    ///
    /// A separate view from the signal graph, and a separate question: the
    /// graph answers "how close is it", this answers "was it there, and when
    /// did it stop". A device that sleeps, or one that walks out of the
    /// building, shows up here as a gap and on the graph as nothing at all.
    fn timeline_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_timeline;
        egui::Window::new("Timeline")
            .open(&mut open)
            .resizable(true)
            .default_size([em_of(ctx) * 34.0, em_of(ctx) * 16.0])
            .show(ctx, |ui| {
                let rows: Vec<_> = self
                    .model
                    .visible_devices()
                    .into_iter()
                    .filter(|row| !row.history.is_empty())
                    .cloned()
                    .collect();
                if rows.is_empty() {
                    ui.label(RichText::new("Nothing heard yet.").weak());
                    return;
                }

                // The window is however far back the oldest kept sample goes.
                let span = rows
                    .iter()
                    .filter_map(|row| row.history.front())
                    .map(|(at, _)| at.elapsed().as_secs_f32())
                    .fold(1.0_f32, f32::max);
                ui.label(
                    RichText::new(format!(
                        "last {} — a mark is an advertisement, a gap is silence",
                        ago(Duration::from_secs_f32(span))
                    ))
                    .small()
                    .weak(),
                );
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_salt("timeline-scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let em = em(ui);
                        for (index, row) in rows.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [em * 9.0, em],
                                    egui::Label::new(
                                        RichText::new(self.model.display_name(row)).small(),
                                    )
                                    .truncate(),
                                );

                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), em * 0.8),
                                    egui::Sense::hover(),
                                );
                                let painter = ui.painter_at(rect);
                                painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
                                let colour = series_colour(index);
                                for (at, _) in &row.history {
                                    let age = at.elapsed().as_secs_f32();
                                    let x =
                                        rect.right() - rect.width() * (age / span).clamp(0.0, 1.0);
                                    // One thin mark per advertisement: the
                                    // density *is* the information, so they are
                                    // deliberately not joined into a bar.
                                    painter.line_segment(
                                        [
                                            egui::pos2(x, rect.top() + 1.0),
                                            egui::pos2(x, rect.bottom() - 1.0),
                                        ],
                                        egui::Stroke::new(1.0, colour),
                                    );
                                }
                            })
                            .response
                            .on_hover_text(format!(
                                "{}\n{} sightings, last {} ago",
                                row.id,
                                row.sightings,
                                ago(row.last_seen.elapsed())
                            ));
                        }
                    });
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            });
        self.show_timeline = open;
    }

    /// The scan filters that do not fit in the toolbar.
    ///
    /// nRF Connect puts these behind a filter menu for the same reason: in a
    /// building full of radios the useful question is rarely "what is called
    /// X", it is "what is advertising this service", "what is from this
    /// company", or "everything except that lot".
    fn filters_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_filters;
        let mut changed = false;
        egui::Window::new("Filters")
            .open(&mut open)
            .resizable(true)
            .default_width(em_of(ctx) * 22.0)
            .show(ctx, |ui| {
                egui::Grid::new("filters")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        let mut row = |ui: &mut egui::Ui,
                                       label: &str,
                                       hint: &str,
                                       value: &mut String,
                                       help: &str| {
                            ui.label(label).on_hover_text(help);
                            changed |= ui
                                .add(
                                    egui::TextEdit::singleline(value)
                                        .hint_text(hint)
                                        .desired_width(em(ui) * 14.0),
                                )
                                .changed();
                            ui.end_row();
                        };
                        row(
                            ui,
                            "Name or id",
                            "thermostat",
                            &mut self.model.filter,
                            "Matches the name shown, so a device you renamed matches its new name.",
                        );
                        row(
                            ui,
                            "Exclude",
                            "beacon, printer",
                            &mut self.model.exclude,
                            "Comma separated. Anything matching any term is hidden.",
                        );
                        row(
                            ui,
                            "Service",
                            "180f or battery",
                            &mut self.model.service_filter,
                            "Matches an advertised service UUID, or the name it is shown under \
                             — including names you gave it.",
                        );
                        row(
                            ui,
                            "Company",
                            "0x004C or 76",
                            &mut self.model.company_filter,
                            "Manufacturer-data company identifier. A value that does not parse \
                             matches nothing, rather than quietly matching everything.",
                        );
                        row(
                            ui,
                            "Data contains",
                            "02 15",
                            &mut self.model.data_filter,
                            "Hex, matched against manufacturer data, service data, and the raw \
                             packet where the platform provides one.",
                        );
                    });

                ui.separator();
                changed |= ui
                    .checkbox(&mut self.model.named_only, "Named devices only")
                    .changed();
                changed |= ui
                    .checkbox(&mut self.model.connectable_only, "Connectable only")
                    .on_hover_text("A device that did not say either way stays visible.")
                    .changed();
                changed |= ui
                    .checkbox(&mut self.model.favourites_first, "Favourites first")
                    .changed();

                ui.horizontal(|ui| {
                    let mut floor = self.model.rssi_floor.unwrap_or(-100);
                    let mut limited = self.model.rssi_floor.is_some();
                    if ui.checkbox(&mut limited, "Signal floor").changed() {
                        self.model.rssi_floor = limited.then_some(floor);
                        changed = true;
                    }
                    if limited
                        && ui
                            .add(
                                egui::DragValue::new(&mut floor)
                                    .range(-100..=-30)
                                    .suffix(" dBm"),
                            )
                            .changed()
                    {
                        self.model.rssi_floor = Some(floor);
                        changed = true;
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Clear all").clicked() {
                        self.model.filter.clear();
                        self.model.exclude.clear();
                        self.model.service_filter.clear();
                        self.model.company_filter.clear();
                        self.model.data_filter.clear();
                        self.model.rssi_floor = None;
                        self.model.named_only = false;
                        self.model.connectable_only = false;
                        changed = true;
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} of {} devices shown",
                            self.model.visible_devices().len(),
                            self.model.devices.len()
                        ))
                        .small()
                        .weak(),
                    );
                });
            });
        self.show_filters = open;
        if changed {
            self.remember();
        }
    }

    /// Names you have given to UUIDs the SIG never did.
    fn definitions_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_definitions;
        let mut remove: Option<(Kind, u128)> = None;
        let mut changed = false;
        egui::Window::new("UUID names")
            .open(&mut open)
            .resizable(true)
            .default_width(em_of(ctx) * 30.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "The Bluetooth SIG names about four hundred attributes, and none of \
                         them are the ones a device actually publishes. Name them here and \
                         they are named everywhere.",
                    )
                    .small()
                    .weak(),
                );
                ui.separator();

                if self.model.remembered.definitions.is_empty() {
                    ui.label(RichText::new("Nothing named yet.").weak());
                    ui.label(
                        RichText::new(
                            "Right-click a service or characteristic in the tree and choose \
                             \"Name this…\".",
                        )
                        .small()
                        .weak(),
                    );
                    return;
                }

                egui::ScrollArea::vertical()
                    .id_salt("definitions-scroll")
                    .max_height(em(ui) * 20.0)
                    .show(ui, |ui| {
                        egui::Grid::new("definitions")
                            .num_columns(4)
                            .striped(true)
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                for ((kind, uuid), name) in
                                    self.model.remembered.definitions.clone()
                                {
                                    let parsed = webbluetooth::uuid::BluetoothUuid::from_u128(uuid);
                                    ui.label(
                                        RichText::new(match kind {
                                            Kind::Service => "service",
                                            Kind::Characteristic => "characteristic",
                                            Kind::Descriptor => "descriptor",
                                        })
                                        .small()
                                        .weak(),
                                    );
                                    selectable(
                                        ui,
                                        RichText::new(names::short(&parsed)).monospace().small(),
                                    );
                                    let mut edited = name.clone();
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut edited)
                                                .desired_width(em(ui) * 12.0),
                                        )
                                        .changed()
                                    {
                                        self.model
                                            .remembered
                                            .definitions
                                            .insert((kind, uuid), edited);
                                        changed = true;
                                    }
                                    if ui.small_button("Remove").clicked() {
                                        remove = Some((kind, uuid));
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
        self.show_definitions = open;
        if let Some(key) = remove {
            self.model.remembered.definitions.remove(&key);
            changed = true;
        }
        if changed {
            self.remember();
        }
    }

    /// Naming a device, or a UUID, in a small modal of its own.
    fn rename_window(&mut self, ctx: &egui::Context) {
        if let Some((id, mut text)) = self.renaming.take() {
            let mut open = true;
            let mut done = false;
            egui::Window::new("Rename device")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    let own = self
                        .model
                        .devices
                        .get(&id)
                        .map(|row| row.label().to_owned())
                        .unwrap_or_else(|| id.clone());
                    ui.label(
                        RichText::new(format!("It calls itself {own}"))
                            .small()
                            .weak(),
                    );
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text("the one on my desk")
                            .desired_width(em(ui) * 16.0),
                    );
                    field.request_focus();
                    ui.horizontal(|ui| {
                        let entered =
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Rename").clicked() || entered {
                            if text.trim().is_empty() {
                                self.model.remembered.renames.remove(&id);
                            } else {
                                self.model
                                    .remembered
                                    .renames
                                    .insert(id.clone(), text.trim().to_owned());
                            }
                            self.remember();
                            done = true;
                        }
                        if ui.button("Use its own name").clicked() {
                            self.model.remembered.renames.remove(&id);
                            self.remember();
                            done = true;
                        }
                    });
                });
            if open && !done {
                self.renaming = Some((id, text));
            }
        }

        if let Some((uuid, kind, mut text)) = self.defining.take() {
            let mut open = true;
            let mut done = false;
            egui::Window::new("Name this UUID")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    selectable(ui, RichText::new(uuid.as_str()).monospace().small().weak());
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text("what it actually is")
                            .desired_width(em(ui) * 18.0),
                    );
                    field.request_focus();
                    ui.horizontal(|ui| {
                        let entered =
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Name it").clicked() || entered {
                            let key = (kind, uuid.as_u128());
                            if text.trim().is_empty() {
                                self.model.remembered.definitions.remove(&key);
                            } else {
                                self.model
                                    .remembered
                                    .definitions
                                    .insert(key, text.trim().to_owned());
                            }
                            self.remember();
                            done = true;
                        }
                        if ui.button("Forget the name").clicked() {
                            self.model
                                .remembered
                                .definitions
                                .remove(&(kind, uuid.as_u128()));
                            self.remember();
                            done = true;
                        }
                    });
                });
            if open && !done {
                self.defining = Some((uuid, kind, text));
            }
        }
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui, compact: bool) {
        ui.menu_button(icon::SETTINGS, |ui| {
            ui.set_min_width(190.0);

            // On a narrow window the toolbar has no room for these, so they
            // live here instead. Same buttons, one line of chrome rather than
            // two.
            if compact {
                ui.label(RichText::new("WINDOWS").small().weak().strong());
                self.window_buttons(ui);
                ui.separator();
            }

            ui.label(RichText::new("THEME").small().weak().strong());
            for (preference, label) in [
                (egui::ThemePreference::System, "System"),
                (egui::ThemePreference::Light, "Light"),
                (egui::ThemePreference::Dark, "Dark"),
            ] {
                if ui
                    .selectable_label(self.theme == preference, label)
                    .clicked()
                {
                    self.theme = preference;
                    // egui resolves `System` against what the window server
                    // reports, and follows it if the desktop changes later.
                    ui.ctx().set_theme(preference);
                }
            }

            ui.separator();
            ui.label(RichText::new("SIZE").small().weak().strong());
            ui.horizontal(|ui| {
                let zoom = ui.ctx().zoom_factor();
                if ui.small_button(icon::SMALLER).clicked() {
                    ui.ctx().set_zoom_factor((zoom - 0.1).max(0.5));
                }
                ui.label(format!("{:.0}%", zoom * 100.0));
                if ui.small_button(icon::LARGER).clicked() {
                    ui.ctx().set_zoom_factor((zoom + 0.1).min(3.0));
                }
                if ui.small_button("Reset").clicked() {
                    ui.ctx().set_zoom_factor(1.0);
                }
            });

            ui.separator();
            if ui.button("UUID names…").clicked() {
                self.show_definitions = true;
                ui.close();
            }

            ui.separator();
            ui.label(RichText::new("VIEW").small().weak().strong());
            ui.checkbox(&mut self.show_devices, "Device list")
                .on_hover_text(
                    "Put the list away to give the whole window to one device. \
                     Scanning carries on either way.",
                );
            ui.checkbox(&mut self.show_log, "Log pane");
            ui.checkbox(&mut self.flash_updates, "Flash on update")
                .on_hover_text(
                    "Highlight a value, a characteristic row or a device row for \
                     a moment when it changes. A device in range advertises \
                     several times a second, so its row flashes that often.",
                );
            ui.checkbox(&mut self.autoconnect, "Autoconnect to favourites")
                .on_hover_text(
                    "Connect to the strongest favourite in range whenever nothing else is \
                     connected.",
                );
            ui.checkbox(&mut self.track_rssi, "Track signal while connected")
                .on_hover_text("Read RSSI once a second. This is traffic on the link.");
        })
        .response
        .on_hover_text("Settings");
    }

    /// `flash` if the setting is on, otherwise nothing.
    ///
    /// Returned as a closure rather than a method because the drawing code is
    /// nested inside `&mut self` closures, which cannot then call `&self`
    /// methods. Captures one `bool`, so it is `Copy` and nests freely.
    fn flasher(&self) -> impl Fn(Option<Instant>) -> f32 + Copy {
        let enabled = self.flash_updates;
        move |updated| if enabled { flash(updated) } else { 0.0 }
    }

    /// One pane at a time, with a selector, for a window too narrow to split.
    ///
    /// The same panes as the wide layout — a different arrangement of the same
    /// window rather than a reduced version of it, so nothing is unreachable on
    /// a phone that is reachable on a laptop.
    fn compact_panes(&mut self, ui: &mut egui::Ui) {
        // With nothing connected there is only one pane worth showing, and a
        // selector offering two empty ones is noise.
        let connected = self.model.session().is_some();
        if !connected {
            self.pane = Pane::Devices;
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if connected {
                ui.horizontal_wrapped(|ui| {
                    for (pane, label) in [
                        (Pane::Devices, "Devices"),
                        (Pane::Gatt, "GATT"),
                        (Pane::Detail, "Detail"),
                    ] {
                        if ui.selectable_label(self.pane == pane, label).clicked() {
                            self.pane = pane;
                        }
                    }
                });
                ui.separator();
                self.device_tabs(ui, true);
            }

            match self.pane {
                Pane::Devices => {
                    let before = self.model.active.clone();
                    self.device_list(ui);
                    // Picking a device is the point of this pane; having done
                    // it, move on rather than making somebody find the
                    // selector.
                    if self.model.active != before && self.model.active.is_some() {
                        self.pane = Pane::Gatt;
                    }
                }
                Pane::Gatt => {
                    let before = self.selected_characteristic();
                    self.gatt_tree(ui);
                    if self.selected_characteristic() != before {
                        self.pane = Pane::Detail;
                    }
                }
                Pane::Detail => {
                    self.device_header(ui);
                    ui.separator();
                    self.detail(ui);
                }
            }
        });
    }

    fn device_pane(&mut self, ui: &mut egui::Ui) {
        let em = em(ui);
        // A share of the window rather than a fixed width. `max_size` is
        // applied every frame, so shrinking the window shrinks the panel
        // instead of squeezing the detail pane down to nothing — which at the
        // smallest size this program allows left it about a hundred and
        // seventy points wide, with the UUID and the Write button cut off.
        let share = ui.available_width();
        egui::Panel::left("devices")
            .resizable(true)
            .default_size((share * 0.24).clamp(em * 11.0, em * 18.0))
            .min_size(em * PANEL_MIN)
            .max_size((share - em * KEEP_FOR_NEIGHBOUR).max(em * PANEL_MIN))
            .show(ui, |ui| self.device_list(ui));
    }

    /// The device list itself, without the panel around it.
    ///
    /// Separate because the compact layout shows the same list filling the
    /// window instead of down one side.
    fn device_list(&mut self, ui: &mut egui::Ui) {
        let flash_for = self.flasher();
        {
            {
                ui.add_space(4.0);
                let open: Vec<String> = self
                    .model
                    .sessions
                    .iter()
                    .map(|session| session.id.clone())
                    .collect();
                let favourites = self.model.remembered.favourites.clone();
                let mut connect_to = None;
                let mut favourite_toggle = None;
                let mut rename = None;

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        // Collected because the click handler below mutates the
                        // model, which cannot happen while it is borrowed.
                        let rows: Vec<_> =
                            self.model.visible_devices().into_iter().cloned().collect();
                        let shown: std::collections::HashMap<String, String> = rows
                            .iter()
                            .map(|row| (row.id.clone(), self.model.display_name(row).to_owned()))
                            .collect();
                        let hovers: std::collections::HashMap<String, String> = rows
                            .iter()
                            .map(|row| {
                                let name = shown.get(&row.id).map_or("", String::as_str);
                                (
                                    row.id.clone(),
                                    hover_text(&self.model.remembered.definitions, name, row),
                                )
                            })
                            .collect();

                        if rows.is_empty() {
                            ui.add_space(12.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(if self.model.scanning {
                                        "listening…"
                                    } else {
                                        "not scanning"
                                    })
                                    .weak(),
                                );
                            });
                        }

                        for row in rows {
                            let is_connected = open.contains(&row.id);
                            // Computed once per row: the closures below borrow
                            // `self` mutably, so they cannot ask for it later.
                            let shown_name = shown.get(&row.id).cloned().unwrap_or_default();
                            let hover = hovers.get(&row.id).cloned().unwrap_or_default();
                            // The labels inside a row are selectable, so
                            // they take the pointer for text selection and the
                            // row underneath never sees the click — clicking a
                            // device's name did nothing at all. Their
                            // responses are unioned into the row's instead of
                            // being thrown away, which gives both behaviours
                            // from one gesture: a drag selects the text, a
                            // click selects the device.
                            let framed = ui.push_id(&row.id, |ui| {
                                egui::Frame::default()
                                    .inner_margin(egui::Margin::symmetric(6, 4))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            signal_meter(ui, row.bars);
                                            ui.vertical(|ui| {
                                                // Wrapped: the name and the
                                                // badge are two widgets on one
                                                // line, and a line that cannot
                                                // wrap sets a floor on how
                                                // narrow the pane goes.
                                                let names = ui
                                                    .horizontal_wrapped(|ui| {
                                                        let name =
                                                            RichText::new(&shown_name).strong();
                                                        // When something
                                                        // actually changed —
                                                        // not every packet.
                                                        let fresh =
                                                            flash_for(Some(row.last_changed));
                                                        let mut hit = ui.add(
                                                            egui::Label::new(if is_connected {
                                                                name.color(
                                                                    ui.visuals().hyperlink_color,
                                                                )
                                                            } else if fresh > 0.0 {
                                                                name.color(flashed(
                                                                    ui.visuals().text_color(),
                                                                    fresh,
                                                                ))
                                                            } else {
                                                                name
                                                            })
                                                            .truncate(),
                                                        );
                                                        if is_connected {
                                                            hit |= ui.add(
                                                                egui::Label::new(
                                                                    RichText::new("connected")
                                                                        .small()
                                                                        .color(
                                                                            ui.visuals()
                                                                                .hyperlink_color,
                                                                        ),
                                                                )
                                                                .truncate(),
                                                            );
                                                        }
                                                        hit
                                                    })
                                                    .inner;
                                                names
                                                    | ui.add(
                                                        egui::Label::new(
                                                            RichText::new(subtitle(&row))
                                                                .small()
                                                                .weak(),
                                                        )
                                                        .truncate(),
                                                    )
                                                    .on_hover_text(hover.clone())
                                            })
                                            .inner
                                        })
                                        .inner
                                    })
                                    .inner
                            });
                            let response =
                                framed.response.interact(egui::Sense::click()) | framed.inner;

                            if response.hovered() {
                                ui.painter().rect_stroke(
                                    response.rect,
                                    2.0,
                                    ui.visuals().widgets.hovered.bg_stroke,
                                    egui::StrokeKind::Inside,
                                );
                            }
                            let response = response.on_hover_text(hover.clone());
                            // A tooltip cannot be selected, so everything in it
                            // is reachable from here instead.
                            response.context_menu(|ui| {
                                let starred = favourites.contains(&row.id);
                                if ui
                                    .button(if starred {
                                        "Remove from favourites"
                                    } else {
                                        "Add to favourites"
                                    })
                                    .clicked()
                                {
                                    favourite_toggle = Some(row.id.clone());
                                    ui.close();
                                }
                                if ui.button("Rename…").clicked() {
                                    rename = Some(row.id.clone());
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button("Copy identifier").clicked() {
                                    ui.ctx().copy_text(row.id.clone());
                                    ui.close();
                                }
                                if ui.button("Copy advertisement").clicked() {
                                    ui.ctx().copy_text(hover.clone());
                                    ui.close();
                                }
                                if let Some(raw) = &row.adv.raw {
                                    if ui.button("Copy raw packet").clicked() {
                                        ui.ctx().copy_text(format::hex(raw));
                                        ui.close();
                                    }
                                }
                            });
                            if response.clicked() && !is_connected {
                                connect_to = Some(row.id.clone());
                            }
                            ui.separator();
                        }
                    });

                if let Some(id) = connect_to {
                    self.engine.send(Command::Connect(id));
                }
                if let Some(id) = favourite_toggle {
                    // A set, so toggling is "remove, or add if it was not
                    // there".
                    if !self.model.remembered.favourites.remove(&id) {
                        self.model.remembered.favourites.insert(id);
                    }
                    self.remember();
                }
                if let Some(id) = rename {
                    let current = self
                        .model
                        .remembered
                        .renames
                        .get(&id)
                        .cloned()
                        .unwrap_or_default();
                    self.renaming = Some((id, current));
                }
            }
        }
    }

    fn gatt_panes(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            self.device_tabs(ui, false);
            if self.model.sessions.is_empty() {
                let why = self.model.last_disconnect.clone();
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.heading(RichText::new("No device").weak());
                    ui.label(
                        RichText::new(
                            "Pick one from the list to connect. More than one can be open \
                             at a time.",
                        )
                        .weak(),
                    );
                    if let Some(why) = why {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!("last connection ended: {why}"))
                                .small()
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                });
                return;
            }

            let connected = self
                .model
                .session()
                .map(|session| (session.id.clone(), session.connected));
            match connected {
                Some((_, true)) => self.connected_panes(ui),
                Some((id, false)) => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 3.0);
                        ui.spinner();
                        ui.label(format!("connecting to {id}…"));
                    });
                }
                None => {}
            }
        });
    }

    /// One tab per open device, as nRF Connect does it.
    ///
    /// Drawn even for a single connection, because the close button is how a
    /// tab is closed and a bar that appears only at two devices would hide it.
    fn device_tabs(&mut self, ui: &mut egui::Ui, compact: bool) {
        if self.model.sessions.is_empty() {
            return;
        }
        // Measured, this was fifteen points square — a hard target with a
        // mouse and an impossible one with a thumb, sitting eight points from
        // a tab that also takes clicks. So: a real square, bigger where the
        // window is phone-shaped, and room between it and its neighbours.
        let em = em(ui);
        let reach = em * if compact { 2.6 } else { 1.7 };
        let mut select = None;
        let mut close = None;

        ui.horizontal_wrapped(|ui| {
            let sessions: Vec<(String, String, bool)> = self
                .model
                .sessions
                .iter()
                .map(|session| {
                    let name = self.model.devices.get(&session.id).map_or_else(
                        || session.id.clone(),
                        |row| self.model.display_name(row).to_owned(),
                    );
                    (session.id.clone(), name, session.connected)
                })
                .collect();

            for (id, name, connected) in sessions {
                let active = self.model.active.as_deref() == Some(id.as_str());
                let label = if connected {
                    name.clone()
                } else {
                    format!("{name}…")
                };
                if ui
                    .selectable_label(active, label)
                    .on_hover_text(&id)
                    .clicked()
                {
                    select = Some(id.clone());
                }
                if ui
                    .add_sized([reach, reach], egui::Button::new(icon::CLOSE).frame(false))
                    .on_hover_text(format!("Disconnect {name}"))
                    .clicked()
                {
                    close = Some(id);
                }
                // Space before the next tab, so overshooting the close control
                // does not select the tab beside it.
                ui.add_space(em * 0.5);
                ui.separator();
            }
        });

        if let Some(id) = select {
            self.model.active = Some(id);
        }
        if let Some(id) = close {
            self.engine.send(Command::Disconnect(id));
        }
        ui.separator();
    }

    fn connected_panes(&mut self, ui: &mut egui::Ui) {
        self.device_header(ui);
        ui.separator();

        let em = em(ui);
        let share = ui.available_width();
        // One tree rather than a services column and a characteristics column.
        // Two columns cost about fifteen line-heights of width and showed one
        // service's characteristics at a time; a collapsing tree shows the
        // shape of the whole device and leaves that width to the detail pane,
        // which is the pane that actually ran out of room. This is how nRF
        // Connect lays the same information out.
        egui::Panel::left("gatt-tree")
            .resizable(true)
            .default_size((share * 0.30).clamp(em * 12.0, em * 22.0))
            .min_size(em * PANEL_MIN)
            .max_size((share - em * KEEP_FOR_NEIGHBOUR).max(em * PANEL_MIN))
            .show(ui, |ui| self.gatt_tree(ui));

        egui::CentralPanel::default().show(ui, |ui| self.detail(ui));
    }

    fn device_header(&mut self, ui: &mut egui::Ui) {
        let Some((id, discovered, mtu)) = self.model.session().map(|session| {
            (
                session.id.clone(),
                session.discovered,
                // One source of truth, so the header and the Link menu cannot
                // print different MTUs for the same link.
                session.link_detail.mtu,
            )
        }) else {
            return;
        };
        let name = self
            .model
            .devices
            .get(&id)
            .and_then(|row| row.name.clone())
            .unwrap_or_else(|| id.clone());
        let rssi = self.model.devices.get(&id).and_then(|row| row.rssi);

        // Wrapped, not a single row. A long device name plus four buttons
        // does not fit a narrow window, and a fixed row silently clipped
        // whatever came last — which was the Disconnect button.
        ui.horizontal_wrapped(|ui| {
            ui.heading(&name);
            selectable(ui, RichText::new(&id).small().weak().monospace());
            let services = self.session_services().to_vec();
            let services = &services;
            let definitions = &self.model.remembered.definitions;
            copy_button(ui, "Copy tree", "the whole GATT tree as text", || {
                device_report(definitions, &name, &id, mtu, services)
            });
            if let Some(rssi) = rssi {
                ui.label(RichText::new(format!("{rssi} dBm")).small().weak());
            }
            if !discovered {
                ui.spinner();
                ui.label(RichText::new("discovering…").small().weak());
            }

            ui.separator();
            self.link_menu(ui);
            if ui.button("Rediscover").clicked() {
                self.engine.send(Command::Rediscover(id.clone()));
            }
            if ui.button("Disconnect").clicked() {
                self.engine.send(Command::Disconnect(id.clone()));
            }
        });
    }

    /// Services, with their characteristics nested under them.
    ///
    /// Selection lives on the characteristic: clicking one sets both indices,
    /// so there is no way to have a service selected whose characteristic list
    /// belongs to a different service — which was possible with two columns and
    /// took a guard in the click handler to prevent.
    /// What the platform will say and do about the link itself.
    ///
    /// Taken straight from nRF Connect's per-device menu — request MTU, set
    /// PHY, connection priority, bond — because every one of those is a single
    /// call in `webbluetooth` and none of them were reachable from here.
    fn link_menu(&mut self, ui: &mut egui::Ui) {
        let Some((id, detail)) = self
            .model
            .session()
            .map(|session| (session.id.clone(), session.link_detail.clone()))
        else {
            return;
        };
        ui.menu_button("Link", |ui| {
            ui.set_min_width(em(ui) * 15.0);

            ui.label(RichText::new("STATE").small().weak().strong());
            egui::Grid::new("link-detail")
                .num_columns(2)
                .show(ui, |ui| {
                    let mut row = |label: &str, value: String| {
                        ui.label(RichText::new(label).small().weak());
                        selectable(ui, RichText::new(value).monospace().small());
                        ui.end_row();
                    };
                    row(
                        "ATT MTU",
                        detail
                            .mtu
                            .map_or_else(|| String::from("—"), |m| m.to_string()),
                    );
                    row(
                        "PHY",
                        detail.phy.map_or_else(
                            || String::from("—"),
                            |(tx, rx)| format!("{} tx / {} rx", phy_name(tx), phy_name(rx)),
                        ),
                    );
                    row(
                        "bonded",
                        match detail.paired {
                            Some(true) => "yes".into(),
                            Some(false) => "no".into(),
                            None => "—".into(),
                        },
                    );
                    // Printed in the controller's own units as well as milliseconds:
                    // a datasheet states the raw number, a human wants the time.
                    row(
                        "interval",
                        detail.parameters.map_or_else(
                            || String::from("—"),
                            |(interval, _, _)| {
                                format!("{interval} ({:.2} ms)", f32::from(interval) * 1.25)
                            },
                        ),
                    );
                    row(
                        "latency",
                        detail.parameters.map_or_else(
                            || String::from("—"),
                            |(_, latency, _)| latency.to_string(),
                        ),
                    );
                    row(
                        "timeout",
                        detail.parameters.map_or_else(
                            || String::from("—"),
                            |(_, _, timeout)| format!("{timeout} ({} ms)", u32::from(timeout) * 10),
                        ),
                    );
                });
            ui.horizontal(|ui| {
                if ui.small_button("Re-read").clicked() {
                    self.engine.send(Command::ReadLinkDetail(id.clone()));
                }
                copy_button(ui, "Copy", "the link state", || link_report(&detail));
            });

            ui.separator();
            // Learned from running this on macOS: CoreBluetooth negotiates the
            // MTU itself and has no PHY or connection-parameter API at all, so
            // three of the four sections below refuse there. Saying so beats
            // leaving somebody to work it out from the log.
            ui.label(
                RichText::new(
                    "Not every platform offers all of these. Refusals appear in the log.",
                )
                .small()
                .weak(),
            );

            ui.separator();
            ui.label(RichText::new("ATT MTU").small().weak().strong());
            ui.horizontal(|ui| {
                // 23 is the mandatory minimum, 517 the largest ATT allows.
                ui.add(egui::DragValue::new(&mut self.wanted_mtu).range(23..=517));
                if ui.small_button("Request").clicked() {
                    self.engine
                        .send(Command::RequestMtu(id.clone(), self.wanted_mtu));
                }
            });

            ui.separator();
            ui.label(RichText::new("PHY").small().weak().strong());
            for (tx, rx, label) in [
                (Phy::Le1M, Phy::Le1M, "1M — every LE device has it"),
                (Phy::Le2M, Phy::Le2M, "2M — faster, Bluetooth 5, optional"),
                (Phy::LeCoded, Phy::LeCoded, "Coded — long range, slower"),
            ] {
                if ui.button(label).clicked() {
                    self.engine.send(Command::SetPhy {
                        device: id.clone(),
                        tx,
                        rx,
                    });
                }
            }

            ui.separator();
            ui.label(RichText::new("CONNECTION PRIORITY").small().weak().strong());
            for (priority, label) in [
                (ConnectionPriority::High, "High — lower latency, more power"),
                (ConnectionPriority::Balanced, "Balanced"),
                (ConnectionPriority::LowPower, "Low power — higher latency"),
            ] {
                if ui.button(label).clicked() {
                    self.engine
                        .send(Command::SetConnectionPriority(id.clone(), priority));
                }
            }

            ui.separator();
            if ui
                .button("Pair")
                .on_hover_text(
                    "Bond with the device. The platform may show its own prompt, \
                     and encrypted characteristics are unreadable until this \
                     succeeds.",
                )
                .clicked()
            {
                self.engine.send(Command::Pair(id.clone()));
            }
        })
        .response
        .on_hover_text("MTU, PHY, bonding and connection parameters");
    }

    fn gatt_tree(&mut self, ui: &mut egui::Ui) {
        let flash_for = self.flasher();
        ui.add_space(4.0);
        // Wrapped, and the whole pane's floor depends on it: a row that cannot
        // wrap sets a minimum width for the panel that no `min_size` can go
        // below, which is what stopped this pane being narrowed at all on a
        // small window.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("GATT").small().weak().strong());
            let count: usize = self
                .session_services()
                .iter()
                .map(|s| s.characteristics.len())
                .sum();
            ui.label(
                RichText::new(format!(
                    "{} service{}, {count} characteristic{}",
                    self.session_services().len(),
                    if self.session_services().len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    if count == 1 { "" } else { "s" }
                ))
                .small()
                .weak(),
            );
        });
        ui.add_space(2.0);

        egui::ScrollArea::vertical()
            .id_salt("gatt-scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if self.session_services().is_empty() {
                    ui.label(RichText::new("nothing discovered").weak());
                    return;
                }

                let mut select: Option<(usize, usize)> = None;
                // Collected rather than applied inside the menus: those
                // closures already hold `self`.
                let mut define: Option<(webbluetooth::uuid::BluetoothUuid, Kind)> = None;

                for si in 0..self.session_services().len() {
                    let (header, service_uuid, count) = {
                        let service = &self.session_services()[si];
                        (
                            one_line(
                                ui,
                                &names::label_with(
                                    &self.model.remembered.definitions,
                                    &service.uuid,
                                    Kind::Service,
                                ),
                                &format!(
                                    "  {}{}",
                                    names::short(&service.uuid),
                                    if service.is_primary {
                                        ""
                                    } else {
                                        " · included"
                                    }
                                ),
                                None,
                            ),
                            service.uuid,
                            service.characteristics.len(),
                        )
                    };

                    // `CollapsingHeader` is deliberately not used here, even
                    // though this is exactly what it is for. It lays its title
                    // out with `TextWrapMode::Extend` — never wrapping, never
                    // truncating — so the title's full width becomes the
                    // widget's desired width, and an egui panel is as wide as
                    // its widest row that cannot shrink. One service called
                    // "Device Information" was therefore setting a floor of
                    // about 183 points on how narrow this pane could be
                    // dragged, whatever `min_size` said.
                    //
                    // Driving the collapsing state directly means the header is
                    // an ordinary `Ui`, and an ordinary `Ui` can hold a label
                    // that truncates.
                    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id(("service", si)),
                        // The first service open, the rest closed: a device
                        // with twenty services is unreadable fully expanded,
                        // and the one being looked at is usually near the top.
                        si == 0,
                    );
                    let header = state
                        .show_header(ui, |ui| ui.add(egui::Label::new(header).truncate()))
                        .body(|ui| {
                            if count == 0 {
                                ui.label(RichText::new("no characteristics").small().weak());
                                return;
                            }
                            for ci in 0..count {
                                let (title, tail, updated, notifying, uuid) = {
                                    let view = &self.session_services()[si].characteristics[ci];
                                    (
                                        names::label_with(
                                            &self.model.remembered.definitions,
                                            &view.info.uuid,
                                            Kind::Characteristic,
                                        ),
                                        format!(
                                            "  {}  {}",
                                            names::short(&view.info.uuid),
                                            property_summary(view.info.properties)
                                        ),
                                        view.updated,
                                        view.notifying,
                                        view.info.uuid,
                                    )
                                };
                                let selected = self.selected_service() == Some(si)
                                    && self.selected_characteristic() == Some(ci);
                                let amount = flash_for(updated);
                                let tint = (amount > 0.0)
                                    .then(|| flashed(ui.visuals().text_color(), amount));

                                ui.horizontal(|ui| {
                                    if notifying {
                                        ui.label(
                                            RichText::new(icon::SUBSCRIBED)
                                                .color(ui.visuals().hyperlink_color)
                                                .small(),
                                        )
                                        .on_hover_text("subscribed");
                                    }
                                    // `.truncate()` for the same reason the
                                    // service header above is a plain `Label`:
                                    // a widget that refuses to shrink sets a
                                    // floor on how narrow the whole pane can be
                                    // dragged. A characteristic row is the
                                    // widest thing in this pane.
                                    let row = ui.add(
                                        egui::Button::selectable(
                                            selected,
                                            one_line(ui, &title, &tail, tint),
                                        )
                                        .truncate(),
                                    );
                                    row.context_menu(|ui| {
                                        if ui.button("Name this…").clicked() {
                                            define = Some((uuid, Kind::Characteristic));
                                            ui.close();
                                        }
                                        if ui.button("Copy UUID").clicked() {
                                            ui.ctx().copy_text(uuid.as_str().to_owned());
                                            ui.close();
                                        }
                                    });
                                    if row.clicked() && !selected {
                                        select = Some((si, ci));
                                    }
                                });
                            }
                        });

                    header.1.response.context_menu(|ui| {
                        if ui.button("Name this service…").clicked() {
                            define = Some((service_uuid, Kind::Service));
                            ui.close();
                        }
                        if ui.button("Copy service UUID").clicked() {
                            ui.ctx().copy_text(service_uuid.as_str().to_owned());
                            ui.close();
                        }
                    });
                }

                if let Some((si, ci)) = select {
                    self.select(Some(si), Some(ci));
                    self.reset_write_field();
                }
                if let Some((uuid, kind)) = define {
                    let existing = self
                        .model
                        .remembered
                        .definitions
                        .get(&(kind, uuid.as_u128()))
                        .cloned()
                        .unwrap_or_default();
                    self.defining = Some((uuid, kind, existing));
                }
            });
    }

    fn detail(&mut self, ui: &mut egui::Ui) {
        let Some(at) = self.model.selected_ref() else {
            ui.add_space(8.0);
            ui.label(RichText::new("Select a characteristic.").weak());
            return;
        };
        let Some(view) = self.model.characteristic().cloned() else {
            return;
        };
        let mut name_this: Option<(webbluetooth::uuid::BluetoothUuid, Kind)> = None;
        let mut recall: Option<Vec<u8>> = None;

        // Both directions. A long vendor UUID, a wide hex dump or a narrowed
        // window all make this pane's content wider than the pane, and without
        // a horizontal scrollbar the right-hand end was simply not reachable.
        egui::ScrollArea::both()
            .id_salt("detail-scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading(names::label_with(
                        &self.model.remembered.definitions,
                        &view.info.uuid,
                        Kind::Characteristic,
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let snapshot = describe(&self.model.remembered.definitions, &view);
                        copy_button(
                            ui,
                            "Copy all",
                            "everything about this characteristic",
                            || snapshot,
                        );
                    });
                });
                // Wrapped: a 128-bit UUID is 36 characters of monospace and
                // does not fit a narrow detail pane beside a button.
                ui.horizontal_wrapped(|ui| {
                    selectable(
                        ui,
                        RichText::new(view.info.uuid.as_str())
                            .monospace()
                            .small()
                            .weak(),
                    );
                    let uuid = view.info.uuid;
                    copy_button(ui, "Copy", "this UUID", || uuid.as_str().to_owned());
                    if ui
                        .small_button("Name…")
                        .on_hover_text("Give this UUID a name of your own")
                        .clicked()
                    {
                        name_this = Some((uuid, Kind::Characteristic));
                    }
                });
                ui.add_space(6.0);

                self.properties_row(ui, view.info.properties);
                ui.add_space(8.0);
                self.actions_row(ui, at.clone(), &view, &mut recall);
                ui.add_space(8.0);
                ui.separator();
                self.value_block(ui, &view);

                if !view.info.descriptors.is_empty() {
                    ui.separator();
                    self.descriptor_block(ui, at.clone(), &view);
                }
            });

        if let Some((uuid, kind)) = name_this {
            let existing = self
                .model
                .remembered
                .definitions
                .get(&(kind, uuid.as_u128()))
                .cloned()
                .unwrap_or_default();
            self.defining = Some((uuid, kind, existing));
        }
        if let Some(value) = recall {
            self.write_as = WriteAs::Hex;
            self.write_text = format::hex(&value);
            self.write_error = None;
        }
    }

    fn properties_row(&self, ui: &mut egui::Ui, properties: CharacteristicProperties) {
        ui.horizontal_wrapped(|ui| {
            for (label, on) in [
                ("read", properties.read()),
                ("write", properties.write()),
                ("write no rsp", properties.write_without_response()),
                ("notify", properties.notify()),
                ("indicate", properties.indicate()),
                ("broadcast", properties.broadcast()),
                ("signed write", properties.authenticated_signed_writes()),
                ("reliable write", properties.reliable_write()),
                ("aux", properties.writable_auxiliaries()),
            ] {
                if !on {
                    continue;
                }
                ui.label(
                    RichText::new(format!(" {label} "))
                        .small()
                        .background_color(ui.visuals().faint_bg_color)
                        .color(ui.visuals().strong_text_color()),
                );
            }
            if properties.0 == 0 {
                ui.label(RichText::new("no properties advertised").small().weak());
            }
        });
    }

    fn actions_row(
        &mut self,
        ui: &mut egui::Ui,
        at: CharRef,
        view: &CharacteristicView,
        recall: &mut Option<Vec<u8>>,
    ) {
        let properties = view.info.properties;
        let uuid = view.info.uuid;
        let writable = properties.write() || properties.write_without_response();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(properties.read(), egui::Button::new("Read"))
                .on_disabled_hover_text("this characteristic has no read property")
                .clicked()
            {
                self.record(crate::macros::Step::Read(uuid));
                self.engine.send(Command::Read(at.clone()));
            }

            if properties.notify() || properties.indicate() {
                let label = if view.notifying {
                    "Unsubscribe"
                } else {
                    "Subscribe"
                };
                if ui.button(label).clicked() {
                    self.record(crate::macros::Step::Subscribe {
                        characteristic: uuid,
                        on: !view.notifying,
                    });
                    self.engine.send(Command::SetNotifying {
                        at: at.clone(),
                        on: !view.notifying,
                    });
                }
            }
        });

        if !writable {
            return;
        }

        ui.add_space(6.0);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("WRITE").small().weak().strong());
                ui.selectable_value(&mut self.write_as, WriteAs::Hex, "Hex");
                ui.selectable_value(&mut self.write_as, WriteAs::Text, "Text");

                // A characteristic can support one, the other, or both. Only
                // offer the choice where there is one, so the button cannot
                // send a write the peer will reject.
                if properties.write() && properties.write_without_response() {
                    ui.separator();
                    ui.checkbox(&mut self.write_with_response, "With response");
                } else {
                    self.write_with_response = properties.write();
                }
            });

            ui.horizontal_wrapped(|ui| {
                let hint = match self.write_as {
                    WriteAs::Hex => "01 A2 FF",
                    WriteAs::Text => "hello",
                };
                // Leave room for the button, but never ask for a negative
                // width: `available_width() - 90.0` went negative in a narrow
                // pane, and a negative desired width is what pushed the Write
                // button off the edge.
                let unit = em(ui);
                let width = (ui.available_width() - unit * 4.5).max(unit * 6.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.write_text)
                        .hint_text(hint)
                        .desired_width(width)
                        .font(egui::TextStyle::Monospace),
                );
                let submitted = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                if ui.button("Write").clicked() || submitted {
                    match self.pending_write() {
                        Ok(value) => {
                            self.write_error = None;
                            // Remembered before sending, not after: whether the
                            // peer accepted it has no bearing on whether you
                            // will want to send it again.
                            self.model
                                .remembered
                                .remember_write(uuid.as_u128(), value.clone());
                            self.dirty = true;
                            self.record(crate::macros::Step::Write {
                                characteristic: uuid,
                                value: value.clone(),
                                with_response: self.write_with_response,
                            });
                            self.engine.send(Command::Write {
                                at: at.clone(),
                                value,
                                with_response: self.write_with_response,
                            });
                        }
                        Err(why) => self.write_error = Some(why),
                    }
                }
            });

            // What the profile says this control point takes, where the
            // profile says anything. Above the history, because a value the
            // specification defines beats one you happened to send before.
            let presets = crate::presets::for_characteristic(&uuid);
            if !presets.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("KNOWN").small().weak().strong());
                    for preset in presets {
                        if ui
                            .small_button(preset.label)
                            .on_hover_text(format!(
                                "{}\n\n{}",
                                format::hex(preset.value),
                                preset.note
                            ))
                            .clicked()
                        {
                            *recall = Some(preset.value.to_vec());
                        }
                    }
                });
            }

            let history = self
                .model
                .remembered
                .writes
                .get(&uuid.as_u128())
                .cloned()
                .unwrap_or_default();
            if !history.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("RECENT").small().weak().strong());
                    for value in &history {
                        // Shown as hex however it was typed, because hex is the
                        // representation that always round-trips.
                        let label = format::hex(value);
                        let shown = if label.len() > 26 {
                            format!("{}…", &label[..24])
                        } else {
                            label.clone()
                        };
                        if ui
                            .small_button(RichText::new(shown).monospace())
                            .on_hover_text(format!(
                                "{label}\n{} byte{}\nClick to put it back in the field",
                                value.len(),
                                if value.len() == 1 { "" } else { "s" }
                            ))
                            .clicked()
                        {
                            *recall = Some(value.clone());
                        }
                    }
                });
            }

            match self.pending_write() {
                Ok(value) if !value.is_empty() => {
                    ui.label(
                        RichText::new(format!("{} bytes — {}", value.len(), format::hex(&value)))
                            .small()
                            .weak()
                            .monospace(),
                    );
                }
                Ok(_) => {}
                Err(why) => {
                    ui.label(
                        RichText::new(why)
                            .small()
                            .color(ui.visuals().error_fg_color),
                    );
                }
            }
            if let Some(why) = &self.write_error {
                ui.label(
                    RichText::new(why)
                        .small()
                        .color(ui.visuals().error_fg_color),
                );
            }
        });
    }

    fn value_block(&mut self, ui: &mut egui::Ui, view: &CharacteristicView) {
        let amount = self.flasher()(view.updated);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let heading = RichText::new("VALUE").small().weak().strong();
            ui.label(if amount > 0.0 {
                heading.color(flashed(ui.visuals().weak_text_color(), amount))
            } else {
                heading
            });
            if let Some(updated) = view.updated {
                ui.label(
                    RichText::new(format!(
                        "{} ago{}",
                        ago(updated.elapsed()),
                        if view.from_notification {
                            ", notified"
                        } else {
                            ""
                        }
                    ))
                    .small()
                    .weak(),
                );
                // A live subscription's "n ago" has to keep counting up on its
                // own, not only when the next packet arrives.
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
        });
        ui.add_space(2.0);

        if let Some((len, at)) = view.last_write {
            let wrote = self.flasher()(Some(at));
            let text = RichText::new(format!(
                "wrote {len} byte{} {} ago",
                if len == 1 { "" } else { "s" },
                ago(at.elapsed())
            ))
            .small();
            ui.label(if wrote > 0.0 {
                text.color(flashed(ui.visuals().weak_text_color(), wrote))
            } else {
                text.weak()
            });
        }

        if let Some(bytes) = &view.value {
            if !bytes.is_empty() {
                ui.horizontal(|ui| {
                    copy_button(ui, "Copy hex", "the value as hex", || format::hex(bytes));
                    copy_button(ui, "Copy text", "the value as text", || {
                        format::ascii(bytes)
                    });
                    if bytes.len() > 16 {
                        copy_button(ui, "Copy dump", "the hex dump", || format::hexdump(bytes));
                    }
                });
            }
        }

        let Some(value) = &view.value else {
            ui.label(RichText::new("nothing read yet").weak());
            return;
        };
        if value.is_empty() {
            ui.label(RichText::new("empty — zero bytes").weak());
            return;
        }
        let flash_color = (amount > 0.0).then(|| flashed(ui.visuals().text_color(), amount));

        // What the profile says these bytes mean, where anybody has said.
        // Above the generic renderings, because `87 %` is the answer and
        // `u8 87` is the raw material it was derived from.
        let decoded = crate::decode::characteristic(&view.info.uuid, value);
        if !decoded.is_empty() {
            egui::Grid::new("decoded-grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for field in &decoded {
                        ui.label(RichText::new(field.name).small().weak());
                        selectable(
                            ui,
                            RichText::new(&field.value)
                                .strong()
                                .color(match flash_color {
                                    Some(colour) => colour,
                                    None => ui.visuals().text_color(),
                                }),
                        );
                        ui.end_row();
                    }
                });
            ui.add_space(4.0);
        }

        egui::Grid::new("value-grid")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                let mut row = |label: &str, text: String| {
                    ui.label(RichText::new(label).small().weak());
                    let value = RichText::new(text).monospace();
                    // Selectable, so one reading can be dragged out without
                    // taking the whole block with it.
                    selectable(
                        ui,
                        match flash_color {
                            Some(color) => value.color(color),
                            None => value,
                        },
                    );
                    ui.end_row();
                };
                row("bytes", value.len().to_string());
                if value.len() <= 16 {
                    row("hex", format::hex(value));
                    row("ascii", format::ascii(value));
                    row("decimal", format::decimal(value));
                }
                for (label, text) in format::interpretations(value) {
                    row(label, text);
                }
            });

        if value.len() > 16 {
            ui.add_space(4.0);
            let dump = RichText::new(format::hexdump(value)).monospace().small();
            selectable(
                ui,
                if amount > 0.0 {
                    dump.color(flashed(ui.visuals().text_color(), amount))
                } else {
                    dump
                },
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    fn descriptor_block(&mut self, ui: &mut egui::Ui, at: CharRef, view: &CharacteristicView) {
        ui.add_space(6.0);
        ui.label(RichText::new("DESCRIPTORS").small().weak().strong());
        ui.add_space(2.0);
        let flash_for = self.flasher();
        let mut descriptor_write: Option<(usize, String)> = None;

        for (index, uuid) in view.info.descriptors.iter().enumerate() {
            // Wrapped: a descriptor row is a name, a UUID, a Read, however many
            // presets the profile defines, a field and a Write. That does not
            // fit a narrow detail pane on one line, and the thing that fell off
            // the end was the Write button.
            ui.horizontal_wrapped(|ui| {
                ui.label(names::label_with(
                    &self.model.remembered.definitions,
                    uuid,
                    Kind::Descriptor,
                ));
                ui.label(RichText::new(names::short(uuid)).small().weak().monospace());
                if ui.small_button("Read").clicked() {
                    self.engine
                        .send(Command::ReadDescriptor(descriptor_ref(&at, index)));
                }
                if let Some(value) = view.descriptor_values.get(&index) {
                    // What the descriptor's own encoding says, where anybody
                    // has said. A CCCD is two bytes of which two bits matter.
                    for field in crate::decode::descriptor(uuid, &value.bytes) {
                        ui.label(
                            RichText::new(format!("{}: {}", field.name, field.value))
                                .small()
                                .strong(),
                        );
                    }
                    let amount = flash_for(Some(value.at));
                    let text = RichText::new(if value.bytes.is_empty() {
                        "empty".to_owned()
                    } else {
                        format::hex(&value.bytes)
                    })
                    .monospace()
                    .small();
                    selectable(
                        ui,
                        if amount > 0.0 {
                            text.color(flashed(ui.visuals().text_color(), amount))
                        } else {
                            text
                        },
                    );
                }

                // Writing a descriptor is how a Client Characteristic
                // Configuration is set by hand, which is occasionally the only
                // way to see what a peripheral does with one.
                for preset in crate::presets::for_descriptor(uuid) {
                    if ui
                        .small_button(preset.label)
                        .on_hover_text(format!("{}\n\n{}", format::hex(preset.value), preset.note))
                        .clicked()
                    {
                        descriptor_write = Some((index, format::hex(preset.value)));
                    }
                }

                let text = self.descriptor_writes.entry(index).or_default();
                let changed = ui
                    .add(
                        egui::TextEdit::singleline(text)
                            .hint_text("hex")
                            .desired_width(80.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .changed();
                let parsed = format::parse_hex(text);
                let ready = parsed.as_ref().is_ok_and(|bytes| !bytes.is_empty());
                if ui
                    .add_enabled(ready, egui::Button::new("Write").small())
                    .clicked()
                {
                    if let Ok(value) = parsed {
                        self.engine.send(Command::WriteDescriptor {
                            at: descriptor_ref(&at, index),
                            value,
                        });
                    }
                } else if changed {
                    if let Err(why) = parsed {
                        ui.label(
                            RichText::new(why)
                                .small()
                                .color(ui.visuals().error_fg_color),
                        );
                    }
                }
            });
        }

        // Applied after the loop: the row above borrows the field map while it
        // is being drawn.
        if let Some((index, hex)) = descriptor_write {
            self.descriptor_writes.insert(index, hex);
        }
    }

    fn log_pane(&mut self, ui: &mut egui::Ui) {
        let em = em(ui);
        // Set inside the panel closure and acted on after it: writing the file
        // needs `&mut self.model` in order to log the outcome, and that is
        // borrowed for as long as the pane is being drawn.
        let mut save = false;
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(em * 9.0)
            .min_size(em * 4.0)
            .max_size(em * 26.0)
            .show(ui, |ui| {
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("LOG").small().weak().strong());
                    egui::ComboBox::from_id_salt("log-level")
                        .selected_text(self.model.log_level.name())
                        .width(em * 5.0)
                        .show_ui(ui, |ui| {
                            for level in crate::engine::Level::ALL {
                                if ui
                                    .selectable_value(
                                        &mut self.model.log_level,
                                        level,
                                        level.name(),
                                    )
                                    .clicked()
                                {
                                    self.dirty = true;
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "The quietest level shown. Debug is every read, write and \
                             notification, which a live subscription will bury everything \
                             else under.",
                        );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.model.log_filter)
                            .hint_text("filter")
                            .desired_width(em * 8.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear").clicked() {
                            self.model.log.clear();
                        }
                        if ui
                            .small_button("Save…")
                            .on_hover_text("Choose where to write the log")
                            .clicked()
                        {
                            save = true;
                        }
                        // What is copied is what is shown: copying lines a
                        // filter is hiding would be a surprise.
                        let shown: Vec<crate::model::LogLine> =
                            self.model.visible_log().cloned().collect();
                        copy_button(ui, "Copy", "the log as shown", || render_log(&shown));
                        let shown = self.model.visible_log().count();
                        let total = self.model.log.len();
                        ui.label(
                            RichText::new(if shown == total {
                                format!("{total} line{}", if total == 1 { "" } else { "s" })
                            } else {
                                // Say so, rather than letting a level set an
                                // hour ago look like nothing is happening.
                                format!("{shown} of {total}")
                            })
                            .small()
                            .weak(),
                        );
                    });
                });

                egui::ScrollArea::vertical()
                    .id_salt("log-scroll")
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in self.model.visible_log() {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.label(RichText::new(clock(line.at)).monospace().small().weak())
                                    .on_hover_text("UTC");
                                let text = RichText::new(&line.text).monospace().small();
                                selectable(
                                    ui,
                                    match line.level {
                                        Level::Debug => text.weak(),
                                        Level::Info => text,
                                        Level::Warn => text.color(ui.visuals().warn_fg_color),
                                        Level::Error => text.color(ui.visuals().error_fg_color),
                                    },
                                );
                            });
                        }
                    });
            });

        if save {
            let shown: Vec<crate::model::LogLine> = self.model.visible_log().cloned().collect();
            match save_log_interactively(&shown) {
                // Dismissed. Not an error, and not worth a line in the log.
                None => {}
                Some(Ok(path)) => self.model.apply(crate::engine::Event::Log(
                    Level::Info,
                    format!("log written to {}", path.display()),
                )),
                Some(Err(why)) => self.model.apply(crate::engine::Event::Log(
                    Level::Error,
                    format!("could not write the log: {why}"),
                )),
            }
        }
    }

    /// Whether any highlight is still fading, and so whether the window needs
    /// to keep redrawing without being prompted.
    fn anything_fading(&self) -> bool {
        self.model
            .devices
            .values()
            .any(|row| flash(Some(row.last_changed)) > 0.0)
            || self.session_services().iter().any(|service| {
                service.characteristics.iter().any(|view| {
                    flash(view.updated) > 0.0
                        || view.last_write.is_some_and(|(_, at)| flash(Some(at)) > 0.0)
                        || view
                            .descriptor_values
                            .values()
                            .any(|value| flash(Some(value.at)) > 0.0)
                })
            })
    }

    /// The bytes the write field currently describes.
    fn pending_write(&self) -> Result<Vec<u8>, String> {
        match self.write_as {
            WriteAs::Hex => format::parse_hex(&self.write_text),
            WriteAs::Text => Ok(self.write_text.as_bytes().to_vec()),
        }
    }

    /// Clear the write field when the selection moves.
    ///
    /// Carrying a value across would make it very easy to send bytes meant for
    /// one characteristic to a different one.
    fn reset_write_field(&mut self) {
        self.write_text.clear();
        self.write_error = None;
        self.descriptor_writes.clear();
    }
}

/// One line of body text, in points, from a `Context` rather than a `Ui`.
///
/// Windows are built from a `Context`, and there is no `Ui` yet at the point
/// their default size has to be decided.
fn em_of(ctx: &egui::Context) -> f32 {
    egui::TextStyle::Body
        .resolve(&ctx.style_of(egui::Theme::Dark))
        .size
        * 1.2
}

/// One line of body text, in points.
///
/// Every size in this file is a multiple of this rather than a pixel count.
/// Pixels stop being the right unit the moment the zoom factor is not 1.0 or
/// the user's font is not the default: a 24-pixel meter beside 22-point text
/// is a different thing entirely from the same meter beside 13-point text, and
/// a panel sized in pixels either clips its contents or leaves a gutter.
fn em(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Body)
}

/// One line, a name then a dimmer smaller tail — `Battery Level  0x2A19  RN`.
///
/// Same reasoning as [`two_line`]: `RichText` has one size for all of it, so a
/// name and its UUID in one label came out the same weight and the lists read
/// as undifferentiated blocks.
fn one_line(
    ui: &egui::Ui,
    name: &str,
    tail: &str,
    name_color: Option<Color32>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    job.append(
        name,
        0.0,
        egui::TextFormat {
            font_id: egui::TextStyle::Body.resolve(ui.style()),
            color: name_color.unwrap_or_else(|| ui.visuals().text_color()),
            ..Default::default()
        },
    );
    job.append(
        tail,
        0.0,
        egui::TextFormat {
            font_id: egui::TextStyle::Small.resolve(ui.style()),
            color: ui.visuals().weak_text_color(),
            ..Default::default()
        },
    );
    job
}

/// A small button that puts `text` on the clipboard.
///
/// An explorer's whole output is identifiers and bytes that have to go
/// somewhere else — a datasheet, a bug report, another tool's command line —
/// so anything worth reading here is worth copying.
///
/// Labelled with a word rather than an icon. The only glyph in egui's fonts
/// that is anywhere near "copy" is `⎘`, which renders as a curved arrow and
/// reads as "redo"; the obvious `⧉` is not in the fonts at all and comes out as
/// an empty rectangle. A button whose purpose has to be guessed at is worse
/// than a slightly wider one.
fn copy_button(ui: &mut egui::Ui, label: &str, what: &str, text: impl FnOnce() -> String) {
    if ui
        .small_button(label)
        .on_hover_text(format!("Copy {what}"))
        .clicked()
    {
        ui.ctx().copy_text(text());
    }
}

/// A label whose text can be selected and copied with the mouse.
fn selectable(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).selectable(true))
}

/// Below this width, in line heights, the window shows one pane at a time.
///
/// Three panes side by side need roughly forty line-heights before any of them
/// is readable. A phone in portrait has about half that, so the side-by-side
/// layout is not a tight fit there — it is an unusable one, and the answer is a
/// different arrangement rather than smaller panes.
const COMPACT_BELOW: f32 = 44.0;

/// Which single pane a compact window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    /// The device list.
    Devices,
    /// The GATT tree of the device in front.
    Gatt,
    /// The selected characteristic.
    Detail,
}

/// How close to a panel edge counts as grabbing its splitter, in points.
///
/// egui's default is three. Wide enough to hit without aiming, narrow enough
/// not to swallow clicks meant for the first control inside the pane.
const SPLITTER_GRAB: f32 = 8.0;

/// How much room a panel must leave for whatever is beside it, in line heights.
///
/// This is what caps a panel's width, rather than a fraction of the window.
/// Capping by fraction was the mistake: at the smallest window the GATT tree
/// could travel fifty points in total, and its minimum and its default were the
/// same number — so dragging left did nothing at all and the panel read as
/// fixed. Expressing the cap as "leave the neighbour this much" gives the full
/// remaining width to drag through and still keeps the detail pane usable.
const KEEP_FOR_NEIGHBOUR: f32 = 16.0;

/// The narrowest a panel may be dragged, in line heights.
///
/// Only half the story: an egui panel is as wide as its widest row that cannot
/// shrink, whatever `min_size` says, so the rows inside a resizable pane have
/// to be able to wrap or truncate or they hold it open by themselves. That is
/// what [`Label::truncate`](egui::Label::truncate) is doing in the device rows
/// and the tree.
const PANEL_MIN: f32 = 7.0;

/// The delay inserted between recorded steps, in milliseconds.
///
/// A sequence replayed at full speed almost never works against hardware that
/// was, while you were recording it, keeping up with a human clicking buttons.
/// Something is better than nothing here, and it is editable in the file.
const RECORDED_GAP: u64 = 150;

/// How long a value stays highlighted after it changes.
///
/// Long enough to catch out of the corner of an eye, short enough that a
/// characteristic notifying at 10 Hz reads as a steady glow rather than a
/// strobe.
const FLASH: Duration = Duration::from_millis(700);

/// The highlight colour. Deliberately the same red the weakest signal bar uses:
/// in both places it means "look here", not "something is wrong".
const FLASH_COLOR: Color32 = Color32::from_rgb(0xE0, 0x4B, 0x3F);

/// How much of a flash is left: `1.0` the instant a value changed, `0.0` once
/// [`FLASH`] has passed. `None` — never updated — is `0.0`.
fn flash(updated: Option<Instant>) -> f32 {
    let Some(at) = updated else {
        return 0.0;
    };
    let elapsed = at.elapsed().as_secs_f32();
    let span = FLASH.as_secs_f32();
    if elapsed >= span {
        return 0.0;
    }
    // Squared, so the colour drains away quickly and then lingers faintly,
    // rather than stepping off a cliff at 700ms.
    let remaining = 1.0 - elapsed / span;
    remaining * remaining
}

/// Blend towards [`FLASH_COLOR`] by `amount`, in the caller's own text colour.
///
/// Takes the base colour rather than assuming one, so this works in light and
/// dark without a second constant.
fn flashed(base: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |from: u8, to: u8| {
        (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
    };
    Color32::from_rgb(
        mix(base.r(), FLASH_COLOR.r()),
        mix(base.g(), FLASH_COLOR.g()),
        mix(base.b(), FLASH_COLOR.b()),
    )
}

/// A four-bar signal meter, painted rather than drawn with glyphs so it does not
/// depend on what the font has.
///
/// Deliberately not red at the bottom of the scale. It was, and a room with
/// several distant devices in it then painted a column of small red rectangles
/// that flipped in and out as each reading crossed a bucket boundary — which
/// reads as an error, and as flicker. Red in this window means exactly one
/// thing: a value changed just now. A weak signal is not a fault.
fn signal_meter(ui: &mut egui::Ui, bars: u32) {
    // Sized against the row it sits in, so it stays the same visual weight at
    // any zoom factor or font size.
    let unit = em(ui);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(unit * 1.5, unit * 0.85), egui::Sense::hover());
    // One colour for every lit bar, and the count carries the strength.
    //
    // It used to run green / amber / red down the scale. Measuring the rendered
    // frames showed the amber tripping a red test on its own, and the bar count
    // changing bucket often enough that the colour changed with it — so a room
    // with a distant device in it painted a small rectangle that changed colour
    // several times a second. A meter does not need a colour axis as well as a
    // length one.
    let lit = Color32::from_rgb(0x3F, 0xB9, 0x50);
    let dim = ui.visuals().weak_text_color().gamma_multiply(0.3);

    let step = rect.width() / 4.0;
    let bar_width = (step * 0.65).max(1.0);
    for i in 0..4u32 {
        let height = rect.height() * (0.3 + 0.7 * (i as f32 + 1.0) / 4.0);
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() + i as f32 * step, rect.bottom() - height),
            egui::vec2(bar_width, height),
        );
        ui.painter()
            .rect_filled(bar, 1.0, if i < bars { lit } else { dim });
    }
}

/// The second line of a device row.
fn subtitle(row: &crate::model::DeviceRow) -> String {
    let mut parts = Vec::new();
    match row.rssi {
        Some(dbm) => parts.push(format!("{dbm} dBm")),
        None => parts.push("no signal reading".to_owned()),
    }
    if row.name.is_some() {
        // The id is the only way to tell two identically named devices apart.
        parts.push(shorten(&row.id));
    }
    let services = row.adv.service_uuids.len();
    if services > 0 {
        parts.push(format!("{services} svc"));
    }
    // A beacon is the single most useful thing to say about a row, and
    // without it the frame is an unlabelled run of manufacturer bytes.
    if let Some(beacon) = beacon_of(row) {
        parts.push(beacon.kind.to_owned());
    } else if let Some((company, _)) = row.adv.manufacturer_data.first() {
        parts.push(
            names::company(*company)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("company 0x{company:04X}")),
        );
    }
    parts.join(" · ")
}

/// A rough distance from signal strength.
///
/// The usual log-distance path-loss model with an exponent of 2, which assumes
/// a clear path. It is wrong indoors — a wall costs several dB and reads as
/// several metres — so this is shown as an approximation and never as a number
/// to act on. It is still the difference between "somewhere in the building"
/// and "on this desk".
fn distance(dbm: f32, at_one_metre: i32) -> String {
    let metres = 10f32.powf((at_one_metre as f32 - dbm) / 20.0);
    if metres < 1.0 {
        format!("{:.1} m", metres)
    } else if metres < 100.0 {
        format!("{:.0} m", metres)
    } else {
        "far".to_owned()
    }
}

/// A distinct colour per graphed device.
///
/// Spread around the hue circle rather than taken from a fixed palette, so any
/// number of devices stay distinguishable, and kept away from full saturation
/// so the lines sit on either theme.
fn series_colour(index: usize) -> Color32 {
    // Golden-ratio steps: consecutive indices land far apart on the circle, so
    // the first few devices — the ones actually being watched — differ most.
    let hue = (index as f32 * 0.618_034) % 1.0;
    let (r, g, b) = hsv_to_rgb(hue, 0.65, 0.95);
    Color32::from_rgb(r, g, b)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32) % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// Ask where to put a CSV, then write it.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn save_csv_interactively(csv: &str) -> Option<std::io::Result<std::path::PathBuf>> {
    let name = log_filename(SystemTime::now()).replace(".log", "-signal.csv");
    let path = rfd::FileDialog::new()
        .set_title("Export signal readings")
        .set_file_name(name)
        .set_directory(log_directory())
        .add_filter("Comma-separated values", &["csv"])
        .save_file()?;
    Some(std::fs::write(&path, csv).map(|()| path))
}

/// As above: no save panel on mobile, so it goes where the app may write.
#[cfg(any(target_os = "ios", target_os = "android"))]
fn save_csv_interactively(csv: &str) -> Option<std::io::Result<std::path::PathBuf>> {
    let name = log_filename(SystemTime::now()).replace(".log", "-signal.csv");
    let path = log_directory().join(name);
    Some(std::fs::write(&path, csv).map(|()| path))
}

/// Read a UUID typed by hand: `180F`, `0x180F`, or one written out in full.
fn parse_uuid(text: &str) -> Result<webbluetooth::uuid::BluetoothUuid, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("no UUID given".into());
    }
    let short = text.trim_start_matches("0x").trim_start_matches("0X");
    // A bare 16-bit number is the common case by a long way, and writing out
    // the Bluetooth base UUID by hand to publish a battery service would be
    // absurd.
    if short.len() <= 4 {
        if let Ok(value) = u16::from_str_radix(short, 16) {
            return Ok(webbluetooth::uuid::BluetoothUuid::from_u16(value));
        }
    }
    webbluetooth::uuid::BluetoothUuid::parse(text)
        .map_err(|why| format!("{text:?} is not a UUID: {why}"))
}

/// A PHY, as a datasheet writes it.
fn phy_name(phy: Phy) -> &'static str {
    match phy {
        Phy::Le1M => "1M",
        Phy::Le2M => "2M",
        Phy::LeCoded => "Coded",
    }
}

/// The link state as pasteable text.
fn link_report(detail: &crate::engine::LinkDetail) -> String {
    let mut out = String::new();
    // `—` throughout for "the platform did not say", which is a different fact
    // from any particular value and has to survive being pasted somewhere.
    out.push_str(&format!(
        "ATT MTU: {}\n",
        detail.mtu.map_or_else(|| "—".to_owned(), |m| m.to_string())
    ));
    out.push_str(&format!(
        "PHY: {}\n",
        detail.phy.map_or_else(
            || "—".to_owned(),
            |(tx, rx)| format!("{} tx / {} rx", phy_name(tx), phy_name(rx))
        )
    ));
    out.push_str(&format!(
        "bonded: {}\n",
        match detail.paired {
            Some(true) => "yes",
            Some(false) => "no",
            None => "—",
        }
    ));
    match detail.parameters {
        Some((interval, latency, timeout)) => {
            out.push_str(&format!(
                "interval: {interval} ({:.2} ms)\nlatency: {latency}\ntimeout: {timeout} ({} ms)\n",
                f32::from(interval) * 1.25,
                u32::from(timeout) * 10
            ));
        }
        None => out.push_str("interval: —\nlatency: —\ntimeout: —\n"),
    }
    out
}

/// The log as one pasteable, writable block of text.
fn render_log(lines: &[crate::model::LogLine]) -> String {
    lines
        .iter()
        .map(|line| {
            // Marked rather than coloured: a saved log is read without colour,
            // and the errors are usually the reason it was saved.
            format!("{} {} {}", clock(line.at), line.level.mark(), line.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A filename for a saved log, from a wall-clock instant.
///
/// UTC and sortable, so a directory of these is in the order they were taken
/// whatever the reader's locale does with dates.
fn log_filename(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let days = secs / 86_400;
    // Days since the epoch to a civil date, by the usual era arithmetic. Small
    // enough not to justify a calendar crate, and exact.
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "webbluetooth-explorer-{year:04}{month:02}{day:02}-{:02}{:02}{:02}Z.log",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

/// Days since 1970-01-01 to a `(year, month, day)` civil date, in UTC.
///
/// Howard Hinnant's `civil_from_days`. Here so that a saved log can be named
/// after its date without taking on a calendar dependency for one filename.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Where the save panel should open.
///
/// `~/Downloads` if it exists, else the temporary directory. Only a starting
/// point — where the file actually goes is whatever the person picks.
fn log_directory() -> std::path::PathBuf {
    #[allow(deprecated)] // `home_dir` is un-deprecated as of Rust 1.85.
    let downloads = std::env::home_dir().map(|home| home.join("Downloads"));
    match downloads {
        Some(path) if path.is_dir() => path,
        _ => std::env::temp_dir(),
    }
}

/// Ask where the log should go, then write it there.
///
/// `None` means the save panel was dismissed, which is not an error and should
/// not be reported as one.
///
/// This goes through the platform's own panel rather than picking a path and
/// writing to it. On macOS that is the difference between working and not: file
/// access is mediated, and a location the *user* chose in the system panel
/// carries the permission to write there with it. A path this program invented
/// — `~/Downloads/…` — is one the operating system has every reason to refuse,
/// and refuses silently enough that it looks like the feature is broken.
///
/// On iOS the equivalent is not a save panel at all but a share sheet, which is
/// what puts a file into Files, iCloud Drive or AirDrop. This build does not
/// target iOS; see the README.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn save_log_interactively(
    lines: &[crate::model::LogLine],
) -> Option<std::io::Result<std::path::PathBuf>> {
    let path = rfd::FileDialog::new()
        .set_title("Save log")
        .set_file_name(log_filename(SystemTime::now()))
        .set_directory(log_directory())
        .add_filter("Log file", &["log"])
        .add_filter("Text file", &["txt"])
        .save_file()?;
    Some(write_log_at(&path, lines))
}

/// Mobile has no save panel, so the file goes where the app may write and the
/// path is reported.
///
/// This is not the right answer and is not pretending to be. iOS saves a file
/// through a share sheet — `UIActivityViewController`, which is what puts it
/// into Files, iCloud Drive or AirDrop — and Android through a Storage Access
/// Framework intent. Both need platform bindings this program does not have, so
/// until they exist the file lands somewhere real and reachable rather than
/// nowhere.
#[cfg(any(target_os = "ios", target_os = "android"))]
fn save_log_interactively(
    lines: &[crate::model::LogLine],
) -> Option<std::io::Result<std::path::PathBuf>> {
    Some(write_log_to(&log_directory(), lines))
}

/// Write the log to an exact path.
fn write_log_at(
    path: &std::path::Path,
    lines: &[crate::model::LogLine],
) -> std::io::Result<std::path::PathBuf> {
    let mut text = render_log(lines);
    // A trailing newline, so the file is a well-formed text file and `cat`
    // does not leave the shell prompt halfway across a line.
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(path.to_path_buf())
}

/// As [`write_log_at`], choosing the name itself inside `directory`.
///
/// What the save panel would do if it were not interactive — which is how the
/// writing gets tested without one, and how mobile saves at all.
#[cfg(any(test, target_os = "ios", target_os = "android"))]
fn write_log_to(
    directory: &std::path::Path,
    lines: &[crate::model::LogLine],
) -> std::io::Result<std::path::PathBuf> {
    write_log_at(&directory.join(log_filename(SystemTime::now())), lines)
}

/// Everything about one characteristic, as pasteable text.
///
/// The detail pane's contents in one string. What gets pasted into a datasheet
/// margin or a bug report is "this UUID, these properties, these bytes" — and
/// reassembling that by hand from four separate copies is how a transcription
/// error gets into a bug report.
fn describe(definitions: &names::Definitions, view: &CharacteristicView) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} ({})\n",
        names::label_with(definitions, &view.info.uuid, Kind::Characteristic),
        view.info.uuid.as_str()
    ));
    out.push_str(&format!(
        "properties: {}\n",
        property_names(view.info.properties)
    ));
    match &view.value {
        Some(bytes) if bytes.is_empty() => out.push_str("value: empty (zero bytes)\n"),
        Some(bytes) => {
            out.push_str(&format!("value: {} bytes\n", bytes.len()));
            out.push_str(&format!("  hex     {}\n", format::hex(bytes)));
            out.push_str(&format!("  ascii   {}\n", format::ascii(bytes)));
            out.push_str(&format!("  decimal {}\n", format::decimal(bytes)));
            for (label, text) in format::interpretations(bytes) {
                out.push_str(&format!("  {label:<7} {text}\n"));
            }
        }
        None => out.push_str("value: not read\n"),
    }
    if view.notifying {
        out.push_str("subscribed\n");
    }
    for (index, uuid) in view.info.descriptors.iter().enumerate() {
        let value = view
            .descriptor_values
            .get(&index)
            .map(|value| format::hex(&value.bytes))
            .unwrap_or_else(|| "not read".to_owned());
        out.push_str(&format!(
            "descriptor {} ({}): {value}\n",
            names::label_with(definitions, uuid, Kind::Descriptor),
            uuid.as_str()
        ));
    }
    out
}

/// The whole GATT tree of the connected device, as pasteable text.
///
/// The single most useful thing to be able to hand somebody else about a
/// device, and the one thing four panes of scrolling lists are worst at giving
/// you.
fn device_report(
    definitions: &names::Definitions,
    name: &str,
    id: &str,
    mtu: Option<u16>,
    services: &[crate::model::ServiceView],
) -> String {
    let mut out = format!("{name}\n{id}\n");
    if let Some(mtu) = mtu {
        out.push_str(&format!("ATT MTU {mtu}\n"));
    }
    for service in services {
        out.push_str(&format!(
            "\n{} ({}){}\n",
            names::label_with(definitions, &service.uuid, Kind::Service),
            service.uuid.as_str(),
            if service.is_primary {
                ""
            } else {
                " [included]"
            }
        ));
        for view in &service.characteristics {
            out.push_str(&format!(
                "  {} ({}) {}\n",
                names::label_with(definitions, &view.info.uuid, Kind::Characteristic),
                view.info.uuid.as_str(),
                property_names(view.info.properties)
            ));
            if let Some(bytes) = &view.value {
                out.push_str(&format!("      = {}\n", format::hex(bytes)));
            }
            for (index, uuid) in view.info.descriptors.iter().enumerate() {
                let value = view
                    .descriptor_values
                    .get(&index)
                    .map(|value| format!(" = {}", format::hex(&value.bytes)))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "      descriptor {} ({}){value}\n",
                    names::label_with(definitions, uuid, Kind::Descriptor),
                    uuid.as_str()
                ));
            }
        }
    }
    out
}

/// A characteristic's properties spelled out, for text that is going to be read
/// somewhere this program's `R W w N I` shorthand means nothing.
fn property_names(properties: CharacteristicProperties) -> String {
    let named: Vec<&str> = [
        ("read", properties.read()),
        ("write", properties.write()),
        (
            "write-without-response",
            properties.write_without_response(),
        ),
        ("notify", properties.notify()),
        ("indicate", properties.indicate()),
        ("broadcast", properties.broadcast()),
        ("signed-write", properties.authenticated_signed_writes()),
        ("reliable-write", properties.reliable_write()),
        ("writable-auxiliaries", properties.writable_auxiliaries()),
    ]
    .into_iter()
    .filter_map(|(label, on)| on.then_some(label))
    .collect();
    if named.is_empty() {
        return "none".to_owned();
    }
    named.join(", ")
}

/// The beacon frame a device is advertising, if it is advertising one.
///
/// Manufacturer data first, then service data, because the iBeacon and
/// AltBeacon layouts live in the former and Eddystone in the latter.
fn beacon_of(row: &crate::model::DeviceRow) -> Option<crate::beacons::Beacon> {
    row.adv
        .manufacturer_data
        .iter()
        .find_map(|(company, data)| crate::beacons::from_manufacturer_data(*company, data))
        .or_else(|| {
            row.adv
                .service_data
                .iter()
                .find_map(|(uuid, data)| crate::beacons::from_service_data(uuid.as_u16()?, data))
        })
}

/// Everything known about a device, for its tooltip.
fn hover_text(
    definitions: &names::Definitions,
    shown: &str,
    row: &crate::model::DeviceRow,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("{shown}\n{}\n", row.id));
    if shown != row.label() {
        // The name you gave it and the name it gives itself are different
        // facts; a report showing only one of them would mislead.
        out.push_str(&format!("advertises as: {}\n", row.label()));
    }
    out.push_str(&format!(
        "{} sighting{}, last {} ago\n",
        row.sightings,
        if row.sightings == 1 { "" } else { "s" },
        ago(row.last_seen.elapsed())
    ));
    if let Some(local) = &row.adv.local_name {
        out.push_str(&format!("local name: {local}\n"));
    }
    if let Some(power) = row.adv.tx_power {
        out.push_str(&format!("tx power: {power} dBm\n"));
    }
    if let Some(appearance) = row.adv.appearance {
        out.push_str(&format!("appearance: 0x{appearance:04X}\n"));
    }
    match row.adv.connectable {
        Some(true) => out.push_str("connectable\n"),
        Some(false) => out.push_str("not connectable\n"),
        None => {}
    }
    for (label, uuids) in [
        ("services", &row.adv.service_uuids),
        ("overflow", &row.adv.overflow_service_uuids),
        ("solicited", &row.adv.solicited_service_uuids),
    ] {
        if uuids.is_empty() {
            continue;
        }
        out.push_str(&format!("{label}:\n"));
        for uuid in uuids {
            let name = names::named_with(definitions, uuid, Kind::Service)
                .map(|n| format!(" ({n})"))
                .unwrap_or_default();
            out.push_str(&format!("  {}{name}\n", names::short(uuid)));
        }
    }
    if let Some(beacon) = beacon_of(row) {
        out.push_str(&format!("{}:\n", beacon.kind));
        for (name, value) in &beacon.fields {
            out.push_str(&format!("  {name}: {value}\n"));
        }
    }
    for (company, data) in &row.adv.manufacturer_data {
        let who = names::company(*company)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("0x{company:04X}"));
        out.push_str(&format!("manufacturer {who}: {}\n", format::hex(data)));
    }
    for (uuid, data) in &row.adv.service_data {
        out.push_str(&format!(
            "service data {}: {}\n",
            names::short(uuid),
            format::hex(data)
        ));
    }
    // The bytes as received, where the transport hands them over — which only a
    // raw HCI socket does — and then split into the records they are made of.
    // Everything above is derived from these, and the derivation loses the
    // types nothing has a field for.
    if let Some(raw) = &row.adv.raw {
        out.push_str(&format!("raw: {}\n", format::hex(raw)));
        for record in crate::adtypes::parse(raw) {
            out.push_str(&format!(
                "  0x{:02X} {}: {}{}\n",
                record.kind,
                record.name,
                format::hex(&record.value),
                record
                    .reading
                    .map(|reading| format!("  → {reading}"))
                    .unwrap_or_default()
            ));
        }
    }
    out.trim_end().to_owned()
}

/// A long identifier, middle-elided. CoreBluetooth's ids are full UUIDs and do
/// not fit down the side of a list.
fn shorten(id: &str) -> String {
    if id.len() <= 12 {
        return id.to_owned();
    }
    format!("{}…{}", &id[..6], &id[id.len() - 4..])
}

/// A coarse, stable duration. Never more precise than the thing being measured.
fn ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0 => format!("{}ms", elapsed.subsec_millis()),
        1..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}

/// Time of day in UTC. Local time would need a timezone database, and the log's
/// job is ordering events relative to each other.
fn clock(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
        since.subsec_millis()
    )
}

/// The properties a characteristic has, in the space a list row allows.
fn property_summary(properties: CharacteristicProperties) -> String {
    let mut parts = Vec::new();
    if properties.read() {
        parts.push("R");
    }
    if properties.write() {
        parts.push("W");
    }
    if properties.write_without_response() {
        parts.push("w");
    }
    if properties.notify() {
        parts.push("N");
    }
    if properties.indicate() {
        parts.push("I");
    }
    if parts.is_empty() {
        return "—".to_owned();
    }
    parts.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_identifier_is_elided_in_the_middle() {
        let uuid = "1A2B3C4D-5E6F-7081-9203-A4B5C6D7E8F9";
        let short = shorten(uuid);
        assert!(short.starts_with("1A2B3C"));
        assert!(short.ends_with("E8F9"));
        assert!(short.chars().count() < uuid.chars().count());
        // A Bluetooth address is short enough to print whole.
        assert_eq!(shorten("AA:BB:CC:DD"), "AA:BB:CC:DD");
    }

    #[test]
    fn durations_read_at_the_right_scale() {
        assert_eq!(ago(Duration::from_millis(250)), "250ms");
        assert_eq!(ago(Duration::from_secs(5)), "5s");
        assert_eq!(ago(Duration::from_secs(90)), "1m");
        assert_eq!(ago(Duration::from_secs(7200)), "2h");
    }

    #[test]
    fn the_clock_wraps_a_day_rather_than_counting_hours_forever() {
        // 1970-01-02T03:04:05.006Z — the day rolls over and the hour restarts.
        let at = UNIX_EPOCH + Duration::from_millis(((24 + 3) * 3600 + 4 * 60 + 5) * 1000 + 6);
        assert_eq!(clock(at), "03:04:05.006");
    }

    /// Every red rectangle egui paints of its own accord is an id clash, and an
    /// id clash is a bug in this file: two widgets sharing an id fight over one
    /// piece of interaction state, so the wrong row highlights, a collapsing
    /// header opens the wrong service, and the outline flickers in and out as
    /// the list re-sorts underneath it.
    ///
    /// Rather than reason about which ids collide, this runs the whole interface
    /// headlessly and reads egui's own verdict back out of the paint list.
    fn clash_warnings(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::epaint::Shape, found: &mut Vec<String>) {
            match shape {
                // egui marks a clash with `debug_text`, which is a galley whose
                // text begins with a flame.
                egui::epaint::Shape::Text(text) => {
                    let body = text.galley.text();
                    if body.contains('🔥') {
                        found.push(body.to_owned());
                    }
                }
                egui::epaint::Shape::Vec(inner) => {
                    for shape in inner {
                        walk(shape, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut found);
        }
        found
    }

    /// Drive the interface for a few frames and collect egui's complaints.
    ///
    /// More than one frame because the first pass of an immediate-mode UI has no
    /// stored state to collide with: collapsing headers are not open, scroll
    /// areas have no remembered offset, and hover has not happened.
    fn run_frames(app: &mut App, ctx: &egui::Context, frames: usize) -> Vec<String> {
        // A real window size. With the default (no screen rect) the panels get
        // no room, nothing is laid out, and the check passes vacuously.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1180.0, 760.0),
            )),
            ..Default::default()
        };
        let mut complaints = Vec::new();
        for _ in 0..frames {
            // `run_ui` hands over a root `Ui`, which is exactly what eframe
            // passes to `App::ui` — so this exercises the real entry point.
            let output = ctx.run_ui(input.clone(), |ui| app.draw(ui));
            complaints.extend(clash_warnings(&output.shapes));
        }
        complaints
    }

    fn populated() -> (egui::Context, App) {
        let ctx = egui::Context::default();
        // A real size: at zero the panels collapse and nothing is laid out.
        ctx.set_pixels_per_point(1.0);
        let mut app = App::detached();

        // Two devices, one of which is connected, with two services — the
        // second holding two characteristics that share a UUID, which is legal
        // and is exactly the case a UUID-keyed id would collide on.
        for id in ["AA:BB:CC", "DD:EE:FF"] {
            app.model.apply(crate::engine::Event::Sighting {
                id: id.to_owned(),
                name: Some(format!("dev {id}")),
                rssi: Some(-50),
                adv: crate::engine::AdvSummary::default(),
            });
        }
        let duplicate = webbluetooth::uuid::characteristics::BATTERY_LEVEL;
        let info = |uuid| crate::engine::CharacteristicInfo {
            uuid,
            properties: CharacteristicProperties(
                CharacteristicProperties::READ
                    | CharacteristicProperties::WRITE
                    | CharacteristicProperties::NOTIFY,
            ),
            descriptors: vec![webbluetooth::uuid::BluetoothUuid::from_u16(0x2902)],
        };
        // Two open, so the tab bar is part of every render and every clash
        // check.
        for id in ["DD:EE:FF", "AA:BB:CC"] {
            app.model
                .apply(crate::engine::Event::Connecting(id.to_owned()));
            app.model.apply(crate::engine::Event::Connected {
                id: id.to_owned(),
                mtu: Some(23),
            });
        }
        app.model.apply(crate::engine::Event::Tree {
            id: "AA:BB:CC".into(),
            services: vec![
                crate::engine::ServiceInfo {
                    uuid: webbluetooth::uuid::services::BATTERY_SERVICE,
                    is_primary: true,
                    characteristics: vec![info(duplicate)],
                },
                crate::engine::ServiceInfo {
                    uuid: webbluetooth::uuid::services::DEVICE_INFORMATION,
                    is_primary: true,
                    // The same UUID twice under one service. Real devices do it.
                    characteristics: vec![info(duplicate), info(duplicate)],
                },
            ],
        });
        app.model.apply(crate::engine::Event::Value {
            at: CharRef {
                device: "AA:BB:CC".into(),
                service: 0,
                characteristic: 0,
            },
            value: vec![0x64],
            notified: false,
        });
        app.model
            .apply(crate::engine::Event::Log(Level::Info, "a log line".into()));
        (ctx, app)
    }

    /// Render the interface to a PNG, with no window and no screen capture.
    ///
    /// `screencapture` needs a Screen Recording grant the build environment does
    /// not have, which left every claim about how this *looks* resting on
    /// reading the code. `egui_kittest` rasterises the same paint list egui
    /// would send to a window, through wgpu, headlessly.
    ///
    /// Off by default: it brings up a GPU device, and the log column contains a
    /// wall clock, so the images are not byte-identical between runs and are for
    /// looking at rather than asserting on.
    ///
    /// ```sh
    /// WEBBLUETOOTH_EXPLORER_SNAPSHOTS=1 cargo test -p webbluetooth-explorer render
    /// ```
    fn render(name: &str, size: egui::Vec2, pixels_per_point: f32, mut app: App) {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(size)
            .with_pixels_per_point(pixels_per_point)
            .wgpu()
            .build_ui(move |ui| app.draw(ui));
        // A fixed number of passes rather than `run()`, which loops until the
        // UI stops asking to be repainted and this one never does: a fading
        // highlight and a counting-up "n ago" both animate on purpose. Two
        // passes is enough for stored state — an open collapsing header, a
        // scroll offset, a panel width — to settle.
        harness.step();
        harness.step();
        if let Err(why) = harness.try_snapshot(name) {
            // A missing baseline is the normal case here; the image is written
            // either way, which is the point.
            println!("  {name}: {why}");
        }
    }

    /// Anything on screen that reads as red, as bounding boxes.
    ///
    /// "Red" here means a pixel whose red channel dominates the other two by a
    /// clear margin — which catches both a filled red rectangle and red text,
    /// and ignores the greens, ambers and greys everything else is drawn in.
    /// Neighbouring red pixels are merged into runs and runs into boxes, so the
    /// answer is "there are two red things, here and here" rather than a pixel
    /// count.
    fn red_regions(image: &image::RgbaImage) -> Vec<(u32, u32, u32, u32)> {
        let is_red = |p: &image::Rgba<u8>| {
            let [r, g, b, a] = p.0;
            a > 128
                && i32::from(r) - i32::from(g) > 45
                && i32::from(r) - i32::from(b) > 45
                && r > 90
        };

        let mut boxes: Vec<(u32, u32, u32, u32)> = Vec::new();
        for (x, y, pixel) in image.enumerate_pixels() {
            if !is_red(pixel) {
                continue;
            }
            // Merge into an existing box if this pixel touches it, allowing a
            // few pixels of gap so that the strokes of one glyph do not each
            // become their own region.
            match boxes.iter_mut().find(|(x0, y0, x1, y1)| {
                x + 3 >= *x0 && x <= *x1 + 3 && y + 3 >= *y0 && y <= *y1 + 3
            }) {
                Some((x0, y0, x1, y1)) => {
                    *x0 = (*x0).min(x);
                    *y0 = (*y0).min(y);
                    *x1 = (*x1).max(x);
                    *y1 = (*y1).max(y);
                }
                None => boxes.push((x, y, x, y)),
            }
        }
        // Ignore specks: a single stray pixel is antialiasing, not a rectangle.
        boxes.retain(|(x0, y0, x1, y1)| (x1 - x0) * (y1 - y0) > 4);
        boxes
    }

    /// The question "why do red rectangles flicker", answered by measurement.
    ///
    /// Screen recording needs a permission this environment does not have, so
    /// instead this renders the interface over a run of frames while feeding it
    /// the thing that was actually causing the flicker — a stream of noisy
    /// advertisements, as a real scan delivers — and looks at the pixels.
    ///
    /// Nothing may be red except a fading update highlight. A signal meter, a
    /// list row or an icon that comes out red is the bug.
    #[test]
    fn nothing_flickers_red_during_a_scan() {
        if std::env::var("WEBBLUETOOTH_EXPLORER_SNAPSHOTS").is_err() {
            return;
        }

        // A room like the one the radio test sees: a few devices at a range of
        // strengths, several of them weak enough to sit near a bar boundary.
        const DEVICES: [(&str, i32); 5] = [
            ("near", -45),
            ("mid", -66),
            ("boundary", -80),
            ("far", -85),
            ("distant", -95),
        ];
        let app = std::cell::RefCell::new(App::detached());
        let noise = std::cell::Cell::new(0x1234_5678_u32);

        // Advertisements arrive between frames, each a few dB off the device's
        // true strength — which is what made the meter change bucket, and the
        // list reorder, on every packet. Fed from inside the draw closure so
        // that one rendered frame is one round of packets.
        let mut frame = |ui: &mut egui::Ui| {
            let mut app = app.borrow_mut();
            for (id, base) in DEVICES {
                let next = noise
                    .get()
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                noise.set(next);
                let jitter = (next >> 16) as i32 % 9 - 4;
                app.model.apply(crate::engine::Event::Sighting {
                    id: id.to_owned(),
                    name: Some(id.to_owned()),
                    rssi: Some(base + jitter),
                    adv: crate::engine::AdvSummary::default(),
                });
            }
            app.draw(ui);
        };

        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1180.0, 760.0))
            .with_pixels_per_point(1.0)
            .wgpu()
            .build_ui(&mut frame);

        // Warm-up: let every device be discovered, then wait out its arrival
        // highlight. The fade is keyed to the wall clock, not to frames, so
        // this has to be a real wait — stepping without rendering is fast
        // enough that a dozen frames went by well inside one fade.
        for _ in 0..3 {
            harness.step();
        }
        std::thread::sleep(FLASH + Duration::from_millis(150));
        harness.step();

        // Steady state: devices known, names settled, only signal moving. The
        // list must be still.
        let mut frames = Vec::new();
        let mut offenders = Vec::new();
        for index in 0..24 {
            harness.step();
            let image = harness.render().expect("the frame rasterises");
            let red = red_regions(&image);
            if !red.is_empty() {
                offenders.push((index, red.clone()));
            }
            frames.push((index, red.len()));
        }

        let with_red: Vec<usize> = frames
            .iter()
            .filter(|(_, count)| *count > 0)
            .map(|(index, _)| *index)
            .collect();
        println!(
            "  {} of {} frames contain red; frames: {with_red:?}",
            with_red.len(),
            frames.len()
        );
        for (index, boxes) in &offenders {
            println!("  frame {index}: {boxes:?}");
        }

        assert!(
            offenders.is_empty(),
            "red appeared in {} of 24 steady-state frames — devices already \
             discovered, names settled, only the signal moving. The first \
             region of the first bad frame is at {:?}. Red is reserved for \
             something actually changing; a scan that is merely running must \
             paint none of it.",
            offenders.len(),
            offenders.first().map(|(_, boxes)| boxes[0])
        );
    }

    #[test]
    fn render_the_interface_for_inspection() {
        if std::env::var("WEBBLUETOOTH_EXPLORER_SNAPSHOTS").is_err() {
            return;
        }
        // The sizes that matter: the default window, a window narrow enough to
        // squeeze the detail pane, and a zoomed one.
        render("idle", egui::vec2(1180.0, 760.0), 1.0, App::detached());
        render("connected", egui::vec2(1180.0, 760.0), 1.0, populated().1);
        render(
            "connected-narrow",
            egui::vec2(900.0, 620.0),
            1.0,
            populated().1,
        );
        render(
            "connected-small",
            egui::vec2(760.0, 480.0),
            1.0,
            populated().1,
        );
        render(
            "connected-zoomed",
            egui::vec2(1180.0, 760.0),
            1.5,
            populated().1,
        );
        // A phone in portrait, which is a stated target and gets the
        // one-pane-at-a-time layout.
        render("phone", egui::vec2(390.0, 750.0), 1.0, populated().1);
    }

    /// The window, driven by synthetic pointer input.
    ///
    /// egui tracks the pointer across frames, so a drag is a sequence: hover,
    /// press, move, release — each its own pass.
    struct Driver {
        ctx: egui::Context,
        app: App,
        rect: egui::Rect,
    }

    impl Driver {
        fn new(app: App, size: egui::Vec2) -> Self {
            let ctx = egui::Context::default();
            Self {
                ctx,
                app,
                rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(self.rect),
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            let _ = self.ctx.run_ui(input, |ui| app.draw(ui));
        }

        fn settle(&mut self, passes: usize) {
            for _ in 0..passes {
                self.frame(Vec::new());
            }
        }

        /// The outer rect a panel rendered at last frame.
        fn panel(&self, id: &str) -> Option<egui::Rect> {
            egui::PanelState::load(&self.ctx, egui::Id::new(id)).map(|state| state.outer_rect)
        }

        /// Grab the right-hand edge of `id` and drag it `by` points sideways.
        fn drag_edge(&mut self, id: &str, by: f32) {
            let rect = self.panel(id).expect("the panel rendered");
            let from = egui::pos2(rect.right(), rect.center().y);
            // Kept inside the window: egui discards a pointer position outside
            // the screen rect, so a drag aimed past the edge simply stops at
            // the last position it saw — which looks exactly like hitting a
            // minimum that is not really there.
            let to = egui::pos2(
                (rect.right() + by).clamp(self.rect.left() + 2.0, self.rect.right() - 2.0),
                rect.center().y,
            );

            // Hover first: the handle is read from the previous frame's
            // response, so it has to have been interacted with once already.
            self.frame(vec![egui::Event::PointerMoved(from)]);
            self.frame(vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }]);
            // Move in two steps, so the drag threshold is comfortably passed.
            self.frame(vec![egui::Event::PointerMoved(egui::pos2(
                (from.x + to.x) / 2.0,
                from.y,
            ))]);
            self.frame(vec![egui::Event::PointerMoved(to)]);
            self.frame(vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }]);
            self.settle(1);
        }
    }

    /// Both side panels have to be draggable. The middle one is the one that
    /// matters most — it is the tree, and how much of the window the detail
    /// pane gets is the main thing worth adjusting.
    #[test]
    fn both_side_panels_can_be_dragged_wider() {
        for (panel, label) in [("devices", "device list"), ("gatt-tree", "GATT tree")] {
            let mut driver = Driver::new(populated().1, egui::vec2(1180.0, 760.0));
            driver.settle(3);

            let before = driver.panel(panel).expect("rendered").width();
            driver.drag_edge(panel, 70.0);
            let after = driver.panel(panel).expect("still rendered").width();

            assert!(
                after > before + 40.0,
                "the {label} panel did not resize: {before} → {after} after a \
                 70 point drag on its edge"
            );
        }
    }

    /// The splitters have to be worth dragging, and findable without aiming.
    ///
    /// Both halves of this were broken and both were measured, not guessed.
    /// Capping each panel at a *fraction* of the window left the GATT tree
    /// fifty points of travel at the smallest window, with its minimum equal to
    /// its default — so dragging left did nothing and the pane read as fixed.
    /// And egui's default grab radius is three points, which is a target you
    /// have to aim at: a grab six points off the splitter missed entirely.
    /// A pane has to reach the minimum it declares, not one some widget inside
    /// it imposes by refusing to shrink.
    ///
    /// This is the bug that made the middle pane feel broken: it would narrow
    /// smoothly under the pointer and then stop dead, well short of anything
    /// `min_size` asked for. An egui panel is as wide as its widest row that
    /// cannot shrink, and three widgets in here could not:
    ///
    /// * `CollapsingHeader` lays its title out with `TextWrapMode::Extend` —
    ///   never wrapping, never truncating — so one service called "Device
    ///   Information" set the floor for the whole pane. The tree drives
    ///   `CollapsingState` directly now, which makes the header an ordinary
    ///   `Ui` that can hold an ordinary truncating `Label`.
    /// * `selectable_label` is a `Button`, and a button sizes to its content
    ///   unless told otherwise. The rows are `Button::selectable(…).truncate()`.
    /// * The device rows' name, subtitle and "connected" badge, for the same
    ///   reason.
    ///
    /// Each of those was found by bisection against this measurement, and each
    /// looked equally plausible beforehand — which is why this is a test rather
    /// than a comment.
    #[test]
    fn a_pane_can_be_narrowed_to_the_minimum_it_declares() {
        for panel in ["devices", "gatt-tree"] {
            let mut d = Driver::new(populated().1, egui::vec2(1180.0, 760.0));
            d.settle(3);
            let em = 15.2_f32; // Body line height at the default style.
            let declared = em * PANEL_MIN;

            d.drag_edge(panel, -4000.0);
            let narrowest = d.panel(panel).expect("rendered").width();

            assert!(
                narrowest < declared * 1.35,
                "{panel} stops at {narrowest:.0} points but asks for a minimum \
                 of about {declared:.0} — something inside it refuses to shrink \
                 and is holding the pane open"
            );
        }
    }

    #[test]
    fn the_splitters_have_room_to_move_and_are_easy_to_grab() {
        for (size, least_travel) in [
            (egui::vec2(1180.0, 760.0), 300.0_f32),
            // A smaller window has less to give, but "some" is the point.
            (egui::vec2(760.0, 480.0), 100.0),
        ] {
            for panel in ["devices", "gatt-tree"] {
                let mut d = Driver::new(populated().1, size);
                d.settle(3);

                d.drag_edge(panel, 4000.0);
                let widest = d.panel(panel).unwrap().width();
                d.drag_edge(panel, -4000.0);
                let narrowest = d.panel(panel).unwrap().width();

                assert!(
                    widest - narrowest >= least_travel,
                    "{panel} at {}x{} can only travel {:.0} points \
                     ({narrowest:.0}..{widest:.0}); a panel with nowhere to go \
                     reads as one that cannot be resized at all",
                    size.x,
                    size.y,
                    widest - narrowest
                );
            }
        }
    }

    /// The splitter has to be grabbable from either side of the line, not only
    /// from exactly on it.
    #[test]
    fn the_splitter_can_be_grabbed_slightly_off_centre() {
        for offset in [-6.0_f32, -3.0, 0.0, 3.0, 6.0] {
            let mut d = Driver::new(populated().1, egui::vec2(1180.0, 760.0));
            d.settle(3);
            let rect = d.panel("gatt-tree").expect("rendered");
            let before = rect.width();

            let from = egui::pos2(rect.right() + offset, rect.center().y);
            let to = egui::pos2(from.x + 60.0, from.y);
            d.frame(vec![egui::Event::PointerMoved(from)]);
            d.frame(vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }]);
            d.frame(vec![egui::Event::PointerMoved(egui::pos2(
                from.x + 30.0,
                from.y,
            ))]);
            d.frame(vec![egui::Event::PointerMoved(to)]);
            d.frame(vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }]);
            d.settle(1);

            let after = d.panel("gatt-tree").expect("rendered").width();
            assert!(
                after > before + 20.0,
                "a grab {offset:+.0} points from the splitter did not take: \
                 {before:.0} → {after:.0}"
            );
        }
    }

    /// Closing a tab has to be possible, which is two separate claims: that
    /// the control does the thing, and that it can be hit.
    ///
    /// It measured fifteen points square, eight points from a tab that also
    /// takes clicks — fine to find in a screenshot, hard with a mouse, and
    /// hopeless with a thumb on the phone targets. "Impossible to close tabs"
    /// was an accurate description of a control that worked perfectly.
    fn tab_bar(compact: bool) -> (std::cell::RefCell<App>, egui::Vec2) {
        let size = if compact {
            egui::vec2(390.0, 750.0)
        } else {
            egui::vec2(1180.0, 760.0)
        };
        (std::cell::RefCell::new(populated().1), size)
    }

    #[test]
    fn a_tab_can_be_closed() {
        use egui_kittest::kittest::Queryable;

        let (app, size) = tab_bar(false);
        assert_eq!(app.borrow().model.sessions.len(), 2);

        let mut frame = |ui: &mut egui::Ui| app.borrow_mut().draw(ui);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(size)
            .with_pixels_per_point(1.0)
            // Fixed steps: this UI animates on purpose, so `run()` never
            // settles.
            .build_ui(&mut frame);
        harness.step();
        harness.step();

        let closers: Vec<_> = harness.get_all_by_label(icon::CLOSE).collect();
        assert_eq!(closers.len(), 2, "one close control per open tab");
        closers[1].click();
        harness.step();
        harness.step();

        let commands = app.borrow_mut().engine.drain_commands_for_test();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Disconnect(_))),
            "clicking close sent {commands:?}, not a Disconnect"
        );
    }

    /// Clicking a device's *name* has to connect to it.
    ///
    /// egui labels are selectable by default, so they take the pointer for text
    /// selection and the clickable row underneath never sees the click. The
    /// name is the obvious thing to click and was the one part of the row that
    /// did nothing — the meter and the margins worked fine, which made it look
    /// intermittent rather than broken.
    ///
    /// The fix unions the labels' responses into the row's, so one gesture
    /// still does both: a drag selects the text, a click selects the device.
    #[test]
    fn clicking_a_device_name_connects_to_it() {
        use egui_kittest::kittest::Queryable;

        let app = std::cell::RefCell::new({
            let mut app = App::detached();
            // A device seen but not connected, so clicking it has somewhere to
            // go.
            app.model.apply(crate::engine::Event::Sighting {
                id: "AA:BB:CC".to_owned(),
                name: Some("thermostat".to_owned()),
                rssi: Some(-50),
                adv: crate::engine::AdvSummary::default(),
            });
            app
        });

        let mut frame = |ui: &mut egui::Ui| app.borrow_mut().draw(ui);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1180.0, 760.0))
            .with_pixels_per_point(1.0)
            .build_ui(&mut frame);
        harness.step();
        harness.step();

        // Aim at the name itself, which is what anybody would click.
        let name = harness.get_by_label("thermostat");
        name.click();
        harness.step();
        harness.step();

        let commands = app.borrow_mut().engine.drain_commands_for_test();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Connect(id) if id == "AA:BB:CC")),
            "clicking the name sent {commands:?}, not a Connect"
        );
    }

    /// And the text is still selectable, which is the other half of "both".
    #[test]
    fn a_device_name_is_still_selectable_text() {
        let ctx = egui::Context::default();
        assert!(
            ctx.style_of(egui::Theme::Dark)
                .interaction
                .selectable_labels,
            "labels must stay selectable — the row click is unioned on top of \
             that, not bought by turning it off"
        );
    }

    #[test]
    fn a_tab_close_control_is_big_enough_to_hit() {
        use egui_kittest::kittest::Queryable;

        for compact in [false, true] {
            let (app, size) = tab_bar(compact);
            let mut frame = |ui: &mut egui::Ui| app.borrow_mut().draw(ui);
            let mut harness = egui_kittest::Harness::builder()
                .with_size(size)
                .with_pixels_per_point(1.0)
                .build_ui(&mut frame);
            harness.step();
            harness.step();

            // A mouse wants about twenty-four points; a thumb wants far more,
            // so a phone-shaped window gets a bigger one.
            let least = if compact { 36.0 } else { 22.0 };
            let closers: Vec<egui::Rect> = harness
                .query_all_by_label(icon::CLOSE)
                .map(|node| node.rect())
                .collect();
            assert!(
                !closers.is_empty(),
                "no close control at all (compact: {compact})"
            );

            for rect in &closers {
                assert!(
                    rect.width() >= least && rect.height() >= least,
                    "close control is {:.0}x{:.0}, below {least:.0} (compact: {compact})",
                    rect.width(),
                    rect.height()
                );
            }

            // And it must not abut the tab beside it, or overshooting selects
            // a different device instead of closing this one.
            let tabs: Vec<egui::Rect> = harness
                .query_all_by_label_contains("dev ")
                .map(|node| node.rect())
                .filter(|rect| rect.min.y < 200.0)
                .collect();
            for closer in &closers {
                for tab in &tabs {
                    if tab.intersects(*closer) {
                        panic!(
                            "close control at {:?} overlaps a tab at {:?}",
                            closer.min, tab.min
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_interface_paints_no_id_clashes() {
        let (ctx, mut app) = populated();
        let complaints = run_frames(&mut app, &ctx, 4);
        assert!(
            complaints.is_empty(),
            "egui reported {} id clash(es), which is what the red rectangles are:\n{}",
            complaints.len(),
            complaints.join("\n")
        );
    }

    /// The empty case has a different set of widgets in it, so it is its own
    /// chance to collide.
    #[test]
    fn an_idle_interface_paints_no_id_clashes() {
        let ctx = egui::Context::default();
        let mut app = App::detached();
        let complaints = run_frames(&mut app, &ctx, 3);
        assert!(complaints.is_empty(), "{}", complaints.join("\n"));
    }

    /// Red in this window means one thing: a value changed just now. The signal
    /// meter used to paint its weakest bar in alarm red, which put small red
    /// rectangles down a list of distant devices and made them look like faults.
    /// Red in this window means one thing: something changed just now. The
    /// meter used to run green / amber / red down the scale, which put a small
    /// rectangle beside every distant device that both read as a fault and
    /// changed colour whenever the bucket did.
    /// A glyph the font does not have is drawn as an empty rectangle. That is
    /// not a compile error and not a runtime error — it is a button that looks
    /// broken — so the whole icon set is checked against the real font here.
    ///
    /// Every obvious choice failed this when it was written: `⧉` for copy, `▶`
    /// for play, `◉` for a live subscription, `▾` for a menu, `⚠` for a
    /// warning. egui ships Ubuntu-Light and a small icon font, and none of
    /// those are in either.
    #[test]
    fn every_icon_has_a_glyph() {
        let ctx = egui::Context::default();
        // One pass, so the fonts are built before they are asked anything.
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});

        // Resolved before taking the font lock: `style_of` takes a lock of its
        // own, and asking for it from inside `fonts_mut` deadlocks.
        let style = ctx.style_of(egui::Theme::Dark);
        let fonts: Vec<(egui::TextStyle, egui::FontId)> = [
            egui::TextStyle::Body,
            egui::TextStyle::Button,
            egui::TextStyle::Small,
        ]
        .into_iter()
        .map(|text_style| {
            let font = text_style.resolve(&style);
            (text_style, font)
        })
        .collect();

        let mut missing = Vec::new();
        ctx.fonts_mut(|loaded| {
            for (name, glyphs) in icon::ALL {
                for (text_style, font) in &fonts {
                    for c in glyphs.chars() {
                        if !loaded.has_glyph(font, c) {
                            missing.push(format!("{name} ({c:?}) in {text_style:?}"));
                        }
                    }
                }
            }
        });

        assert!(
            missing.is_empty(),
            "these icons will render as empty rectangles:\n  {}",
            missing.join("\n  ")
        );
    }

    /// A distance from signal strength is an estimate and has to read like
    /// one, but it still has to be monotonic and roughly right.
    #[test]
    fn distance_falls_off_with_signal() {
        let reference = -66;
        // At the reference strength, one metre by definition.
        assert_eq!(distance(-66.0, reference), "1 m");
        // Stronger is nearer, weaker is further, always.
        let near = distance(-46.0, reference);
        let far = distance(-86.0, reference);
        assert!(near.starts_with("0."), "{near}");
        assert_eq!(far, "10 m");

        // Monotonic across the range.
        let metres = |dbm: f32| 10f32.powf((reference as f32 - dbm) / 20.0);
        let mut previous = f32::MIN;
        for step in 0..40 {
            let value = metres(-40.0 - step as f32);
            assert!(value > previous, "not monotonic at {step}");
            previous = value;
        }

        // Absurdly weak reads as "far" rather than a number nobody should act on.
        assert_eq!(distance(-200.0, reference), "far");
    }

    #[test]
    fn nothing_but_a_flash_is_painted_red() {
        assert_eq!(
            FLASH_COLOR,
            Color32::from_rgb(0xE0, 0x4B, 0x3F),
            "if this changes, check the meter is still not using it"
        );
        // The meter has exactly one colour now, and it is not red.
        let lit = Color32::from_rgb(0x3F, 0xB9, 0x50);
        assert!(i32::from(lit.g()) > i32::from(lit.r()) + 60, "{lit:?}");
    }

    /// A device parked on a bar boundary must pick an answer and keep it.
    #[test]
    fn the_bar_count_does_not_oscillate_on_a_boundary() {
        use crate::model::bars_with_hysteresis;

        // Plain buckets, coming from below.
        assert_eq!(bars_with_hysteresis(-40.0, 0), 4);
        assert_eq!(bars_with_hysteresis(-120.0, 0), 1, "never zero once heard");

        // Sitting exactly on the two-bar edge: whatever it showed, it holds.
        assert_eq!(bars_with_hysteresis(-80.0, 1), 1);
        assert_eq!(bars_with_hysteresis(-80.0, 2), 2);

        // It takes a clear margin to move either way.
        assert_eq!(bars_with_hysteresis(-79.0, 1), 1, "not yet");
        assert_eq!(bars_with_hysteresis(-77.9, 1), 2, "now");
        assert_eq!(bars_with_hysteresis(-81.0, 2), 2, "not yet");
        assert_eq!(bars_with_hysteresis(-82.1, 2), 1, "now");

        // And a value wobbling either side of an edge never changes the count.
        let mut bars = bars_with_hysteresis(-80.0, 0);
        let settled = bars;
        for step in 0..40 {
            let wobble = if step % 2 == 0 { 1.4 } else { -1.4 };
            bars = bars_with_hysteresis(-80.0 + wobble, bars);
            assert_eq!(bars, settled, "changed on step {step}");
        }
    }

    #[test]
    fn a_flash_starts_full_and_reaches_zero() {
        assert_eq!(flash(None), 0.0, "never updated is never highlighted");
        // Just updated: near 1.0, allowing for the time this test takes.
        assert!(flash(Some(Instant::now())) > 0.99);
        // Long past: nothing, rather than a faint permanent tint.
        assert_eq!(flash(Some(Instant::now() - FLASH)), 0.0);
        assert_eq!(flash(Some(Instant::now() - FLASH * 10)), 0.0);
    }

    #[test]
    fn a_flash_fades_monotonically() {
        let amounts: Vec<f32> = (0..=10)
            .map(|step| flash(Some(Instant::now() - (FLASH * step) / 10)))
            .collect();
        for pair in amounts.windows(2) {
            assert!(
                pair[0] >= pair[1],
                "the fade went back up: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        assert!(amounts[0] > amounts[amounts.len() - 1]);
    }

    /// The highlight has to work in light and dark, so it blends from whatever
    /// the current text colour is rather than from a second constant.
    #[test]
    fn a_full_flash_is_the_flash_colour_whatever_the_theme() {
        for base in [Color32::BLACK, Color32::WHITE, Color32::from_gray(128)] {
            assert_eq!(flashed(base, 1.0), FLASH_COLOR);
            assert_eq!(flashed(base, 0.0), base);
        }
        // And halfway is between the two, not either end.
        let half = flashed(Color32::BLACK, 0.5);
        assert!(half != Color32::BLACK && half != FLASH_COLOR);
        assert_eq!(half.r(), (f32::from(FLASH_COLOR.r()) * 0.5).round() as u8);
    }

    #[test]
    fn an_out_of_range_blend_is_clamped_rather_than_wrapping() {
        assert_eq!(flashed(Color32::BLACK, 5.0), FLASH_COLOR);
        assert_eq!(flashed(Color32::BLACK, -1.0), Color32::BLACK);
    }

    fn a_view(value: Option<Vec<u8>>) -> CharacteristicView {
        CharacteristicView {
            info: crate::engine::CharacteristicInfo {
                uuid: webbluetooth::uuid::characteristics::BATTERY_LEVEL,
                properties: CharacteristicProperties(
                    CharacteristicProperties::READ | CharacteristicProperties::NOTIFY,
                ),
                descriptors: vec![webbluetooth::uuid::BluetoothUuid::from_u16(0x2902)],
            },
            value,
            updated: None,
            from_notification: false,
            notifying: true,
            descriptor_values: std::collections::HashMap::new(),
            last_write: None,
        }
    }

    /// What gets pasted has to carry the UUID, the properties and the bytes —
    /// reassembling those from separate copies is how a transcription error
    /// reaches a bug report.
    #[test]
    fn a_characteristic_description_carries_everything_on_screen() {
        let text = describe(&names::Definitions::new(), &a_view(Some(vec![0x64])));

        assert!(text.contains("Battery Level"), "{text}");
        assert!(
            text.contains("00002a19-0000-1000-8000-00805f9b34fb"),
            "{text}"
        );
        assert!(text.contains("read, notify"), "{text}");
        assert!(text.contains("hex     64"), "{text}");
        assert!(text.contains("u8"), "the readings come too: {text}");
        assert!(text.contains("100"), "{text}");
        assert!(text.contains("subscribed"), "{text}");
        // A descriptor that has not been read says so rather than being absent.
        assert!(text.contains("not read"), "{text}");
    }

    #[test]
    fn an_unread_characteristic_says_so_rather_than_looking_empty() {
        let unread = describe(&names::Definitions::new(), &a_view(None));
        assert!(unread.contains("value: not read"), "{unread}");

        // "Read and came back with nothing" is a different fact, and the two
        // must not print the same way.
        let empty = describe(&names::Definitions::new(), &a_view(Some(Vec::new())));
        assert!(empty.contains("value: empty (zero bytes)"), "{empty}");
    }

    #[test]
    fn a_device_report_nests_characteristics_under_their_service() {
        let services = vec![crate::model::ServiceView {
            uuid: webbluetooth::uuid::services::BATTERY_SERVICE,
            is_primary: true,
            characteristics: vec![a_view(Some(vec![0x64]))],
        }];
        let text = device_report(
            &names::Definitions::new(),
            "Sensor",
            "AA:BB:CC",
            Some(185),
            &services,
        );

        assert!(text.starts_with("Sensor\nAA:BB:CC\n"), "{text}");
        assert!(text.contains("ATT MTU 185"), "{text}");
        assert!(text.contains("Battery Service"), "{text}");
        assert!(text.contains("= 64"), "the value comes with it: {text}");

        // Indentation is the nesting, so it has to actually be there.
        let characteristic = text
            .lines()
            .find(|line| line.contains("Battery Level"))
            .expect("the characteristic is listed");
        assert!(characteristic.starts_with("  "), "{characteristic:?}");
    }

    #[test]
    fn an_included_service_is_labelled_in_a_report() {
        let services = vec![crate::model::ServiceView {
            uuid: webbluetooth::uuid::services::BATTERY_SERVICE,
            is_primary: false,
            characteristics: Vec::new(),
        }];
        assert!(
            device_report(&names::Definitions::new(), "d", "id", None, &services)
                .contains("[included]")
        );
        // And no MTU line at all when the platform did not report one.
        assert!(
            !device_report(&names::Definitions::new(), "d", "id", None, &services).contains("MTU")
        );
    }

    /// The shorthand in the lists is for the lists. Text leaving the program
    /// goes somewhere `R W w N I` means nothing.
    /// A filename carrying a wrong date is worse than one carrying no date, so
    /// the era arithmetic is checked against dates with known answers —
    /// including the leap-year rules that catch naive implementations.
    #[test]
    fn the_saved_log_is_named_after_its_date() {
        let at = |secs: u64| UNIX_EPOCH + Duration::from_secs(secs);

        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2000 is a leap year (divisible by 400); 1900 was not (by 100).
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));

        assert_eq!(
            log_filename(at(0)),
            "webbluetooth-explorer-19700101-000000Z.log"
        );
        assert_eq!(
            log_filename(at(19_723 * 86_400 + 13 * 3600 + 5 * 60 + 9)),
            "webbluetooth-explorer-20240101-130509Z.log"
        );
    }

    /// Sortable by name, because a directory of these should list in the order
    /// they were taken whatever the reader's locale does with dates.
    #[test]
    fn saved_log_names_sort_chronologically() {
        let a = log_filename(UNIX_EPOCH + Duration::from_secs(19_723 * 86_400));
        let b = log_filename(UNIX_EPOCH + Duration::from_secs(19_724 * 86_400));
        let c = log_filename(UNIX_EPOCH + Duration::from_secs(19_724 * 86_400 + 60));
        let mut all = [c.clone(), a.clone(), b.clone()];
        all.sort();
        assert_eq!(all, [a, b, c]);
    }

    #[test]
    fn saving_a_log_writes_what_copying_it_would_have_copied() {
        let lines = vec![
            crate::model::LogLine {
                at: UNIX_EPOCH,
                level: Level::Info,
                text: "scanning".into(),
            },
            crate::model::LogLine {
                at: UNIX_EPOCH,
                level: Level::Error,
                text: "connect failed".into(),
            },
        ];
        let directory = std::env::temp_dir().join(format!("wbe-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a temporary directory");

        let path = write_log_to(&directory, &lines).expect("the log is written");
        let written = std::fs::read_to_string(&path).expect("and readable");

        assert!(path.starts_with(&directory));
        assert!(path.extension().is_some_and(|e| e == "log"));
        // The file and the clipboard must not disagree about what the log said.
        assert_eq!(written.trim_end(), render_log(&lines));
        assert!(written.ends_with('\n'), "a text file ends in a newline");

        // An empty log still produces a file, rather than an error nobody
        // expected from pressing Save.
        let empty = write_log_to(&directory, &[]).expect("an empty log still writes");
        assert_eq!(std::fs::read_to_string(&empty).unwrap(), "\n");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_rendered_log_marks_the_errors() {
        let lines = vec![
            crate::model::LogLine {
                at: UNIX_EPOCH,
                level: Level::Info,
                text: "scanning".into(),
            },
            crate::model::LogLine {
                at: UNIX_EPOCH + Duration::from_secs(1),
                level: Level::Error,
                text: "connect failed".into(),
            },
        ];
        let text = render_log(&lines);
        let rendered: Vec<&str> = text.lines().collect();

        assert_eq!(rendered.len(), 2);
        assert!(rendered[0].contains("scanning"), "{:?}", rendered[0]);
        // A saved log has no colour, so severity has to survive as text.
        assert!(
            rendered[1].contains(&format!("{} connect failed", Level::Error.mark())),
            "{:?}",
            rendered[1]
        );
        assert_ne!(
            Level::Error.mark(),
            Level::Info.mark(),
            "severities that print the same are not severities"
        );
        assert!(rendered[0].starts_with("00:00:00.000"));
        assert!(render_log(&[]).is_empty());
    }

    /// "The platform did not say" has to survive being pasted somewhere, rather
    /// than coming out as a plausible-looking zero.
    #[test]
    fn an_unknown_link_state_reports_as_unknown() {
        let text = link_report(&crate::engine::LinkDetail::default());
        assert!(text.contains("ATT MTU: —"), "{text}");
        assert!(text.contains("PHY: —"), "{text}");
        assert!(text.contains("bonded: —"), "{text}");
        assert!(text.contains("interval: —"), "{text}");
        assert!(!text.contains(": 0"), "nothing should read as zero: {text}");
    }

    #[test]
    fn a_known_link_state_converts_the_controller_units() {
        let text = link_report(&crate::engine::LinkDetail {
            mtu: Some(247),
            phy: Some((Phy::Le2M, Phy::Le1M)),
            paired: Some(true),
            // 24 × 1.25 ms = 30 ms; 500 × 10 ms = 5000 ms.
            parameters: Some((24, 0, 500)),
        });
        assert!(text.contains("ATT MTU: 247"), "{text}");
        assert!(text.contains("2M tx / 1M rx"), "{text}");
        assert!(text.contains("bonded: yes"), "{text}");
        assert!(text.contains("30.00 ms"), "{text}");
        assert!(text.contains("5000 ms"), "{text}");
    }

    #[test]
    fn property_names_are_spelled_out_for_text_that_leaves_the_program() {
        let props = CharacteristicProperties(
            CharacteristicProperties::WRITE | CharacteristicProperties::WRITE_WITHOUT_RESPONSE,
        );
        assert_eq!(property_names(props), "write, write-without-response");
        assert_eq!(property_names(CharacteristicProperties(0)), "none");
    }

    #[test]
    fn property_summaries_distinguish_the_two_kinds_of_write() {
        let props = |bits| CharacteristicProperties(bits);
        assert_eq!(
            property_summary(props(
                CharacteristicProperties::READ | CharacteristicProperties::NOTIFY
            )),
            "RN"
        );
        assert_eq!(
            property_summary(props(
                CharacteristicProperties::WRITE | CharacteristicProperties::WRITE_WITHOUT_RESPONSE
            )),
            "Ww"
        );
        assert_eq!(property_summary(props(0)), "—");
    }
}
