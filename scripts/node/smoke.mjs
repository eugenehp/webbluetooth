// The addon, loaded and called from Deno.
//
// Separate from the node check because Deno needs `createRequire` to load a
// native addon, and because the point is that it is the *same* binary — no
// second build, no shim between them.
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const addon = require('../../target/node/webbluetooth.node');

const names = Object.keys(addon);
if (!names.includes('requestDevice')) {
  console.error('exports missing:', names);
  Deno.exit(1);
}

const available = await addon.getAvailability();
if (typeof available !== 'boolean') {
  console.error('expected a boolean, got', typeof available);
  Deno.exit(1);
}

// The surface is two calls; everything else hangs off the device they return,
// as an object graph built in Rust rather than reassembled here. Nothing in
// this file knows the shape of a service or a characteristic, which is the
// point — there is no second copy of the API to drift.
if (typeof addon.requestDevice !== 'function') {
  console.error('requestDevice is not callable');
  Deno.exit(1);
}
// requestDevice needs a radio and a device to find, so it is not called here;
// the hardware tests exercise it.
