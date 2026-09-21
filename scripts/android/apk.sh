#!/usr/bin/env bash
# Build, install and run the Android harness on an emulator — real BLE traffic.
#
#   ./scripts/android/apk.sh central
#   ./scripts/android/apk.sh peripheral        AVD=wbtest2 PORT=5558
#
# No Gradle: aapt2 links the manifest, d8 makes the dex, the .so is added to
# the archive, and apksigner signs it. That is the whole of what Gradle would
# have done here, and it keeps the build the same shape as the rest of this
# workspace.
set -euo pipefail
cd "$(dirname "$0")/../.."

ROLE=${1:-central}
: "${ANDROID_HOME:=/opt/homebrew/share/android-commandlinetools}"
: "${JAVA_HOME:=/opt/homebrew/opt/openjdk}"
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$PATH"
AVD=${AVD:-wbtest}
PORT=${PORT:-5556}
SERIAL=emulator-$PORT
BUILD_TOOLS=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
# Compile against the same API level the manifest targets, rather than
# whatever happens to sort last.
TARGET_API=${TARGET_API:-34}
PLATFORM="$ANDROID_HOME/platforms/android-$TARGET_API"
[ -d "$PLATFORM" ] || PLATFORM=$(ls -d "$ANDROID_HOME"/platforms/android-3[0-9] | sort -V | tail -1)
PKG=io.webbluetooth.harness
WORK=target/android-apk
SRC=scripts/android/apk

say() { printf '  \033[2m%s\033[0m\n' "$*"; }

rm -rf "$WORK"; mkdir -p "$WORK/lib/arm64-v8a"

say "building the native library"
NDK=$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=\
$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android35-clang
cargo build -q -p webbluetooth --example android-harness \
      --target aarch64-linux-android --release
cp target/aarch64-linux-android/release/examples/libandroid_harness.so \
   "$WORK/lib/arm64-v8a/"

say "compiling the activity"
javac -d "$WORK/classes" -classpath "$PLATFORM/android.jar" \
      --release 11 "$SRC/Harness.java" 2>&1 | grep -v 'bootstrap class path' || true
"$BUILD_TOOLS/d8" --min-api 31 --output "$WORK" \
    "$WORK"/classes/io/webbluetooth/harness/*.class >/dev/null

say "linking resources"
"$BUILD_TOOLS/aapt2" link -o "$WORK/unsigned.apk" \
    --manifest "$SRC/AndroidManifest.xml" \
    -I "$PLATFORM/android.jar" \
    --min-sdk-version 31 --target-sdk-version "$TARGET_API"

# The dex and the .so go in by hand; aapt2 only handles resources.
( cd "$WORK" && zip -q unsigned.apk classes.dex && zip -q -r unsigned.apk lib )

say "signing"
KEYSTORE=target/debug.keystore
if [ ! -f "$KEYSTORE" ]; then
    # Errors are not swallowed here: a missing keystore surfaces as an
    # apksigner failure three lines later, which says nothing useful.
    keytool -genkeypair -keystore "$KEYSTORE" \
        -storepass webbluetooth -keypass webbluetooth -alias harness \
        -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=webbluetooth harness" >/dev/null
    [ -f "$KEYSTORE" ] || { echo "  ✗ could not create $KEYSTORE" >&2; exit 1; }
fi
"$BUILD_TOOLS/zipalign" -f 4 "$WORK/unsigned.apk" "$WORK/aligned.apk"
"$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" \
    --ks-pass pass:webbluetooth --key-pass pass:webbluetooth \
    --out "$WORK/harness.apk" "$WORK/aligned.apk" 2>&1 | grep -v '^WARNING:' || true
[ -f "$WORK/harness.apk" ] || { echo "  ✗ signing produced no apk" >&2; exit 1; }

# ── device ──────────────────────────────────────────────────────────────────
if ! adb devices | grep -q "^$SERIAL"; then
    say "booting $AVD on port $PORT"
    "$ANDROID_HOME/emulator/emulator" -avd "$AVD" -no-window -no-audio \
        -no-snapshot -no-boot-anim -gpu swiftshader_indirect -port "$PORT" \
        >"$WORK/emulator-$PORT.log" 2>&1 &
fi
adb -s "$SERIAL" wait-for-device
until [ "$(adb -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do
    sleep 5
done

adb -s "$SERIAL" install -r -g "$WORK/harness.apk" >/dev/null
# `-g` grants the manifest permissions, but be explicit: a denied scan is the
# single most common reason an Android BLE harness finds nothing.
for p in BLUETOOTH_SCAN BLUETOOTH_CONNECT BLUETOOTH_ADVERTISE; do
    adb -s "$SERIAL" shell pm grant "$PKG" "android.permission.$p" 2>/dev/null || true
done
say "installed on $SERIAL, role $ROLE"

adb -s "$SERIAL" shell am force-stop "$PKG" >/dev/null 2>&1 || true
adb -s "$SERIAL" logcat -c
adb -s "$SERIAL" shell am start -n "$PKG/.Harness" --es role "$ROLE" >/dev/null
echo
adb -s "$SERIAL" logcat -s wbharness:I
