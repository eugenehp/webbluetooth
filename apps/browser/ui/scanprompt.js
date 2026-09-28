// The scanning prompt.
//
// Deliberately not defaulted to allow, and deliberately not dismissible into
// a "yes": closing the window is a refusal, handled on the Rust side.

const invoke = (command, args = {}) =>
  window.__TAURI_INTERNALS__.invoke(command, args);

document
  .getElementById('allow')
  .addEventListener('click', () => invoke('scanprompt_decide', { allow: true }));
document
  .getElementById('block')
  .addEventListener('click', () => invoke('scanprompt_decide', { allow: false }));
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    invoke('scanprompt_decide', { allow: false });
  }
});

invoke('scanprompt_ready')
  .then((ask) => {
    document.getElementById('origin').textContent = ask.origin;
    const wants = document.getElementById('wants');
    if (ask.acceptAll) {
      wants.textContent = 'It asked for every advertisement, unfiltered.';
    } else if (ask.wants.length) {
      wants.textContent = `It asked for advertisements from devices that ${ask.wants.join(
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
