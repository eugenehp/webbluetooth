// The Web Bluetooth API, for a page that has none.
//
// This file is the *body* of an IIFE; `src/shell.rs` wraps it and prepends the
// `ASSIGNED` name tables that `build.rs` generates from the same registry the
// Rust side resolves names against. It is not valid standalone JavaScript, and
// that is deliberate: nothing here should be reachable as a global.
//
// It is injected before any of the page's own scripts run, so a site that
// feature-detects `navigator.bluetooth` at the top of its bundle finds it.
//
// Nothing in this file is a security control. A page can delete it, patch it,
// or call the underlying commands itself. Every check that matters — the
// origin, the chooser, the per-origin service allowlist — is in the Rust
// commands, and this only has to make the honest case pleasant.

if (window.__WEBBLUETOOTH_SHIM__) {
  return;
}

const internals = window.__TAURI_INTERNALS__;
if (!internals || typeof internals.invoke !== 'function') {
  // Not our webview, or Tauri's bootstrap has not run. Installing a
  // `navigator.bluetooth` that cannot work is worse than leaving it absent:
  // sites feature-detect it, and one that throws on every call looks like a
  // broken adapter rather than an unsupported browser.
  return;
}

// Web Bluetooth is a secure-context API. On an insecure origin the property is
// absent in a browser, and absent here, so feature detection gives the same
// answer it would give in Chrome. `isSecureContext` already covers https and
// loopback; the extra case is this browser's own bundled pages, which a custom
// scheme does not always get credit for.
const secure =
  window.isSecureContext ||
  location.protocol === 'tauri:' ||
  ((location.protocol === 'http:' || location.protocol === 'ws:') &&
    (location.hostname === 'localhost' ||
      location.hostname === '127.0.0.1' ||
      location.hostname === '[::1]' ||
      location.hostname.endsWith('.localhost')));
if (!secure) {
  return;
}

// ---------------------------------------------------------------- plumbing

/// Turn `{ name, message }` from a rejected command into the exception the
/// specification says that failure raises.
function toError(raw) {
  if (raw && typeof raw === 'object' && typeof raw.name === 'string') {
    const message = typeof raw.message === 'string' ? raw.message : '';
    return raw.name === 'TypeError'
      ? new TypeError(message)
      : new DOMException(message, raw.name);
  }
  // A transport failure rather than a refusal — the command never ran.
  return new DOMException(String(raw), 'NetworkError');
}

function call(command, args) {
  return internals.invoke(command, args).then(undefined, (raw) => {
    throw toError(raw);
  });
}

// ------------------------------------------------------------------- UUIDs

const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function canonicalUUID(alias) {
  const value = Number(alias);
  if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) {
    throw new TypeError(`Invalid UUID alias: ${alias}`);
  }
  return `${(value >>> 0).toString(16).padStart(8, '0')}-0000-1000-8000-00805f9b34fb`;
}

// The registry reuses names across namespaces — `current_time` is the service
// 0x1805 and also the characteristic 0x2A2B — so which table is consulted is
// part of the question, and only the call site knows the answer. This is why
// the resolution happens here rather than in Rust: the three entry points are
// synchronous in the specification.
function resolveUUID(namespace, name) {
  if (typeof name === 'number') {
    return canonicalUUID(name);
  }
  if (typeof name !== 'string') {
    throw new TypeError(`${namespace} UUID must be a string or a number`);
  }
  const lowered = name.toLowerCase();
  if (UUID_PATTERN.test(lowered)) {
    return lowered;
  }
  const table = ASSIGNED[namespace];
  if (Object.prototype.hasOwnProperty.call(table, name)) {
    return table[name];
  }
  throw new TypeError(
    `Invalid ${namespace} name: '${name}'. ` +
      'It must be a valid 128-bit UUID, a 16- or 32-bit alias as a number, ' +
      'or a name from the Bluetooth assigned-numbers registry.'
  );
}

const BluetoothUUID = {
  canonicalUUID,
  getService: (name) => resolveUUID('service', name),
  getCharacteristic: (name) => resolveUUID('characteristic', name),
  getDescriptor: (name) => resolveUUID('descriptor', name),
};
Object.defineProperty(BluetoothUUID, Symbol.toStringTag, {
  value: 'BluetoothUUID',
});

// ------------------------------------------------------------------ values

function toDataView(bytes) {
  return new DataView(Uint8Array.from(bytes).buffer);
}

function fromBufferSource(value) {
  if (value instanceof ArrayBuffer) {
    return Array.from(new Uint8Array(value));
  }
  if (ArrayBuffer.isView(value)) {
    return Array.from(
      new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
    );
  }
  throw new TypeError('The value must be an ArrayBuffer or a typed array');
}

// -------------------------------------------------------- user activation

// `requestDevice` may only open a chooser off a real user gesture.
// `navigator.userActivation` is the right answer where it exists, but it is
// not in every webview this could run in, and deferring to something absent
// means no check at all: a site could pop the picker on load. So activation
// is tracked here too, and used when the native one is missing.
//
// `isTrusted` is what makes the fallback sound. A page can dispatch a `click`
// at itself, but it cannot forge a trusted one, so nothing a script does on
// its own will satisfy this.
let lastGesture = 0;
const ACTIVATION_WINDOW_MS = 5000; // the transient activation lifetime

for (const type of ['pointerdown', 'pointerup', 'keydown', 'touchend']) {
  window.addEventListener(
    type,
    (event) => {
      if (event.isTrusted) {
        lastGesture = Date.now();
      }
    },
    { capture: true, passive: true }
  );
}

function hasUserActivation() {
  // Set by the host only for a self-test run that was pointed at a device by
  // name, which has already given up the chooser as well. Nothing a page can
  // reach touches this.
  if (SELFTEST_UNGATED) {
    return true;
  }
  const native = navigator.userActivation;
  if (native && typeof native.isActive === 'boolean') {
    return native.isActive;
  }
  return Date.now() - lastGesture < ACTIVATION_WINDOW_MS;
}

// ------------------------------------------------------------------ events

/// Dispatch `type` at `target`, then walk it up the GATT tree.
///
/// Web Bluetooth's events bubble — a page may listen for
/// `characteristicvaluechanged` on the service or the device — but these
/// objects are plain `EventTarget`s with no tree for the DOM to walk. So the
/// walk is done here, with `target` pinned to the object the event happened
/// on: an own data property shadows `Event.prototype.target`, which is an
/// accessor, so a listener reading `event.target` sees the characteristic
/// however far up it was caught.
function fire(target, type, ancestors) {
  for (const at of [target, ...ancestors]) {
    const event = new Event(type, { bubbles: true });
    Object.defineProperty(event, 'target', {
      value: target,
      configurable: true,
    });
    at.dispatchEvent(event);
  }
}

/// Install an `onwhatever` attribute that behaves like a DOM one: assigning
/// replaces the previous handler, and `null` removes it.
function defineEventHandler(prototype, type) {
  const slot = Symbol(`on${type}`);
  Object.defineProperty(prototype, `on${type}`, {
    configurable: true,
    enumerable: true,
    get() {
      return this[slot] || null;
    },
    set(handler) {
      if (this[slot]) {
        this.removeEventListener(type, this[slot]);
      }
      this[slot] = typeof handler === 'function' ? handler : null;
      if (this[slot]) {
        this.addEventListener(type, this[slot]);
      }
    },
  });
}

// ------------------------------------------------------------- the objects

// Identity matters: the specification caches these, and a page that calls
// `getPrimaryService` twice and adds a listener to the second result expects
// the first one's notifications. Handles are per-call, so the stable key is
// the position in the GATT tree and the handle is refreshed underneath.
const deviceCache = new Map(); // device id -> BluetoothDevice
const byKey = new Map(); // tree path -> object
const byHandle = new Map(); // current and past handles -> object
const handleDevice = new Map(); // handle -> device id, so a changed GATT
// tree can be dropped wholesale

/// Record a handle against its object, and which device it belongs to.
///
/// Past handles are kept deliberately: a notification already in flight is
/// labelled with the handle that was live when it was subscribed, and the
/// value still belongs to the same characteristic.
function remember(handle, object, deviceId) {
  byHandle.set(handle, object);
  handleDevice.set(handle, deviceId);
}

/// Forget everything cached for one device.
function forgetDevice(deviceId) {
  const prefix = `${deviceId}|`;
  for (const key of [...byKey.keys()]) {
    if (key.startsWith(prefix)) {
      byKey.delete(key);
    }
  }
  for (const [handle, owner] of [...handleDevice.entries()]) {
    if (owner === deviceId) {
      handleDevice.delete(handle);
      byHandle.delete(handle);
    }
  }
}

/// Build a `readonly maplike<K, DataView>` interface.
///
/// The specification gives `manufacturerData` and `serviceData` their own
/// interfaces rather than plain `Map`s, and the difference is visible: a
/// `Map` would let a page call `set` or `clear` on something it is only
/// supposed to read, and `Object.prototype.toString` would say `[object Map]`
/// where a page checking the interface expects the real name.
function maplike(name) {
  const cls = class {
    #entries;

    constructor(entries) {
      this.#entries = new Map(entries);
    }

    get size() {
      return this.#entries.size;
    }
    get(key) {
      return this.#entries.get(key);
    }
    has(key) {
      return this.#entries.has(key);
    }
    keys() {
      return this.#entries.keys();
    }
    values() {
      return this.#entries.values();
    }
    entries() {
      return this.#entries.entries();
    }
    forEach(callback, thisArg) {
      // Second argument is the key and third the map itself, as maplike
      // requires — `this`, not the backing store.
      for (const [key, value] of this.#entries) {
        callback.call(thisArg, value, key, this);
      }
    }
    [Symbol.iterator]() {
      return this.#entries[Symbol.iterator]();
    }
    get [Symbol.toStringTag]() {
      return name;
    }
  };
  Object.defineProperty(cls, 'name', { value: name });
  return cls;
}

/// Keyed by 16-bit company identifier.
const BluetoothManufacturerDataMap = maplike('BluetoothManufacturerDataMap');
/// Keyed by canonical service UUID.
const BluetoothServiceDataMap = maplike('BluetoothServiceDataMap');

class BluetoothCharacteristicProperties {
  #p;
  constructor(properties) {
    this.#p = properties;
  }
  get broadcast() {
    return this.#p.broadcast;
  }
  get read() {
    return this.#p.read;
  }
  get writeWithoutResponse() {
    return this.#p.writeWithoutResponse;
  }
  get write() {
    return this.#p.write;
  }
  get notify() {
    return this.#p.notify;
  }
  get indicate() {
    return this.#p.indicate;
  }
  get authenticatedSignedWrites() {
    return this.#p.authenticatedSignedWrites;
  }
  get reliableWrite() {
    return this.#p.reliableWrite;
  }
  get writableAuxiliaries() {
    return this.#p.writableAuxiliaries;
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothCharacteristicProperties';
  }
}

class BluetoothRemoteGATTDescriptor {
  #handle;
  #characteristic;
  #uuid;
  #value = null;

  constructor(characteristic, dto) {
    this.#characteristic = characteristic;
    this.#uuid = dto.uuid;
    this.#handle = dto.handle;
  }

  get characteristic() {
    return this.#characteristic;
  }
  get uuid() {
    return this.#uuid;
  }
  get value() {
    return this.#value;
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothRemoteGATTDescriptor';
  }

  _rebind(handle) {
    this.#handle = handle;
  }

  async readValue() {
    const bytes = await call('wb_read_descriptor', { handle: this.#handle });
    this.#value = toDataView(bytes);
    return this.#value;
  }

  async writeValue(value) {
    await call('wb_write_descriptor', {
      handle: this.#handle,
      value: fromBufferSource(value),
    });
  }
}

class BluetoothRemoteGATTCharacteristic extends EventTarget {
  #handle;
  #service;
  #uuid;
  #properties;
  #value = null;
  #notifying = false;

  constructor(service, dto) {
    super();
    this.#service = service;
    this.#uuid = dto.uuid;
    this.#handle = dto.handle;
    this.#properties = new BluetoothCharacteristicProperties(dto.properties);
  }

  get service() {
    return this.#service;
  }
  get uuid() {
    return this.#uuid;
  }
  get properties() {
    return this.#properties;
  }
  get value() {
    return this.#value;
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothRemoteGATTCharacteristic';
  }

  _rebind(handle) {
    this.#handle = handle;
  }

  /// A new value arrived, from a read or from the peer.
  _received(bytes) {
    this.#value = toDataView(bytes);
    fire(this, 'characteristicvaluechanged', [
      this.#service,
      this.#service.device,
    ]);
  }

  async readValue() {
    const bytes = await call('wb_read_characteristic', { handle: this.#handle });
    // A read fires the event too — the specification routes every value
    // through the same "characteristic value changed" step.
    this._received(bytes);
    return this.#value;
  }

  async writeValue(value) {
    // Deprecated in favour of the two explicit forms, and defined as the
    // with-response one where the characteristic supports it.
    return this.writeValueWithResponse(value);
  }

  async writeValueWithResponse(value) {
    await call('wb_write_characteristic', {
      handle: this.#handle,
      value: fromBufferSource(value),
      withResponse: true,
    });
  }

  async writeValueWithoutResponse(value) {
    await call('wb_write_characteristic', {
      handle: this.#handle,
      value: fromBufferSource(value),
      withResponse: false,
    });
  }

  async startNotifications() {
    await call('wb_start_notifications', { handle: this.#handle });
    this.#notifying = true;
    return this;
  }

  async stopNotifications() {
    await call('wb_stop_notifications', { handle: this.#handle });
    this.#notifying = false;
    return this;
  }

  async getDescriptor(descriptor) {
    const found = await this.#descriptors(
      resolveUUID('descriptor', descriptor)
    );
    return found[0];
  }

  async getDescriptors(descriptor) {
    return this.#descriptors(
      descriptor === undefined ? null : resolveUUID('descriptor', descriptor)
    );
  }

  async #descriptors(uuid) {
    const found = await call('wb_descriptors', {
      characteristicHandle: this.#handle,
      descriptor: uuid,
    });
    const deviceId = this.#service.device.id;
    return found.map((dto) => {
      const key = `${deviceId}|${this.#service.uuid}|${this.#uuid}|${dto.uuid}`;
      let existing = byKey.get(key);
      if (existing) {
        existing._rebind(dto.handle);
      } else {
        existing = new BluetoothRemoteGATTDescriptor(this, dto);
        byKey.set(key, existing);
      }
      remember(dto.handle, existing, deviceId);
      return existing;
    });
  }
}
defineEventHandler(
  BluetoothRemoteGATTCharacteristic.prototype,
  'characteristicvaluechanged'
);

class BluetoothRemoteGATTService extends EventTarget {
  #handle;
  #device;
  #uuid;
  #isPrimary;

  constructor(device, dto) {
    super();
    this.#device = device;
    this.#uuid = dto.uuid;
    this.#isPrimary = dto.isPrimary;
    this.#handle = dto.handle;
  }

  get device() {
    return this.#device;
  }
  get uuid() {
    return this.#uuid;
  }
  get isPrimary() {
    return this.#isPrimary;
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothRemoteGATTService';
  }

  _rebind(handle) {
    this.#handle = handle;
  }

  async getCharacteristic(characteristic) {
    const found = await this.#characteristics(
      resolveUUID('characteristic', characteristic)
    );
    return found[0];
  }

  async getCharacteristics(characteristic) {
    return this.#characteristics(
      characteristic === undefined
        ? null
        : resolveUUID('characteristic', characteristic)
    );
  }

  async #characteristics(uuid) {
    const found = await call('wb_characteristics', {
      serviceHandle: this.#handle,
      characteristic: uuid,
    });
    return found.map((dto) => {
      const key = `${this.#device.id}|${this.#uuid}|${dto.uuid}`;
      let existing = byKey.get(key);
      if (existing) {
        existing._rebind(dto.handle);
      } else {
        existing = new BluetoothRemoteGATTCharacteristic(this, dto);
        byKey.set(key, existing);
      }
      remember(dto.handle, existing, this.#device.id);
      return existing;
    });
  }

  async getIncludedService(service) {
    const found = await this.#included(resolveUUID('service', service));
    return found[0];
  }

  async getIncludedServices(service) {
    return this.#included(
      service === undefined ? null : resolveUUID('service', service)
    );
  }

  async #included(uuid) {
    const found = await call('wb_included_services', {
      serviceHandle: this.#handle,
      service: uuid,
    });
    return found.map((dto) => adoptService(this.#device, dto, 'included'));
  }
}

function adoptService(device, dto, kind) {
  const key = `${device.id}|${kind}|${dto.uuid}`;
  let existing = byKey.get(key);
  if (existing) {
    existing._rebind(dto.handle);
  } else {
    existing = new BluetoothRemoteGATTService(device, dto);
    byKey.set(key, existing);
  }
  remember(dto.handle, existing, device.id);
  return existing;
}

class BluetoothRemoteGATTServer {
  #device;
  #connected = false;

  constructor(device) {
    this.#device = device;
  }

  get device() {
    return this.#device;
  }
  /// A local flag, as the specification defines it: what this page believes
  /// about the link, not a fresh query of the controller.
  get connected() {
    return this.#connected;
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothRemoteGATTServer';
  }

  _disconnected() {
    this.#connected = false;
  }

  async connect() {
    await call('wb_gatt_connect', { deviceId: this.#device.id });
    this.#connected = true;
    return this;
  }

  disconnect() {
    this.#connected = false;
    // Synchronous in the specification, so the rejection has nowhere to go.
    call('wb_gatt_disconnect', { deviceId: this.#device.id }).catch(() => {});
  }

  async getPrimaryService(service) {
    const found = await this.#primary(resolveUUID('service', service));
    return found[0];
  }

  async getPrimaryServices(service) {
    return this.#primary(
      service === undefined ? null : resolveUUID('service', service)
    );
  }

  async #primary(uuid) {
    const found = await call('wb_primary_services', {
      deviceId: this.#device.id,
      service: uuid,
    });
    return found.map((dto) => adoptService(this.#device, dto, 'primary'));
  }
}

class BluetoothAdvertisingEvent extends Event {
  #detail;
  #device;

  constructor(device, detail) {
    super('advertisementreceived', { bubbles: true });
    this.#device = device;
    this.#detail = detail;
  }

  get device() {
    return this.#device;
  }
  get name() {
    return this.#detail.name ?? undefined;
  }
  get appearance() {
    return this.#detail.appearance ?? undefined;
  }
  get txPower() {
    return this.#detail.txPower ?? undefined;
  }
  get rssi() {
    return this.#detail.rssi ?? undefined;
  }
  get uuids() {
    return Object.freeze([...this.#detail.uuids]);
  }
  /// Keyed by company identifier, as the specification's integer-keyed map.
  /// JSON has no integer keys, so they arrive as decimal strings.
  get manufacturerData() {
    return new BluetoothManufacturerDataMap(
      Object.entries(this.#detail.manufacturerData).map(([company, bytes]) => [
        Number(company),
        toDataView(bytes),
      ])
    );
  }
  get serviceData() {
    return new BluetoothServiceDataMap(
      Object.entries(this.#detail.serviceData).map(([uuid, bytes]) => [
        uuid,
        toDataView(bytes),
      ])
    );
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothAdvertisingEvent';
  }
}

class BluetoothLEScan {
  #id;
  #filters;
  #keepRepeatedDevices;
  #acceptAllAdvertisements;
  #active = true;

  constructor(id, filters, dto) {
    this.#id = id;
    this.#filters = Object.freeze(filters.map((f) => Object.freeze({ ...f })));
    this.#keepRepeatedDevices = dto.keepRepeatedDevices;
    this.#acceptAllAdvertisements = dto.acceptAllAdvertisements;
  }

  get filters() {
    return this.#filters;
  }
  get keepRepeatedDevices() {
    return this.#keepRepeatedDevices;
  }
  get acceptAllAdvertisements() {
    return this.#acceptAllAdvertisements;
  }
  /// A local flag, as with `gatt.connected`.
  get active() {
    return this.#active;
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothLEScan';
  }

  _ended() {
    this.#active = false;
  }

  stop() {
    this.#active = false;
    scans.delete(this.#id);
    // Void in the specification, so a rejection has nowhere to go.
    call('wb_stop_le_scan', { scanId: this.#id }).catch(() => {});
  }
}

/** Scan identifier to the live `BluetoothLEScan`. */
const scans = new Map();

class BluetoothDevice extends EventTarget {
  #id;
  #name;
  #gatt;
  #watching = false;

  constructor(dto) {
    super();
    this.#id = dto.id;
    this.#name = dto.name ?? undefined;
    this.#gatt = new BluetoothRemoteGATTServer(this);
  }

  get id() {
    return this.#id;
  }
  get name() {
    return this.#name;
  }
  get gatt() {
    return this.#gatt;
  }
  get watchingAdvertisements() {
    return this.#watching;
  }
  get [Symbol.toStringTag]() {
    return 'BluetoothDevice';
  }

  _refresh(dto) {
    if (dto.name != null) {
      this.#name = dto.name;
    }
  }

  _disconnected() {
    this.#gatt._disconnected();
    fire(this, 'gattserverdisconnected', []);
  }

  _advertisement(detail) {
    if (!this.#watching) {
      return;
    }
    if (detail.name != null) {
      this.#name = detail.name;
    }
    this.dispatchEvent(new BluetoothAdvertisingEvent(this, detail));
  }

  async watchAdvertisements(options = {}) {
    const signal = options && options.signal;
    if (signal && signal.aborted) {
      throw new DOMException('watchAdvertisements was aborted', 'AbortError');
    }
    await call('wb_watch_advertisements', { deviceId: this.#id });
    this.#watching = true;
    if (signal) {
      signal.addEventListener(
        'abort',
        () => {
          this.#watching = false;
          call('wb_unwatch_advertisements', { deviceId: this.#id }).catch(
            () => {}
          );
        },
        { once: true }
      );
    }
  }

  async forget() {
    await call('wb_forget_device', { deviceId: this.#id });
    this.#gatt._disconnected();
    deviceCache.delete(this.#id);
  }
}
defineEventHandler(BluetoothDevice.prototype, 'gattserverdisconnected');
defineEventHandler(BluetoothDevice.prototype, 'advertisementreceived');

function adoptDevice(dto) {
  let device = deviceCache.get(dto.id);
  if (device) {
    device._refresh(dto);
    return device;
  }
  device = new BluetoothDevice(dto);
  deviceCache.set(dto.id, device);
  return device;
}

class Bluetooth extends EventTarget {
  get [Symbol.toStringTag]() {
    return 'Bluetooth';
  }

  async getAvailability() {
    return call('wb_availability', {});
  }

  async getDevices() {
    const found = await call('wb_get_devices', {});
    return found.map(adoptDevice);
  }

  async requestDevice(options = {}) {
    // Arguments first. WebIDL conversion happens before the algorithm runs,
    // so a malformed dictionary is a TypeError whether or not a gesture is
    // active — and checking the gesture first would report the wrong reason
    // for a call that was never going to work.
    const request = normaliseRequest(options);

    // The chooser is the consent step the whole API rests on, and the
    // specification ties it to a user gesture so that a page cannot open it
    // unprompted. Same wording Chrome uses, because sites match on it.
    if (!hasUserActivation()) {
      throw new DOMException(
        'Must be handling a user gesture to show a permission request.',
        'SecurityError'
      );
    }

    const dto = await call('wb_request_device', { options: request });
    return adoptDevice(dto);
  }

  async requestLEScan(options = {}) {
    const request = normaliseScan(options);

    // A scan is a permission prompt too, and a stronger one than the
    // chooser's, so it is gated on a gesture for the same reason.
    if (!hasUserActivation()) {
      throw new DOMException(
        'Must be handling a user gesture to show a permission request.',
        'SecurityError'
      );
    }

    const dto = await call('wb_request_le_scan', { options: request });
    const scan = new BluetoothLEScan(dto.id, request.filters || [], dto);
    scans.set(dto.id, scan);
    return scan;
  }
}
defineEventHandler(Bluetooth.prototype, 'availabilitychanged');
defineEventHandler(Bluetooth.prototype, 'advertisementreceived');

/// Validate and canonicalise `requestDevice`'s dictionary.
///
/// The argument checks are here rather than in Rust because the specification
/// makes them `TypeError`s thrown synchronously from the call, before any
/// chooser appears — a page that passes an empty `filters` array should not
/// see a picker open and then fail.
function normaliseRequest(options) {
  const out = {
    optionalServices: (options.optionalServices || []).map((s) =>
      resolveUUID('service', s)
    ),
    optionalManufacturerData: (options.optionalManufacturerData || []).map(
      (id) => {
        const value = Number(id);
        if (!Number.isInteger(value) || value < 0 || value > 0xffff) {
          throw new TypeError(`Invalid company identifier: ${id}`);
        }
        return value;
      }
    ),
    acceptAllDevices: Boolean(options.acceptAllDevices),
  };

  if (options.filters !== undefined) {
    if (!Array.isArray(options.filters)) {
      throw new TypeError('filters must be an array');
    }
    if (options.filters.length === 0) {
      throw new TypeError(
        "Failed to execute 'requestDevice' on 'Bluetooth': " +
          "'filters' member must be non-empty to find any devices."
      );
    }
    out.filters = options.filters.map(normaliseFilter);
  }
  if (options.exclusionFilters !== undefined) {
    if (
      !Array.isArray(options.exclusionFilters) ||
      options.exclusionFilters.length === 0
    ) {
      throw new TypeError('exclusionFilters must be a non-empty array');
    }
    if (options.filters === undefined) {
      throw new TypeError('exclusionFilters requires filters');
    }
    out.exclusionFilters = options.exclusionFilters.map(normaliseFilter);
  }

  if (out.acceptAllDevices && out.filters) {
    throw new TypeError('acceptAllDevices and filters cannot both be given');
  }
  if (!out.acceptAllDevices && !out.filters) {
    throw new TypeError(
      "Failed to execute 'requestDevice' on 'Bluetooth': " +
        'Either filters should be present or acceptAllDevices should be true.'
    );
  }
  return out;
}

/// Validate and canonicalise `requestLEScan`'s dictionary.
///
/// Same shape as `requestDevice`'s filters, and the same rule about needing
/// either filters or the accept-all flag — but `optionalServices` has no
/// meaning here, because a scan grants no access to any service.
function normaliseScan(options) {
  const out = {
    keepRepeatedDevices: Boolean(options.keepRepeatedDevices),
    acceptAllAdvertisements: Boolean(options.acceptAllAdvertisements),
  };

  if (options.filters !== undefined) {
    if (!Array.isArray(options.filters) || options.filters.length === 0) {
      throw new TypeError("'filters' member must be non-empty to find any devices.");
    }
    out.filters = options.filters.map(normaliseFilter);
  }
  if (out.acceptAllAdvertisements && out.filters) {
    throw new TypeError(
      'acceptAllAdvertisements and filters cannot both be given'
    );
  }
  if (!out.acceptAllAdvertisements && !out.filters) {
    throw new TypeError(
      'Either filters should be present or acceptAllAdvertisements should be true.'
    );
  }
  return out;
}

function normaliseFilter(filter) {
  if (!filter || typeof filter !== 'object') {
    throw new TypeError('each filter must be an object');
  }
  const out = {
    services: (filter.services || []).map((s) => resolveUUID('service', s)),
    manufacturerData: (filter.manufacturerData || []).map((entry) => ({
      companyIdentifier: Number(entry.companyIdentifier),
      dataPrefix: entry.dataPrefix ? fromBufferSource(entry.dataPrefix) : null,
      mask: entry.mask ? fromBufferSource(entry.mask) : null,
    })),
    serviceData: (filter.serviceData || []).map((entry) => ({
      service: resolveUUID('service', entry.service),
      dataPrefix: entry.dataPrefix ? fromBufferSource(entry.dataPrefix) : null,
      mask: entry.mask ? fromBufferSource(entry.mask) : null,
    })),
  };
  if (filter.name !== undefined) {
    out.name = String(filter.name);
  }
  if (filter.namePrefix !== undefined) {
    if (String(filter.namePrefix) === '') {
      throw new TypeError('namePrefix cannot be empty');
    }
    out.namePrefix = String(filter.namePrefix);
  }
  if (
    out.services.length === 0 &&
    out.manufacturerData.length === 0 &&
    out.serviceData.length === 0 &&
    out.name === undefined &&
    out.namePrefix === undefined
  ) {
    throw new TypeError('a filter must restrict something');
  }
  return out;
}

// ------------------------------------------------------- events from Rust

const bluetooth = new Bluetooth();

function dispatch(message) {
  try {
    const detail = message.detail || {};
    switch (message.type) {
      case 'characteristicvaluechanged': {
        const characteristic = byHandle.get(detail.handle);
        if (characteristic) {
          // A burst arrives as one message but is still a sequence of
          // values, each of which the page is entitled to see in order.
          for (const value of detail.values) {
            characteristic._received(value);
          }
        }
        break;
      }
      case 'gattserverdisconnected': {
        const device = deviceCache.get(detail.deviceId);
        if (device) {
          device._disconnected();
        }
        break;
      }
      case 'advertisementreceived': {
        if (detail.scanId) {
          // A scan reports devices the page has no grant for, so the event
          // fires at `navigator.bluetooth` and carries a `BluetoothDevice`
          // that cannot be connected to — `gatt.connect()` on it is refused
          // by the permission check, which is the correct outcome.
          const scan = scans.get(detail.scanId);
          if (!scan || !scan.active) {
            break;
          }
          bluetooth.dispatchEvent(
            new BluetoothAdvertisingEvent(
              adoptDevice({ id: detail.deviceId, name: detail.name }),
              detail
            )
          );
          break;
        }
        const device = deviceCache.get(detail.deviceId);
        if (device) {
          device._advertisement(detail);
        }
        break;
      }
      case 'serviceschanged': {
        // The peer re-advertised a different service set, which invalidates
        // every handle into the old one — the host has already dropped them,
        // so the cached JavaScript objects in front of them are dead too and
        // must not be handed out again.
        //
        // No `serviceadded` / `servicechanged` / `serviceremoved` is fired.
        // Those events carry the service that changed, and the platform
        // reports only *that* the set changed, not which member — inventing
        // an event with an undefined `service` would be worse than the
        // absence, and no shipping browser fires them either.
        forgetDevice(detail.deviceId);
        break;
      }
      case 'availabilitychanged': {
        const event = new Event('availabilitychanged');
        Object.defineProperty(event, 'value', { value: detail.available });
        bluetooth.dispatchEvent(event);
        break;
      }
      default:
        break;
    }
  } catch (error) {
    // This is called from the host with `eval`, so a throw here would
    // surface as an unexplained console error with no stack into the page.
    console.error('[webbluetooth] dispatch failed', error);
  }
}

// ------------------------------------------------------------- installation

const bridge = Object.freeze({ dispatch });
Object.defineProperty(window, '__WEBBLUETOOTH_SHIM__', {
  value: bridge,
  // Not writable or configurable: a page overwriting it would only break its
  // own notifications, but the host has no way to find out that happened.
  writable: false,
  configurable: false,
  enumerable: false,
});

// The constructors are exposed because pages use them: `instanceof
// BluetoothDevice`, and `BluetoothUUID.getService` most of all.
for (const [name, value] of [
  ['Bluetooth', Bluetooth],
  ['BluetoothDevice', BluetoothDevice],
  ['BluetoothRemoteGATTServer', BluetoothRemoteGATTServer],
  ['BluetoothRemoteGATTService', BluetoothRemoteGATTService],
  ['BluetoothRemoteGATTCharacteristic', BluetoothRemoteGATTCharacteristic],
  ['BluetoothRemoteGATTDescriptor', BluetoothRemoteGATTDescriptor],
  ['BluetoothCharacteristicProperties', BluetoothCharacteristicProperties],
  ['BluetoothAdvertisingEvent', BluetoothAdvertisingEvent],
  ['BluetoothLEScan', BluetoothLEScan],
  ['BluetoothUUID', BluetoothUUID],
]) {
  Object.defineProperty(window, name, {
    value,
    writable: true,
    configurable: true,
    enumerable: false,
  });
}

Object.defineProperty(Navigator.prototype, 'bluetooth', {
  get() {
    return bluetooth;
  },
  configurable: true,
  enumerable: true,
});

call('wb_shim_hello', {}).catch(() => {});
