//! A libdispatch serial queue.
//!
//! `CBCentralManager` needs a queue to deliver delegate callbacks on. Passing
//! `nil` means the main queue, which in a Rust program nothing is draining — so
//! the callbacks would never arrive. A private serial queue is serviced by
//! libdispatch's own worker threads, which is what lets a plain `fn main()`
//! talk to Bluetooth without spinning an `NSRunLoop`.

use crate::objc::Id;
use core::ffi::{c_char, c_void};

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> Id;
    fn objc_release(obj: Id);
}

/// A serial dispatch queue, released on drop.
#[derive(Debug)]
pub struct Queue(Id);

impl Queue {
    /// Create a serial queue with the given label.
    ///
    /// A null attribute is `DISPATCH_QUEUE_SERIAL`, so callbacks arrive one at
    /// a time and the delegate never re-enters itself.
    pub fn serial(label: &core::ffi::CStr) -> Self {
        Self(unsafe { dispatch_queue_create(label.as_ptr(), core::ptr::null()) })
    }

    #[inline]
    pub fn as_ptr(&self) -> Id {
        self.0
    }
}

impl Drop for Queue {
    // Under `OS_OBJECT_USE_OBJC` a dispatch queue *is* an Objective-C object,
    // so it is released like one.
    fn drop(&mut self) {
        unsafe { objc_release(self.0) }
    }
}

unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}
