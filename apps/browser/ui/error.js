// The failed address arrives in the fragment, so it never leaves the browser.

const params = new URLSearchParams(location.hash.slice(1));
const failed = (params.get('url') || '').replace(/\+/g, ' ');
const reason = (params.get('reason') || 'could not be reached').replace(/\+/g, ' ');

document.getElementById('url').textContent = failed;
document.getElementById('reason').textContent = `The site ${reason}.`;

document.getElementById('retry').addEventListener('click', () => {
  if (failed) {
    // Straight back to the address that failed; the toolbar's own navigate
    // path would re-run the search heuristics on it.
    location.replace(failed);
  }
});
