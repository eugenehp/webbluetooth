//! The delegate class, built at runtime.
//!
//! CoreBluetooth is entirely delegate-driven: `scanForPeripherals` returns
//! nothing, `discoverServices` returns nothing, `readValueForCharacteristic`
//! returns nothing. Every answer arrives as a message to an object conforming
//! to `CBCentralManagerDelegate` / `CBPeripheralDelegate`.
//!
//! Rather than ship an Objective-C or Swift file to be that object, the class
//! is synthesised on first use with [`crate::objc::ClassBuilder`]: allocate a
//! pair under `NSObject`, install one `extern "C"` Rust function per delegate
//! selector, declare both protocols, register. CoreBluetooth cannot tell the
//! difference — it looks the selectors up through the same runtime that made
//! them.
//!
//! Each delegate *instance* is paired with an [`EventSink`] in a process-wide
//! registry keyed by the object's address, which is how one shared set of
//! method implementations serves any number of `CBCentralManager`s.

use crate::cb::{self, Advertisement, ManagerState};
use crate::objc::Id;
use crate::objc::{error_message, ClassBuilder, Retained};
use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Anything CoreBluetooth tells us, normalised.
///
/// `error` is `Some` exactly when the underlying delegate method was handed a
/// non-nil `NSError`, carrying its `code` and `localizedDescription`.
#[derive(Debug)]
pub enum Event {
    /// The central's Bluetooth state changed. Always delivered once shortly
    /// after the manager is created — nothing else may be called before it.
    StateChanged(ManagerState),
    /// An advertising packet was received.
    Discovered {
        peripheral: Retained,
        advertisement: Box<Advertisement>,
        rssi: i32,
    },
    Connected {
        peripheral: Retained,
    },
    ConnectFailed {
        peripheral: Retained,
        error: Option<(i64, String)>,
    },
    Disconnected {
        peripheral: Retained,
        error: Option<(i64, String)>,
    },
    ServicesDiscovered {
        peripheral: Retained,
        error: Option<(i64, String)>,
    },
    /// `discoverIncludedServices:forService:` finished for one service.
    IncludedServicesDiscovered {
        peripheral: Retained,
        service: Retained,
        error: Option<(i64, String)>,
    },
    /// The peripheral's service set changed; every handle into it is now stale.
    ServicesModified {
        peripheral: Retained,
        invalidated: Vec<Retained>,
    },
    CharacteristicsDiscovered {
        peripheral: Retained,
        service: Retained,
        error: Option<(i64, String)>,
    },
    DescriptorsDiscovered {
        peripheral: Retained,
        characteristic: Retained,
        error: Option<(i64, String)>,
    },
    /// A read completed *or* a notification arrived — CoreBluetooth uses one
    /// callback for both and does not distinguish them, so the layer above
    /// correlates against its own outstanding reads.
    CharacteristicValue {
        peripheral: Retained,
        characteristic: Retained,
        value: Option<Vec<u8>>,
        error: Option<(i64, String)>,
    },
    CharacteristicWritten {
        peripheral: Retained,
        characteristic: Retained,
        error: Option<(i64, String)>,
    },
    NotifyStateChanged {
        peripheral: Retained,
        characteristic: Retained,
        notifying: bool,
        error: Option<(i64, String)>,
    },
    DescriptorValue {
        peripheral: Retained,
        descriptor: Retained,
        value: Option<Vec<u8>>,
        error: Option<(i64, String)>,
    },
    DescriptorWritten {
        peripheral: Retained,
        descriptor: Retained,
        error: Option<(i64, String)>,
    },
    RssiRead {
        peripheral: Retained,
        rssi: i32,
        error: Option<(i64, String)>,
    },
    NameChanged {
        peripheral: Retained,
        name: Option<String>,
    },
    /// The write-without-response queue drained and can take more.
    ReadyToWrite {
        peripheral: Retained,
    },
    /// An `openL2CAPChannel:` completed. `channel` is `None` on failure.
    L2capChannelOpened {
        peripheral: Retained,
        channel: Option<Retained>,
        error: Option<(i64, String)>,
    },
    /// The system relaunched this process and is handing back what it preserved.
    ///
    /// Delivered **before** the first `StateChanged`, and only when the manager
    /// was created with a restore identifier.
    WillRestoreState(Box<crate::cb::RestoredCentralState>),
}

/// Where a delegate instance sends what it hears.
///
/// Called on the central's dispatch queue, one event at a time per delegate,
/// so an implementation must not block: hand the event off and return.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

// ── Instance registry ───────────────────────────────────────────────────────

static SINKS: OnceLock<Mutex<HashMap<usize, Arc<dyn EventSink>>>> = OnceLock::new();

fn sinks() -> &'static Mutex<HashMap<usize, Arc<dyn EventSink>>> {
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn emit(delegate: Id, event: Event) {
    let sink = sinks()
        .lock()
        .ok()
        .and_then(|m| m.get(&(delegate as usize)).cloned());
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

/// A live delegate object. Deregisters its sink and releases the object on drop.
#[derive(Debug)]
pub struct Delegate {
    object: Retained,
}

impl Delegate {
    /// Build a delegate bound to `sink`, synthesising the class if needed.
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        let class = delegate_class();
        // SAFETY: `class` is a registered class whose `init` is NSObject's.
        let object = unsafe {
            let obj: Id = msg_send![class, alloc];
            let obj: Id = msg_send![obj, init];
            Retained::adopt(obj).expect("delegate allocation failed")
        };
        sinks()
            .lock()
            .expect("sink registry poisoned")
            .insert(object.key(), sink);
        Self { object }
    }

    #[inline]
    pub fn as_ptr(&self) -> Id {
        self.object.as_ptr()
    }
}

impl Drop for Delegate {
    fn drop(&mut self) {
        if let Ok(mut m) = sinks().lock() {
            m.remove(&self.object.key());
        }
    }
}

// ── The class itself ────────────────────────────────────────────────────────

static CLASS: OnceLock<usize> = OnceLock::new();

fn delegate_class() -> Id {
    *CLASS.get_or_init(|| build_class() as usize) as Id
}

fn build_class() -> Id {
    // A second copy of this crate in the same process would collide on the
    // name, so try suffixes until the runtime accepts one.
    for attempt in 0..64u32 {
        let name = if attempt == 0 {
            c"WBRustCBDelegate".to_owned()
        } else {
            std::ffi::CString::new(format!("WBRustCBDelegate{attempt}")).expect("class name")
        };
        // SAFETY: NSObject is always registered; every IMP below matches the
        // `v@:` encoding it is installed with.
        let built = unsafe { ClassBuilder::new(c"NSObject", &name) };
        let Some(b) = built else { continue };
        return unsafe {
            b.method(
                c"centralManagerDidUpdateState:",
                did_update_state as *const c_void,
                c"v@:@",
            )
            .method(
                c"centralManager:didDiscoverPeripheral:advertisementData:RSSI:",
                did_discover as *const c_void,
                c"v@:@@@@",
            )
            .method(
                c"centralManager:didConnectPeripheral:",
                did_connect as *const c_void,
                c"v@:@@",
            )
            .method(
                c"centralManager:willRestoreState:",
                will_restore_state as *const c_void,
                c"v@:@@",
            )
            .method(
                c"peripheral:didOpenL2CAPChannel:error:",
                did_open_l2cap as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"centralManager:didFailToConnectPeripheral:error:",
                did_fail_connect as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"centralManager:didDisconnectPeripheral:error:",
                did_disconnect as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didDiscoverServices:",
                did_discover_services as *const c_void,
                c"v@:@@",
            )
            .method(
                c"peripheral:didModifyServices:",
                did_modify_services as *const c_void,
                c"v@:@@",
            )
            .method(
                c"peripheral:didDiscoverIncludedServicesForService:error:",
                did_discover_included_services as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didDiscoverCharacteristicsForService:error:",
                did_discover_characteristics as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didDiscoverDescriptorsForCharacteristic:error:",
                did_discover_descriptors as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didUpdateValueForCharacteristic:error:",
                did_update_characteristic as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didWriteValueForCharacteristic:error:",
                did_write_characteristic as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didUpdateNotificationStateForCharacteristic:error:",
                did_update_notification_state as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didUpdateValueForDescriptor:error:",
                did_update_descriptor as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didWriteValueForDescriptor:error:",
                did_write_descriptor as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheral:didReadRSSI:error:",
                did_read_rssi as *const c_void,
                c"v@:@@@",
            )
            .method(
                c"peripheralDidUpdateName:",
                did_update_name as *const c_void,
                c"v@:@",
            )
            .method(
                c"peripheralIsReadyToSendWriteWithoutResponse:",
                is_ready_to_write as *const c_void,
                c"v@:@",
            )
            .conforms(c"CBCentralManagerDelegate")
            .conforms(c"CBPeripheralDelegate")
            .register()
        };
    }
    panic!("could not register a delegate class after 64 attempts");
}

// ── CBCentralManagerDelegate ────────────────────────────────────────────────

unsafe extern "C" fn did_update_state(this: Id, _cmd: *const c_void, central: Id) {
    let state = unsafe { cb::central_state(central) };
    emit(this, Event::StateChanged(state));
}

unsafe extern "C" fn did_discover(
    this: Id,
    _cmd: *const c_void,
    _central: Id,
    peripheral: Id,
    advertisement_data: Id,
    rssi: Id,
) {
    unsafe {
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        let advertisement = Box::new(cb::parse_advertisement(advertisement_data));
        let rssi = crate::objc::number_i64(rssi).unwrap_or(127) as i32;
        emit(
            this,
            Event::Discovered {
                peripheral,
                advertisement,
                rssi,
            },
        );
    }
}

unsafe extern "C" fn did_connect(this: Id, _cmd: *const c_void, _central: Id, peripheral: Id) {
    let Some(peripheral) = (unsafe { Retained::retain(peripheral) }) else {
        return;
    };
    emit(this, Event::Connected { peripheral });
}

unsafe extern "C" fn did_fail_connect(
    this: Id,
    _cmd: *const c_void,
    _central: Id,
    peripheral: Id,
    error: Id,
) {
    unsafe {
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        emit(
            this,
            Event::ConnectFailed {
                peripheral,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_disconnect(
    this: Id,
    _cmd: *const c_void,
    _central: Id,
    peripheral: Id,
    error: Id,
) {
    unsafe {
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        emit(
            this,
            Event::Disconnected {
                peripheral,
                error: error_message(error),
            },
        );
    }
}

// ── CBPeripheralDelegate ────────────────────────────────────────────────────

unsafe extern "C" fn did_discover_services(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    error: Id,
) {
    unsafe {
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        emit(
            this,
            Event::ServicesDiscovered {
                peripheral,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_modify_services(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    invalidated: Id,
) {
    unsafe {
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        let invalidated = crate::objc::array_map(invalidated, |s| Retained::retain(s))
            .into_iter()
            .flatten()
            .collect();
        emit(
            this,
            Event::ServicesModified {
                peripheral,
                invalidated,
            },
        );
    }
}

unsafe extern "C" fn did_discover_included_services(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    service: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(service)) =
            (Retained::retain(peripheral), Retained::retain(service))
        else {
            return;
        };
        emit(
            this,
            Event::IncludedServicesDiscovered {
                peripheral,
                service,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_discover_characteristics(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    service: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(service)) =
            (Retained::retain(peripheral), Retained::retain(service))
        else {
            return;
        };
        emit(
            this,
            Event::CharacteristicsDiscovered {
                peripheral,
                service,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_discover_descriptors(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    characteristic: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(characteristic)) = (
            Retained::retain(peripheral),
            Retained::retain(characteristic),
        ) else {
            return;
        };
        emit(
            this,
            Event::DescriptorsDiscovered {
                peripheral,
                characteristic,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_update_characteristic(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    characteristic: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(characteristic)) = (
            Retained::retain(peripheral),
            Retained::retain(characteristic),
        ) else {
            return;
        };
        let value = cb::characteristic_value(characteristic.as_ptr());
        emit(
            this,
            Event::CharacteristicValue {
                peripheral,
                characteristic,
                value,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_write_characteristic(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    characteristic: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(characteristic)) = (
            Retained::retain(peripheral),
            Retained::retain(characteristic),
        ) else {
            return;
        };
        emit(
            this,
            Event::CharacteristicWritten {
                peripheral,
                characteristic,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_update_notification_state(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    characteristic: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(characteristic)) = (
            Retained::retain(peripheral),
            Retained::retain(characteristic),
        ) else {
            return;
        };
        let notifying = cb::characteristic_is_notifying(characteristic.as_ptr());
        emit(
            this,
            Event::NotifyStateChanged {
                peripheral,
                characteristic,
                notifying,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_update_descriptor(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    descriptor: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(descriptor)) =
            (Retained::retain(peripheral), Retained::retain(descriptor))
        else {
            return;
        };
        let value = cb::descriptor_value(descriptor.as_ptr());
        emit(
            this,
            Event::DescriptorValue {
                peripheral,
                descriptor,
                value,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_write_descriptor(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    descriptor: Id,
    error: Id,
) {
    unsafe {
        let (Some(peripheral), Some(descriptor)) =
            (Retained::retain(peripheral), Retained::retain(descriptor))
        else {
            return;
        };
        emit(
            this,
            Event::DescriptorWritten {
                peripheral,
                descriptor,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_read_rssi(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    rssi: Id,
    error: Id,
) {
    unsafe {
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        let rssi = crate::objc::number_i64(rssi).unwrap_or(127) as i32;
        emit(
            this,
            Event::RssiRead {
                peripheral,
                rssi,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_update_name(this: Id, _cmd: *const c_void, peripheral: Id) {
    unsafe {
        let name = cb::peripheral_name(peripheral);
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        emit(this, Event::NameChanged { peripheral, name });
    }
}

unsafe extern "C" fn will_restore_state(this: Id, _cmd: *const c_void, _central: Id, dict: Id) {
    let restored = unsafe { cb::parse_restored_central_state(dict) };
    emit(this, Event::WillRestoreState(Box::new(restored)));
}

unsafe extern "C" fn did_open_l2cap(
    this: Id,
    _cmd: *const c_void,
    peripheral: Id,
    channel: Id,
    error: Id,
) {
    unsafe {
        let Some(peripheral) = Retained::retain(peripheral) else {
            return;
        };
        emit(
            this,
            Event::L2capChannelOpened {
                peripheral,
                channel: Retained::retain(channel),
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn is_ready_to_write(this: Id, _cmd: *const c_void, peripheral: Id) {
    let Some(peripheral) = (unsafe { Retained::retain(peripheral) }) else {
        return;
    };
    emit(this, Event::ReadyToWrite { peripheral });
}
