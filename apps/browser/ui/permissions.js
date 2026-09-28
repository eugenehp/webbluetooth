// The permissions manager.
//
// Reads `perm_list` and draws it. Like the toolbar it holds no state of its
// own, so there is one description of what has been granted rather than two
// that can disagree.

const invoke = (command, args = {}) =>
  window.__TAURI_INTERNALS__.invoke(command, args);

const list = document.getElementById('list');
const empty = document.getElementById('empty');

function scanningBadge(scanning) {
  const badge = document.createElement('span');
  badge.className = 'badge';
  if (scanning === true) {
    badge.classList.add('on');
    badge.textContent = 'scanning allowed';
    badge.title =
      'This site may receive every matching advertisement in range while it is open.';
  } else if (scanning === false) {
    badge.classList.add('off');
    badge.textContent = 'scanning refused';
    badge.title = 'Remembered refusal — the site cannot ask again until this is cleared.';
  } else {
    return null;
  }
  return badge;
}

function drawOrigin(entry) {
  const box = document.createElement('section');
  box.className = 'origin';

  const head = document.createElement('div');
  head.className = 'head';

  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = entry.origin;
  head.append(name);

  const badge = scanningBadge(entry.scanning);
  if (badge) {
    head.append(badge);
    const clear = document.createElement('button');
    clear.textContent = 'Ask again';
    clear.title = 'Forget this answer, so the next scan request prompts.';
    clear.addEventListener('click', async () => {
      await invoke('perm_clear_scanning', { origin: entry.origin });
      refresh();
    });
    head.append(clear);
  }

  const forget = document.createElement('button');
  forget.textContent = 'Forget site';
  forget.addEventListener('click', async () => {
    await invoke('perm_forget_origin', { origin: entry.origin });
    refresh();
  });
  head.append(forget);
  box.append(head);

  for (const device of entry.devices) {
    const row = document.createElement('div');
    row.className = 'device';

    const dot = document.createElement('span');
    dot.className = device.connected ? 'dot connected' : 'dot';
    dot.title = device.connected ? 'Connected now' : 'Not connected';

    const label = document.createElement('div');
    label.className = 'label';
    const title = document.createElement('div');
    title.textContent = device.name || 'Unnamed device';
    const id = document.createElement('div');
    id.className = 'id';
    id.textContent = `${device.id} · ${device.services} service${
      device.services === 1 ? '' : 's'
    }`;
    label.append(title, id);

    const revoke = document.createElement('button');
    revoke.textContent = 'Revoke';
    revoke.addEventListener('click', async () => {
      await invoke('perm_forget_device', {
        origin: entry.origin,
        deviceId: device.id,
      });
      refresh();
    });

    row.append(dot, label, revoke);
    box.append(row);
  }

  return box;
}

async function refresh() {
  let entries;
  try {
    entries = await invoke('perm_list');
  } catch (error) {
    console.error('perm_list failed', error);
    return;
  }
  empty.hidden = entries.length > 0;
  list.replaceChildren(...entries.map(drawOrigin));
}

refresh();
// Connection state changes underneath this window.
setInterval(refresh, 2500);
