//! The Bluetooth half, on its own thread.
//!
//! egui is immediate-mode: the UI function runs every frame and must not block,
//! so nothing here is called from it. Instead one thread owns every
//! `webbluetooth` handle and runs a `LocalPool`, and the two halves exchange
//! [`Command`] and [`Event`] over channels. The UI never holds a GATT handle —
//! only ids, UUIDs and bytes — which is what keeps "the radio is busy" and "the
//! window is responsive" independent of each other.
//!
//! There is no async runtime. Long operations are spawned onto the pool as
//! separate tasks so that a `connect()` against an out-of-range device cannot
//! stall the scan, the log, or a read on some other device.

use futures_channel::{mpsc, oneshot};
use futures_util::task::LocalSpawnExt;
use futures_util::StreamExt;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use webbluetooth::uuid::BluetoothUuid;
use webbluetooth::{
    Bluetooth, BluetoothDevice, CharacteristicProperties, ConnectionPriority, Grant, LeScanOptions,
    Phy, RemoteGattCharacteristic, RemoteGattDescriptor,
};

/// Where a characteristic sits, on which device.
///
/// Indices rather than UUIDs, because a UUID does not identify a
/// characteristic: the same one may appear under two services, and a service
/// may legitimately publish two characteristics with identical UUIDs. Reading
/// "the" `0x2A19` would then read whichever one happened to be found first.
///
/// The device is part of the address because more than one can be connected at
/// once. It used to be implicit — there was one connection, so a pair of
/// indices was unambiguous — and every event would have needed a second field
/// beside it to say which device it came from. Carrying it here means a value
/// that arrives late, after the operator has moved to another tab, still lands
/// on the row it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CharRef {
    /// Which device.
    pub device: String,
    /// Index into the device's services, in discovery order.
    pub service: usize,
    /// Index into that service's characteristics.
    pub characteristic: usize,
}

/// Where a descriptor sits, by the same reasoning as [`CharRef`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DescRef {
    /// Which device.
    pub device: String,
    /// Index into the device's services.
    pub service: usize,
    /// Index into that service's characteristics.
    pub characteristic: usize,
    /// Index into that characteristic's descriptors.
    pub descriptor: usize,
}

impl DescRef {
    /// The characteristic this descriptor hangs off.
    pub fn parent(&self) -> CharRef {
        CharRef {
            device: self.device.clone(),
            service: self.service,
            characteristic: self.characteristic,
        }
    }
}

/// What the UI asks the radio to do.
#[derive(Debug, Clone)]
pub enum Command {
    /// Start or stop the scan.
    SetScanning(bool),
    /// Connect and discover everything, replacing any current connection.
    Connect(String),
    /// Drop one connection.
    Disconnect(String),
    /// Re-walk a device's GATT tree.
    Rediscover(String),
    /// Read one characteristic.
    Read(CharRef),
    /// Write one characteristic.
    Write {
        /// Which characteristic.
        at: CharRef,
        /// The bytes to write.
        value: Vec<u8>,
        /// `true` for a write-with-response, `false` for a command.
        with_response: bool,
    },
    /// Subscribe to or unsubscribe from notifications.
    SetNotifying {
        /// Which characteristic.
        at: CharRef,
        /// `true` to subscribe.
        on: bool,
    },
    /// Read one descriptor.
    ReadDescriptor(DescRef),
    /// Write one descriptor.
    WriteDescriptor {
        /// Which descriptor.
        at: DescRef,
        /// The bytes to write.
        value: Vec<u8>,
    },
    /// Ask a device for a fresh RSSI reading.
    ReadRssi(String),
    /// Re-read everything the platform will say about one link.
    ReadLinkDetail(String),
    /// Ask for a larger ATT MTU. The peer decides; the result is reported.
    RequestMtu(String, u16),
    /// Ask a link to move to a different physical layer.
    SetPhy {
        /// Which device.
        device: String,
        /// What this side should transmit on.
        tx: Phy,
        /// What it should receive on.
        rx: Phy,
    },
    /// Ask for a shorter or longer connection interval.
    SetConnectionPriority(String, ConnectionPriority),
    /// Pair with a device, prompting if the platform wants to.
    Pair(String),
    /// Publish a GATT server and start advertising it.
    ///
    /// The other half of the radio. Everything above this point is the central
    /// role — scanning, connecting, reading somebody else's services. This is
    /// the peripheral role: publishing services of your own and letting a
    /// central come and read them, which is how you test the *other* side of
    /// whatever you are building.
    StartServer(ServerSetup),
    /// Withdraw the services and stop advertising.
    StopServer,
    /// Run a recorded sequence against the connected device.
    RunMacro {
        /// Which device to run it against.
        device: String,
        /// What it is called, for the log.
        name: String,
        /// What to do.
        steps: Vec<crate::macros::Step>,
        /// How many times. `0` runs until stopped.
        repeat: u32,
    },
    /// Stop a running sequence at the end of its current step.
    StopMacro,
    /// Run a suite across its targets, asserting as it goes.
    RunSuite(crate::suites::Suite),
    /// Set a published characteristic's value and notify anyone subscribed.
    NotifySubscribers {
        /// Which characteristic, by UUID.
        characteristic: BluetoothUuid,
        /// The new value.
        value: Vec<u8>,
    },
}

/// A GATT server to publish, as the UI describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerSetup {
    /// The name to advertise. Centrals show this.
    pub local_name: String,
    /// The services to publish, in order.
    pub services: Vec<ServiceSetup>,
}

/// One service to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSetup {
    /// Its UUID.
    pub uuid: BluetoothUuid,
    /// Whether to name it in the advertising packet, so a central filtering
    /// for it can find this device.
    pub advertise: bool,
    /// Its characteristics.
    pub characteristics: Vec<CharacteristicSetup>,
}

/// One characteristic to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacteristicSetup {
    /// Its UUID.
    pub uuid: BluetoothUuid,
    /// What it supports.
    pub properties: CharacteristicProperties,
    /// Its initial value.
    pub value: Vec<u8>,
}

/// What was in an advertising packet, flattened for the UI.
#[derive(Debug, Clone, Default)]
pub struct AdvSummary {
    /// The name in this packet, which can differ from the cached GAP name.
    pub local_name: Option<String>,
    /// Advertised transmit power in dBm.
    pub tx_power: Option<i16>,
    /// The GAP appearance code. CoreBluetooth never reports one.
    pub appearance: Option<u16>,
    /// Whether the peripheral said it accepts connections.
    pub connectable: Option<bool>,
    /// Services named in the packet.
    pub service_uuids: Vec<BluetoothUuid>,
    /// Services in the Apple-only overflow area.
    pub overflow_service_uuids: Vec<BluetoothUuid>,
    /// Services the peripheral is soliciting.
    pub solicited_service_uuids: Vec<BluetoothUuid>,
    /// Payload per company identifier.
    pub manufacturer_data: Vec<(u16, Vec<u8>)>,
    /// Payload per service.
    pub service_data: Vec<(BluetoothUuid, Vec<u8>)>,
    /// The packet as it came off the air, where the transport hands it over.
    ///
    /// `None` everywhere except a raw HCI socket. Asked for explicitly by the
    /// scan below, because the bytes carry every company's manufacturer data
    /// and are withheld from any grant not already covering all of it.
    pub raw: Option<Vec<u8>>,
}

/// What the platform will say about a live link beyond its GATT tree.
///
/// Every field is optional and separately absent: these are four different
/// platform calls, any of which can be unsupported, and "this platform does not
/// report the PHY" is not the same fact as "the PHY is 1M". A field left `None`
/// in an event means *not learned*, so a later answer does not erase an earlier
/// one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkDetail {
    /// The negotiated ATT MTU, in bytes.
    pub mtu: Option<u16>,
    /// What the link transmits and receives on.
    pub phy: Option<(Phy, Phy)>,
    /// Whether the device is bonded.
    pub paired: Option<bool>,
    /// Connection interval in 1.25 ms steps, peripheral latency in skipped
    /// events, supervision timeout in 10 ms steps — as the controller reports
    /// them, unconverted, because that is how a datasheet states them.
    pub parameters: Option<(u16, u16, u16)>,
}

/// One characteristic, as the UI needs to know it.
#[derive(Debug, Clone)]
pub struct CharacteristicInfo {
    /// Its UUID.
    pub uuid: BluetoothUuid,
    /// What it says it supports.
    pub properties: CharacteristicProperties,
    /// Its descriptors, in discovery order.
    pub descriptors: Vec<BluetoothUuid>,
}

/// One service, as the UI needs to know it.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    /// Its UUID.
    pub uuid: BluetoothUuid,
    /// Whether it is primary rather than included.
    pub is_primary: bool,
    /// Its characteristics, in discovery order.
    pub characteristics: Vec<CharacteristicInfo>,
}

/// How severe a log line is.
///
/// Ordered, so the pane can show "this and worse". nRF Connect offers six
/// levels; four is enough here, and each earns its place: `Debug` is traffic
/// you only want while chasing something, `Info` is what happened, `Warn` is a
/// refusal the program carried on through, `Error` is a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Individual operations — every read, every notification.
    Debug,
    /// Something happened worth knowing about.
    Info,
    /// The platform refused something, and the program continued.
    Warn,
    /// Something failed.
    Error,
}

impl Level {
    /// How it appears in a saved log, where there is no colour.
    pub fn mark(self) -> &'static str {
        match self {
            Self::Debug => "·",
            Self::Info => " ",
            Self::Warn => "!",
            Self::Error => "x",
        }
    }

    /// Its name, for the level selector.
    pub fn name(self) -> &'static str {
        match self {
            Self::Debug => "Debug",
            Self::Info => "Info",
            Self::Warn => "Warn",
            Self::Error => "Error",
        }
    }

    /// Every level, quietest first.
    pub const ALL: [Self; 4] = [Self::Debug, Self::Info, Self::Warn, Self::Error];
}

/// What the radio tells the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// The adapter's state, as a message when it is not usable.
    Availability(Result<(), String>),
    /// The scan started or stopped.
    Scanning(bool),
    /// A device advertised.
    Sighting {
        /// Per-host identifier.
        id: String,
        /// Its name, if it has one.
        name: Option<String>,
        /// Signal strength in dBm, when the radio reported one.
        rssi: Option<i32>,
        /// Everything else in the packet.
        adv: AdvSummary,
    },
    /// A connection attempt began.
    Connecting(String),
    /// The link is up.
    Connected {
        /// Which device.
        id: String,
        /// The negotiated ATT MTU, where the platform reports one.
        mtu: Option<u16>,
    },
    /// Discovery finished.
    Tree {
        /// Which device.
        id: String,
        /// Everything found on it.
        services: Vec<ServiceInfo>,
    },
    /// A characteristic's value.
    Value {
        /// Which characteristic.
        at: CharRef,
        /// The bytes.
        value: Vec<u8>,
        /// `true` if this arrived as a notification rather than a read.
        notified: bool,
    },
    /// A descriptor's value.
    DescriptorValue {
        /// Which descriptor.
        at: DescRef,
        /// The bytes.
        value: Vec<u8>,
    },
    /// A subscription started or stopped.
    Notifying {
        /// Which characteristic.
        at: CharRef,
        /// Whether it is now subscribed.
        on: bool,
    },
    /// A write completed.
    Wrote {
        /// Which characteristic.
        at: CharRef,
        /// How many bytes went out.
        len: usize,
    },
    /// The link went down.
    Disconnected {
        /// Which device.
        id: String,
        /// Why, if the platform said.
        why: Option<String>,
    },
    /// A fresh signal strength reading for the connected device.
    Rssi {
        /// Which device.
        id: String,
        /// dBm.
        rssi: i32,
    },
    /// Something new learned about a link. Absent fields are unchanged.
    LinkDetail {
        /// Which device.
        device: String,
        /// What was learned.
        detail: LinkDetail,
    },
    /// Whether a sequence is running.
    MacroRunning(bool),
    /// A suite finished, or was stopped.
    SuiteResults(crate::suites::Results),
    /// Whether a server of ours is published and advertising.
    ServerState {
        /// `true` once the services are published and advertising started.
        running: bool,
        /// Why it is not, if it failed.
        why: Option<String>,
    },
    /// Something worth showing in the log pane.
    Log(Level, String),
}

/// The UI's end of the engine.
pub struct Handle {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<Event>,
    /// The other end of `commands`, kept only on a detached handle so a test
    /// can read back what the UI asked for.
    #[cfg(test)]
    sent: Option<mpsc::UnboundedReceiver<Command>>,
}

impl Handle {
    /// Ask the engine to do something. Dropped silently if it has stopped —
    /// which only happens as the process exits.
    pub fn send(&self, command: Command) {
        let _ = self.commands.unbounded_send(command);
    }

    /// Everything that has arrived since the last frame.
    ///
    /// Never blocks: the UI calls this at the top of a frame and is not allowed
    /// to wait for the radio.
    pub fn drain(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }
}

impl Handle {
    /// Everything the UI has asked for since the last call.
    ///
    /// Only useful on a detached handle, where nothing is consuming the other
    /// end: it is how a test asks "did that button send the command it should
    /// have" without a radio in the way.
    #[cfg(test)]
    pub(crate) fn drain_commands_for_test(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        if let Some(sent) = self.sent.as_mut() {
            while let Ok(command) = sent.try_recv() {
                out.push(command);
            }
        }
        out
    }

    /// A handle with no engine behind it.
    ///
    /// Commands go nowhere and no events ever arrive. For rendering the
    /// interface in a test: `start` spawns a thread that opens the radio and
    /// begins scanning, which makes a snapshot depend on what is in the room.
    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        let (commands, sent) = mpsc::unbounded();
        let (_, events) = mpsc::unbounded();
        Self {
            commands,
            events,
            sent: Some(sent),
        }
    }
}

/// Start the engine thread.
///
/// `ctx` is woken on every event, so the window repaints when something
/// arrives rather than polling at a fixed rate.
pub fn start(ctx: egui::Context) -> Handle {
    let (command_tx, command_rx) = mpsc::unbounded();
    let (event_tx, event_rx) = mpsc::unbounded();

    std::thread::Builder::new()
        .name("wbe-engine".into())
        .spawn(move || {
            let mut pool = futures_executor::LocalPool::new();
            let spawner = pool.spawner();
            pool.run_until(run(command_rx, event_tx, ctx, spawner));
        })
        .expect("spawning the engine thread");

    Handle {
        commands: command_tx,
        events: event_rx,
        #[cfg(test)]
        sent: None,
    }
}

/// A connected device and every handle discovered on it.
///
/// Kept here rather than in the UI because these are live GATT handles: they
/// carry a generation that the backend invalidates when the link drops or the
/// peripheral re-advertises a different service set.
struct Connection {
    id: String,
    device: BluetoothDevice,
    services: Vec<Vec<(RemoteGattCharacteristic, Vec<RemoteGattDescriptor>)>>,
}

impl Connection {
    /// Where a characteristic with this UUID is, and a handle to it.
    ///
    /// The first match, walking services in discovery order. A device may
    /// publish the same UUID twice; a macro naming only the UUID cannot say
    /// which, and the first is the least surprising answer.
    fn find(&self, uuid: &BluetoothUuid) -> Option<(CharRef, RemoteGattCharacteristic)> {
        for (service, characteristics) in self.services.iter().enumerate() {
            for (characteristic, (handle, _)) in characteristics.iter().enumerate() {
                if handle.uuid() == uuid {
                    return Some((
                        CharRef {
                            device: self.id.clone(),
                            service,
                            characteristic,
                        },
                        handle.clone(),
                    ));
                }
            }
        }
        None
    }

    fn characteristic(&self, at: &CharRef) -> Option<&RemoteGattCharacteristic> {
        self.services
            .get(at.service)?
            .get(at.characteristic)
            .map(|(ch, _)| ch)
    }

    fn descriptor(&self, at: &DescRef) -> Option<&RemoteGattDescriptor> {
        self.services
            .get(at.service)?
            .get(at.characteristic)?
            .1
            .get(at.descriptor)
    }
}

/// Everything the engine's tasks share. One thread, so `Rc<RefCell<_>>` is the
/// whole story — no locks and no `Send` bounds.
struct State {
    /// Every open connection, by device id.
    ///
    /// nRF Connect gives each one a closable tab; this is the model underneath
    /// that. One device at a time was the original shape, and the thing it made
    /// impossible was the common case of comparing two units on a bench, or
    /// watching a central and a peripheral at once.
    connections: HashMap<String, Connection>,
    /// Dropping the sender ends the corresponding task's stream.
    scan: Option<oneshot::Sender<()>>,
    /// Whether a macro is running. Cleared to stop it.
    macro_running: bool,
    /// Whether scanning was *asked for*, which is not the same as whether it is
    /// running.
    ///
    /// Starting a scan with the radio off fails once and stays failed: nothing
    /// retries, so turning Bluetooth on after launch left the window sitting
    /// there reporting nothing with no indication that a click would fix it.
    /// Keeping the intent means the availability watcher can act on it.
    scan_wanted: bool,
    subscriptions: HashMap<CharRef, oneshot::Sender<()>>,
    /// The peripheral, once something of ours is published.
    ///
    /// Held so it stays alive: dropping a `Peripheral` withdraws its services
    /// and stops advertising, which is exactly what `StopServer` wants and
    /// exactly what must not happen by accident.
    #[cfg(peripheral_role)]
    server: Option<ServerState>,
}

/// A published server of ours, and what it published.
#[cfg(peripheral_role)]
struct ServerState {
    peripheral: webbluetooth::peripheral::Peripheral,
    /// Published characteristics by UUID, so a notify can find one.
    characteristics: HashMap<BluetoothUuid, webbluetooth::peripheral::PublishedCharacteristic>,
    /// What each characteristic currently holds.
    ///
    /// Kept here rather than in the platform because a characteristic that can
    /// notify must not be published with a fixed value — CoreBluetooth serves a
    /// fixed value itself and never forwards the read — so answering a read
    /// means remembering the value ourselves.
    values: HashMap<BluetoothUuid, Vec<u8>>,
    /// Ends the task pumping requests from centrals.
    stop: oneshot::Sender<()>,
}

/// The engine, shared by every spawned task.
#[derive(Clone)]
struct Engine {
    bluetooth: Bluetooth,
    events: mpsc::UnboundedSender<Event>,
    ctx: egui::Context,
    state: Rc<RefCell<State>>,
    spawner: futures_executor::LocalSpawner,
}

impl Engine {
    fn emit(&self, event: Event) {
        let _ = self.events.unbounded_send(event);
        // Immediate mode only redraws when asked. Without this the window sits
        // on a stale frame until the mouse moves over it.
        self.ctx.request_repaint();
    }

    fn log(&self, text: impl Into<String>) {
        self.emit(Event::Log(Level::Info, text.into()));
    }

    /// Per-operation traffic: reads, writes, notifications. Filtered out of
    /// the pane by default, because a subscription at 10 Hz buries everything.
    fn debug(&self, text: impl Into<String>) {
        self.emit(Event::Log(Level::Debug, text.into()));
    }

    /// The platform declined something and the program carried on.
    fn warn(&self, text: impl Into<String>) {
        self.emit(Event::Log(Level::Warn, text.into()));
    }

    fn fail(&self, text: impl Into<String>) {
        self.emit(Event::Log(Level::Error, text.into()));
    }

    /// Run `task` on the engine thread. Used for anything that awaits the
    /// radio, so the command loop stays free to accept the next command.
    fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        if self.spawner.spawn_local(task).is_err() {
            // Only reachable once the pool is shutting down, i.e. at exit.
        }
    }
}

async fn run(
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
    ctx: egui::Context,
    spawner: futures_executor::LocalSpawner,
) {
    let engine = Engine {
        bluetooth: Bluetooth::new(),
        events,
        ctx,
        state: Rc::new(RefCell::new(State {
            connections: HashMap::new(),
            scan: None,
            macro_running: false,
            scan_wanted: false,
            subscriptions: HashMap::new(),
            #[cfg(peripheral_role)]
            server: None,
        })),
        spawner,
    };

    match engine.bluetooth.availability().await {
        Ok(()) => engine.emit(Event::Availability(Ok(()))),
        Err(why) => {
            engine.emit(Event::Availability(Err(why.to_string())));
            engine.fail(format!("Bluetooth unavailable: {why}"));
        }
    }

    // The adapter can come up, go away, or be authorised after launch. Watching
    // it means the UI explains itself instead of just finding nothing.
    {
        let engine = engine.clone();
        engine.spawn({
            let engine = engine.clone();
            async move {
                let mut changes = engine.bluetooth.watch_availability();
                while let Some(state) = changes.next().await {
                    let usable = state.is_ok();
                    engine.emit(Event::Availability(state.map_err(|why| why.to_string())));

                    // The radio just became usable and a scan was asked for
                    // but is not running — which is what happens when
                    // Bluetooth is switched on after launch.
                    let resume = {
                        let state = engine.state.borrow();
                        usable && state.scan_wanted && state.scan.is_none()
                    };
                    if resume {
                        engine.log("radio available, resuming scan");
                        set_scanning(&engine, true);
                    }
                }
            }
        });
    }

    while let Some(command) = commands.next().await {
        dispatch(&engine, command);
    }
}

fn dispatch(engine: &Engine, command: Command) {
    match command {
        Command::SetScanning(on) => set_scanning(engine, on),

        Command::Connect(id) => {
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move { connect(&engine, id).await }
            });
        }

        Command::Disconnect(id) => disconnect(engine, &id),

        Command::Rediscover(id) => {
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move { discover(&engine, id).await }
            });
        }

        Command::Read(at) => {
            // Clone the handle out before awaiting: holding a `RefCell` borrow
            // across an await would panic the moment another task touched the
            // state.
            let Some(ch) = characteristic_at(engine, &at) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    match ch.read_value().await {
                        Ok(value) => {
                            engine.debug(format!(
                                "read {} — {} byte{}",
                                crate::names::short(ch.uuid()),
                                value.len(),
                                if value.len() == 1 { "" } else { "s" }
                            ));
                            engine.emit(Event::Value {
                                at: at.clone(),
                                value,
                                notified: false,
                            });
                        }
                        Err(why) => engine.fail(format!(
                            "read {} failed: {why}",
                            crate::names::short(ch.uuid())
                        )),
                    }
                }
            });
        }

        Command::Write {
            at,
            value,
            with_response,
        } => {
            let Some(ch) = characteristic_at(engine, &at) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    let len = value.len();
                    let result = if with_response {
                        ch.write_value_with_response(&value).await
                    } else {
                        ch.write_value_without_response(&value).await
                    };
                    let how = if with_response {
                        "write"
                    } else {
                        "write (no response)"
                    };
                    match result {
                        Ok(()) => {
                            engine.debug(format!(
                                "{how} {} — {len} byte{}",
                                crate::names::short(ch.uuid()),
                                if len == 1 { "" } else { "s" }
                            ));
                            engine.emit(Event::Wrote {
                                at: at.clone(),
                                len,
                            });
                        }
                        Err(why) => engine.fail(format!(
                            "{how} {} failed: {why}",
                            crate::names::short(ch.uuid())
                        )),
                    }
                }
            });
        }

        Command::SetNotifying { at, on } => set_notifying(engine, at, on),

        Command::ReadDescriptor(at) => {
            let Some(descriptor) = descriptor_at(engine, &at) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    match descriptor.read_value().await {
                        Ok(value) => engine.emit(Event::DescriptorValue {
                            at: at.clone(),
                            value,
                        }),
                        Err(why) => engine.fail(format!(
                            "read descriptor {} failed: {why}",
                            crate::names::short(descriptor.uuid())
                        )),
                    }
                }
            });
        }

        Command::WriteDescriptor { at, value } => {
            let Some(descriptor) = descriptor_at(engine, &at) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    let uuid = crate::names::short(descriptor.uuid());
                    match descriptor.write_value(&value).await {
                        Ok(()) => {
                            engine.log(format!("wrote descriptor {uuid}"));
                            engine.emit(Event::DescriptorValue {
                                at: at.clone(),
                                value,
                            });
                        }
                        Err(why) => engine.fail(format!("write descriptor {uuid} failed: {why}")),
                    }
                }
            });
        }

        Command::RunMacro {
            device,
            name,
            steps,
            repeat,
        } => run_macro(engine, device, name, steps, repeat),

        Command::RunSuite(suite) => run_suite(engine, suite),

        Command::StopMacro => {
            // Cleared by the runner, which checks it between steps.
            engine.state.borrow_mut().macro_running = false;
        }

        Command::StartServer(setup) => start_server(engine, setup),

        Command::StopServer => stop_server(engine),

        Command::NotifySubscribers {
            characteristic,
            value,
        } => notify_subscribers(engine, &characteristic, &value),

        Command::ReadLinkDetail(id) => {
            let Some(device) = connected_device(engine, &id) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    // Four independent calls, each of which may simply not be
                    // available. Whatever answers is reported; whatever does
                    // not is left unknown rather than guessed at.
                    let detail = LinkDetail {
                        mtu: device.mtu().await.ok(),
                        phy: device.phy().await.ok().map(|phy| (phy.tx, phy.rx)),
                        paired: device.is_paired().await.ok(),
                        parameters: device
                            .connection_parameters()
                            .await
                            .ok()
                            .map(|p| (p.interval, p.latency, p.timeout)),
                    };
                    engine.emit(Event::LinkDetail { device: id, detail });
                }
            });
        }

        Command::RequestMtu(id, wanted) => {
            let Some(device) = connected_device(engine, &id) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    match device.request_mtu(wanted).await {
                        Ok(got) => {
                            engine.log(format!("asked for MTU {wanted}, link is at {got}"));
                            engine.emit(Event::LinkDetail {
                                device: id,
                                detail: LinkDetail {
                                    mtu: Some(got),
                                    ..LinkDetail::default()
                                },
                            });
                        }
                        // A platform with no such API is not a failure of this
                        // program and should not look like one.
                        Err(why) => engine.warn(format!("MTU {wanted} refused: {why}")),
                    }
                }
            });
        }

        Command::SetPhy { device: id, tx, rx } => {
            let Some(device) = connected_device(engine, &id) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    match device.set_preferred_phy(tx, rx).await {
                        // A peer that does not have the PHY asked for leaves
                        // the link where it was and reports no error, so the
                        // answer is what to show rather than the request.
                        Ok(phy) => {
                            engine.log(format!(
                                "asked for {tx:?}/{rx:?}, link is on {:?}/{:?}",
                                phy.tx, phy.rx
                            ));
                            engine.emit(Event::LinkDetail {
                                device: id,
                                detail: LinkDetail {
                                    phy: Some((phy.tx, phy.rx)),
                                    ..LinkDetail::default()
                                },
                            });
                        }
                        Err(why) => engine.warn(format!("PHY change refused: {why}")),
                    }
                }
            });
        }

        Command::SetConnectionPriority(id, priority) => {
            let Some(device) = connected_device(engine, &id) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    match device.request_connection_priority(priority).await {
                        Ok(()) => {
                            engine.log(format!("connection priority set to {priority:?}"));
                            // The controller re-negotiates, so the numbers that
                            // were on screen are now stale.
                            let parameters = device
                                .connection_parameters()
                                .await
                                .ok()
                                .map(|p| (p.interval, p.latency, p.timeout));
                            engine.emit(Event::LinkDetail {
                                device: id,
                                detail: LinkDetail {
                                    parameters,
                                    ..LinkDetail::default()
                                },
                            });
                        }
                        Err(why) => engine.warn(format!("connection priority refused: {why}")),
                    }
                }
            });
        }

        Command::Pair(id) => {
            let Some(device) = connected_device(engine, &id) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    match device.pair().await {
                        Ok(state) => {
                            engine.log(format!("pairing: {state:?}"));
                            engine.emit(Event::LinkDetail {
                                device: id,
                                detail: LinkDetail {
                                    paired: device.is_paired().await.ok(),
                                    ..LinkDetail::default()
                                },
                            });
                        }
                        Err(why) => engine.fail(format!("pairing failed: {why}")),
                    }
                }
            });
        }

        Command::ReadRssi(id) => {
            let Some(device) = connected_device(engine, &id) else {
                return;
            };
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    if let Ok(rssi) = device.rssi().await {
                        engine.emit(Event::Rssi {
                            id: device.id().to_owned(),
                            rssi,
                        });
                    }
                }
            });
        }
    }
}

/// Run a recorded sequence, one step at a time.
///
/// Steps name characteristics by UUID rather than by position, so a macro
/// recorded against one device runs against another with the same services —
/// and survives that device being rediscovered, which reorders the tree.
///
/// A step that cannot be resolved or that fails stops the run. Carrying on
/// after a failed unlock and writing the rest of the sequence to a device that
/// did not accept it is worse than stopping.
fn run_macro(
    engine: &Engine,
    device: String,
    name: String,
    steps: Vec<crate::macros::Step>,
    repeat: u32,
) {
    use crate::macros::Step;

    if !engine.state.borrow().connections.contains_key(&device) {
        engine.warn(format!("{name}: {device} is not connected"));
        return;
    }
    if engine.state.borrow().macro_running {
        engine.warn("a sequence is already running");
        return;
    }
    engine.state.borrow_mut().macro_running = true;
    engine.emit(Event::MacroRunning(true));

    let engine = engine.clone();
    engine.spawn({
        let engine = engine.clone();
        async move {
            let finish = |engine: &Engine| {
                engine.state.borrow_mut().macro_running = false;
                engine.emit(Event::MacroRunning(false));
            };

            engine.log(match repeat {
                0 => format!("running {name} — {} step(s), looping", steps.len()),
                1 => format!("running {name} — {} step(s)", steps.len()),
                n => format!("running {name} — {} step(s), {n} times", steps.len()),
            });

            let mut pass = 0_u32;
            loop {
                pass += 1;
                if repeat != 0 && pass > repeat {
                    break;
                }
                if !engine.state.borrow().macro_running {
                    engine.log(format!("{name} stopped"));
                    finish(&engine);
                    return;
                }
                if repeat != 1 {
                    engine.debug(format!("{name}: pass {pass}"));
                }

                for (index, step) in steps.iter().enumerate() {
                    if !engine.state.borrow().macro_running {
                        engine.log(format!("{name} stopped"));
                        finish(&engine);
                        return;
                    }
                    let position = format!("{name} step {}", index + 1);

                    // Resolved per step, not once up front: a macro may legitimately
                    // rediscover the device partway through.
                    let resolve = |uuid: &BluetoothUuid| {
                        engine
                            .state
                            .borrow()
                            .connections
                            .get(&device)
                            .and_then(|connection| connection.find(uuid))
                    };

                    match step {
                        Step::Delay(ms) => {
                            engine.debug(format!("{position}: waiting {ms} ms"));
                            webbluetooth::timer::sleep(Duration::from_millis(*ms)).await;
                        }
                        Step::Read(uuid) => {
                            let Some((at, characteristic)) = resolve(uuid) else {
                                engine.fail(format!(
                                    "{position}: this device has no {}",
                                    crate::names::short(uuid)
                                ));
                                finish(&engine);
                                return;
                            };
                            match characteristic.read_value().await {
                                Ok(value) => {
                                    engine
                                        .debug(format!("{position}: read {} byte(s)", value.len()));
                                    engine.emit(Event::Value {
                                        at,
                                        value,
                                        notified: false,
                                    });
                                }
                                Err(why) => {
                                    engine.fail(format!("{position}: read failed: {why}"));
                                    finish(&engine);
                                    return;
                                }
                            }
                        }
                        Step::Write {
                            characteristic: uuid,
                            value,
                            with_response,
                        } => {
                            let Some((at, characteristic)) = resolve(uuid) else {
                                engine.fail(format!(
                                    "{position}: this device has no {}",
                                    crate::names::short(uuid)
                                ));
                                finish(&engine);
                                return;
                            };
                            let result = if *with_response {
                                characteristic.write_value_with_response(value).await
                            } else {
                                characteristic.write_value_without_response(value).await
                            };
                            match result {
                                Ok(()) => {
                                    engine.debug(format!(
                                        "{position}: wrote {} byte(s)",
                                        value.len()
                                    ));
                                    engine.emit(Event::Wrote {
                                        at,
                                        len: value.len(),
                                    });
                                }
                                Err(why) => {
                                    engine.fail(format!("{position}: write failed: {why}"));
                                    finish(&engine);
                                    return;
                                }
                            }
                        }
                        Step::WaitFor {
                            characteristic: uuid,
                            timeout,
                            expect,
                        } => {
                            let Some((_, characteristic)) = resolve(uuid) else {
                                engine.fail(format!(
                                    "{position}: this device has no {}",
                                    crate::names::short(uuid)
                                ));
                                finish(&engine);
                                return;
                            };
                            // A subscription of its own for the wait, dropped when
                            // it ends. The library's scan hub allows more than one
                            // watcher, so this does not disturb a subscription the
                            // operator already started.
                            let notifications = match characteristic.start_notifications().await {
                                Ok(notifications) => notifications,
                                Err(why) => {
                                    engine.fail(format!("{position}: cannot listen: {why}"));
                                    finish(&engine);
                                    return;
                                }
                            };
                            engine.debug(format!("{position}: waiting up to {timeout} ms"));

                            let wanted = expect.clone();
                            let matching = async {
                                let mut notifications = notifications;
                                while let Some(value) = notifications.next().await {
                                    // A prefix, not an exact match: a status
                                    // notification usually carries an opcode and
                                    // then whatever it has to say.
                                    if wanted.is_empty() || value.starts_with(&wanted) {
                                        return Some(value);
                                    }
                                }
                                None
                            };

                            match webbluetooth::timer::timeout(
                                Duration::from_millis(*timeout),
                                matching,
                            )
                            .await
                            {
                                Ok(Some(value)) => {
                                    engine.debug(format!("{position}: got {} byte(s)", value.len()))
                                }
                                Ok(None) => {
                                    engine
                                        .fail(format!("{position}: the link ended while waiting"));
                                    finish(&engine);
                                    return;
                                }
                                Err(_) => {
                                    engine.fail(format!(
                                        "{position}: nothing arrived in {timeout} ms"
                                    ));
                                    finish(&engine);
                                    return;
                                }
                            }
                        }
                        Step::Subscribe {
                            characteristic: uuid,
                            on,
                        } => {
                            let Some((at, _)) = resolve(uuid) else {
                                engine.fail(format!(
                                    "{position}: this device has no {}",
                                    crate::names::short(uuid)
                                ));
                                finish(&engine);
                                return;
                            };
                            set_notifying(&engine, at, *on);
                        }
                    }
                }
            }

            engine.log(format!("{name} finished"));
            finish(&engine);
        }
    });
}

/// Run a suite: every test, on every target, with assertions.
///
/// Reuses the macro runner's resolution — characteristics named by UUID, so a
/// suite written against one device runs against another with the same
/// services, which is the entire point of having targets at all.
///
/// A failing test stops *that* test and moves to the next one. A failing macro
/// stops everything, because a macro is one sequence and half of it is worse
/// than none; a suite is many independent questions and the answers to the rest
/// are still worth having.
fn run_suite(engine: &Engine, suite: crate::suites::Suite) {
    use crate::macros::Step;
    use crate::suites::{Operation, Outcome, Results};

    if engine.state.borrow().macro_running {
        engine.warn("a sequence is already running");
        return;
    }

    // Empty targets means whatever is connected, which is the convenient
    // default for a suite being written rather than replayed.
    let targets: Vec<String> = if suite.targets.is_empty() {
        engine.state.borrow().connections.keys().cloned().collect()
    } else {
        suite
            .targets
            .iter()
            .filter(|id| engine.state.borrow().connections.contains_key(*id))
            .cloned()
            .collect()
    };
    if targets.is_empty() {
        engine.warn(format!("{}: none of its targets are connected", suite.name));
        return;
    }

    engine.state.borrow_mut().macro_running = true;
    engine.emit(Event::MacroRunning(true));

    let engine = engine.clone();
    engine.spawn({
        let engine = engine.clone();
        async move {
            let mut results = Results {
                suite: suite.name.clone(),
                outcomes: Vec::new(),
                finished: false,
            };
            engine.log(format!(
                "running suite {} — {} test(s) on {} device(s)",
                suite.name,
                suite.tests.len(),
                targets.len()
            ));

            'outer: for device in &targets {
                for test in &suite.tests {
                    if !engine.state.borrow().macro_running {
                        break 'outer;
                    }
                    let started = std::time::Instant::now();
                    let failure = run_test(&engine, device, &test.operations).await;
                    let outcome = Outcome {
                        device: device.clone(),
                        test: test.name.clone(),
                        failure,
                        took: started.elapsed().as_millis() as u64,
                    };
                    if let Some(why) = &outcome.failure {
                        engine.warn(format!("FAIL {} on {device}: {why}", test.name));
                    } else {
                        engine.debug(format!("PASS {} on {device}", test.name));
                    }
                    results.outcomes.push(outcome);
                }
            }

            results.finished = engine.state.borrow().macro_running;
            engine.log(format!("{}: {}", suite.name, results.summary()));
            engine.emit(Event::SuiteResults(results));
            engine.state.borrow_mut().macro_running = false;
            engine.emit(Event::MacroRunning(false));
        }
    });

    /// Run one test's operations. `None` if every one succeeded.
    async fn run_test(engine: &Engine, device: &str, operations: &[Operation]) -> Option<String> {
        for operation in operations {
            let resolve = |uuid: &BluetoothUuid| {
                engine
                    .state
                    .borrow()
                    .connections
                    .get(device)
                    .and_then(|connection| connection.find(uuid))
            };

            match operation {
                Operation::Do(Step::Delay(ms)) => {
                    webbluetooth::timer::sleep(Duration::from_millis(*ms)).await;
                }
                Operation::Do(Step::Read(uuid)) => {
                    let Some((_, characteristic)) = resolve(uuid) else {
                        return Some(format!("no {}", crate::names::short(uuid)));
                    };
                    if let Err(why) = characteristic.read_value().await {
                        return Some(format!("read failed: {why}"));
                    }
                }
                Operation::Do(Step::Write {
                    characteristic: uuid,
                    value,
                    with_response,
                }) => {
                    let Some((_, characteristic)) = resolve(uuid) else {
                        return Some(format!("no {}", crate::names::short(uuid)));
                    };
                    let result = if *with_response {
                        characteristic.write_value_with_response(value).await
                    } else {
                        characteristic.write_value_without_response(value).await
                    };
                    if let Err(why) = result {
                        return Some(format!("write failed: {why}"));
                    }
                }
                Operation::Do(Step::Subscribe { .. }) | Operation::Do(Step::WaitFor { .. }) => {
                    // Subscriptions and waits belong to macros, where there is
                    // somebody watching. A suite that needs one should use an
                    // expectation instead.
                }
                Operation::Expect {
                    characteristic: uuid,
                    prefix,
                } => {
                    let Some((_, characteristic)) = resolve(uuid) else {
                        return Some(format!("no {}", crate::names::short(uuid)));
                    };
                    let value = match characteristic.read_value().await {
                        Ok(value) => value,
                        Err(why) => return Some(format!("read failed: {why}")),
                    };
                    if !prefix.is_empty() && !value.starts_with(prefix) {
                        return Some(format!(
                            "got {}, expected it to start {}",
                            crate::format::hex(&value),
                            crate::format::hex(prefix)
                        ));
                    }
                }
            }
        }
        None
    }
}

/// Publish a server and start advertising it.
///
/// Absent on platforms with no peripheral role — watchOS, tvOS and visionOS
/// have the CoreBluetooth initialisers marked `API_UNAVAILABLE`, and no Linux
/// backend implements it — where this reports why rather than pretending.
#[cfg(peripheral_role)]
fn start_server(engine: &Engine, setup: ServerSetup) {
    use webbluetooth::peripheral::{Advertising, Characteristic, Peripheral, Service};

    stop_server(engine);
    let engine = engine.clone();
    engine.spawn({
        let engine = engine.clone();
        async move {
            let (peripheral, requests) = Peripheral::new();
            if let Err(why) = peripheral.availability().await {
                engine.fail(format!("cannot publish: {why}"));
                engine.emit(Event::ServerState {
                    running: false,
                    why: Some(why.to_string()),
                });
                return;
            }

            let mut published = HashMap::new();
            let mut advertised = Vec::new();
            for service in &setup.services {
                let mut definition = match Service::new(service.uuid) {
                    Ok(definition) => definition,
                    Err(why) => {
                        engine.fail(format!("service {}: {why}", service.uuid));
                        continue;
                    }
                };
                for characteristic in &service.characteristics {
                    let Ok(mut built) = Characteristic::new(characteristic.uuid) else {
                        engine.fail(format!(
                            "characteristic {} is not usable",
                            characteristic.uuid
                        ));
                        continue;
                    };
                    let p = characteristic.properties;
                    if p.read() {
                        built = built.read();
                    }
                    if p.write() {
                        built = built.write();
                    }
                    if p.write_without_response() {
                        built = built.write_without_response();
                    }
                    if p.notify() {
                        built = built.notify();
                    }
                    if p.indicate() {
                        built = built.indicate();
                    }
                    // A fixed value is served by the platform without ever
                    // reaching us; a characteristic that can change must not
                    // have one, or reads never arrive as requests.
                    if !characteristic.value.is_empty()
                        && !p.notify()
                        && !p.indicate()
                        && !p.write()
                    {
                        built = built.value(characteristic.value.clone());
                    }
                    definition = definition.characteristic(built);
                }

                match peripheral.publish(definition).await {
                    Ok(handle) => {
                        for characteristic in &service.characteristics {
                            if let Some(found) = handle.characteristic(characteristic.uuid) {
                                published.insert(characteristic.uuid, found);
                            }
                        }
                        engine.log(format!("published service {}", service.uuid));
                        if service.advertise {
                            advertised.push(service.uuid);
                        }
                    }
                    Err(why) => engine.fail(format!("publishing {} failed: {why}", service.uuid)),
                }
            }

            let mut advertising = Advertising::new();
            if !setup.local_name.trim().is_empty() {
                advertising = advertising.local_name(setup.local_name.trim());
            }
            for uuid in advertised {
                advertising = advertising
                    .service(uuid)
                    .unwrap_or_else(|_| Advertising::new());
            }
            if let Err(why) = peripheral.start_advertising(advertising).await {
                engine.fail(format!("advertising failed: {why}"));
                engine.emit(Event::ServerState {
                    running: false,
                    why: Some(why.to_string()),
                });
                return;
            }

            engine.log("advertising");
            engine.emit(Event::ServerState {
                running: true,
                why: None,
            });

            let (stop_tx, stop_rx) = oneshot::channel();
            let values = setup
                .services
                .iter()
                .flat_map(|service| &service.characteristics)
                .map(|characteristic| (characteristic.uuid, characteristic.value.clone()))
                .collect();
            engine.state.borrow_mut().server = Some(ServerState {
                peripheral,
                characteristics: published,
                values,
                stop: stop_tx,
            });

            // Everything a central does to our server, reported into the log —
            // which is the whole point of publishing one from a browser.
            let pump = engine.clone();
            pump.spawn({
                let engine = pump.clone();
                async move {
                    let mut requests = requests.take_until(stop_rx);
                    while let Some(request) = requests.next().await {
                        report_server_request(&engine, request);
                    }
                }
            });
        }
    });
}

/// Log what a central asked of us, and answer the ones that need answering.
#[cfg(peripheral_role)]
fn report_server_request(engine: &Engine, request: webbluetooth::peripheral::Request) {
    use webbluetooth::peripheral::Request;

    match request {
        Request::Read(read) => {
            let uuid = *read.characteristic();
            // Answered with whatever was last set, because a browser's server
            // is a thing to poke at rather than a thing that computes.
            let value = engine
                .state
                .borrow()
                .server
                .as_ref()
                .and_then(|server| server.values.get(&uuid).cloned())
                .unwrap_or_default();
            engine.debug(format!(
                "central read {} — answering {} byte{}",
                crate::names::short(&uuid),
                value.len(),
                if value.len() == 1 { "" } else { "s" }
            ));
            let _ = read.respond(&value);
        }
        Request::Write(write) => {
            for one in write.writes() {
                engine.log(format!(
                    "central wrote {} — {}",
                    crate::names::short(&one.characteristic),
                    crate::format::hex(&one.value)
                ));
            }
            write.accept();
        }
        Request::Subscribed { characteristic, .. } => {
            engine.log(format!(
                "central subscribed to {}",
                crate::names::short(&characteristic)
            ));
        }
        Request::Unsubscribed { characteristic, .. } => {
            engine.log(format!(
                "central unsubscribed from {}",
                crate::names::short(&characteristic)
            ));
        }
        Request::ReadyToNotify => {}
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

/// Withdraw whatever is published.
#[cfg(peripheral_role)]
fn stop_server(engine: &Engine) {
    let previous = engine.state.borrow_mut().server.take();
    if let Some(server) = previous {
        server.peripheral.unpublish_all();
        // Ends the request pump.
        drop(server.stop);
        engine.log("server withdrawn");
        engine.emit(Event::ServerState {
            running: false,
            why: None,
        });
    }
}

/// Set a published characteristic's value and tell anyone subscribed.
#[cfg(peripheral_role)]
fn notify_subscribers(engine: &Engine, characteristic: &BluetoothUuid, value: &[u8]) {
    let mut state = engine.state.borrow_mut();
    let Some(server) = state.server.as_mut() else {
        return;
    };
    server.values.insert(*characteristic, value.to_vec());
    let Some(published) = server.characteristics.get(characteristic) else {
        return;
    };
    // `try_notify` rather than `notify`: the async one waits for queue space,
    // which would mean holding a borrow of the shared state across an await.
    // `false` means the platform's queue is full, which a `ReadyToNotify`
    // request will announce the end of — and for a browser's own server,
    // dropping a value nobody is subscribed to costs nothing.
    let sent = published.try_notify(value);
    drop(state);
    if sent {
        engine.debug(format!(
            "notified {} — {} byte{}",
            crate::names::short(characteristic),
            value.len(),
            if value.len() == 1 { "" } else { "s" }
        ));
    } else {
        engine.warn(format!(
            "notify of {} was queued rather than sent; the radio is busy",
            crate::names::short(characteristic)
        ));
    }
}

/// The peripheral role is not available on this platform.
#[cfg(not(peripheral_role))]
fn start_server(engine: &Engine, _setup: ServerSetup) {
    engine.fail("this platform has no peripheral role, so nothing can be published");
    engine.emit(Event::ServerState {
        running: false,
        why: Some("no peripheral role on this platform".into()),
    });
}

#[cfg(not(peripheral_role))]
fn stop_server(_engine: &Engine) {}

#[cfg(not(peripheral_role))]
fn notify_subscribers(_engine: &Engine, _characteristic: &BluetoothUuid, _value: &[u8]) {}

/// A connected device, cloned out so a task can await on it.
///
/// Cloning rather than borrowing is the whole point: a `RefCell` borrow held
/// across an `.await` panics the moment another task on the same pool touches
/// the state.
fn connected_device(engine: &Engine, id: &str) -> Option<BluetoothDevice> {
    engine
        .state
        .borrow()
        .connections
        .get(id)
        .map(|c| c.device.clone())
}

/// A characteristic handle, cloned out for the same reason.
fn characteristic_at(engine: &Engine, at: &CharRef) -> Option<RemoteGattCharacteristic> {
    engine
        .state
        .borrow()
        .connections
        .get(&at.device)
        .and_then(|connection| connection.characteristic(at).cloned())
}

/// A descriptor handle, ditto.
fn descriptor_at(engine: &Engine, at: &DescRef) -> Option<RemoteGattDescriptor> {
    engine
        .state
        .borrow()
        .connections
        .get(&at.device)
        .and_then(|connection| connection.descriptor(at).cloned())
}

fn set_scanning(engine: &Engine, on: bool) {
    engine.state.borrow_mut().scan_wanted = on;
    if !on {
        // Dropping the cancel sender ends the scan task's stream, which drops
        // the `LeScan`, which stops the radio.
        engine.state.borrow_mut().scan = None;
        engine.emit(Event::Scanning(false));
        engine.log("scan stopped");
        return;
    }
    if engine.state.borrow().scan.is_some() {
        return;
    }

    let (cancel_tx, cancel_rx) = oneshot::channel();
    engine.state.borrow_mut().scan = Some(cancel_tx);

    let engine = engine.clone();
    engine.spawn({
        let engine = engine.clone();
        async move {
            // `keep_repeated_devices` is what makes RSSI live: without it each
            // device is reported once and the list freezes at first sighting.
            // `accept_all_manufacturer_data` is what makes the raw packet and
            // every company's data reachable — same reasoning as the
            // `Grant::unrestricted` used on connect.
            let options = LeScanOptions::accept_all_advertisements()
                .keep_repeated_devices(true)
                .accept_all_manufacturer_data();
            let scan = match engine.bluetooth.request_le_scan(options).await {
                Ok(scan) => scan,
                Err(why) => {
                    engine.fail(format!("cannot scan: {why}"));
                    // `scan_wanted` deliberately stays set: the radio may come
                    // up in a moment, and the watcher will start this again.
                    engine.state.borrow_mut().scan = None;
                    engine.emit(Event::Scanning(false));
                    return;
                }
            };

            engine.emit(Event::Scanning(true));
            engine.log("scanning");

            let mut sightings = scan.take_until(cancel_rx);
            while let Some(sighting) = sightings.next().await {
                let adv = &sighting.advertisement;
                engine.emit(Event::Sighting {
                    id: sighting.id.clone(),
                    name: sighting.name.clone(),
                    rssi: sighting.rssi(),
                    adv: AdvSummary {
                        local_name: adv.local_name.clone(),
                        tx_power: adv.tx_power,
                        appearance: adv.appearance,
                        connectable: adv.is_connectable,
                        service_uuids: adv.service_uuids.clone(),
                        overflow_service_uuids: adv.overflow_service_uuids.clone(),
                        solicited_service_uuids: adv.solicited_service_uuids.clone(),
                        manufacturer_data: adv
                            .manufacturer_data
                            .iter()
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        service_data: adv
                            .service_data
                            .iter()
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        raw: adv.raw.clone(),
                    },
                });
            }
        }
    });
}

fn disconnect(engine: &Engine, id: &str) {
    let mut state = engine.state.borrow_mut();
    // Only this device's subscriptions: the others are still live.
    state.subscriptions.retain(|at, _| at.device != id);
    let connection = state.connections.remove(id);
    drop(state);

    if let Some(connection) = &connection {
        connection.device.gatt().disconnect();
        engine.log(format!("disconnected from {id}"));
    }
    // Reported even when there was nothing to disconnect. A tab is opened by
    // `Connecting`, before the connection exists, so closing one while it is
    // still being established used to send a command that quietly did nothing
    // and left the tab on screen for ever.
    engine.emit(Event::Disconnected {
        id: id.to_owned(),
        why: None,
    });
}

async fn connect(engine: &Engine, id: String) {
    // Already open: bring it forward rather than connecting twice.
    if engine.state.borrow().connections.contains_key(&id) {
        engine.emit(Event::Connected {
            id: id.clone(),
            mtu: None,
        });
        return;
    }

    engine.emit(Event::Connecting(id.clone()));
    engine.log(format!("connecting to {id}"));

    // The explorer's grant. `request_device`'s allowlist cannot serve this: a
    // device's custom services are exactly what is being looked for, and their
    // UUIDs are not knowable before they are read off the device. The GATT
    // blocklist still applies on top, so HID and firmware-update services stay
    // hidden — see `Grant::all_services`.
    let device = match engine
        .bluetooth
        .adopt_device(&id, Grant::unrestricted())
        .await
    {
        Ok(device) => device,
        Err(why) => {
            engine.fail(format!("cannot reach {id}: {why}"));
            engine.emit(Event::Disconnected {
                id,
                why: Some(why.to_string()),
            });
            return;
        }
    };

    if let Err(why) = device.gatt().connect().await {
        engine.fail(format!("connect to {id} failed: {why}"));
        engine.emit(Event::Disconnected {
            id,
            why: Some(why.to_string()),
        });
        return;
    }

    let mtu = device.mtu().await.ok();
    engine.emit(Event::Connected {
        id: id.clone(),
        mtu,
    });
    engine.log(match mtu {
        Some(mtu) => format!("connected to {id}, ATT MTU {mtu}"),
        None => format!("connected to {id}"),
    });

    engine.state.borrow_mut().connections.insert(
        id.clone(),
        Connection {
            id: id.clone(),
            device: device.clone(),
            services: Vec::new(),
        },
    );

    // Report an unsolicited drop. Without this a device that walks out of range
    // leaves the tree on screen looking live.
    {
        let engine = engine.clone();
        let watched = id.clone();
        engine.spawn({
            let engine = engine.clone();
            async move {
                let mut drops = device.watch_disconnect();
                if drops.next().await.is_some() {
                    let ours = engine.state.borrow().connections.contains_key(&watched);
                    if ours {
                        let mut state = engine.state.borrow_mut();
                        state.subscriptions.retain(|at, _| at.device != watched);
                        state.connections.remove(&watched);
                        drop(state);
                        engine.fail(format!("{watched} disconnected"));
                        engine.emit(Event::Disconnected {
                            id: watched,
                            why: Some("the device dropped the link".into()),
                        });
                    }
                }
            }
        });
    }

    // Everything else the platform will say about the link. nRF Connect puts
    // these behind a menu per device; they are all one call each here.
    dispatch(engine, Command::ReadLinkDetail(id.clone()));

    discover(engine, id).await;
}

/// Walk the whole tree: services, then characteristics, then descriptors.
///
/// Descriptors are fetched up front rather than on selection. It is more
/// traffic, but a Client Characteristic Configuration descriptor that appears a
/// second after the row is clicked reads as a bug, and the count matters when
/// deciding whether a characteristic is worth looking at.
async fn discover(engine: &Engine, id: String) {
    let Some(device) = connected_device(engine, &id) else {
        return;
    };

    let gatt = device.gatt();
    let services = match gatt.get_primary_services(None).await {
        Ok(services) => services,
        Err(why) => {
            engine.fail(format!("discovery failed: {why}"));
            engine.emit(Event::Tree {
                id,
                services: Vec::new(),
            });
            return;
        }
    };

    let mut handles = Vec::with_capacity(services.len());
    let mut info = Vec::with_capacity(services.len());

    for service in &services {
        let characteristics = match service.get_characteristics(None).await {
            Ok(found) => found,
            Err(why) => {
                // A service with nothing readable under it is normal — an empty
                // one, or one whose characteristics the blocklist withholds.
                engine.warn(format!(
                    "{}: no characteristics ({why})",
                    crate::names::short(service.uuid())
                ));
                Vec::new()
            }
        };

        let mut service_handles = Vec::with_capacity(characteristics.len());
        let mut service_info = Vec::with_capacity(characteristics.len());

        for characteristic in characteristics {
            let descriptors = characteristic.get_descriptors().await.unwrap_or_default();
            service_info.push(CharacteristicInfo {
                uuid: *characteristic.uuid(),
                properties: characteristic.properties(),
                descriptors: descriptors.iter().map(|d| *d.uuid()).collect(),
            });
            service_handles.push((characteristic, descriptors));
        }

        info.push(ServiceInfo {
            uuid: *service.uuid(),
            is_primary: service.is_primary(),
            characteristics: service_info,
        });
        handles.push(service_handles);
    }

    let characteristics: usize = info.iter().map(|s| s.characteristics.len()).sum();
    engine.log(format!(
        "{} service{}, {characteristics} characteristic{}",
        info.len(),
        if info.len() == 1 { "" } else { "s" },
        if characteristics == 1 { "" } else { "s" }
    ));

    if let Some(connection) = engine.state.borrow_mut().connections.get_mut(&id) {
        connection.services = handles;
    }
    engine.emit(Event::Tree { id, services: info });
}

fn set_notifying(engine: &Engine, at: CharRef, on: bool) {
    // Cloned up front: `CharRef` names a device as well as a position, so it is
    // no longer `Copy`, and this function hands it to two spawned tasks.
    let key = at.clone();
    if !on {
        // Dropping the cancel sender ends the notification stream; the library
        // stops the subscription when the stream is dropped, but say so
        // explicitly too so the peripheral's CCCD is cleared promptly.
        let had = engine
            .state
            .borrow_mut()
            .subscriptions
            .remove(&at)
            .is_some();
        if !had {
            return;
        }
        let ch = characteristic_at(engine, &at);
        if let Some(ch) = ch {
            let engine = engine.clone();
            engine.spawn({
                let engine = engine.clone();
                async move {
                    let _ = ch.stop_notifications().await;
                    engine.log(format!("unsubscribed {}", crate::names::short(ch.uuid())));
                    engine.emit(Event::Notifying { at: key, on: false });
                }
            });
        }
        return;
    }

    if engine.state.borrow().subscriptions.contains_key(&at) {
        return;
    }
    let Some(ch) = characteristic_at(engine, &at) else {
        return;
    };

    let (cancel_tx, cancel_rx) = oneshot::channel();
    engine
        .state
        .borrow_mut()
        .subscriptions
        .insert(key.clone(), cancel_tx);

    let engine = engine.clone();
    engine.spawn({
        let engine = engine.clone();
        async move {
            let uuid = crate::names::short(ch.uuid());
            let notifications = match ch.start_notifications().await {
                Ok(notifications) => notifications,
                Err(why) => {
                    engine.fail(format!("subscribe {uuid} failed: {why}"));
                    engine.state.borrow_mut().subscriptions.remove(&key);
                    engine.emit(Event::Notifying { at: key, on: false });
                    return;
                }
            };

            engine.log(format!("subscribed {uuid}"));
            engine.emit(Event::Notifying {
                at: key.clone(),
                on: true,
            });

            let mut values = notifications.take_until(cancel_rx);
            while let Some(value) = values.next().await {
                engine.emit(Event::Value {
                    at: key.clone(),
                    value,
                    notified: true,
                });
            }

            // Reached when the stream ends on its own — the link dropped, or
            // the handles were invalidated. The UI's toggle has to follow.
            //
            // Deliberately two statements: a `RefCell` guard created in an
            // `if` condition lives until the end of the whole `if`, so the
            // borrow would still be held while the body runs. Nothing in
            // `emit` borrows the state today, which is exactly what makes it
            // the kind of thing that breaks later.
            let was_subscribed = engine
                .state
                .borrow_mut()
                .subscriptions
                .remove(&at)
                .is_some();
            if was_subscribed {
                engine.emit(Event::Notifying { at, on: false });
            }
        }
    });
}
