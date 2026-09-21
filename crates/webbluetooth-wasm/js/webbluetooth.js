// The browser half of the WebAssembly boundary.
//
// A wasm module cannot call navigator.bluetooth: it has linear memory and
// numbers, no DOM, and no way to get one. Something in JavaScript has to make
// the call and hand the answer back. wasm-bindgen generates a file like this;
// this one is written down, so the whole boundary is two files you can read
// rather than a build step you have to trust.
//
// It imports nothing.
//
//   import { start } from './webbluetooth.js';
//   const app = await start('./your_app.wasm');
//
// The ABI is in ../src/host.rs. The two must agree on the op codes, the error
// codes and the message layout; a Rust test checks that every op below has a
// case here, which catches the commonest way they drift.

// ── Message layout ──────────────────────────────────────────────────────────
//
// Little-endian u32 for numbers, length-prefix before anything variable. See
// ../src/codec.rs — this is the same format, read from the other end.

class Reader {
  constructor(bytes) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    this.bytes = bytes;
    this.at = 0;
  }
  u8() { const v = this.view.getUint8(this.at); this.at += 1; return v; }
  u16() { const v = this.view.getUint16(this.at, true); this.at += 2; return v; }
  u32() { const v = this.view.getUint32(this.at, true); this.at += 4; return v; }
  i32() { const v = this.view.getInt32(this.at, true); this.at += 4; return v; }
  bool() { return this.u8() !== 0; }
  bytes_() {
    const len = this.u32();
    const out = this.bytes.subarray(this.at, this.at + len);
    this.at += len;
    return out;
  }
  str() { return new TextDecoder().decode(this.bytes_()); }
  optionStr() { return this.bool() ? this.str() : null; }
  list(each) {
    const n = this.u32();
    const out = [];
    for (let i = 0; i < n; i++) out.push(each(this));
    return out;
  }
}

class Writer {
  constructor() { this.parts = []; this.len = 0; }
  push(bytes) { this.parts.push(bytes); this.len += bytes.length; return this; }
  u8(v) { return this.push(new Uint8Array([v & 0xff])); }
  u16(v) {
    const b = new Uint8Array(2);
    new DataView(b.buffer).setUint16(0, v, true);
    return this.push(b);
  }
  u32(v) {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setUint32(0, v >>> 0, true);
    return this.push(b);
  }
  i32(v) {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setInt32(0, v, true);
    return this.push(b);
  }
  bool(v) { return this.u8(v ? 1 : 0); }
  bytes(v) {
    const a = v instanceof Uint8Array ? v : new Uint8Array(v);
    return this.u32(a.length).push(a);
  }
  str(v) { return this.bytes(new TextEncoder().encode(v ?? '')); }
  optionStr(v) {
    if (v === null || v === undefined) return this.bool(false);
    return this.bool(true).str(v);
  }
  list(items, each) {
    this.u32(items.length);
    for (const item of items) each(this, item);
    return this;
  }
  finish() {
    const out = new Uint8Array(this.len);
    let at = 0;
    for (const part of this.parts) { out.set(part, at); at += part.length; }
    return out;
  }
}

// ── Error mapping ───────────────────────────────────────────────────────────
//
// Web Bluetooth rejects with DOMExceptions. The names carry the distinction
// the caller acts on — a NetworkError is worth retrying, a SecurityError never
// is — so they are mapped rather than flattened into one failure.

const ERRORS = {
  NotFoundError: 1,
  SecurityError: 2,
  NetworkError: 3,
  InvalidStateError: 4,
  NotSupportedError: 5,
  InvalidModificationError: 6,
  AbortError: 7,
  NotAllowedError: 8,
};

function errorCode(e) {
  if (e && typeof e.name === 'string' && ERRORS[e.name]) return ERRORS[e.name];
  // A TypeError from argument validation is the spec's own signal for a
  // malformed request, and maps onto the same place InvalidModification does.
  if (e instanceof TypeError) return 6;
  return 0; // Unknown
}

const OPS = {
  AVAILABILITY: 1,
  REQUEST_DEVICE: 2,
  GET_DEVICES: 3,
  FORGET: 4,
  CONNECT: 5,
  DISCONNECT: 6,
  DISCOVER_SERVICES: 7,
  DISCOVER_INCLUDED_SERVICES: 8,
  DISCOVER_CHARACTERISTICS: 9,
  DISCOVER_DESCRIPTORS: 10,
  READ_CHARACTERISTIC: 11,
  WRITE_CHARACTERISTIC: 12,
  READ_DESCRIPTOR: 13,
  WRITE_DESCRIPTOR: 14,
  SET_NOTIFY: 15,
  WATCH_ADVERTISEMENTS: 16,
  UNWATCH_ADVERTISEMENTS: 17,
  SET_TIMEOUT: 18,
};

const EVENTS = {
  CHARACTERISTIC_VALUE: 1,
  DISCONNECTED: 2,
  SERVICE_CHANGED: 3,
  ADVERTISEMENT: 4,
  AVAILABILITY_CHANGED: 5,
};

export function start(wasmUrl, imports = {}) {
  return instantiate(fetch(wasmUrl), imports);
}

export async function instantiate(source, extraImports = {}) {
  const state = {
    exports: null,
    nextToken: 1,
    // Everything the page is holding on behalf of the module. WebAssembly
    // cannot hold a JS object, so each one lives here under a string the
    // module passes back — the device id, or an attribute path built from it.
    devices: new Map(),        // id -> BluetoothDevice
    services: new Map(),       // "dev/svc/n" -> BluetoothRemoteGATTService
    characteristics: new Map(),// "dev/svc/n/chr/n" -> characteristic
    descriptors: new Map(),    // ".../dsc/n" -> descriptor
    watchers: new Map(),       // device id -> AbortController
  };

  const memory = () => new Uint8Array(state.exports.memory.buffer);

  function read(ptr, len) {
    return memory().slice(ptr, ptr + len);
  }

  // Hand a buffer to the module: it allocates, we fill, it frees after the
  // call returns. Copying is unavoidable — the module owns its memory.
  function withBuffer(bytes, f) {
    if (!bytes || bytes.length === 0) return f(0, 0);
    const ptr = state.exports.wbt_alloc(bytes.length);
    try {
      memory().set(bytes, ptr);
      return f(ptr, bytes.length);
    } finally {
      state.exports.wbt_free(ptr, bytes.length);
    }
  }

  function settle(token, bytes) {
    withBuffer(bytes, (ptr, len) => state.exports.wbt_settle(token, 0, ptr, len));
  }

  function fail(token, e) {
    state.exports.wbt_settle(token, errorCode(e), 0, 0);
  }

  function emit(kind, bytes) {
    withBuffer(bytes, (ptr, len) => state.exports.wbt_event(kind, ptr, len));
  }

  // ── Handles ───────────────────────────────────────────────────────────────

  function device(id) {
    const d = state.devices.get(id);
    if (!d) throw new DOMException(`no device ${id}`, 'NotFoundError');
    return d;
  }

  function gatt(id) {
    const server = device(id).gatt;
    if (!server) throw new DOMException(`no GATT server on ${id}`, 'NotSupportedError');
    return server;
  }

  function need(map, key, what) {
    const v = map.get(key);
    if (!v) throw new DOMException(`no ${what} ${key}`, 'InvalidStateError');
    return v;
  }

  // A device's attributes are keyed by path so that a disconnect can drop all
  // of them at once — every handle into a device goes stale together.
  function forgetAttributes(id) {
    for (const map of [state.services, state.characteristics, state.descriptors]) {
      for (const key of [...map.keys()]) {
        if (key === id || key.startsWith(id + '/')) map.delete(key);
      }
    }
  }

  function recordServices(id, list, prefix) {
    const out = [];
    list.forEach((service, i) => {
      const key = `${prefix}/svc${i}`;
      state.services.set(key, service);
      out.push({ key, uuid: service.uuid, primary: service.isPrimary !== false });
    });
    return out;
  }

  // ── Advertisements ────────────────────────────────────────────────────────

  function advertisementBytes(id, event) {
    const w = new Writer();
    w.str(id);
    w.optionStr(event.name ?? null);
    w.i32(typeof event.rssi === 'number' ? event.rssi : -127);
    if (typeof event.txPower === 'number') w.bool(true).i32(event.txPower);
    else w.bool(false);
    if (typeof event.appearance === 'number') w.bool(true).u32(event.appearance);
    else w.bool(false);
    w.list([...(event.uuids ?? [])], (w, u) => w.str(String(u)));
    w.list([...(event.manufacturerData ?? new Map())], (w, [code, data]) =>
      w.u16(code).bytes(new Uint8Array(data.buffer, data.byteOffset, data.byteLength)));
    w.list([...(event.serviceData ?? new Map())], (w, [uuid, data]) =>
      w.str(String(uuid)).bytes(new Uint8Array(data.buffer, data.byteOffset, data.byteLength)));
    return w.finish();
  }

  function attachDevice(id, dev) {
    if (state.devices.has(id)) return;
    state.devices.set(id, dev);
    dev.addEventListener('gattserverdisconnected', () => {
      forgetAttributes(id);
      emit(EVENTS.DISCONNECTED, new Writer().str(id).finish());
    });
    // serviceadded/serviceremoved/servicechanged are specified but not
    // implemented in any shipping browser. Listening costs nothing and this
    // starts working the day one of them ships.
    for (const name of ['serviceadded', 'servicechanged', 'serviceremoved']) {
      dev.addEventListener(name, () => {
        emit(EVENTS.SERVICE_CHANGED, new Writer().str(id).finish());
      });
    }
  }

  // ── Requests ──────────────────────────────────────────────────────────────

  function parseFilters(r) {
    return r.list((r) => {
      const filter = {};
      const services = r.list((r) => r.str());
      if (services.length) filter.services = services;
      const name = r.optionStr();
      if (name !== null) filter.name = name;
      const prefix = r.optionStr();
      if (prefix !== null) filter.namePrefix = prefix;
      const manufacturerData = r.list((r) => {
        const entry = { companyIdentifier: r.u16() };
        const prefix = r.bytes_();
        if (prefix.length) entry.dataPrefix = prefix.slice();
        if (r.bool()) entry.mask = r.bytes_().slice();
        return entry;
      });
      if (manufacturerData.length) filter.manufacturerData = manufacturerData;
      const serviceData = r.list((r) => {
        const entry = { service: r.str() };
        const prefix = r.bytes_();
        if (prefix.length) entry.dataPrefix = prefix.slice();
        if (r.bool()) entry.mask = r.bytes_().slice();
        return entry;
      });
      if (serviceData.length) filter.serviceData = serviceData;
      return filter;
    });
  }

  async function handle(op, r) {
    switch (op) {
      // Reported as a number rather than a boolean so "no Bluetooth API at
      // all" stays distinct from "the API says no adapter".
      case OPS.AVAILABILITY: {
        if (!navigator.bluetooth) return new Writer().u32(1).finish(); // unsupported
        const ok = await navigator.bluetooth.getAvailability();
        return new Writer().u32(ok ? 4 : 3).finish(); // on : off
      }

      case OPS.REQUEST_DEVICE: {
        const filters = parseFilters(r);
        const exclusions = parseFilters(r);
        const optionalServices = r.list((r) => r.str());
        const optionalManufacturerData = r.list((r) => r.u16());
        const acceptAll = r.bool();

        const options = { optionalServices, optionalManufacturerData };
        if (acceptAll) options.acceptAllDevices = true;
        else options.filters = filters;
        if (exclusions.length) options.exclusionFilters = exclusions;

        const dev = await navigator.bluetooth.requestDevice(options);
        attachDevice(dev.id, dev);
        return new Writer().str(dev.id).optionStr(dev.name ?? null).finish();
      }

      case OPS.GET_DEVICES: {
        // Chrome ships this behind a flag; without it a page only knows the
        // devices it was granted this session, which is what the map holds.
        const known = navigator.bluetooth.getDevices
          ? await navigator.bluetooth.getDevices()
          : [...state.devices.values()];
        for (const dev of known) attachDevice(dev.id, dev);
        const w = new Writer();
        w.list(known, (w, d) => w.str(d.id).optionStr(d.name ?? null));
        return w.finish();
      }

      case OPS.FORGET: {
        const id = r.str();
        const dev = state.devices.get(id);
        if (dev && dev.forget) await dev.forget();
        forgetAttributes(id);
        state.devices.delete(id);
        return new Uint8Array();
      }

      case OPS.CONNECT: {
        const id = r.str();
        await gatt(id).connect();
        return new Uint8Array();
      }

      case OPS.DISCONNECT: {
        const id = r.str();
        const dev = state.devices.get(id);
        if (dev && dev.gatt && dev.gatt.connected) dev.gatt.disconnect();
        forgetAttributes(id);
        return new Uint8Array();
      }

      case OPS.DISCOVER_SERVICES: {
        const id = r.str();
        const uuid = r.optionStr();
        const server = gatt(id);
        const found = uuid
          ? [await server.getPrimaryService(uuid)]
          : await server.getPrimaryServices();
        const w = new Writer();
        w.list(recordServices(id, found, id), (w, s) =>
          w.str(s.key).str(s.uuid).bool(s.primary));
        return w.finish();
      }

      case OPS.DISCOVER_INCLUDED_SERVICES: {
        const key = r.str();
        const uuid = r.optionStr();
        const service = need(state.services, key, 'service');
        const found = uuid
          ? [await service.getIncludedService(uuid)]
          : await service.getIncludedServices();
        const id = key.split('/')[0];
        const w = new Writer();
        w.list(recordServices(id, found, key), (w, s) =>
          w.str(s.key).str(s.uuid).bool(s.primary));
        return w.finish();
      }

      case OPS.DISCOVER_CHARACTERISTICS: {
        const key = r.str();
        const uuid = r.optionStr();
        const service = need(state.services, key, 'service');
        const found = uuid
          ? [await service.getCharacteristic(uuid)]
          : await service.getCharacteristics();
        // Written out rather than through list() so the index is in scope
        // for the key each characteristic is stored under.
        const out = new Writer();
        out.u32(found.length);
        found.forEach((characteristic, i) => {
          const ckey = `${key}/chr${i}`;
          state.characteristics.set(ckey, characteristic);
          // The nine flags, in order, as booleans — not assembled into a
          // bitfield here. The bit values are Bluetooth's and they live in
          // the Rust side; writing them out again on this side would be a
          // second copy that nothing compares, and a wrong one is silent:
          // `notify` reading the wrong bit means startNotifications is
          // refused or allowed for no visible reason.
          const p = characteristic.properties;
          out.str(ckey).str(characteristic.uuid);
          for (const flag of [
            p.broadcast, p.read, p.writeWithoutResponse, p.write,
            p.notify, p.indicate, p.authenticatedSignedWrites,
            p.reliableWrite, p.writableAuxiliaries,
          ]) out.bool(!!flag);
        });
        return out.finish();
      }

      case OPS.DISCOVER_DESCRIPTORS: {
        const key = r.str();
        const uuid = r.optionStr();
        const characteristic = need(state.characteristics, key, 'characteristic');
        const found = uuid
          ? [await characteristic.getDescriptor(uuid)]
          : await characteristic.getDescriptors();
        const out = new Writer();
        out.u32(found.length);
        found.forEach((descriptor, i) => {
          const dkey = `${key}/dsc${i}`;
          state.descriptors.set(dkey, descriptor);
          out.str(dkey).str(descriptor.uuid);
        });
        return out.finish();
      }

      case OPS.READ_CHARACTERISTIC: {
        const key = r.str();
        const view = await need(state.characteristics, key, 'characteristic').readValue();
        return new Writer().bytes(new Uint8Array(view.buffer)).finish();
      }

      case OPS.WRITE_CHARACTERISTIC: {
        const key = r.str();
        const value = r.bytes_().slice();
        const withResponse = r.bool();
        const characteristic = need(state.characteristics, key, 'characteristic');
        // The two explicit forms rather than the deprecated writeValue, so
        // the caller's choice about acknowledgement is the one that happens.
        if (withResponse) await characteristic.writeValueWithResponse(value);
        else await characteristic.writeValueWithoutResponse(value);
        return new Uint8Array();
      }

      case OPS.READ_DESCRIPTOR: {
        const key = r.str();
        const view = await need(state.descriptors, key, 'descriptor').readValue();
        return new Writer().bytes(new Uint8Array(view.buffer)).finish();
      }

      case OPS.WRITE_DESCRIPTOR: {
        const key = r.str();
        const value = r.bytes_().slice();
        await need(state.descriptors, key, 'descriptor').writeValue(value);
        return new Uint8Array();
      }

      case OPS.SET_NOTIFY: {
        const key = r.str();
        const on = r.bool();
        const characteristic = need(state.characteristics, key, 'characteristic');
        if (on) {
          if (!characteristic[LISTENER]) {
            const id = key.split('/')[0];
            const service = characteristic.service ? characteristic.service.uuid : '';
            const listener = (event) => {
              const view = event.target.value;
              emit(EVENTS.CHARACTERISTIC_VALUE, new Writer()
                .str(id)
                .str(service)
                .str(event.target.uuid)
                .bytes(new Uint8Array(view.buffer))
                .finish());
            };
            characteristic.addEventListener('characteristicvaluechanged', listener);
            characteristic[LISTENER] = listener;
          }
          await characteristic.startNotifications();
        } else {
          await characteristic.stopNotifications();
          if (characteristic[LISTENER]) {
            characteristic.removeEventListener(
              'characteristicvaluechanged', characteristic[LISTENER]);
            delete characteristic[LISTENER];
          }
        }
        return new Uint8Array();
      }

      case OPS.WATCH_ADVERTISEMENTS: {
        const id = r.str();
        const dev = device(id);
        if (!state.watchers.has(id)) {
          const controller = new AbortController();
          state.watchers.set(id, controller);
          dev.addEventListener('advertisementreceived', (event) => {
            emit(EVENTS.ADVERTISEMENT, advertisementBytes(id, event));
          }, { signal: controller.signal });
          await dev.watchAdvertisements({ signal: controller.signal });
        }
        return new Uint8Array();
      }

      case OPS.UNWATCH_ADVERTISEMENTS: {
        const id = r.str();
        const controller = state.watchers.get(id);
        if (controller) {
          // Aborting is how watchAdvertisements is stopped, and it removes
          // the listener with it — both were given this signal.
          controller.abort();
          state.watchers.delete(id);
        }
        return new Uint8Array();
      }

      case OPS.SET_TIMEOUT: {
        const ms = r.u32();
        await new Promise((resolve) => setTimeout(resolve, ms));
        return new Uint8Array();
      }

      default:
        throw new DOMException(`unknown operation ${op}`, 'NotSupportedError');
    }
  }

  const LISTENER = Symbol('webbluetooth.listener');

  const imports = {
    ...extraImports,
    webbluetooth: {
      wbt_call(op, ptr, len) {
        const token = state.nextToken++;
        // The message is copied out now: the module frees its buffer as soon
        // as this returns, and the work below is asynchronous.
        const body = read(ptr, len);
        // Deliberately not awaited. wbt_call must return the token
        // synchronously, and settling re-enters the module — which cannot
        // happen while it is still inside this call.
        (async () => {
          try {
            settle(token, await handle(op, new Reader(body)));
          } catch (e) {
            fail(token, e);
          }
        })();
        return token;
      },
    },
  };

  const module = source instanceof Promise || source instanceof Response
    ? await WebAssembly.instantiateStreaming(source, imports).catch(async (e) => {
        // instantiateStreaming needs the right MIME type, which a plain file
        // server often does not send. Falling back rather than failing over a
        // Content-Type header.
        const response = await source;
        const bytes = await response.arrayBuffer();
        return WebAssembly.instantiate(bytes, imports);
      })
    : await WebAssembly.instantiate(source, imports);

  state.exports = module.instance.exports;

  if (navigator.bluetooth && navigator.bluetooth.addEventListener) {
    navigator.bluetooth.addEventListener('availabilitychanged', (event) => {
      emit(EVENTS.AVAILABILITY_CHANGED, new Writer().bool(!!event.value).finish());
    });
  }

  return module.instance;
}
