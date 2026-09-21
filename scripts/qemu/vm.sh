#!/usr/bin/env bash
# A Linux VM with a real Bluetooth stack, for testing the Linux backends.
#
#   ./scripts/qemu/vm.sh setup     # download, provision, build the module (slow, once)
#   ./scripts/qemu/vm.sh start     # boot it
#   ./scripts/qemu/vm.sh radio     # virtual controllers + bluetoothd + the peer
#   ./scripts/qemu/vm.sh test      # sync the source, build, run the round-trip
#   ./scripts/qemu/vm.sh ssh ...   # run a command in the VM
#   ./scripts/qemu/vm.sh stop
#
# Why a VM at all: Docker Desktop's linuxkit kernel is built with
# `# CONFIG_BT is not set`, so a container has no Bluetooth subsystem, no
# AF_BLUETOOTH sockets and no controller — the D-Bus mock in docker/ is as far
# as a container can go. A VM brings its own kernel.
#
# Two limitations of the emulated radio are worth knowing before reading a
# failure as a bug; both are in PLATFORMS.md under "Linux under QEMU".
set -euo pipefail
cd "$(dirname "$0")/../.."

DIR=target/qemu
IMG=$DIR/debian.qcow2
KEY=$DIR/id_vm
PORT=2222
IMAGE_URL=https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-arm64.qcow2
FIRMWARE=/opt/homebrew/share/qemu/edk2-aarch64-code.fd

vmssh() { ssh -q -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
              -o LogLevel=ERROR -i "$KEY" -p "$PORT" tester@127.0.0.1 "$@"; }
say() { printf '  \033[2m%s\033[0m\n' "$*"; }

wait_for_ssh() {
    for _ in $(seq 1 90); do vmssh true 2>/dev/null && return 0; sleep 5; done
    echo "  ✗ VM did not come up" >&2; return 1
}

case "${1:?usage: vm.sh setup|start|radio|test|ssh|stop}" in

setup)
    mkdir -p "$DIR/seed"
    [ -f "$IMG" ] || { say "downloading the cloud image"; curl -sSL -o "$IMG" "$IMAGE_URL"; }
    [ -f "$KEY" ] || ssh-keygen -q -t ed25519 -N '' -f "$KEY" -C webbluetooth-vm
    qemu-img resize -q "$IMG" 13G 2>/dev/null || true

    cat > "$DIR/seed/meta-data" <<EOF
instance-id: wbtest-1
local-hostname: wbtest
EOF
    cat > "$DIR/seed/user-data" <<EOF
#cloud-config
hostname: wbtest
users:
  - name: tester
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - $(cat "$KEY.pub")
EOF
    rm -f "$DIR/seed.iso"
    hdiutil makehybrid -quiet -iso -joliet -default-volume-name cidata -o "$DIR/seed.iso" "$DIR/seed"

    "$0" start
    wait_for_ssh

    # The Debian *cloud* kernel ships no Bluetooth modules at all, and the
    # generic arm64 kernel ships Bluetooth but is built with
    # `# CONFIG_BT_HCIVHCI is not set` — so the virtual-controller driver has to
    # be built out of tree. It is one self-contained file.
    say "installing the generic kernel and bluez (slow)"
    vmssh 'sudo DEBIAN_FRONTEND=noninteractive apt-get -qq update
           sudo DEBIAN_FRONTEND=noninteractive apt-get -qq -y install \
                linux-image-arm64 bluez python3-dbus python3-gi >/dev/null
           sudo DEBIAN_FRONTEND=noninteractive apt-get -qq -y purge \
                "linux-image-*-cloud-arm64" linux-image-cloud-arm64 >/dev/null 2>&1 || true
           sudo update-grub >/dev/null 2>&1'
    vmssh 'sudo reboot' 2>/dev/null || true
    sleep 10; wait_for_ssh

    say "building hci_vhci out of tree"
    vmssh 'set -e
           sudo DEBIAN_FRONTEND=noninteractive apt-get -qq -y install \
                linux-headers-$(uname -r) linux-source-6.1 build-essential >/dev/null
           mkdir -p ~/vhci && cd ~/vhci
           tar -xf /usr/src/linux-source-6.1.tar.xz --strip-components=3 \
               linux-source-6.1/drivers/bluetooth/hci_vhci.c
           printf "obj-m += hci_vhci.o\n" > Makefile
           make -s -C /lib/modules/$(uname -r)/build M=$HOME/vhci modules >/dev/null 2>&1'

    # Debian ships no `btvirt`, which is the userspace side of /dev/vhci: the
    # kernel driver only creates the device node, something has to emulate the
    # controller behind it.
    say "building btvirt from bluez source"
    vmssh 'set -e
           sudo DEBIAN_FRONTEND=noninteractive apt-get -qq -y install \
                libglib2.0-dev libdbus-1-dev libudev-dev libical-dev libreadline-dev \
                libbluetooth-dev pkg-config curl xz-utils >/dev/null
           cd ~ && [ -d bluez-5.66 ] || {
               curl -sSLO https://www.kernel.org/pub/linux/bluetooth/bluez-5.66.tar.xz
               tar xf bluez-5.66.tar.xz; }
           cd bluez-5.66
           [ -f emulator/btvirt ] || {
               ./configure --prefix=/usr --enable-testing --enable-tools \
                   --disable-systemd --disable-manpages --disable-cups \
                   --disable-obex --disable-udev >/dev/null 2>&1
               make -j4 emulator/btvirt tools/btgatt-server >/dev/null 2>&1; }'

    say "installing rust"
    vmssh 'test -x ~/.cargo/bin/cargo || curl -sSf https://sh.rustup.rs | \
           sh -s -- -y --default-toolchain 1.94.0 --profile minimal >/dev/null 2>&1'
    say "setup complete — ./scripts/qemu/vm.sh radio"
    ;;

start)
    pgrep -f "qemu-system-aarch64.*$IMG" >/dev/null && { say "already running"; exit 0; }
    nohup qemu-system-aarch64 \
        -machine virt,accel=hvf -cpu host -smp 4 -m 4096 \
        -bios "$FIRMWARE" \
        -drive if=virtio,file="$IMG",format=qcow2 \
        -drive if=virtio,file="$DIR/seed.iso",format=raw,readonly=on \
        -netdev user,id=n0,hostfwd=tcp::$PORT-:22 -device virtio-net-pci,netdev=n0 \
        -nographic >"$DIR/console.log" 2>&1 &
    say "booting (console in $DIR/console.log)"
    ;;

radio)
    wait_for_ssh
    vmssh 'sudo modprobe bluetooth
           lsmod | grep -q hci_vhci || sudo insmod ~/vhci/hci_vhci.ko
           pgrep -x btvirt >/dev/null || sudo sh -c \
               "nohup /home/tester/bluez-5.66/emulator/btvirt -l2 >/tmp/btvirt.log 2>&1 &"
           sleep 2
           sudo systemctl restart bluetooth; sleep 2'
    scp -q -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
        -i "$KEY" -P "$PORT" scripts/qemu/peer.py tester@127.0.0.1:~/peer.py
    vmssh 'sudo pkill -x python3 2>/dev/null; sleep 1
           sudo sh -c "nohup python3 /home/tester/peer.py --adapter hci1 >/tmp/peer.log 2>&1 &"
           sleep 5; cat /tmp/peer.log'
    vmssh 'hciconfig -a 2>/dev/null | grep -E "^hci|BD Address" | head -4'
    ;;

test)
    tar czf - --exclude=target --exclude='*.qcow2' . 2>/dev/null | \
        vmssh 'rm -rf ~/webbluetooth && mkdir -p ~/webbluetooth && tar xzf - -C ~/webbluetooth'

    echo "── BlueZ backend ──"
    vmssh 'cd ~/webbluetooth && ~/.cargo/bin/cargo build -q --release --examples 2>&1 | tail -3
           ./target/release/examples/doctor | head -12
           timeout 90 ./target/release/examples/roundtrip'

    # The daemon-free backend, against the same controller and the same peer.
    #
    # This is the only place it can be exercised at all: it speaks ATT over an
    # L2CAP socket and HCI over a raw one, so a container cannot run it — and
    # until now nothing did, which left the Service Changed subscription and
    # the whole ATT client verified by inspection only.
    #
    # `bluetoothd` keeps running, and that is not a compromise.
    #
    # This backend never speaks to it: scanning is a raw HCI socket, GATT is
    # ATT over an L2CAP socket, and neither goes near D-Bus. So the daemon
    # cannot answer on its behalf or mask a failure — there is no path from
    # one to the other.
    #
    # It also cannot be stopped, because the peer advertises *through* it on
    # hci1. Stopping it removes the device this is supposed to find, which
    # fails with "no device was chosen" and looks exactly like a discovery
    # bug in the code under test.
    echo "── linux-hci backend (no daemon involved) ──"
    vmssh 'cd ~/webbluetooth
           ~/.cargo/bin/cargo build -q --release --features linux-hci --examples 2>&1 | tail -3
           # Scanning needs CAP_NET_RAW; GATT over L2CAP needs nothing.
           sudo timeout 90 ./target/release/examples/roundtrip'
    ;;

ssh)  shift; vmssh "$@" ;;
stop) pkill -f "qemu-system-aarch64.*$IMG" && say "stopped" || say "not running" ;;
*)    echo "unknown subcommand: $1" >&2; exit 1 ;;
esac
