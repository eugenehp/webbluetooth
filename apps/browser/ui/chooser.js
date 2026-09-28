// The device picker.
//
// Rust drives this: `chooser_ready` tells it what is being asked for and, as
// a side effect, starts the scan feeding `__CHOOSER__.candidate`. Exactly one
// identifier ever goes back the other way — the page learns nothing about the
// devices the user did not choose.

const invoke = (command, args = {}) =>
  window.__TAURI_INTERNALS__.invoke(command, args);

const list = document.getElementById('list');
const empty = document.getElementById('empty');
const pair = document.getElementById('pair');
const status = document.getElementById('status');
const statusText = document.getElementById('statusText');

/** After this, a still-empty list is worth explaining rather than spinning at. */
const QUIET_MS = 8000;
let listeningSince = Date.now();

/** Device id -> { row, seen }. */
const rows = new Map();
let selected = null;

function bars(rssi) {
  // Rough, and only has to be monotonic: -50 is across the desk, -90 is
  // through a wall.
  if (rssi == null) return 0;
  if (rssi >= -55) return 4;
  if (rssi >= -70) return 3;
  if (rssi >= -85) return 2;
  return 1;
}

function select(id) {
  selected = id;
  for (const [key, entry] of rows) {
    entry.row.setAttribute('aria-selected', String(key === id));
  }
  pair.disabled = selected === null;
}

function retitle() {
  const found = rows.size;
  statusText.textContent =
    found === 0
      ? 'Scanning…'
      : `${found} ${found === 1 ? 'device' : 'devices'} found — still scanning`;
}

function candidate(device) {
  empty.hidden = true;

  let entry = rows.get(device.deviceId);
  if (!entry) {
    const row = document.createElement('li');
    row.className = 'row';
    row.setAttribute('role', 'option');
    row.setAttribute('aria-selected', 'false');

    const identity = document.createElement('div');
    identity.className = 'identity';
    const name = document.createElement('div');
    name.className = 'name';
    const sub = document.createElement('div');
    sub.className = 'sub';
    identity.append(name, sub);

    const signal = document.createElement('div');
    signal.className = 'signal';
    for (let i = 0; i < 4; i += 1) {
      signal.append(document.createElement('i'));
    }

    row.append(identity, signal);
    row.addEventListener('click', () => select(device.deviceId));
    row.addEventListener('dblclick', () => {
      select(device.deviceId);
      confirm();
    });
    list.append(row);

    entry = { row, name, sub, signal };
    rows.set(device.deviceId, entry);
  }

  entry.name.textContent = device.name || 'Unnamed device';
  const detail = [device.deviceId];
  if (device.rssi != null) {
    detail.push(`${device.rssi} dBm`);
  }
  entry.sub.textContent = detail.join(' · ');

  const strength = bars(device.rssi);
  [...entry.signal.children].forEach((bar, index) => {
    bar.classList.toggle('on', index < strength);
  });

  retitle();
}

// Clear what is on screen and let it repopulate. The radio never stopped —
// the scan runs for as long as the picker is open — so this is a genuine
// re-listen rather than a button that only looks like one: anything still
// advertising comes back within an interval or two, and anything that has
// gone away does not.
function rescan() {
  rows.clear();
  selected = null;
  pair.disabled = true;
  list.replaceChildren();
  empty.hidden = true;
  listeningSince = Date.now();
  retitle();
}

document.getElementById('rescan').addEventListener('click', rescan);

// Say something useful once a room has had time to answer and has not.
setInterval(() => {
  empty.hidden = !(rows.size === 0 && Date.now() - listeningSince > QUIET_MS);
}, 1000);

function confirm() {
  if (selected === null) {
    return;
  }
  pair.disabled = true;
  invoke('chooser_pick', { id: selected }).catch(() => {});
}

document.getElementById('cancel').addEventListener('click', () => {
  invoke('chooser_cancel').catch(() => {});
});
pair.addEventListener('click', confirm);
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    invoke('chooser_cancel').catch(() => {});
  } else if (event.key === 'Enter') {
    confirm();
  }
});

window.__CHOOSER__ = { candidate };

invoke('chooser_ready')
  .then((prompt) => {
    document.getElementById('origin').textContent = prompt.origin;
    const wants = document.getElementById('wants');
    if (prompt.acceptAll) {
      wants.textContent = 'It asked for any device nearby.';
    } else if (prompt.wants.length) {
      wants.textContent = `It asked for a device that ${prompt.wants.join(
        ', or that '
      )}.`;
    } else {
      wants.textContent = '';
    }
  })
  .catch((error) => {
    document.getElementById('wants').textContent = String(
      (error && error.message) || error
    );
  });
