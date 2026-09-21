//! What every handle in a device tree shares.
//!
//! A `BluetoothDevice`, the `RemoteGattServer` under it, and every service,
//! characteristic and descriptor below that all need the same two things: the
//! backend to talk to, and whatever else belongs to the *instance* that
//! created them rather than to any one handle.
//!
//! Carrying those separately means every new instance-wide thing has to be
//! threaded through five types and every construction site of each — which is
//! how a grant store ends up copied into `RemoteGattDescriptor` so that
//! `descriptor.characteristic().service().device().forget()` revokes the right
//! thing. One shared value carries them all instead.
//!
//! It dereferences to the backend, so the handles reach it exactly as they did
//! when they held one directly.

use crate::backend::Inner;
use crate::grants::GrantStore;
use std::sync::Arc;

/// The backend, and everything else scoped to one [`crate::Bluetooth`].
pub(crate) struct Session {
    backend: Arc<Inner>,
    /// Where grants are kept between runs, if anywhere.
    ///
    /// `None` means this run's grants go with it, which is the default.
    pub(crate) grants: Option<Arc<GrantStore>>,
}

impl Session {
    pub(crate) fn new(backend: Arc<Inner>, grants: Option<Arc<GrantStore>>) -> Arc<Self> {
        Arc::new(Self { backend, grants })
    }

    /// Write the current grants out, if this session keeps a store.
    ///
    /// Every path that changes what is granted calls this: `request_device`,
    /// `request_device_with`, `adopt_device` and `forget`. It lives here
    /// rather than on `Bluetooth` because that is what it is about — the
    /// grants belong to the session, not to the handle that happened to
    /// change them — and because the version that lived up there was called
    /// from only two of the four. `request_device_with` was the miss, which
    /// is the call an application with its own chooser makes: its grants were
    /// live for the run and absent from the store, so the next run went back
    /// through the chooser with nothing to explain why.
    ///
    /// Failures are deliberately not surfaced to the caller: a grant that
    /// could not be written is still live for this run, and turning a full
    /// disk into a failed `requestDevice` would deny access the user just
    /// agreed to.
    pub(crate) fn persist_grants(&self) {
        let Some(store) = &self.grants else {
            return;
        };
        let mut devices = std::collections::BTreeMap::new();
        for id in self.granted_devices() {
            devices.insert(
                id.clone(),
                crate::grants::Stored {
                    name: self.device_name(&id),
                    grant: self.grant(&id),
                },
            );
        }
        let _ = store.save(&devices);
    }

    /// The backend as an `Arc`, for the few methods that need one.
    ///
    /// Most of the backend is reached through [`Deref`](std::ops::Deref). A
    /// handful of methods take `self: &Arc<Self>` because they hand a weak
    /// reference to a platform callback that outlives the call, and those need
    /// the real `Arc` rather than a borrow of what is inside it.
    pub(crate) fn backend(&self) -> &Arc<Inner> {
        &self.backend
    }
}

// The backend is reached through the session, not beside it: a handle holding
// a `Session` calls `self.inner.connect()` exactly as it did when it held an
// `Inner`, so adding something instance-wide costs nothing at the call sites.
impl std::ops::Deref for Session {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.backend
    }
}
