// The toolbar.
//
// It owns no state: `ui_state` is the single description of what the browser
// is showing, and everything here is a redraw of that. Rust calls
// `__CHROME__.refresh()` whenever something it knows about changes.

const invoke = (command, args = {}) =>
  window.__TAURI_INTERNALS__.invoke(command, args);

const $ = (id) => document.getElementById(id);
const urlField = $('url');
const context = $('context');
const devices = $('devices');
const adapter = $('adapter');
const themeButton = $('theme');
const progress = $('progress');
const strip = $('strip');

/** True while the user is editing, so a redraw does not overwrite typing. */
let editing = false;
let theme = 'system';

const THEMES = ['system', 'light', 'dark'];
const THEME_TITLE = {
  system: 'Theme: follow the system. Click for light.',
  light: 'Theme: light. Click for dark.',
  dark: 'Theme: dark. Click to follow the system.',
};

const ADAPTER = {
  ready: ['ready', 'Bluetooth is ready.'],
  off: ['bad', 'Bluetooth is turned off.'],
  unauthorized: [
    'bad',
    'This process is not authorised to use Bluetooth. On macOS that means the ' +
      'binary has no NSBluetoothAlwaysUsageDescription, or you declined the prompt.',
  ],
  unsupported: ['bad', 'This machine has no Bluetooth LE radio.'],
  unknown: ['', 'Waiting for the adapter to report its state.'],
};

async function refresh() {
  let state;
  try {
    state = await invoke('ui_state');
  } catch (error) {
    console.error('ui_state failed', error);
    return;
  }

  if (!editing && document.activeElement !== urlField) {
    urlField.value = displayUrl(state.url);
  }

  // Whether the page can use the radio at all is the one thing worth a
  // permanent indicator: on an insecure origin `navigator.bluetooth` is
  // absent, and a site failing for that reason gives no other clue.
  context.classList.toggle('warn', Boolean(state.url) && !state.secure);
  if (state.secure) {
    context.textContent = '🔒';
    context.title = `${state.origin} — a secure context, so Web Bluetooth is offered here`;
  } else if (state.url) {
    context.textContent = '⚠︎';
    context.title =
      'Not a secure context. Web Bluetooth is only offered over https or on a ' +
      'loopback host, so navigator.bluetooth is absent on this page.';
  } else {
    context.textContent = '';
    context.title = '';
  }

  const [tone, title] = ADAPTER[state.adapter] || ADAPTER.unknown;
  adapter.className = `adapter ${tone}`;
  adapter.title = title;

  if (state.theme !== theme) {
    applyTheme(state.theme);
  }

  drawDevices(state.devices);
  drawTabs(state.tabs, state.activeTab);

  // The address bar's own indicator follows the tab in front; a background
  // tab's loading shows on its own tab instead.
  const active = state.tabs.find((tab) => tab.label === state.activeTab);
  progress.hidden = !(active && active.loading);
}

function drawTabs(tabs, activeLabel) {
  // One tab is not a choice, so the strip stays out of the way until there
  // is something to choose between.
  strip.replaceChildren(
    ...(tabs.length < 2
      ? []
      : tabs.map((tab) => {
          const el = document.createElement('div');
          el.className = tab.label === activeLabel ? 'tab active' : 'tab';
          el.title = tab.url || tab.title;
          el.addEventListener('mousedown', (event) => {
            if (event.button === 0) {
              invoke('ui_tab_select', { label: tab.label }).catch(() => {});
            } else if (event.button === 1) {
              invoke('ui_tab_close', { label: tab.label }).catch(() => {});
            }
          });

          if (tab.loading) {
            const spinner = document.createElement('span');
            spinner.className = 'spinner';
            el.append(spinner);
          }

          const label = document.createElement('span');
          label.className = 'label';
          label.textContent = tab.title || 'New tab';

          const close = document.createElement('button');
          close.className = 'close';
          close.textContent = '×';
          close.title = 'Close tab';
          close.addEventListener('click', (event) => {
            event.stopPropagation();
            invoke('ui_tab_close', { label: tab.label }).catch(() => {});
          });

          el.append(label, close);
          return el;
        }))
  );
}

function applyTheme(next) {
  theme = next;
  document.body.dataset.theme = next;
  themeButton.title = THEME_TITLE[next] || '';
}

function drawDevices(granted) {
  devices.replaceChildren(
    ...granted.map((device) => {
      const chip = document.createElement('span');
      chip.className = device.connected ? 'chip connected' : 'chip';
      chip.title = `${device.connected ? 'Connected' : 'Permitted, not connected'} — ${device.id}`;

      const dot = document.createElement('span');
      dot.className = 'dot';

      const label = document.createElement('span');
      label.textContent = device.name || device.id.slice(0, 8);

      const revoke = document.createElement('button');
      revoke.textContent = '×';
      revoke.title = 'Take this device away from the site';
      revoke.addEventListener('click', async () => {
        await invoke('ui_revoke_origin', { deviceId: device.id });
        refresh();
      });

      chip.append(dot, label, revoke);
      return chip;
    })
  );
}

/** `tauri://localhost/home.html` is an implementation detail, not an address. */
function displayUrl(url) {
  return url.startsWith('tauri://') || url.startsWith('http://tauri.localhost')
    ? ''
    : url;
}

// ---- wiring --------------------------------------------------------------

$('go').addEventListener('submit', async (event) => {
  event.preventDefault();
  const input = urlField.value.trim();
  if (!input) {
    return;
  }
  urlField.blur();
  editing = false;
  try {
    await invoke('ui_navigate', { input });
  } catch (error) {
    console.error('navigation refused', error);
  }
});

urlField.addEventListener('focus', () => {
  editing = true;
  urlField.select();
});
urlField.addEventListener('blur', () => {
  editing = false;
  refresh();
});
urlField.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    editing = false;
    urlField.blur();
  }
});

themeButton.addEventListener('click', async () => {
  const next = THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
  applyTheme(next);
  await invoke('ui_set_theme', { theme: next }).catch(() => {});
});

for (const [id, command] of [
  ['back', 'ui_back'],
  ['forward', 'ui_forward'],
  ['reload', 'ui_reload'],
  ['home', 'ui_home'],
  ['newtab', 'ui_tab_new'],
  ['perms', 'ui_open_permissions'],
  ['devtools', 'ui_open_devtools'],
]) {
  $(id).addEventListener('click', () => invoke(command).catch(() => {}));
}

// Rust pushes these: `navigated` as a page starts loading, `refresh` whenever
// a grant or the adapter changes.
window.__CHROME__ = {
  refresh,
  /** ⌘L, from the menu — the page has focus, so this cannot come from here. */
  focusAddress() {
    urlField.focus();
    urlField.select();
  },
  navigated(url) {
    if (!editing) {
      urlField.value = displayUrl(url);
    }
    // The webview reports its new URL slightly after the navigation starts,
    // so read the authoritative state once it has settled rather than
    // trusting this one for anything but the address field.
    setTimeout(refresh, 60);
    setTimeout(refresh, 400);
  },
};

applyTheme('system');
refresh();
// Connects, disconnects and grants are all pushed. This is the backstop for
// what is not: a link dropping while nothing is watching it, and the address
// changing through a navigation the page made itself.
setInterval(refresh, 2000);
