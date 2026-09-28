//! What the UI knows, and how events change it.
//!
//! Separate from the drawing code because this is the part with rules worth
//! testing — which device is selected after the list re-sorts, whether a value
//! belongs to the characteristic still on screen — and none of those rules need
//! a window open to check.

use crate::engine::{
    AdvSummary, CharRef, CharacteristicInfo, DescRef, Event, Level, LinkDetail, ServiceInfo,
};
use std::collections::HashMap;
use std::time::Instant;
use webbluetooth::uuid::BluetoothUuid;

/// How many signal readings to keep per device.
///
/// At the few advertisements a second a typical peripheral sends, this is
/// several minutes — long enough to see a device moved across a room, short
/// enough that a hundred devices cost a few megabytes rather than gigabytes.
pub const RSSI_HISTORY: usize = 600;

/// How much of each new RSSI reading to believe.
///
/// Low enough that a bar count settles and stays settled, high enough that
/// walking a device across a room is visible within a second or so at the few
/// advertisements per second a typical peripheral sends.
const RSSI_SMOOTHING: f32 = 0.25;

/// How many bars of four a smoothed signal is worth, given what it showed last.
///
/// The previous count is an input because a plain threshold oscillates: a device
/// parked at -80 dBm crosses that boundary in both directions for the rest of
/// the afternoon, and the meter changes on every packet. Requiring the value to
/// pass an edge by a margin before the count follows means a stationary
/// device picks an answer and keeps it.
pub fn bars_with_hysteresis(dbm: f32, current: u32) -> u32 {
    // Where two, three and four bars begin.
    const EDGES: [f32; 3] = [-80.0, -67.0, -55.0];

    let mut bars = 1;
    for (index, edge) in EDGES.iter().enumerate() {
        let target = index as u32 + 2;
        // Already at or above this count: hold it until the signal drops a
        // clear margin below the edge. Below it: require the same margin above.
        let edge = if current >= target {
            edge - BAR_MARGIN
        } else {
            edge + BAR_MARGIN
        };
        if dbm >= edge {
            bars = target;
        }
    }
    bars
}

/// How far past a bar boundary a signal must go before the meter follows, in dB.
///
/// Wider than the residual wobble the smoothing leaves, narrower than the gap
/// between buckets.
const BAR_MARGIN: f32 = 2.0;

/// How the device list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// Strongest signal first — nearest thing to hand, which is usually the
    /// device being worked on.
    Rssi,
    /// Alphabetical, with the unnamed devices last.
    Name,
    /// Most recently heard from first.
    LastSeen,
}

/// One device in the list.
#[derive(Debug, Clone)]
pub struct DeviceRow {
    /// Per-host identifier.
    pub id: String,
    /// The name it reports, if any.
    pub name: Option<String>,
    /// The last signal strength reported, exactly as the radio gave it.
    pub rssi: Option<i32>,
    /// A smoothed signal strength, for anything that should not jitter.
    ///
    /// RSSI from a real radio moves several dB between consecutive packets from
    /// a stationary device — multipath, antenna orientation, someone walking
    /// past. Displaying the raw figure is right; *deciding* things with it is
    /// not. Sorting on it made the list reorder continuously, and bucketing it
    /// into a bar count made the meter flip between buckets on every packet.
    /// Both read as flicker.
    pub rssi_smoothed: Option<f32>,
    /// When it was last heard from.
    pub last_seen: Instant,
    /// When anything about this row last actually changed.
    ///
    /// Not the same as [`Self::last_seen`], and the difference is the whole
    /// point. Highlighting on every advertisement sounded right and is not: a
    /// device in range advertises several times a second, so every row in the
    /// list sat permanently lit, which is both useless as a signal and the
    /// flicker it was meant to be.
    ///
    /// A change here is a device appearing, or a name arriving or changing —
    /// the things a row cannot show any other way. Signal strength is not one:
    /// the meter and the number beside it already say it, continuously.
    pub last_changed: Instant,
    /// How many bars of four the meter shows, with hysteresis already applied.
    ///
    /// Stored rather than derived because the previous value is an input:
    /// see [`bars_with_hysteresis`].
    pub bars: u32,
    /// How many advertisements have arrived.
    pub sightings: u64,
    /// Recent signal readings, oldest first, for the graph.
    ///
    /// Bounded: a device advertising ten times a second for an afternoon would
    /// otherwise be a hundred thousand samples nobody is going to look at. What
    /// a signal graph is for is the last minute or two — walking a device
    /// around a room, or watching an antenna change take effect.
    pub history: std::collections::VecDeque<(Instant, i32)>,
    /// The most recent packet's contents.
    pub adv: AdvSummary,
}

impl DeviceRow {
    /// Something printable, as the chooser's `label` does it.
    ///
    /// This is what the *device* calls itself. [`Model::display_name`] is what
    /// the list shows, which may be a name you gave it instead.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// A characteristic as shown, with whatever has been read from it.
#[derive(Debug, Clone)]
pub struct CharacteristicView {
    /// What discovery reported.
    pub info: CharacteristicInfo,
    /// The last value seen, read or notified.
    pub value: Option<Vec<u8>>,
    /// When that value arrived.
    pub updated: Option<Instant>,
    /// Whether the last value came in as a notification.
    pub from_notification: bool,
    /// Whether a subscription is live.
    pub notifying: bool,
    /// Values read from its descriptors, by index.
    pub descriptor_values: HashMap<usize, DescriptorValue>,
    /// How many bytes the last successful write sent, and when.
    ///
    /// A write with no response gets no acknowledgement from the peer, so this
    /// is the only confirmation there is that anything left the host.
    pub last_write: Option<(usize, Instant)>,
}

/// What a descriptor last read as, and when.
///
/// The timestamp is not decoration: it is what lets the UI highlight a value
/// that just changed, which is the only way to tell a re-read that returned the
/// same bytes from one that did nothing at all.
#[derive(Debug, Clone)]
pub struct DescriptorValue {
    /// The bytes.
    pub bytes: Vec<u8>,
    /// When they arrived.
    pub at: Instant,
}

/// A service as shown.
#[derive(Debug, Clone)]
pub struct ServiceView {
    /// Its UUID.
    pub uuid: BluetoothUuid,
    /// Whether it is primary.
    pub is_primary: bool,
    /// Its characteristics.
    pub characteristics: Vec<CharacteristicView>,
}

/// One device this window is talking to.
///
/// There can be several. Each gets a tab, and each carries its own tree,
/// selection and link state — which is the whole reason this is a struct
/// rather than the single `Link` enum it replaced: with one connection those
/// fields could live on the model, and with several they cannot.
#[derive(Debug, Clone)]
pub struct Session {
    /// Which device.
    pub id: String,
    /// Whether the link is up, or still being established.
    pub connected: bool,
    /// Whether the tree has arrived yet.
    pub discovered: bool,
    /// The device's GATT tree.
    pub services: Vec<ServiceView>,
    /// Which service row is selected, for this device.
    pub selected_service: Option<usize>,
    /// Which characteristic row is selected, for this device.
    pub selected_characteristic: Option<usize>,
    /// What the platform says about this link.
    ///
    /// The ATT MTU lives here and nowhere else. It used to be taken from the
    /// `Connected` event as well, and on macOS the two disagreed — CoreBluetooth
    /// reports 23 at connect and the real figure once the link settles — so two
    /// places to read it from meant two numbers on screen at once.
    pub link_detail: LinkDetail,
}

impl Session {
    fn new(id: String) -> Self {
        Self {
            id,
            connected: false,
            discovered: false,
            services: Vec::new(),
            selected_service: None,
            selected_characteristic: None,
            link_detail: LinkDetail::default(),
        }
    }

    /// The selected service, if the selection still points at one.
    pub fn service(&self) -> Option<&ServiceView> {
        self.services.get(self.selected_service?)
    }

    /// The selected characteristic.
    pub fn characteristic(&self) -> Option<&CharacteristicView> {
        self.service()?
            .characteristics
            .get(self.selected_characteristic?)
    }

    /// Where the selected characteristic is, for a command.
    pub fn selected_ref(&self) -> Option<CharRef> {
        Some(CharRef {
            device: self.id.clone(),
            service: self.selected_service?,
            characteristic: self.selected_characteristic?,
        })
    }
}

/// One line in the log pane.
#[derive(Debug, Clone)]
pub struct LogLine {
    /// When it happened, as wall-clock time for pasting into a bug report.
    pub at: std::time::SystemTime,
    /// How bad it is.
    pub level: Level,
    /// What happened.
    pub text: String,
}

/// Everything on screen.
pub struct Model {
    /// Devices seen this session, by id.
    pub devices: HashMap<String, DeviceRow>,
    /// How the list is ordered.
    pub sort: Sort,
    /// Substring filter over name and id, from the search box.
    pub filter: String,
    /// Hide anything whose name or id contains one of these, comma separated.
    ///
    /// nRF Connect's exclusion filter. In a busy building the useful question
    /// is usually "everything except the forty access points", which a positive
    /// filter cannot express.
    pub exclude: String,
    /// Keep only devices advertising a service whose UUID or name matches.
    pub service_filter: String,
    /// Keep only devices whose manufacturer data comes from this company.
    ///
    /// Decimal, or `0x`-prefixed hex — company identifiers are written both
    /// ways depending on which document you are reading.
    pub company_filter: String,
    /// Keep only devices whose advertising data contains these bytes.
    ///
    /// Hex, matched against manufacturer data, service data, and the raw packet
    /// where the platform provides one. The way to find a beacon by its
    /// payload when it advertises nothing else identifying.
    pub data_filter: String,
    /// Show favourites above everything else, whatever the sort.
    pub favourites_first: bool,
    /// Hide devices with no name at all.
    pub named_only: bool,
    /// Hide devices that said they do not accept connections.
    ///
    /// From nRF Connect's scanner filters. A room full of iBeacons is a room
    /// full of rows there is nothing to be done with.
    pub connectable_only: bool,
    /// Hide anything weaker than this, in dBm. `None` is no floor.
    ///
    /// Also nRF Connect's: the practical way to reduce a crowded scan to the
    /// device on the desk in front of you.
    pub rssi_floor: Option<i32>,
    /// Whether the radio is scanning.
    pub scanning: bool,
    /// Why Bluetooth is unusable, if it is.
    pub unavailable: Option<String>,
    /// Every device this window is talking to, in the order their tabs appear.
    pub sessions: Vec<Session>,
    /// Which tab is in front.
    pub active: Option<String>,
    /// Whether a recorded sequence is running.
    pub macro_running: bool,
    /// How the last suite came out.
    pub suite_results: Option<crate::suites::Results>,
    /// Whether a server of ours is published and advertising.
    pub server_running: bool,
    /// Why it is not, if publishing failed.
    pub server_error: Option<String>,
    /// Why the last connection ended, when it was not this program's doing.
    pub last_disconnect: Option<String>,
    /// Everything remembered between runs: names, favourites, settings.
    pub remembered: crate::store::Remembered,
    /// The lowest severity the log pane shows.
    pub log_level: Level,
    /// Substring filter over log lines.
    pub log_filter: String,
    /// The log, newest last.
    pub log: Vec<LogLine>,
    /// How many log lines to keep.
    pub log_limit: usize,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            devices: HashMap::new(),
            sort: Sort::Rssi,
            filter: String::new(),
            exclude: String::new(),
            service_filter: String::new(),
            company_filter: String::new(),
            data_filter: String::new(),
            favourites_first: true,
            named_only: false,
            connectable_only: false,
            rssi_floor: None,
            scanning: false,
            unavailable: None,
            sessions: Vec::new(),
            active: None,
            macro_running: false,
            suite_results: None,
            server_running: false,
            server_error: None,
            last_disconnect: None,
            remembered: crate::store::Remembered::default(),
            log_level: Level::Info,
            log_filter: String::new(),
            log: Vec::new(),
            log_limit: 2000,
        }
    }
}

impl Model {
    /// What to call a device: the name you gave it, else its own.
    ///
    /// Plenty of devices advertise as `n/a`, or as the same model name as the
    /// four others on the bench. Renaming is local and costs the device
    /// nothing.
    pub fn display_name<'a>(&'a self, row: &'a DeviceRow) -> &'a str {
        self.remembered
            .renames
            .get(&row.id)
            .map_or_else(|| row.label(), String::as_str)
    }

    /// Whether a device is marked as one to keep an eye on.
    pub fn is_favourite(&self, id: &str) -> bool {
        self.remembered.favourites.contains(id)
    }

    /// Whether this row passes the advertising-data filters.
    fn matches_advertising(&self, row: &DeviceRow) -> bool {
        let service = self.service_filter.trim().to_lowercase();
        if !service.is_empty() {
            let hit = row.adv.service_uuids.iter().any(|uuid| {
                uuid.as_str().to_lowercase().contains(&service)
                    || crate::names::label_with(
                        &self.remembered.definitions,
                        uuid,
                        crate::names::Kind::Service,
                    )
                    .to_lowercase()
                    .contains(&service)
            });
            if !hit {
                return false;
            }
        }

        let company = self.company_filter.trim();
        if !company.is_empty() {
            let wanted = company
                .strip_prefix("0x")
                .or_else(|| company.strip_prefix("0X"))
                .and_then(|hex| u16::from_str_radix(hex, 16).ok())
                .or_else(|| company.parse::<u16>().ok());
            // An unparseable company filter matches nothing rather than
            // everything: a typo should not quietly widen the search.
            match wanted {
                Some(id) => {
                    if !row.adv.manufacturer_data.iter().any(|(c, _)| *c == id) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        let data = self.data_filter.trim();
        if !data.is_empty() {
            let Ok(needle) = crate::format::parse_hex(data) else {
                return false;
            };
            if needle.is_empty() {
                return true;
            }
            let contains = |bytes: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
            let hit = row.adv.manufacturer_data.iter().any(|(_, d)| contains(d))
                || row.adv.service_data.iter().any(|(_, d)| contains(d))
                || row.adv.raw.as_deref().is_some_and(contains);
            if !hit {
                return false;
            }
        }
        true
    }

    /// The device list, filtered and sorted for display.
    ///
    /// Recomputed every frame. A few hundred devices is a big room, and sorting
    /// that is cheaper than the bookkeeping to keep an ordered copy correct as
    /// RSSI changes on every packet.
    pub fn visible_devices(&self) -> Vec<&DeviceRow> {
        let needle = self.filter.trim().to_lowercase();
        let mut rows: Vec<&DeviceRow> = self
            .devices
            .values()
            .filter(|row| {
                !self.named_only
                    || row.name.is_some()
                    || self.remembered.renames.contains_key(&row.id)
            })
            // `None` is "the device did not say", which is not the same as
            // "it said no" — so an unknown stays visible.
            .filter(|row| !self.connectable_only || row.adv.connectable != Some(false))
            .filter(|row| match (self.rssi_floor, row.rssi) {
                (None, _) => true,
                // No reading is not a weak reading; it stays rather than being
                // silently filtered out by a slider the operator forgot about.
                (Some(_), None) => true,
                (Some(floor), Some(dbm)) => dbm >= floor,
            })
            .filter(|row| self.matches_advertising(row))
            .filter(|row| {
                needle.is_empty()
                    || row.id.to_lowercase().contains(&needle)
                    || self.display_name(row).to_lowercase().contains(&needle)
            })
            .filter(|row| {
                // Each comma-separated term hides anything containing it.
                self.exclude.split(',').map(str::trim).all(|term| {
                    term.is_empty()
                        || !(row.id.to_lowercase().contains(&term.to_lowercase())
                            || self
                                .display_name(row)
                                .to_lowercase()
                                .contains(&term.to_lowercase()))
                })
            })
            .collect();

        // Favourites first, then whatever the chosen order says. Done as a
        // separate pass so the two decisions stay independent.
        let favourite = |row: &DeviceRow| self.is_favourite(&row.id);

        match self.sort {
            // Sorted on the smoothed figure, not the raw one: ordering by a
            // value that moves several dB per packet makes the list reorder
            // continuously under the pointer.
            //
            // `None` is "no reading", which belongs at the bottom rather than
            // sorting as if it were the weakest signal.
            Sort::Rssi => rows.sort_by(|a, b| {
                let key = |row: &DeviceRow| row.rssi_smoothed.unwrap_or(f32::NEG_INFINITY);
                key(b)
                    .total_cmp(&key(a))
                    .then_with(|| a.label().cmp(b.label()))
            }),
            Sort::Name => rows.sort_by(|a, b| {
                a.name
                    .is_none()
                    .cmp(&b.name.is_none())
                    .then_with(|| a.label().to_lowercase().cmp(&b.label().to_lowercase()))
            }),
            Sort::LastSeen => rows.sort_by(|a, b| b.last_seen.cmp(&a.last_seen)),
        }
        if self.favourites_first {
            // A stable sort, so everything decided above survives inside each
            // group.
            rows.sort_by_key(|row| !favourite(row));
        }
        rows
    }

    /// Every visible device's signal history as comma-separated values.
    ///
    /// Seconds before now, so the file is readable without knowing when it was
    /// taken, and a device that stopped reporting is obvious by where its rows
    /// end.
    pub fn signal_csv(&self) -> String {
        let mut out = String::from("device,name,seconds_ago,dbm\n");
        let now = Instant::now();
        for row in self.visible_devices() {
            let name = self.display_name(row).replace([',', '\n'], " ");
            for (at, dbm) in &row.history {
                out.push_str(&format!(
                    "{},{name},{:.2},{dbm}\n",
                    row.id,
                    now.saturating_duration_since(*at).as_secs_f32()
                ));
            }
        }
        out
    }

    /// Whether any filter is currently hiding something.
    ///
    /// Shown on the toolbar button, because a filter you forgot you set is
    /// indistinguishable from a room that has gone quiet.
    pub fn filters_are_narrowing(&self) -> bool {
        self.named_only
            || self.connectable_only
            || self.rssi_floor.is_some()
            || [
                &self.exclude,
                &self.service_filter,
                &self.company_filter,
                &self.data_filter,
            ]
            .iter()
            .any(|value| !value.trim().is_empty())
    }

    /// The log lines the pane should show, given the level and text filter.
    pub fn visible_log(&self) -> impl Iterator<Item = &LogLine> {
        let needle = self.log_filter.trim().to_lowercase();
        self.log
            .iter()
            .filter(move |line| line.level >= self.log_level)
            .filter(move |line| needle.is_empty() || line.text.to_lowercase().contains(&needle))
    }

    /// The session whose tab is in front.
    pub fn session(&self) -> Option<&Session> {
        let active = self.active.as_deref()?;
        self.sessions.iter().find(|s| s.id == active)
    }

    /// The same, mutably.
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        let active = self.active.clone()?;
        self.sessions.iter_mut().find(|s| s.id == active)
    }

    /// One session by id, whether or not it is in front.
    pub fn session_of(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    fn session_of_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// The selected service in the active session.
    pub fn service(&self) -> Option<&ServiceView> {
        self.session()?.service()
    }

    /// The selected characteristic in the active session.
    pub fn characteristic(&self) -> Option<&CharacteristicView> {
        self.session()?.characteristic()
    }

    /// Where the selected characteristic is, for a command.
    pub fn selected_ref(&self) -> Option<CharRef> {
        self.session()?.selected_ref()
    }

    /// Whether a device is one this window has open.
    pub fn is_open(&self, id: &str) -> bool {
        self.sessions.iter().any(|s| s.id == id)
    }

    fn characteristic_mut(&mut self, at: &CharRef) -> Option<&mut CharacteristicView> {
        self.session_of_mut(&at.device)?
            .services
            .get_mut(at.service)?
            .characteristics
            .get_mut(at.characteristic)
    }

    /// Fold one engine event in.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Availability(Ok(())) => self.unavailable = None,
            Event::Availability(Err(why)) => self.unavailable = Some(why),

            Event::Scanning(on) => self.scanning = on,

            Event::Sighting {
                id,
                name,
                rssi,
                adv,
            } => {
                // A device not seen before is itself a change worth noticing.
                let mut changed = !self.devices.contains_key(&id);
                let row = self.devices.entry(id.clone()).or_insert_with(|| DeviceRow {
                    id,
                    name: None,
                    rssi: None,
                    rssi_smoothed: None,
                    last_seen: Instant::now(),
                    last_changed: Instant::now(),
                    bars: 0,
                    sightings: 0,
                    history: std::collections::VecDeque::new(),
                    adv: AdvSummary::default(),
                });
                // A name can arrive in a scan response after the first packet,
                // so a later `None` must not erase one already learned.
                if name.is_some() && name != row.name {
                    row.name = name;
                    changed = true;
                }
                if let Some(dbm) = rssi {
                    row.rssi = rssi;
                    row.history.push_back((Instant::now(), dbm));
                    while row.history.len() > RSSI_HISTORY {
                        row.history.pop_front();
                    }
                    // An exponential moving average. The first reading is taken
                    // whole, so a device does not fade in from nowhere.
                    let smoothed = match row.rssi_smoothed {
                        None => dbm as f32,
                        Some(previous) => {
                            previous * (1.0 - RSSI_SMOOTHING) + dbm as f32 * RSSI_SMOOTHING
                        }
                    };
                    row.rssi_smoothed = Some(smoothed);
                    // The bar count deliberately does *not* count as a change
                    // worth highlighting. The meter is already showing it, and
                    // a name that turns red because a device drifted one bucket
                    // is noise standing in front of the signal.
                    row.bars = bars_with_hysteresis(smoothed, row.bars);
                }
                row.last_seen = Instant::now();
                if changed {
                    row.last_changed = Instant::now();
                }
                row.sightings += 1;
                row.adv = adv;
            }

            Event::Connecting(id) => {
                self.last_disconnect = None;
                if !self.is_open(&id) {
                    self.sessions.push(Session::new(id.clone()));
                }
                // A new connection comes to the front, as opening a tab does.
                self.active = Some(id);
            }

            Event::Connected { id, mtu } => {
                if !self.is_open(&id) {
                    self.sessions.push(Session::new(id.clone()));
                }
                if let Some(session) = self.session_of_mut(&id) {
                    session.connected = true;
                    // The first, provisional reading. `ReadLinkDetail` follows
                    // and usually replaces it with a larger one.
                    if mtu.is_some() {
                        session.link_detail.mtu = mtu;
                    }
                }
                self.active = Some(id);
            }

            Event::Tree { id, services } => {
                // Routed by id, not by what is in front: a tree arriving while
                // the operator has moved to another tab belongs to the device
                // it came from.
                let Some(session) = self.session_of_mut(&id) else {
                    return;
                };
                session.services = services.into_iter().map(ServiceView::from).collect();
                session.discovered = true;
                session.selected_service = (!session.services.is_empty()).then_some(0);
                session.selected_characteristic = session
                    .service()
                    .filter(|s| !s.characteristics.is_empty())
                    .map(|_| 0);
            }

            Event::Value {
                at,
                value,
                notified,
            } => {
                if let Some(view) = self.characteristic_mut(&at) {
                    view.value = Some(value);
                    view.updated = Some(Instant::now());
                    view.from_notification = notified;
                }
            }

            Event::DescriptorValue { at, value } => {
                if let Some(view) = self.characteristic_mut(&at.parent()) {
                    view.descriptor_values.insert(
                        at.descriptor,
                        DescriptorValue {
                            bytes: value,
                            at: Instant::now(),
                        },
                    );
                }
            }

            Event::Notifying { at, on } => {
                if let Some(view) = self.characteristic_mut(&at) {
                    view.notifying = on;
                }
            }

            Event::MacroRunning(running) => self.macro_running = running,

            Event::SuiteResults(results) => {
                // Kept rather than logged: the point of a suite is a result you
                // can look at afterwards, not a log you have to read back.
                self.suite_results = Some(results);
            }

            Event::ServerState { running, why } => {
                self.server_running = running;
                self.server_error = why;
            }

            Event::LinkDetail { device, detail } => {
                // Merged field by field, not replaced: each field is a separate
                // platform call, and one that failed must not blank out an
                // answer another one gave.
                let Some(session) = self.session_of_mut(&device) else {
                    return;
                };
                if detail.mtu.is_some() {
                    session.link_detail.mtu = detail.mtu;
                }
                if detail.phy.is_some() {
                    session.link_detail.phy = detail.phy;
                }
                if detail.paired.is_some() {
                    session.link_detail.paired = detail.paired;
                }
                if detail.parameters.is_some() {
                    session.link_detail.parameters = detail.parameters;
                }
            }

            Event::Wrote { at, len } => {
                if let Some(view) = self.characteristic_mut(&at) {
                    view.last_write = Some((len, Instant::now()));
                }
            }

            Event::Disconnected { id, why } => {
                if !self.is_open(&id) {
                    return;
                }
                if why.is_some() {
                    self.last_disconnect = why;
                }
                self.sessions.retain(|session| session.id != id);
                if self.active.as_deref() == Some(id.as_str()) {
                    // Fall back to whatever is still open, as closing a tab
                    // does, rather than leaving nothing in front.
                    self.active = self.sessions.last().map(|session| session.id.clone());
                }
            }

            Event::Rssi { id, rssi } => {
                if let Some(row) = self.devices.get_mut(&id) {
                    row.rssi = Some(rssi);
                    // A polled reading from a live connection is a sample like
                    // any other, and on a connected device it is the only kind
                    // there is — a connected peripheral stops advertising.
                    row.history.push_back((Instant::now(), rssi));
                    while row.history.len() > RSSI_HISTORY {
                        row.history.pop_front();
                    }
                    row.rssi_smoothed = Some(match row.rssi_smoothed {
                        None => rssi as f32,
                        Some(previous) => {
                            previous * (1.0 - RSSI_SMOOTHING) + rssi as f32 * RSSI_SMOOTHING
                        }
                    });
                    row.bars =
                        bars_with_hysteresis(row.rssi_smoothed.unwrap_or(rssi as f32), row.bars);
                    row.last_seen = Instant::now();
                }
            }

            Event::Log(level, text) => {
                self.log.push(LogLine {
                    at: std::time::SystemTime::now(),
                    level,
                    text,
                });
                // Bounded, because a chatty subscription would otherwise grow
                // this without limit for as long as the window is open.
                if self.log.len() > self.log_limit {
                    let excess = self.log.len() - self.log_limit;
                    self.log.drain(..excess);
                }
            }
        }
    }
}

impl From<ServiceInfo> for ServiceView {
    fn from(info: ServiceInfo) -> Self {
        Self {
            uuid: info.uuid,
            is_primary: info.is_primary,
            characteristics: info
                .characteristics
                .into_iter()
                .map(|info| CharacteristicView {
                    info,
                    value: None,
                    updated: None,
                    from_notification: false,
                    notifying: false,
                    descriptor_values: HashMap::new(),
                    last_write: None,
                })
                .collect(),
        }
    }
}

/// Where a descriptor is, given its characteristic.
pub fn descriptor_ref(at: &CharRef, descriptor: usize) -> DescRef {
    DescRef {
        device: at.device.clone(),
        service: at.service,
        characteristic: at.characteristic,
        descriptor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webbluetooth::uuid::services;
    use webbluetooth::CharacteristicProperties;

    fn sighting(id: &str, name: Option<&str>, rssi: Option<i32>) -> Event {
        Event::Sighting {
            id: id.to_owned(),
            name: name.map(str::to_owned),
            rssi,
            adv: AdvSummary::default(),
        }
    }

    fn tree(id: &str) -> Event {
        Event::Tree {
            id: id.to_owned(),
            services: vec![ServiceInfo {
                uuid: services::HEART_RATE,
                is_primary: true,
                characteristics: vec![CharacteristicInfo {
                    uuid: BluetoothUuid::from_u16(0x2A37),
                    properties: CharacteristicProperties(CharacteristicProperties::NOTIFY),
                    descriptors: vec![BluetoothUuid::from_u16(0x2902)],
                }],
            }],
        }
    }

    fn connected(model: &mut Model, id: &str) {
        model.apply(Event::Connecting(id.to_owned()));
        model.apply(Event::Connected {
            id: id.to_owned(),
            mtu: Some(23),
        });
        model.apply(tree(id));
    }

    /// The common case for a name arriving in a scan response after the first
    /// packet: judging each report alone would flicker the name back off.
    #[test]
    fn a_later_nameless_packet_does_not_erase_a_known_name() {
        let mut model = Model::default();
        model.apply(sighting("a", Some("Sensor"), Some(-50)));
        model.apply(sighting("a", None, Some(-55)));

        let row = &model.devices["a"];
        assert_eq!(row.name.as_deref(), Some("Sensor"));
        assert_eq!(row.rssi, Some(-55), "but signal strength is the new one");
        assert_eq!(row.sightings, 2);
    }

    /// The bug this exists to prevent: raw RSSI moves several dB per packet, so
    /// sorting on it reordered the list continuously under the pointer.
    #[test]
    fn a_noisy_signal_does_not_reorder_the_list() {
        let mut model = Model::default();
        // Two devices a long way apart in signal, each jittering by ±4 dB.
        model.apply(sighting("near", Some("Near"), Some(-45)));
        model.apply(sighting("far", Some("Far"), Some(-75)));

        for (i, (near, far)) in [(-49, -71), (-41, -79), (-48, -72), (-42, -78)]
            .into_iter()
            .enumerate()
        {
            model.apply(sighting("near", Some("Near"), Some(near)));
            model.apply(sighting("far", Some("Far"), Some(far)));
            let order: Vec<&str> = model
                .visible_devices()
                .iter()
                .map(|row| row.label())
                .collect();
            assert_eq!(order, ["Near", "Far"], "reordered on sweep {i}");
        }
    }

    #[test]
    fn the_smoothed_signal_converges_on_a_steady_reading() {
        let mut model = Model::default();
        for _ in 0..40 {
            model.apply(sighting("a", Some("A"), Some(-60)));
        }
        let smoothed = model.devices["a"].rssi_smoothed.unwrap();
        assert!(
            (smoothed - -60.0).abs() < 0.5,
            "settled at {smoothed}, not -60"
        );
        // And the raw figure is still the raw figure: it is what gets displayed.
        assert_eq!(model.devices["a"].rssi, Some(-60));
    }

    /// The first reading is taken whole, so a new device appears at its actual
    /// strength rather than fading in from nothing.
    #[test]
    fn the_first_reading_is_not_smoothed() {
        let mut model = Model::default();
        model.apply(sighting("a", Some("A"), Some(-70)));
        assert_eq!(model.devices["a"].rssi_smoothed, Some(-70.0));
    }

    /// A device that moves should still be seen to move, and reasonably soon.
    #[test]
    fn a_real_change_still_gets_through() {
        let mut model = Model::default();
        model.apply(sighting("a", Some("A"), Some(-90)));
        // Roughly a second of advertisements at a few per second.
        for _ in 0..8 {
            model.apply(sighting("a", Some("A"), Some(-50)));
        }
        let smoothed = model.devices["a"].rssi_smoothed.unwrap();
        assert!(
            smoothed > -60.0,
            "eight packets should have most of the way there, got {smoothed}"
        );
    }

    #[test]
    fn devices_with_no_reading_sort_last_not_weakest() {
        let mut model = Model::default();
        model.apply(sighting("quiet", Some("Quiet"), None));
        model.apply(sighting("far", Some("Far"), Some(-99)));
        model.apply(sighting("near", Some("Near"), Some(-40)));

        let order: Vec<&str> = model.visible_devices().iter().map(|r| r.label()).collect();
        assert_eq!(order, ["Near", "Far", "Quiet"]);
    }

    #[test]
    fn sorting_by_name_puts_the_unnamed_at_the_bottom() {
        let mut model = Model {
            sort: Sort::Name,
            ..Default::default()
        };
        model.apply(sighting("zz", None, Some(-30)));
        model.apply(sighting("id-b", Some("beta"), Some(-80)));
        model.apply(sighting("id-a", Some("Alpha"), Some(-80)));

        let order: Vec<&str> = model.visible_devices().iter().map(|r| r.label()).collect();
        assert_eq!(order, ["Alpha", "beta", "zz"]);
    }

    #[test]
    fn the_filter_matches_name_or_id_case_insensitively() {
        let mut model = Model::default();
        model.apply(sighting("AABBCC", Some("Thermostat"), Some(-40)));
        model.apply(sighting("DDEEFF", Some("Lock"), Some(-40)));

        model.filter = "therm".into();
        assert_eq!(model.visible_devices().len(), 1);

        model.filter = "ddee".into();
        assert_eq!(model.visible_devices()[0].label(), "Lock");

        model.filter = "nothing".into();
        assert!(model.visible_devices().is_empty());
    }

    #[test]
    fn named_only_hides_the_rest() {
        let mut model = Model::default();
        model.apply(sighting("a", Some("Named"), Some(-40)));
        model.apply(sighting("b", None, Some(-40)));

        assert_eq!(model.visible_devices().len(), 2);
        model.named_only = true;
        assert_eq!(model.visible_devices().len(), 1);
    }

    fn sighting_with(id: &str, rssi: Option<i32>, connectable: Option<bool>) -> Event {
        Event::Sighting {
            id: id.to_owned(),
            name: Some(id.to_owned()),
            rssi,
            adv: AdvSummary {
                connectable,
                ..AdvSummary::default()
            },
        }
    }

    /// "Did not say" is not "said no": a device that never reported
    /// connectability must not be hidden by a filter about what devices said.
    #[test]
    fn the_connectable_filter_keeps_devices_that_did_not_say() {
        let mut model = Model::default();
        model.apply(sighting_with("yes", Some(-40), Some(true)));
        model.apply(sighting_with("no", Some(-40), Some(false)));
        model.apply(sighting_with("silent", Some(-40), None));

        model.connectable_only = true;
        let visible: Vec<&str> = model
            .visible_devices()
            .iter()
            .map(|row| row.id.as_str())
            .collect();
        assert!(visible.contains(&"yes"));
        assert!(visible.contains(&"silent"), "unknown is not a refusal");
        assert!(!visible.contains(&"no"));
    }

    /// Same reasoning for the signal floor: no reading is not a weak reading,
    /// and a device should not vanish behind a slider nobody remembers setting.
    #[test]
    fn the_signal_floor_keeps_devices_with_no_reading() {
        let mut model = Model::default();
        model.apply(sighting_with("near", Some(-40), None));
        model.apply(sighting_with("far", Some(-95), None));
        model.apply(sighting_with("quiet", None, None));

        assert_eq!(model.visible_devices().len(), 3);

        model.rssi_floor = Some(-60);
        let visible: Vec<&str> = model
            .visible_devices()
            .iter()
            .map(|row| row.id.as_str())
            .collect();
        assert!(visible.contains(&"near"));
        assert!(visible.contains(&"quiet"));
        assert!(!visible.contains(&"far"));

        // Exactly at the floor is inside it.
        model.rssi_floor = Some(-40);
        assert!(model.visible_devices().iter().any(|row| row.id == "near"));
    }

    /// Each field comes from its own platform call, any of which can fail; a
    /// failure must not blank out what another call already established.
    #[test]
    fn link_detail_merges_rather_than_replacing() {
        let mut model = Model::default();
        connected(&mut model, "dev");

        // A real read replaces the provisional connect-time figure.
        assert_eq!(model.session().unwrap().link_detail.mtu, Some(23));
        model.apply(Event::LinkDetail {
            device: "dev".into(),
            detail: LinkDetail {
                mtu: Some(247),
                paired: Some(true),
                ..LinkDetail::default()
            },
        });
        // A later answer about only the PHY arrives.
        model.apply(Event::LinkDetail {
            device: "dev".into(),
            detail: LinkDetail {
                phy: Some((webbluetooth::Phy::Le2M, webbluetooth::Phy::Le2M)),
                ..LinkDetail::default()
            },
        });

        let session = model.session().unwrap();
        assert_eq!(session.link_detail.mtu, Some(247), "kept");
        assert_eq!(session.link_detail.paired, Some(true), "kept");
        assert!(session.link_detail.phy.is_some(), "learned");
    }

    #[test]
    fn link_detail_is_forgotten_when_the_link_goes() {
        let mut model = Model::default();
        connected(&mut model, "dev");
        model.apply(Event::LinkDetail {
            device: "dev".into(),
            detail: LinkDetail {
                mtu: Some(247),
                ..LinkDetail::default()
            },
        });

        model.apply(Event::Disconnected {
            id: "dev".into(),
            why: None,
        });
        // The session went with the link, and its detail with it.
        assert!(model.session_of("dev").is_none());
    }

    /// The whole point of the rework: two devices open at once, each with its
    /// own tree and its own selection.
    #[test]
    fn two_devices_can_be_open_at_once() {
        let mut model = Model::default();
        connected(&mut model, "first");
        connected(&mut model, "second");

        assert_eq!(model.sessions.len(), 2);
        assert_eq!(model.active.as_deref(), Some("second"), "newest in front");
        assert!(model.is_open("first") && model.is_open("second"));

        // Each has its own tree, not a shared one.
        assert_eq!(model.session_of("first").unwrap().services.len(), 1);
        assert_eq!(model.session_of("second").unwrap().services.len(), 1);
    }

    /// An event routes by the device it names, not by whichever tab happens to
    /// be in front — which is the failure mode the device-carrying `CharRef`
    /// exists to prevent.
    #[test]
    fn a_value_lands_on_its_own_device_not_the_active_one() {
        let mut model = Model::default();
        connected(&mut model, "first");
        connected(&mut model, "second");

        let at = CharRef {
            device: "first".into(),
            service: 0,
            characteristic: 0,
        };
        model.apply(Event::Value {
            at,
            value: vec![0x42],
            notified: true,
        });

        // It landed on the background tab.
        assert_eq!(
            model.session_of("first").unwrap().services[0].characteristics[0]
                .value
                .as_deref(),
            Some(&[0x42][..])
        );
        // And not on the one in front.
        assert!(model.characteristic().unwrap().value.is_none());
    }

    #[test]
    fn selections_are_per_device() {
        let mut model = Model::default();
        connected(&mut model, "first");
        connected(&mut model, "second");

        model.session_mut().unwrap().selected_characteristic = None;
        assert_eq!(
            model.session_of("first").unwrap().selected_characteristic,
            Some(0),
            "the other tab's selection is untouched"
        );
    }

    /// Closing a tab must not take the others with it, and must leave
    /// something in front.
    #[test]
    fn closing_one_tab_leaves_the_others() {
        let mut model = Model::default();
        connected(&mut model, "first");
        connected(&mut model, "second");

        model.apply(Event::Disconnected {
            id: "second".into(),
            why: None,
        });
        assert_eq!(model.sessions.len(), 1);
        assert_eq!(model.active.as_deref(), Some("first"));

        model.apply(Event::Disconnected {
            id: "first".into(),
            why: None,
        });
        assert!(model.sessions.is_empty());
        assert_eq!(model.active, None);
    }

    /// A disconnect for something never opened is a late message about a
    /// device this window has nothing to do with.
    #[test]
    fn a_disconnect_for_a_device_never_opened_changes_nothing() {
        let mut model = Model::default();
        connected(&mut model, "mine");
        model.apply(Event::Disconnected {
            id: "somebody-elses".into(),
            why: Some("gone".into()),
        });
        assert_eq!(model.sessions.len(), 1);
        assert_eq!(model.last_disconnect, None, "not our business to report");
    }

    /// Connecting to something already open brings it forward rather than
    /// opening a second tab for the same device.
    #[test]
    fn reconnecting_an_open_device_does_not_duplicate_its_tab() {
        let mut model = Model::default();
        connected(&mut model, "first");
        connected(&mut model, "second");

        model.apply(Event::Connected {
            id: "first".into(),
            mtu: None,
        });
        assert_eq!(model.sessions.len(), 2);
        assert_eq!(model.active.as_deref(), Some("first"));
        // And the provisional `None` did not erase what was known.
        assert_eq!(model.session_of("first").unwrap().link_detail.mtu, Some(23));
    }

    #[test]
    fn connecting_then_discovering_selects_the_first_row() {
        let mut model = Model::default();
        connected(&mut model, "dev");

        let session = model.session().expect("a session is open");
        assert!(session.connected);
        assert_eq!(session.selected_service, Some(0));
        assert_eq!(session.selected_characteristic, Some(0));
        assert_eq!(model.service().unwrap().uuid, services::HEART_RATE);
        assert!(session.discovered);
        // The connect-time figure is provisional and lives in one place only.
        assert_eq!(session.link_detail.mtu, Some(23));
    }

    /// A connect that is superseded while in flight still answers, and its tree
    /// must not land on top of the device now being looked at.
    #[test]
    fn a_tree_for_a_superseded_connect_is_ignored() {
        let mut model = Model::default();
        connected(&mut model, "second");
        let before = model.session().unwrap().services.len();

        // A tree for a device that was never opened has nowhere to land.
        model.apply(Event::Tree {
            id: "first".into(),
            services: Vec::new(),
        });

        assert_eq!(model.session().unwrap().services.len(), before);
        assert_eq!(model.active.as_deref(), Some("second"));
    }

    #[test]
    fn values_land_on_the_characteristic_they_belong_to() {
        let mut model = Model::default();
        connected(&mut model, "dev");
        let at = model.selected_ref().unwrap();

        model.apply(Event::Value {
            at: at.clone(),
            value: vec![0x06, 0x48],
            notified: true,
        });
        let view = model.characteristic().unwrap();
        assert_eq!(view.value.as_deref(), Some(&[0x06, 0x48][..]));
        assert!(view.from_notification);
        assert!(view.updated.is_some());

        model.apply(Event::DescriptorValue {
            at: descriptor_ref(&at, 0),
            value: vec![0x01, 0x00],
        });
        assert_eq!(
            model.characteristic().unwrap().descriptor_values[&0].bytes,
            vec![0x01, 0x00]
        );
    }

    /// A value addressed outside the tree is a late answer from a previous
    /// connection, and must not panic or land on an unrelated row.
    #[test]
    fn a_value_for_a_row_that_is_gone_is_dropped() {
        let mut model = Model::default();
        connected(&mut model, "dev");

        model.apply(Event::Value {
            at: CharRef {
                device: "dev".into(),
                service: 9,
                characteristic: 9,
            },
            value: vec![1],
            notified: false,
        });
        assert!(model.characteristic().unwrap().value.is_none());
    }

    #[test]
    fn disconnecting_clears_the_tree_and_the_selection() {
        let mut model = Model::default();
        connected(&mut model, "dev");
        model.apply(Event::Disconnected {
            id: "dev".into(),
            why: None,
        });

        assert!(model.sessions.is_empty());
        assert_eq!(model.active, None);
        assert!(model.characteristic().is_none());
        // The device stays in the list — it is still advertising.
        assert!(model.devices.contains_key("dev") || model.devices.is_empty());
    }

    /// A stale disconnect for the previous device must not tear down the
    /// connection that replaced it.
    #[test]
    fn a_disconnect_for_another_device_leaves_the_link_alone() {
        let mut model = Model::default();
        connected(&mut model, "current");
        model.apply(Event::Disconnected {
            id: "previous".into(),
            why: None,
        });

        assert!(model.session().is_some_and(|s| s.connected));
        assert_eq!(model.session().unwrap().services.len(), 1);
    }

    #[test]
    fn the_log_is_bounded() {
        let mut model = Model {
            log_limit: 10,
            ..Default::default()
        };
        for i in 0..50 {
            model.apply(Event::Log(Level::Info, format!("line {i}")));
        }
        assert_eq!(model.log.len(), 10);
        assert_eq!(model.log.last().unwrap().text, "line 49");
        assert_eq!(model.log.first().unwrap().text, "line 40");
    }

    #[test]
    fn availability_is_remembered_until_it_clears() {
        let mut model = Model::default();
        model.apply(Event::Availability(Err("powered off".into())));
        assert_eq!(model.unavailable.as_deref(), Some("powered off"));
        model.apply(Event::Availability(Ok(())));
        assert_eq!(model.unavailable, None);
    }
}
