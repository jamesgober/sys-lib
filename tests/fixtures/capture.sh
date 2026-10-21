#!/usr/bin/env bash
# Capture a fixture machine for sys-lib's Linux backend tests.
#
#   tests/fixtures/capture.sh <name> [<output-dir>]
#
# Run by hand on a real Linux machine (or inside a container or a limited
# systemd scope). It copies only the files the Linux backend reads, each capped
# at 1 MiB, into <output-dir>/<name>/ (default: next to this script), then
# replaces every identifier with a fixed fake value:
#
#   host name                      -> fixture-host
#   /etc/machine-id, dbus id       -> 5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e (only when valid)
#   MAC addresses                  -> 00:00:5e:00:53:NN (RFC 7042 documentation range)
#   ARM serial in /proc/cpuinfo    -> zeros
#   user names in paths            -> user
#   time zone                      -> Europe/London (a real zone with daylight saving)
#
# It never copies DMI serial numbers or UUIDs, keys, tokens, or Kubernetes
# service-account files (only that the directory exists). Read the result
# before committing it.
#
# It also runs the OS's own tools (nproc, getconf, free, df, systemd-detect-virt)
# and stores their output in TOOLS.txt, so a fixture carries the answers the
# real machine gave. This script may run programs; sys-lib itself never does.

set -euo pipefail
export LC_ALL=C

readonly SCRIPT_VERSION=1
readonly MAX_BYTES=1048576

die() { echo "capture.sh: $*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "run this on Linux"
[ $# -ge 1 ] || die "usage: capture.sh <name> [<output-dir>]"
name="$1"
[[ "$name" =~ ^[a-z0-9][a-z0-9.-]{0,63}$ ]] || die "name: lowercase letters, digits, '.', '-'; at most 64"
base="${2:-$(cd "$(dirname "$0")" && pwd)}"
out="$base/$name"
[ ! -e "$out" ] || die "$out already exists"
mkdir -p "$out"

# copy <path>: one file's content. /proc and /sys report size 0, so read it.
copy() {
    local src="$1"
    [ -f "$src" ] && [ -r "$src" ] || return 0
    local dst="$out$src"
    mkdir -p "$(dirname "$dst")"
    head -c "$MAX_BYTES" "$src" > "$dst" 2>/dev/null || rm -f "$dst"
}

# mark <path>: record that a file or directory exists, without its content.
mark() {
    local src="$1"
    [ -e "$src" ] || return 0
    if [ -d "$src" ] && [ ! -L "$src" ]; then
        mkdir -p "$out$src"
    else
        mkdir -p "$(dirname "$out$src")"
        : > "$out$src"
    fi
}

# link <path>: record a symbolic link's target in <path>.link (fixtures hold no
# real links, so they survive any checkout; the test backend reads the file).
link() {
    local src="$1"
    [ -L "$src" ] || return 0
    mkdir -p "$(dirname "$out$src")"
    readlink "$src" > "$out$src.link"
}

# each <glob> <file...>: copy the named files inside every directory matching the glob.
each() {
    local glob="$1"; shift
    local dir f
    for dir in $glob; do
        [ -d "$dir" ] || continue
        for f in "$@"; do copy "$dir/$f"; done
    done
}

# --- /proc --------------------------------------------------------------------
for f in meminfo stat loadavg uptime cpuinfo pressure/memory pressure/cpu \
         net/dev net/route net/ipv6_route \
         sys/kernel/osrelease sys/kernel/ostype sys/kernel/version sys/kernel/hostname \
         xen/capabilities 1/cgroup \
         self/status self/cgroup self/mountinfo self/statm self/stat self/limits; do
    copy "/proc/$f"
done
mark /proc/sys/fs/binfmt_misc/WSLInterop
each /proc/device-tree model chassis-type compatible
each /proc/device-tree/hypervisor compatible

# --- /sys: CPUs ---------------------------------------------------------------
for f in present possible online kernel_max smt/control; do
    copy "/sys/devices/system/cpu/$f"
done
each "/sys/devices/system/cpu/cpu[0-9]*/topology" core_id physical_package_id die_id cluster_id \
    core_cpus_list thread_siblings_list
each "/sys/devices/system/cpu/cpu[0-9]*/cache/index[0-9]*" level type size coherency_line_size \
    shared_cpu_list
each "/sys/devices/system/cpu/cpu[0-9]*/cpufreq" cpuinfo_max_freq cpuinfo_min_freq base_frequency \
    scaling_cur_freq
each "/sys/devices/system/cpu/cpu[0-9]*" cpu_capacity
each "/sys/devices/system/cpu/cpu[0-9]*/regs/identification" midr_el1
copy /sys/devices/cpu_core/cpus
copy /sys/devices/cpu_atom/cpus
each "/sys/devices/system/node/node[0-9]*" cpulist meminfo

# --- /sys: network, power, block, firmware --------------------------------------
for dir in /sys/class/net/*; do
    [ -d "$dir" ] || continue
    for f in address addr_assign_type operstate mtu speed type flags ifindex carrier; do
        copy "$dir/$f"
    done
    for f in device wireless phy80211; do mark "$dir/$f"; done
    each "$dir/statistics" rx_bytes tx_bytes rx_packets tx_packets rx_errors tx_errors \
        rx_dropped tx_dropped
done
each "/sys/class/power_supply/*" type scope online status capacity uevent
each "/sys/block/*" removable
each "/sys/block/*/queue" rotational
# DMI: the world-readable files only (0444); serials and UUIDs are 0400 and never copied.
each /sys/class/dmi/id chassis_type sys_vendor product_name product_family product_version \
    board_vendor board_name bios_vendor
copy /sys/firmware/acpi/pm_profile
copy /sys/hypervisor/type

# --- cgroups: the files of this process's cgroup and every ancestor ----------------
cgroup_files_v2="cgroup.controllers memory.max memory.high memory.current memory.stat memory.swap.max \
memory.pressure cpu.max cpuset.cpus.effective"
cgroup_files_v1="memory.stat memory.usage_in_bytes memory.limit_in_bytes memory.soft_limit_in_bytes \
memory.memsw.limit_in_bytes cpu.cfs_quota_us cpu.cfs_period_us cpuset.cpus cpuset.effective_cpus"
while IFS=' ' read -r _ _ _ root mount _ rest; do
    fstype="${rest#* - }"; fstype="${fstype%% *}"
    case "$fstype" in
        cgroup2) files="$cgroup_files_v2" ;;
        cgroup) files="$cgroup_files_v1" ;;
        *) continue ;;
    esac
    for path in $(cut -d: -f3 /proc/self/cgroup | sort -u); do
        # Walk from the mount root to the cgroup, at most 64 levels.
        rel="${path#"$root"}"; rel="${rel#/}"
        dir="$mount"; depth=0
        for f in $files; do copy "$dir/$f"; done
        IFS='/' read -r -a parts <<< "$rel"
        for part in "${parts[@]}"; do
            [ -n "$part" ] || continue
            depth=$((depth + 1)); [ "$depth" -le 64 ] || break
            dir="$dir/$part"
            for f in $files; do copy "$dir/$f"; done
        done
    done
done < /proc/self/mountinfo

# --- /etc, /usr, /run, markers ---------------------------------------------------
copy /etc/os-release
copy /usr/lib/os-release
link /etc/os-release
copy /etc/timezone
link /etc/localtime
copy /etc/machine-id
copy /var/lib/dbus/machine-id
if [ -r /etc/machine-info ]; then
    mkdir -p "$out/etc"
    grep -E '^CHASSIS=' /etc/machine-info > "$out/etc/machine-info" || true
fi
copy /run/systemd/container
mark /run/.containerenv
mark /.dockerenv
mark /var/run/secrets/kubernetes.io/serviceaccount

# --- Scrub identifiers ------------------------------------------------------------
[ -f "$out/proc/sys/kernel/hostname" ] && echo fixture-host > "$out/proc/sys/kernel/hostname"

for f in "$out/etc/machine-id" "$out/var/lib/dbus/machine-id"; do
    [ -f "$f" ] || continue
    if grep -qE '^[0-9a-fA-F]{32}$' "$f"; then
        echo 5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e > "$f"
    fi
done

n=0
declare -A fake_mac=()
for f in "$out"/sys/class/net/*/address; do
    [ -f "$f" ] || continue
    mac="$(tr 'A-F' 'a-f' < "$f" | tr -d '\n')"
    case "$mac" in
        00:00:00:00:00:00|ff:ff:ff:ff:ff:ff|"") continue ;;
    esac
    if [ -z "${fake_mac[$mac]:-}" ]; then
        n=$((n + 1)); [ "$n" -le 255 ] || die "more than 255 MAC addresses"
        fake_mac[$mac]="$(printf '00:00:5e:00:53:%02x' "$n")"
    fi
    echo "${fake_mac[$mac]}" > "$f"
done

[ -f "$out/etc/timezone" ] && echo Europe/London > "$out/etc/timezone"
[ -f "$out/etc/localtime.link" ] && echo /usr/share/zoneinfo/Europe/London > "$out/etc/localtime.link"

[ -f "$out/proc/cpuinfo" ] && sed -i -E 's/^(Serial[[:space:]]*:[[:space:]]*).*/\10000000000000000/' "$out/proc/cpuinfo"

for f in "$out/proc/self/mountinfo" "$out/proc/self/cgroup" "$out/proc/1/cgroup"; do
    [ -f "$f" ] || continue
    sed -i -E 's#/home/[^/ ]+#/home/user#g; s#/Users/[^/ ]+#/Users/user#g; s#/mnt/c/Users/[^/ ]+#/mnt/c/Users/user#g' "$f"
done

# --- What the OS's own tools said ---------------------------------------------------
{
    echo "# Output of the OS's tools at capture time (for live-comparison tests)."
    run() { echo "## $*"; "$@" 2>&1 || echo "(exit $?)"; echo; }
    run nproc
    run nproc --all
    run getconf _NPROCESSORS_CONF
    run getconf _NPROCESSORS_ONLN
    run getconf PAGESIZE
    run getconf CLK_TCK
    run free -b
    run df -B1 --output=source,fstype,size,used,avail,target /
    run uname -srvmo
    command -v systemd-detect-virt >/dev/null && run systemd-detect-virt
    command -v systemd-detect-virt >/dev/null && run systemd-detect-virt --container
} > "$out/TOOLS.txt"
sed -i -E "s/$(hostname 2>/dev/null || echo __none__)/fixture-host/g" "$out/TOOLS.txt"

{
    echo "fixture: $name"
    echo "captured: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "capture.sh: version $SCRIPT_VERSION"
    echo "kernel: $(uname -r)"
    echo "machine: $(uname -m)"
    echo "as root: $([ "$(id -u)" = 0 ] && echo yes || echo no)"
    echo "files: $(find "$out" -type f | wc -l)"
} > "$out/CAPTURED.txt"

echo "captured $name into $out ($(find "$out" -type f | wc -l) files). Review it before committing."
