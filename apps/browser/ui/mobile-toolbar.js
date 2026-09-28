// The browser toolbar, injected into every page.
//
// On a desktop this is a separate webview that a site cannot reach. iOS has
// no child webviews — wry documents `build_as_child` as "Android/iOS:
// Unsupported" — so here it lives in the page's own DOM, behind a shadow
// root so the page's stylesheet does not reach into it by accident.
//
// A page that goes looking can still remove or fake this, and that is worth
// being clear about rather than pretending otherwise. It is why nothing
// security-relevant is decided here: the device chooser is a *separate
// window*, and the origin it displays comes from Rust, not from the page. A
// faked address bar can lie about which site you are on; it cannot grant
// itself a device.

(function () {
  'use strict';

  const internals = window.__TAURI_INTERNALS__;
  if (!internals || typeof internals.invoke !== 'function') {
    return;
  }
  // Only the top document gets a toolbar; an iframe is not a browser.
  if (window.top !== window) {
    return;
  }
  if (document.documentElement.hasAttribute('data-wbb-toolbar')) {
    return;
  }
  document.documentElement.setAttribute('data-wbb-toolbar', '');

  const invoke = (command, args = {}) => internals.invoke(command, args);

  const HEIGHT = 52;
  const host = document.createElement('div');
  host.style.cssText = [
    'position:fixed',
    'top:0',
    'left:0',
    'right:0',
    `height:${HEIGHT}px`,
    'z-index:2147483647',
    'pointer-events:auto',
  ].join(';');
  const root = host.attachShadow({ mode: 'closed' });

  root.innerHTML = `
    <style>
      :host { all: initial; }
      .bar {
        display: flex; align-items: center; gap: 4px;
        height: ${HEIGHT}px; padding: 0 8px; box-sizing: border-box;
        font: 13px/1.2 -apple-system, BlinkMacSystemFont, sans-serif;
        background: #f7f7f9; color: #1b1b1f;
        border-bottom: 1px solid #d6d6dc;
      }
      @media (prefers-color-scheme: dark) {
        .bar { background: #2a2a30; color: #f0f0f3; border-bottom-color: #3d3d45; }
        input { background: #1d1d22 !important; color: #f0f0f3 !important;
                border-color: #3d3d45 !important; }
      }
      button {
        appearance: none; border: 0; background: transparent; color: inherit;
        width: 34px; height: 34px; border-radius: 8px; font-size: 17px;
        line-height: 1; flex: none; opacity: 0.75;
      }
      button:active { opacity: 1; background: rgba(127,127,127,0.22); }
      input {
        flex: 1; min-width: 0; height: 34px; padding: 0 12px;
        border: 1px solid #d6d6dc; border-radius: 17px;
        background: #fff; color: #1b1b1f; font: inherit; outline: none;
      }
    </style>
    <div class="bar">
      <button id="back" aria-label="Back">‹</button>
      <button id="forward" aria-label="Forward">›</button>
      <button id="reload" aria-label="Reload">⟳</button>
      <input id="url" type="url" inputmode="url" autocapitalize="off"
             autocorrect="off" spellcheck="false"
             placeholder="Search, or enter an address" />
      <button id="home" aria-label="Start page">⌂</button>
      <button id="perms" aria-label="Bluetooth permissions">⛨</button>
    </div>
  `;

  const $ = (id) => root.getElementById(id);
  const field = $('url');

  // The page's own content would otherwise start underneath the bar.
  function reserveSpace() {
    const style = document.createElement('style');
    style.textContent = `html { padding-top: ${HEIGHT}px !important; box-sizing: border-box; }`;
    document.documentElement.append(style);
  }

  function show() {
    if (!document.body) {
      return;
    }
    document.documentElement.append(host);
    reserveSpace();
    field.value = location.href.startsWith('tauri:') ? '' : location.href;
  }

  field.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      field.blur();
      invoke('ui_navigate', { input: field.value }).catch(() => {});
    }
  });

  for (const [id, command] of [
    ['back', 'ui_back'],
    ['forward', 'ui_forward'],
    ['reload', 'ui_reload'],
    ['home', 'ui_home'],
    ['perms', 'ui_open_permissions'],
  ]) {
    $(id).addEventListener('click', () => invoke(command).catch(() => {}));
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', show, { once: true });
  } else {
    show();
  }
})();
