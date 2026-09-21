# Linux test environment for webbluetooth.
#
# Two jobs:
#
#  * The Linux backend, against a **mocked BlueZ** — no container has a
#    Bluetooth controller, least of all on a macOS host where Docker runs in a
#    VM with no passthrough at all, so python-dbusmock serves the real org.bluez
#    D-Bus interfaces instead.
#  * The Android backend's generated DEX, checked with Android's own `dexdump`.
#    That will not run BLE, but it does prove the classes we emit are ones the
#    platform's verifier accepts.
FROM rust:1.94-slim-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
        dbus \
        python3 \
        python3-dbus \
        python3-gi \
        python3-dbusmock \
        bluez \
        pkg-config \
        dexdump \
        default-jdk-headless \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work
ENV CARGO_TERM_COLOR=always
# Keep the build cache in the image layer rather than the bind mount, so a
# macOS host's target/ directory is never touched.
ENV CARGO_TARGET_DIR=/build

COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
