#![cfg(target_vendor = "apple")]
//! CoreBluetooth from pure Rust — no Swift source, no Objective-C source, no
//! generated shim.
//!
//! # Why the Objective-C runtime and not the Swift one
//!
//! `swiftui-native` reaches SwiftUI by resolving Swift-mangled symbols and
//! synthesising Swift type metadata, because SwiftUI is a Swift-native
//! framework — it exports about twenty thousand `$s`-prefixed symbols.
//! CoreBluetooth exports **none**. Its `.tbd` lists Objective-C classes and its
//! SDK directory has no `.swiftinterface`; `import CoreBluetooth` in Swift is
//! the Clang importer synthesising Swift-shaped declarations from Objective-C
//! headers at compile time, which still lower to `objc_msgSend`.
//!
//! So the runtime this crate builds types in is the Objective-C one. The
//! principle is unchanged: nothing is compiled ahead of time, no source file in
//! another language is shipped, and the delegate class is produced at run time
//! by [`objc::ClassBuilder`]. `build.rs` names three libraries and does nothing
//! else.
//!
//! # Layout
//!
//! * [`objc`] — message sends, Foundation readers, runtime class synthesis.
//! * [`dispatch`] — the serial queue delegate callbacks are delivered on.
//! * [`cb`] — typed accessors for `CBCentralManager` and friends.
//! * [`delegate`] — the synthesised delegate class and its [`delegate::Event`]s.
//! * [`peripheral`] — the mirror image: `CBPeripheralManager`, its own
//!   synthesised delegate, and the mutable attribute types a GATT server
//!   publishes.
//!
//! [`Central`] ties them together. For the Web Bluetooth API built on top, see
//! the `webbluetooth` crate.
//!
//! # Authorization
//!
//! Bluetooth is TCC-gated on both platforms. A process with no
//! `NSBluetoothAlwaysUsageDescription` gets [`cb::ManagerState::Unauthorized`]
//! rather than an error, and no prompt. See the workspace README.
//!
//! ## License
//! MIT — Copyright © 2025 [Eugene Hauptmann](https://github.com/eugenehp)

#[macro_use]
pub mod macros;

// The adapter: CoreBluetooth expressed as the portable model.
// Public so `webbluetooth` can reach it, and not an API anyone else
// should call: the facade is what a caller holds. Hidden from the docs
// for the same reason, which also keeps the same 38 adapter methods from
// being documented six times over.
#[doc(hidden)]
pub mod backend;
// The peripheral-role adapter: a GATT server expressed as the portable model.
#[cfg(peripheral_role)]
pub mod peripheral_backend;

pub mod cb;
pub mod delegate;
pub mod dispatch;
pub mod l2cap;
pub mod objc;
/// The peripheral role. **macOS and iOS only** — see `build.rs`.
#[cfg(peripheral_role)]
pub mod peripheral;

use std::sync::Arc;

pub use cb::RestoredCentralState;
pub use cb::{Advertisement, Authorization, ManagerState, PeripheralState, Properties, WriteType};
pub use delegate::{Event, EventSink};
pub use l2cap::{Channel as L2capChannel, Closed as L2capClosed, Psm};
pub use objc::{AutoreleasePool, Retained};
pub use objc::{Id, NIL};
#[cfg(peripheral_role)]
pub use peripheral::{
    AttError, InvalidAttribute, PeripheralEvent, PeripheralEventSink, PeripheralHost, Permissions,
    RestoredPeripheralState,
};

/// A `CBCentralManager` with a runtime-built delegate on a private queue.
///
/// Dropping this releases the manager, which stops any scan and tears down any
/// connection it owns.
#[derive(Debug)]
pub struct Central {
    manager: Retained,
    /// Held so the delegate object — and its entry in the sink registry —
    /// outlives the manager that points at it. `CBCentralManager` holds its
    /// delegate weakly.
    _delegate: delegate::Delegate,
    /// Likewise: the manager does not keep the queue alive.
    _queue: dispatch::Queue,
}

impl Central {
    /// Create a central manager delivering events to `sink`.
    ///
    /// `show_power_alert` maps to `CBCentralManagerOptionShowPowerAlertKey`: if
    /// Bluetooth is off, the system offers to turn it on. Web Bluetooth has no
    /// equivalent — a browser never raises that dialog — so it defaults off in
    /// the layer above.
    pub fn new(sink: Arc<dyn EventSink>, show_power_alert: bool) -> Self {
        Self::with_restore_identifier(sink, show_power_alert, None)
    }

    /// As [`Central::new`], opting into state preservation and restoration.
    ///
    /// `restore_identifier` must be identical on every launch, or the system
    /// cannot match the manager it preserved. See
    /// [`delegate::Event::WillRestoreState`] — and note that only iOS relaunches
    /// a process to finish Bluetooth work.
    pub fn with_restore_identifier(
        sink: Arc<dyn EventSink>,
        show_power_alert: bool,
        restore_identifier: Option<&str>,
    ) -> Self {
        let queue = dispatch::Queue::serial(c"dev.webbluetooth.central");
        let delegate = delegate::Delegate::new(sink);
        // SAFETY: both the queue and the delegate are stored in the returned
        // value, so they outlive the manager built from them.
        let manager = unsafe {
            let options = cb::central_options(show_power_alert, restore_identifier);
            cb::central_new(
                delegate.as_ptr(),
                queue.as_ptr(),
                options.as_ref().map_or(NIL, |o| o.as_ptr()),
            )
            .expect("CBCentralManager allocation failed")
        };
        Self {
            manager,
            _delegate: delegate,
            _queue: queue,
        }
    }

    #[inline]
    pub fn as_ptr(&self) -> Id {
        self.manager.as_ptr()
    }

    /// The current Bluetooth state.
    pub fn state(&self) -> ManagerState {
        unsafe { cb::central_state(self.as_ptr()) }
    }

    /// Whether a scan is running.
    pub fn is_scanning(&self) -> bool {
        unsafe { cb::central_is_scanning(self.as_ptr()) }
    }

    /// Attach this central's delegate to a peripheral, so its
    /// `CBPeripheralDelegate` callbacks land in the same sink.
    ///
    /// # Safety
    /// `peripheral` must be a `CBPeripheral` retrieved from this central.
    pub unsafe fn adopt_peripheral(&self, peripheral: Id) {
        unsafe { cb::peripheral_set_delegate(peripheral, self._delegate.as_ptr()) }
    }
}

// The manager is safe to message from any thread; callbacks are serialised onto
// the private queue regardless of which thread asked for the work.
unsafe impl Send for Central {}
unsafe impl Sync for Central {}

/// Whether CoreBluetooth is present and this process may use it.
///
/// Distinct from "a radio exists and is on" — see [`Central::state`].
pub fn authorization() -> Authorization {
    cb::authorization()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Collect(mpsc::Sender<ManagerState>);
    impl EventSink for Collect {
        fn emit(&self, event: Event) {
            if let Event::StateChanged(s) = event {
                let _ = self.0.send(s);
            }
        }
    }

    #[test]
    // Reaches into the Objective-C runtime, which Miri cannot call.
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn delegate_class_registers_and_receives_state() {
        let (tx, rx) = mpsc::channel();
        let central = Central::new(Arc::new(Collect(tx)), false);
        // `centralManagerDidUpdateState:` is guaranteed to arrive, and is the
        // proof that a class built at runtime satisfied CoreBluetooth.
        let state = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("centralManagerDidUpdateState: never arrived");
        assert_eq!(state, central.state());
    }

    #[test]
    // Reaches into the Objective-C runtime, which Miri cannot call.
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn two_centrals_get_independent_sinks() {
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        let _a = Central::new(Arc::new(Collect(tx1)), false);
        let _b = Central::new(Arc::new(Collect(tx2)), false);
        let t = std::time::Duration::from_secs(5);
        assert!(rx1.recv_timeout(t).is_ok(), "first central got no state");
        assert!(rx2.recv_timeout(t).is_ok(), "second central got no state");
    }

    /// `CBUUID.UUIDString` is **not** a canonical form: it echoes back whichever
    /// width it was constructed from, uppercased. Web Bluetooth requires the
    /// lowercase 128-bit form everywhere, so `webbluetooth::BluetoothUuid`
    /// normalises in both directions rather than trusting this string.
    #[test]
    // Reaches into the Objective-C runtime, which Miri cannot call.
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn uuid_string_is_not_canonical() {
        let short = unsafe { cb::uuid_from_string("180F") }.expect("short CBUUID");
        assert_eq!(
            unsafe { cb::uuid_string(short.as_ptr()) }.as_deref(),
            Some("180F")
        );

        let long = unsafe { cb::uuid_from_string("0000180f-0000-1000-8000-00805f9b34fb") }
            .expect("128-bit CBUUID");
        assert_eq!(
            unsafe { cb::uuid_string(long.as_ptr()) }.as_deref(),
            Some("0000180F-0000-1000-8000-00805F9B34FB"),
            "same UUID, different string — and uppercased"
        );
    }
}
