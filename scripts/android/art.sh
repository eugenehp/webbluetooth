#!/usr/bin/env bash
# Load the generated callback classes in real ART, on an emulator.
#
#   ./scripts/android/art.sh
#
# `scripts/check.sh android` already checks the DEX with `dexdump` and the JNI
# slot table against `jni.h` and a real JVM. Neither of those *links* a class:
# dexdump parses bytes, and a desktop JVM has no `android.bluetooth` to inherit
# from. This runs the classes on a device, where the superclass is the real one
# and the hand-written constructor actually executes.
set -euo pipefail
cd "$(dirname "$0")/../.."

: "${ANDROID_HOME:=/opt/homebrew/share/android-commandlinetools}"
: "${JAVA_HOME:=/opt/homebrew/opt/openjdk}"
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$PATH"
AVD=${AVD:-wbtest}
PORT=${PORT:-5556}
SERIAL=emulator-$PORT
BUILD_TOOLS=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
WORK=target/android-art
CLASSES="dev.webbluetooth.GattCallback dev.webbluetooth.ScanCallback \
         dev.webbluetooth.GattServerCallback dev.webbluetooth.AdvertiseCallback \
         dev.webbluetooth.Empty"

say() { printf '  \033[2m%s\033[0m\n' "$*"; }

rm -rf "$WORK"; mkdir -p "$WORK/dex"
say "emitting the generated classes"
cargo run -q -p webbluetooth-android --example emit-dex -- "$WORK/dex" >/dev/null

say "compiling the driver"
javac --release 11 -d "$WORK" scripts/android/Driver.java
"$BUILD_TOOLS/d8" --output "$WORK" --min-api 26 "$WORK/Driver.class" >/dev/null
mv "$WORK/classes.dex" "$WORK/driver.dex"

say "compiling the native probe"
javac --release 11 -d "$WORK" scripts/android/Probe.java
"$BUILD_TOOLS/d8" --output "$WORK" --min-api 26 "$WORK/Probe.class" >/dev/null
mv "$WORK/classes.dex" "$WORK/probe.dex"
NDK=$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=\
$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android35-clang
cargo build -q -p webbluetooth-android --example art_probe \
      --target aarch64-linux-android --release

if ! adb devices | grep -q "^$SERIAL"; then
    say "booting $AVD"
    "$ANDROID_HOME/emulator/emulator" -avd "$AVD" -no-window -no-audio \
        -no-snapshot -no-boot-anim -gpu swiftshader_indirect -port "$PORT" \
        >"$WORK/emulator.log" 2>&1 &
fi
adb -s "$SERIAL" wait-for-device
until [ "$(adb -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do
    sleep 5
done
say "device up: android $(adb -s "$SERIAL" shell getprop ro.build.version.release | tr -d '\r')"

adb -s "$SERIAL" shell rm -rf /data/local/tmp/wb && adb -s "$SERIAL" shell mkdir -p /data/local/tmp/wb
adb -s "$SERIAL" push target/aarch64-linux-android/release/examples/libart_probe.so \
    /data/local/tmp/wb/ >/dev/null
for f in "$WORK"/driver.dex "$WORK"/probe.dex "$WORK"/dex/*.dex; do
    adb -s "$SERIAL" push -q "$f" /data/local/tmp/wb/ >/dev/null 2>&1 \
        || adb -s "$SERIAL" push "$f" /data/local/tmp/wb/ >/dev/null
done

# Every generated dex goes on the classpath together; each declares one class.
CP=/data/local/tmp/wb/driver.dex
for f in "$WORK"/dex/*.dex; do CP="$CP:/data/local/tmp/wb/$(basename "$f")"; done

echo "── classes, loaded and linked by ART ──"
adb -s "$SERIAL" shell "cd /data/local/tmp/wb && dalvikvm -cp $CP Driver $CLASSES"

echo
echo "── the crate's own loader and RegisterNatives, inside ART ──"
adb -s "$SERIAL" shell \
    "cd /data/local/tmp/wb && dalvikvm -cp /data/local/tmp/wb/probe.dex Probe"
