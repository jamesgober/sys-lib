#!/usr/bin/env bash
# Captures `docker-cgroup1-limited` on a real cgroup v1 host (D-027). GitHub's
# runners are cgroup v2, so this boots Ubuntu 20.04, whose default is still
# the v1 (hybrid) layout, in a KVM virtual machine, installs Docker there, and
# runs capture.sh in a limited container exactly as the cgroup v2 job does.
# The result comes back over a second serial port as a base64 tarball.
#
# Usage: capture_cgroup1_vm.sh <out-dir>     (Linux with KVM, qemu, cloud-localds)
set -euo pipefail

out="$1"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
mkdir -p "$out"

curl -fsSL -o "$work/base.img" \
  https://cloud-images.ubuntu.com/focal/current/focal-server-cloudimg-amd64.img
qemu-img create -q -f qcow2 -F qcow2 -b "$work/base.img" "$work/disk.qcow2" 10G

capture_b64="$(base64 -w0 "$here/capture.sh")"
cat > "$work/user-data" <<EOF
#cloud-config
package_update: true
packages: [docker.io]
write_files:
  - path: /root/fixture/capture.sh
    encoding: b64
    content: $capture_b64
runcmd:
  - [bash, -c, "stty -F /dev/ttyS1 raw -echo; grep -E 'cgroup' /proc/self/mountinfo > /dev/ttyS1; mkdir -p /out && docker run --rm --memory=256m --memory-reservation=200m --cpus=1.5 --cpuset-cpus=0-1 -v /root/fixture:/work:ro -v /out:/out ubuntu:24.04 bash /work/capture.sh docker-cgroup1-limited /out; echo CAPTURE-BEGIN > /dev/ttyS1; tar -C /out -czf - . | base64 -w0 > /dev/ttyS1; echo > /dev/ttyS1; echo CAPTURE-END > /dev/ttyS1; poweroff"]
EOF
printf 'instance-id: sys-lib-cgroup1\nlocal-hostname: fixture-host\n' > "$work/meta-data"
cloud-localds "$work/seed.iso" "$work/user-data" "$work/meta-data"

# ttyS0 carries the kernel console; ttyS1 carries only the capture.
timeout 35m qemu-system-x86_64 -enable-kvm -cpu host -m 2048 -smp 2 -display none \
  -drive file="$work/disk.qcow2",if=virtio \
  -drive file="$work/seed.iso",format=raw,if=virtio \
  -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
  -serial file:"$work/console.log" -serial file:"$work/capture.log" || true

echo "--- cgroup mounts in the VM"
sed -n '1,/CAPTURE-BEGIN/p' "$work/capture.log" | grep -v CAPTURE-BEGIN || true
if ! grep -q CAPTURE-END "$work/capture.log"; then
  echo "no capture came back; the console says:" >&2
  tail -n 80 "$work/console.log" >&2
  exit 1
fi
sed -n '/CAPTURE-BEGIN/,/CAPTURE-END/p' "$work/capture.log" | sed '1d;$d' | tr -d '\r\n' \
  | base64 -d | tar -C "$out" -xzf -
cat "$out/docker-cgroup1-limited/CAPTURED.txt"
