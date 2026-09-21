//! `BluetoothDevice` — a device this process has been granted access to.

use crate::address::is_bluetooth_address;
use crate::error::Result;
use crate::gatt::RemoteGattServer;
use futures_channel::mpsc;
use futures_core::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// The ATT MTU every LE link carries before any negotiation.
const ATT_DEFAULT_MTU: u16 = 23;

/// A device returned by [`crate::Bluetooth::request_device`].
///
/// Holding one grants access to the services that were requested, and nothing
/// else — the allowlist is fixed at the moment of the grant, exactly as a
/// browser fixes it when the user picks a device.
#[derive(Clone)]
pub struct BluetoothDevice {
    pub(crate) inner: Arc<crate::session::Session>,
    pub(crate) id: String,
}

impl BluetoothDevice {
    /// The per-host device identifier — `BluetoothDevice.id`.
    ///
    /// Stable for this machine and this peripheral across reboots. It is *not*
    /// the Bluetooth address: Apple never exposes that, and rotates this value
    /// per host, so two machines will not agree on it.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The device's name, if it advertised one.
    pub fn name(&self) -> Option<String> {
        self.inner.device_name(&self.id)
    }

    /// Watch this device's advertisements — `watchAdvertisements()`.
    ///
    /// The returned stream reports every packet from *this* device and nothing
    /// else, for as long as it is held. Useful for a peripheral whose payload
    /// lives in the advertisement rather than in a characteristic, and for
    /// noticing that a device has come back into range without connecting.
    ///
    /// The radio scan is shared, so this runs alongside a `request_device` and
    /// alongside any number of other watches.
    pub async fn watch_advertisements(&self) -> Result<crate::Advertisements> {
        self.inner.require_powered_on().await?;
        let (watcher, stream, restart) = self.inner.scan_hub().add(crate::scan::Want::Device {
            id: self.id.clone(),
            grant: self.inner.grant(&self.id),
        });
        if restart {
            if let Err(e) = self.inner.backend().set_radio_scanning(true) {
                self.inner.scan_hub().remove(watcher);
                return Err(e);
            }
        }
        // The web has no radio-wide scan to reference-count: advertisements
        // come from `watchAdvertisements` on one device, so the watch is
        // started here rather than by the hub.
        #[cfg(target_arch = "wasm32")]
        if let Err(e) = self.inner.watch_advertisements(&self.id, true).await {
            self.inner.scan_hub().remove(watcher);
            return Err(e);
        }
        Ok(crate::Advertisements {
            inner: self.inner.clone(),
            watcher,
            stream,
            device_id: self.id.clone(),
        })
    }

    /// Ask for a different connection interval.
    ///
    /// **Not part of Web Bluetooth.** Two platforms can ask —
    /// `BluetoothGatt.requestConnectionPriority` on Android, and
    /// `RequestPreferredConnectionParameters` on Windows 10 2004 or newer —
    /// and the rest return [`crate::Error::NotSupported`] with the reason:
    /// CoreBluetooth does not expose connection parameters at all, and BlueZ
    /// has no D-Bus API for them.
    ///
    /// Even where it works it is a request. The peripheral decides the
    /// interval, and neither platform reports what was agreed, so success here
    /// means the ask was made and nothing more.
    pub async fn request_connection_priority(
        &self,
        priority: crate::ConnectionPriority,
    ) -> Result<()> {
        self.inner
            .request_connection_priority(&self.id, priority)
            .await
    }

    /// Whether [`Self::watch_advertisements`] is running for this device —
    /// `BluetoothDevice.watchingAdvertisements`.
    ///
    /// True while the [`crate::Advertisements`] stream is alive, and false once
    /// it is dropped, since dropping it is what stops the watch.
    pub fn watching_advertisements(&self) -> bool {
        self.inner.scan_hub().is_watching_device(&self.id)
    }

    /// The GATT server on this device.
    pub fn gatt(&self) -> RemoteGattServer {
        RemoteGattServer {
            inner: self.inner.clone(),
            id: self.id.clone(),
        }
    }

    /// A stream that yields once per disconnection —
    /// `ongattserverdisconnected`.
    ///
    /// BLE links drop routinely, so treat this as expected rather than
    /// exceptional. Every service and characteristic handle taken before the
    /// disconnect is stale afterwards and will fail with `InvalidStateError`;
    /// reconnect and re-resolve them.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use webbluetooth::stream::StreamExt;
    /// # async fn example(device: webbluetooth::BluetoothDevice) -> webbluetooth::Result<()> {
    /// let mut disconnects = device.watch_disconnect();
    /// while disconnects.next().await.is_some() {
    ///     // Handles minted before the drop are stale; rediscover after
    ///     // reconnecting rather than reusing them.
    ///     device.gatt().connect().await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn watch_disconnect(&self) -> DisconnectEvents {
        DisconnectEvents {
            rx: self.inner.watch_disconnect(&self.id),
        }
    }

    /// Signal strength in dBm. Requires an active connection.
    ///
    /// Not part of the Web Bluetooth API — CoreBluetooth offers it and it is
    /// useful, so it is here.
    pub async fn rssi(&self) -> Result<i32> {
        self.inner.read_rssi(&self.id).await
    }

    /// What the link actually negotiated — interval, latency and timeout.
    ///
    /// The counterpart to [`Self::request_connection_priority`], which asks;
    /// this reports what came back. Requires an active connection.
    ///
    /// `NotSupported` on every platform but Windows, which is the only one
    /// that hands the negotiated values to an application — and there only on
    /// Windows 10 2004 or newer, where `IBluetoothLEDevice6` exists.
    /// CoreBluetooth, BlueZ and Android all keep them inside the stack, and
    /// even `linux-hci` cannot see them: the kernel owns the link, and the
    /// values appear only in the HCI event that reports it coming up.
    pub async fn connection_parameters(&self) -> Result<crate::ConnectionParameters> {
        self.inner.connection_parameters(&self.id).await
    }

    /// Pair with this device, so encrypted attributes become readable.
    ///
    /// Not part of Web Bluetooth, and the omission is deliberate there: a
    /// browser pairs on the user's behalf when a peer demands it, and a page
    /// is never told. A library's caller *is* the application, so it gets to
    /// ask.
    ///
    /// This matters more than its absence from the standard suggests. A great
    /// many real devices — heart-rate straps, glucose meters, anything with a
    /// privacy requirement — mark their interesting characteristics as
    /// requiring encryption. Reading one without a bond fails with
    /// `insufficient authentication`, and no amount of retrying fixes it.
    ///
    /// What actually happens is the platform's business, and they differ more
    /// than usual:
    ///
    /// | platform | what `pair()` does |
    /// |---|---|
    /// | BlueZ | `org.bluez.Device1.Pair` — the daemon runs the ceremony |
    /// | Android | `createBond`, then waits for the bond to settle |
    /// | Windows | `DeviceInformationPairing.PairAsync` |
    /// | Apple | nothing: [`crate::Pairing::Implicit`], see below |
    /// | `linux-hci` | fails: there is no Security Manager without a daemon |
    ///
    /// On Apple this returns [`crate::Pairing::Implicit`] without doing anything,
    /// because there is nothing to do — CoreBluetooth exposes no pairing API
    /// at all. The system pairs when an encrypted attribute is touched and
    /// shows its own dialog. Reporting success would be a lie and reporting
    /// failure would be worse, since the caller's next read is exactly what
    /// they should do.
    ///
    /// A ceremony needing a passkey or a confirmation is the platform's to
    /// present. On BlueZ that means an agent must be registered — usually the
    /// desktop's — and without one, pairing a device that wants input fails.
    pub async fn pair(&self) -> Result<crate::Pairing> {
        self.inner.pair(&self.id).await
    }

    /// Whether this device is already bonded.
    ///
    /// `false` on platforms that do not say — Apple keeps bonding entirely to
    /// itself, so there is nothing to report and nothing a caller could do
    /// with it.
    pub async fn is_paired(&self) -> Result<bool> {
        self.inner.is_paired(&self.id).await
    }

    /// What the link is running at, in each direction.
    ///
    /// `NotSupported` where the platform does not say — which is most of them.
    /// Android reports it through `readPhy`, and Windows exposes it on
    /// `BluetoothLEDevice`; CoreBluetooth, BlueZ and the web all keep it
    /// inside the stack.
    pub async fn phy(&self) -> Result<crate::ConnectionPhy> {
        self.inner.phy(&self.id).await
    }

    /// Ask for a different physical layer, and report what the link settled
    /// on.
    ///
    /// The largest throughput lever on a connection: [`crate::Phy::Le2M`] doubles the
    /// symbol rate, so roughly twice as much fits in a connection event. Worth
    /// asking for before a firmware upload or a log download.
    ///
    /// A *request*, not a setting. Both ends and both controllers have to
    /// agree, and any of them may decline — 2M and coded are optional even in
    /// Bluetooth 5. The returned value is what the link actually runs at,
    /// which may be exactly what it was before; that is not an error, and
    /// checking it is the only way to know whether the ask took.
    ///
    /// Only Android can do this. Windows exposes the PHY but no way to choose
    /// one, CoreBluetooth exposes neither, and BlueZ has no D-Bus API for it.
    pub async fn set_preferred_phy(
        &self,
        tx: crate::Phy,
        rx: crate::Phy,
    ) -> Result<crate::ConnectionPhy> {
        self.inner.set_preferred_phy(&self.id, tx, rx).await
    }

    /// The negotiated ATT MTU, in bytes.
    ///
    /// Not part of Web Bluetooth, which hides the number and lets the
    /// implementation split long writes. It is exposed because sizing a
    /// payload to one PDU is the difference between one round trip and five,
    /// and because there is no way to work it out from the outside.
    ///
    /// Requires an active connection. Before negotiation — and on a platform
    /// that will not say — this is 23, the minimum every LE link guarantees.
    ///
    /// For the largest value that fits in a single write, prefer
    /// [`crate::RemoteGattCharacteristic::max_write_length`]: it is this minus
    /// the three-byte ATT header, and on Apple it is what CoreBluetooth
    /// reports directly rather than something derived.
    pub async fn mtu(&self) -> Result<u16> {
        // Derived from the write-without-response limit rather than the
        // with-response one: `withResponse` on Apple reports the 512-byte
        // attribute ceiling, which is a different number and not the MTU.
        let payload = self
            .inner
            .max_write_len(&self.id, crate::gatt::WriteType::WithoutResponse)?;
        Ok((payload as u16).saturating_add(3).max(ATT_DEFAULT_MTU))
    }

    /// The device's Bluetooth address, where the platform reveals one.
    ///
    /// `None` on Apple, and only there: CoreBluetooth never exposes a
    /// peripheral's address, giving each one a system-generated UUID instead —
    /// which is what [`Self::id`] returns there. Everywhere else the id *is*
    /// the address and this returns it parsed back out.
    ///
    /// Not part of Web Bluetooth, which deliberately has no address: an
    /// address is a stable identifier for a physical thing, which is the
    /// tracking vector the UUID indirection exists to remove.
    pub fn address(&self) -> Option<String> {
        is_bluetooth_address(&self.id).then(|| self.id.to_uppercase())
    }

    /// Open an L2CAP connection-oriented channel to this device.
    ///
    /// Absent on Windows, which has no such API — see [`crate::L2CAP`].
    #[cfg(l2cap)]
    ///
    /// A byte pipe with no attribute-size ceiling and no ATT round trip per
    /// write — the right tool for a firmware image or a stream. Not part of
    /// Web Bluetooth. Requires an active connection, and a peer that has
    /// published the PSM.
    pub async fn open_l2cap_channel(&self, psm: crate::Psm) -> Result<crate::L2capChannel> {
        let target = self.inner.l2cap_target(&self.id, psm).await?;
        crate::l2cap::from_target(target, psm)
    }

    /// Revoke this process's access — `BluetoothDevice.forget()`.
    ///
    /// Disconnects if connected, and drops the grant, so a later
    /// `request_device` must go through the chooser again.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn example(device: webbluetooth::BluetoothDevice) {
    /// // Revokes the grant and consumes the device — `self`, not `&self`,
    /// // because every handle under it is invalid afterwards.
    /// device.forget();
    /// # }
    /// ```
    pub fn forget(self) {
        self.inner.forget(&self.id);
        // "Remove device from storage". Doing only the first half would revoke
        // the grant for this run and hand it straight back on the next one.
        self.inner.persist_grants();
    }
}

impl std::fmt::Debug for BluetoothDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BluetoothDevice")
            .field("id", &self.id)
            .field("name", &self.name())
            .field("connected", &self.inner.is_connected(&self.id))
            .finish()
    }
}

/// Yields `()` each time the device disconnects.
pub struct DisconnectEvents {
    rx: mpsc::UnboundedReceiver<()>,
}

impl Stream for DisconnectEvents {
    type Item = ();
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<()>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl std::fmt::Debug for DisconnectEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisconnectEvents").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinction [`BluetoothDevice::address`] rests on: everywhere but
    /// Apple the id is an address, and on Apple it is a UUID.
    #[test]
    fn an_address_is_told_apart_from_a_platform_uuid() {
        assert!(is_bluetooth_address("AA:BB:CC:DD:EE:FF"));
        assert!(is_bluetooth_address("00:00:00:00:00:00"));
        assert!(is_bluetooth_address("aa:bb:cc:dd:ee:ff"));

        // CoreBluetooth's identifiers, which must not be reported as addresses.
        assert!(!is_bluetooth_address(
            "6E3C8F1A-2B4D-4E5F-8A9B-0C1D2E3F4A5B"
        ));
        // Near misses.
        assert!(!is_bluetooth_address("AA:BB:CC:DD:EE"));
        assert!(!is_bluetooth_address("AA:BB:CC:DD:EE:FF:00"));
        assert!(!is_bluetooth_address("AA-BB-CC-DD-EE-FF"));
        assert!(!is_bluetooth_address("AA:BB:CC:DD:EE:GG"));
        assert!(!is_bluetooth_address("A:BB:CC:DD:EE:FF"));
        assert!(!is_bluetooth_address(""));
    }
}
