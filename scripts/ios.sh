#!/usr/bin/env bash
# Build a webbluetooth example, wrap it in a signed .app, install it on an
# attached iOS device and run it with the console attached.
#
#   ./scripts/ios.sh doctor
#   ./scripts/ios.sh ios-harness peripheral
#   ./scripts/ios.sh ios-harness central
#
# Everything is discovered: the device, a provisioning profile that covers it,
# and a signing identity in that profile's team. Override with
# WEBBLUETOOTH_IOS_DEVICE (the devicectl identifier), or pass --install-only to
# skip the launch.
set -euo pipefail
cd "$(dirname "$0")/.."

EXAMPLE="${1:?usage: ios.sh <example> [args...]}"; shift || true
LAUNCH=1
if [ "${1:-}" = "--install-only" ]; then LAUNCH=0; shift; fi

say()  { printf '  \033[2m%s\033[0m\n' "$*"; }
die()  { printf '  \033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }

# ── the device ──────────────────────────────────────────────────────────────
DEVICE="${WEBBLUETOOTH_IOS_DEVICE:-}"
if [ -z "$DEVICE" ]; then
    # devicectl reports a usable device as either "available" (paired and
    # reachable) or "connected" (a tunnel is already open), and both appear
    # depending on whether something is talking to it. Match the state column
    # exactly, so "unavailable" does not count as "available".
    DEVICE=$(xcrun devicectl list devices 2>/dev/null \
        | awk '{
                 usable = 0
                 for (i = 1; i <= NF; i++)
                     if ($i == "available" || $i == "connected") usable = 1
                 if (!usable) next
                 for (i = 1; i <= NF; i++)
                     if ($i ~ /^[0-9A-F]{8}-([0-9A-F]{4}-){3}[0-9A-F]{12}$/) { print $i; exit }
               }')
fi
[ -n "$DEVICE" ] || die "no available iOS device (plug one in, or set WEBBLUETOOTH_IOS_DEVICE)"

INFO=$(xcrun devicectl device info details --device "$DEVICE" 2>/dev/null)
UDID=$(printf '%s\n' "$INFO" | awk -F': ' '/• udid:/ {print $2; exit}')
NAME=$(printf '%s\n' "$INFO" | awk -F': ' '/• name:/ {print $2; exit}')
[ -n "$UDID" ] || die "could not read the hardware UDID of $DEVICE"
say "device     $NAME ($UDID)"

# ── a provisioning profile that covers it ───────────────────────────────────
PROFILE=""
for dir in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles" \
           "$HOME/Library/MobileDevice/Provisioning Profiles"; do
    [ -d "$dir" ] || continue
    for candidate in "$dir"/*.mobileprovision; do
        [ -f "$candidate" ] || continue
        plist=$(security cms -D -i "$candidate" 2>/dev/null) || continue
        # ProvisionedDevices is an array of hardware UDIDs.
        printf '%s' "$plist" | grep -q "<string>$UDID</string>" || continue
        PROFILE="$candidate"; PROFILE_PLIST="$plist"; break 2
    done
done
[ -n "$PROFILE" ] || die "no provisioning profile covers $UDID — open the project in Xcode once to have one issued"

TEAM=$(printf '%s' "$PROFILE_PLIST" \
    | plutil -extract Entitlements.com\\.apple\\.developer\\.team-identifier raw -o - - 2>/dev/null) \
    || TEAM=""
if [ -z "$TEAM" ]; then
    TEAM=$(printf '%s' "$PROFILE_PLIST" | plutil -extract TeamIdentifier.0 raw -o - - 2>/dev/null)
fi
[ -n "$TEAM" ] || die "could not read the team from $(basename "$PROFILE")"
say "profile    $(printf '%s' "$PROFILE_PLIST" | plutil -extract Name raw -o - -) [$TEAM]"

# ── a signing identity in that team ─────────────────────────────────────────
IDENTITY=""
while read -r _n hash rest; do
    [ -n "${hash:-}" ] || continue
    cn=${rest#\"}; cn=${cn%\"*}
    subject=$(security find-certificate -c "$cn" -p 2>/dev/null | openssl x509 -noout -subject 2>/dev/null || true)
    case "$subject" in *"OU=$TEAM"*) IDENTITY="$hash"; IDENTITY_CN="$cn"; break ;; esac
done < <(security find-identity -v -p codesigning | grep -E '^\s+[0-9]+\)')
[ -n "$IDENTITY" ] || die "no codesigning identity in team $TEAM"
say "identity   $IDENTITY_CN"

# ── build, bundle, sign ─────────────────────────────────────────────────────
BUNDLE_ID="io.webbluetooth.$(printf '%s' "$EXAMPLE" | tr -cd '[:alnum:]-')"
OUT="target/ios/$EXAMPLE.app"

cargo build -q -p webbluetooth --example "$EXAMPLE" --target aarch64-apple-ios --release
rm -rf "$OUT"; mkdir -p "$OUT"
cp "target/aarch64-apple-ios/release/examples/$EXAMPLE" "$OUT/$EXAMPLE"
cp "$PROFILE" "$OUT/embedded.mobileprovision"

cat > "$OUT/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$EXAMPLE</string>
  <key>CFBundleName</key><string>$EXAMPLE</string>
  <key>CFBundleDisplayName</key><string>wb $EXAMPLE</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>MinimumOSVersion</key><string>14.0</string>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>DTPlatformName</key><string>iphoneos</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
  <key>UILaunchScreen</key><dict/>
  <key>NSBluetoothAlwaysUsageDescription</key><string>webbluetooth test harness talks to Bluetooth Low Energy peripherals.</string>
  <key>NSBluetoothPeripheralUsageDescription</key><string>webbluetooth test harness advertises as a Bluetooth Low Energy peripheral.</string>
  <key>UIBackgroundModes</key><array><string>bluetooth-central</string><string>bluetooth-peripheral</string></array>
</dict></plist>
PLIST

cat > target/ios/entitlements.plist <<ENT
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>$TEAM.$BUNDLE_ID</string>
  <key>com.apple.developer.team-identifier</key><string>$TEAM</string>
  <key>get-task-allow</key><true/>
</dict></plist>
ENT

codesign --force --sign "$IDENTITY" --entitlements target/ios/entitlements.plist \
         --timestamp=none "$OUT" >/dev/null
say "signed     $BUNDLE_ID"

xcrun devicectl device install app --device "$DEVICE" "$OUT" >/dev/null 2>&1 \
    || die "install failed (is the device unlocked and trusted?)"
say "installed"

[ "$LAUNCH" -eq 1 ] || exit 0
echo
exec xcrun devicectl device process launch --device "$DEVICE" \
     --console --terminate-existing "$BUNDLE_ID" "$@"
