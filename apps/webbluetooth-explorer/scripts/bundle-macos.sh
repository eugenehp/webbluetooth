#!/usr/bin/env bash
#
# Build WebBluetoothExplorer.app.
#
# A bundle is not decoration on macOS. Bluetooth is gated behind TCC, and the
# grant is keyed on the code signature: an unsigned binary gets a fresh identity
# every build, so the permission has to be given again each time. Ad-hoc signing
# the bundle keeps one grant across rebuilds.
#
#   ./scripts/bundle-macos.sh              # release build, into target/
#   ./scripts/bundle-macos.sh --install    # and copy to /Applications
#
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="$(cd "$here/../.." && pwd)"
app="$root/target/WebBluetoothExplorer.app"
install=false

for arg in "$@"; do
  case "$arg" in
    --install) install=true ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This packages a macOS bundle; on Linux and Windows the binary needs no wrapper." >&2
  exit 1
fi

echo "==> building"
cargo build --release --manifest-path "$here/Cargo.toml"

echo "==> assembling $app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/webbluetooth-explorer" \
   "$app/Contents/MacOS/webbluetooth-explorer"
cp "$here/Info.plist" "$app/Contents/Info.plist"
# The four-byte type/creator stamp. Ancient, still expected by Launch Services.
printf 'APPL????' > "$app/Contents/PkgInfo"

echo "==> signing (ad-hoc)"
# Ad-hoc is enough to give TCC a stable identity to remember. A distributable
# build would sign with a Developer ID and notarise; this is for the machine it
# was built on.
codesign --force --sign - --timestamp=none "$app" >/dev/null

if $install; then
  echo "==> installing to /Applications"
  rm -rf "/Applications/WebBluetoothExplorer.app"
  cp -R "$app" "/Applications/WebBluetoothExplorer.app"
  app="/Applications/WebBluetoothExplorer.app"
fi

cat <<EOF

Built $app

Open it from Finder, or:

    open "$app"

The first launch asks for Bluetooth. That prompt is attributed to the process
responsible for the launch, so start it from Finder or Terminal.app — launched
from an editor or a build agent it is denied with no dialog, and the app then
reports no adapter.
EOF
