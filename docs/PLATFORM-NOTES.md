# Platform notes

What each operating system and environment supports, what is `Unsupported`, and why. Where each value comes from is in [`SOURCES.md`](./SOURCES.md).

An unknown fact is reported as unknown: `Ok(None)` or `None` when the question has no answer here, `Error::Unsupported` when the reading does not exist on this platform, `PermissionDenied` when the OS refused. `Error::is_unavailable()` is true for the last two and for `NotFound`.

## Linux

The reference platform. Everything in the API works unprivileged except:

| Reading | Needs |
|---|---|
| `mem::modules()` | root (`/sys/firmware/dmi/tables/DMI` is mode 0400) |
| `device::Basis::Hardware`, the serial and UUID anchors | root (`product_uuid`, `board_serial`, and `product_serial` are mode 0400) |

- **Containers.** Memory and CPU answers are the container's: cgroup v1 and v2, hybrid hosts, nested limits, and LXCFS. `disk::list()` shows the container's mounts. The host's disks and distribution are neither visible nor guessed.
- **`host::sessions()`** reads the utmp file. A system that no longer keeps one lists nobody.
- **`host::timezone()`** gets the offset from `localtime_r`, so libc reads `TZ` itself. sys-lib's own reading of `TZ` only accepts a valid name and never opens one.
- **`host::clock_sync()`** is `Ok(None)` when a sandbox refuses `adjtimex`.
- **musl** works throughout. glibc's two utmp layouts are read; a musl system usually has no utmp at all.
- **32-bit.** glibc on x86 (i686) and Arm (armv7) has verified layouts and runs the whole suite in CI (armv7 under qemu). sys-lib calls the 64-bit-offset functions there (`statvfs64`, `statfs64`, `getrlimit64`, `readdir64`), so sizes and limits keep their range; `time_t` stays glibc's default 32 bits, so a local time zone offset after January 2038 is not read correctly on those targets (D-063). A 32-bit program on a 64-bit kernel reports the native machine and `cpu::is_translated() == true`, as WOW64 does. 32-bit musl and 32-bit Android have no verified layouts and answer `Unsupported`.
- **Reports.** `readiness`, `bench`, and `security` read `/proc/sys`, `/sys/kernel`, `/sys/devices/system/cpu`, and `/sys/firmware/efi`; a container usually hides the firmware and cpufreq files, which then leave their checks out or `None`. Temperatures come from hwmon, else the thermal zones; GPUs from DRM, named from `pci.ids` when installed.
- **`probe::Probe`** reads a host mounted elsewhere; mounts and network counters are the host's only when the agent shares the host's process namespace (`hostPID`).

## WSL

- Inside WSL 2, `virt::is_vm()` and `virt::is_wsl()` are both true.
- There is no DMI: `os::form_factor()` is `Unknown`, `host::model()` is `None`, and `Basis::Hardware` is unavailable, which `device::report()` shows as `Unsupported`.
- Memory is the utility VM's, limited by `.wslconfig`. That is correct for a process inside WSL.
- `eth0`'s MAC changes on every boot, so `PermanentMac` is not used.
- Without systemd, `/etc/machine-id` may be missing: `device::id` returns `NoIdentifierSource`.

## macOS

Everything below is checked by compiling and on CI runners, which are virtual machines; hardware-specific answers have not been seen on a real Mac by the test suite.

| Reading | Answer | Why |
|---|---|---|
| `mem::modules()` | `Unsupported` | Apple silicon has unified memory; no source was confirmed for Intel |
| `host::firmware().version` on Intel | `None` | no source was confirmed |
| `host::firmware().kind` on Apple silicon | `Unknown` | iBoot is not UEFI |
| `host::clock_sync()` | `Some` only when the kernel says synchronised | the flag starts set and `timed` may not clear it, so set proves nothing |
| `virt::cloud()` | `None` | firmware strings are not available |
| `power::on_ac()` on a desktop | `Some(true)` | IOKit reports mains power there |
| `cpu::pressure()`, `disk::pressure()`, `mem::events()` | `Ok(None)` | no such measure |
| `cpu::numa_nodes()` | one node with every CPU | macOS has no NUMA |

- Logical CPU numbers in `cpu::topology()` are positions, efficiency cores first: the OS offers no CPU-to-core mapping.
- `host_statistics64` is rate-limited for ordinary processes, so a process reading memory many times a second can get figures a moment old. CPU readings use `host_processor_info`, which is not.
- Under Rosetta 2, `cpu::arch()` is `X86_64`, `cpu::native_arch()` is `Aarch64`, and the instruction-set features are what Rosetta emulates (no AVX). CPUID is not used for virtualization detection there.
- `device`: there is no installation id, so `Basis::Install` uses the platform UUID, and the id changes when the logic board is replaced.
- `net::dns()` links the SystemConfiguration framework, present on every macOS.

- **Reports.** Temperatures are `Unsupported` (no documented API). GPUs come from IOKit: PCI display controllers, else the Apple silicon accelerator. The readiness report reads `kern.maxfiles`, `kern.ipc.somaxconn` (128 by default, so `net.backlog` warns on a default Mac), and the port range; APFS updates access times relatime-style unless mounted `strictatime`. Secure Boot, FileVault, SIP, and the firewall are `None`: no documented unprivileged API.
- **iOS** `mem::free()` is `os_proc_available_memory()`, the app's own budget, found at run time.

## Windows

| Reading | Answer | Why |
|---|---|---|
| `host::clock_sync()` | `Ok(None)` | no unprivileged call tells: a healthy machine can report adjustment disabled with W32Time stopped |
| `cpu::load()`, pressure, `mem::events()` | `Ok(None)` | no such measure |
| `mem::activity()` | `Unsupported` | the layout of the system call's structure cannot be checked against the headers |
| per-core CPU usage past 64 CPUs or with several processor groups | `Unsupported` | the per-group call is not verified; the total stays correct |
| `power::batteries()` | one combined entry | per-battery calls are not verified |
| `cpu::numa_nodes()` memory | `None` with several nodes | Windows offers free memory per node, not attached memory |
| `net::primary()` | IPv4 only | an IPv6-only default route gives `None` |
| `host::sessions()` | `Unsupported` where wtsapi32 is missing; other users' sessions left out without the right | the DLL is loaded at run time |
| `host::timezone().abbreviation`, `os::codename()` | `None` | none exists |

- `process::launch()` calls a process in session 0 with no console a service. A scheduled task running as SYSTEM looks the same.
- A host with Hyper-V, WSL 2, or virtualization-based security enabled is not reported as a VM, though CPUID says "Microsoft Hv": the root-partition flag tells them apart.
- `net::totals()` costs milliseconds here, not microseconds: sample it, do not poll it in a tight loop.
- Nano Server: CI runs a real binary there, so the imports load. user32, tbs, and powrprof are loaded at run time from System32 (D-070): on Nano Server `gpu::list()` is `Unsupported` and the power plan, TPM, and turbo fields are unknown.
- **Reports.** `sensors::temperatures()` lists drives only; there is no unprivileged documented CPU temperature. The readiness report's ephemeral-port check is `Unknown` (no documented source), and the open-file check passes (handles have no per-process limit). `security::posture()` reads the registry for Secure Boot, VBS, HVCI, and the firewall (true or false only when every profile agrees); BitLocker is `None`.
- `process::threads()` takes a process snapshot: tens of milliseconds on a host with many processes.

## Android

Best effort, build-checked in CI, never run. Android uses the Linux paths where SELinux allows them; denied reads are `PermissionDenied`. The form factor comes from `ro.build.characteristics`, with Phone as the default at medium confidence. `host::clock_sync()` is `Ok(None)`.

## iOS

Best effort, build-checked in CI, never run.

| Reading | Answer |
|---|---|
| `power::*` | `Unsupported` |
| `host::sessions()`, `device` anchors | `Unsupported` |
| `host::clock_sync()` | `Ok(None)` |
| `disk::of()` | `statfs` only: no kind or label |

The form factor comes from `uname().machine`, or `SIMULATOR_MODEL_IDENTIFIER` on the simulator.

## FreeBSD

64-bit, FreeBSD 12 and later, from sysctls, `getfsstat`, `getifaddrs`, `kenv`, and `/etc/os-release`. CI runs the whole suite in a FreeBSD 14 virtual machine as an unprivileged user.

| Reading | Answer | Why |
|---|---|---|
| `virt::container()` in a jail | `Container::Other` | jails are reported as containers without a variant of their own yet (D-095) |
| `process::io()`, `gpu::list()`, `mem::modules()` | `Unsupported` | FreeBSD counts block operations, not bytes; `/dev/pci` and the SMBIOS table are root-only |
| `cpu::topology()` | flat | `kern.sched.topology_spec` is XML and not parsed |
| `cpu::quota()`, pressure, `mem::events()`, huge pages | `Ok(None)` | no such measure |
| `mem::Memory::cached` | `None` | no page-cache figure; the ZFS ARC counts as wired |
| `power::batteries()` | one combined entry | `hw.acpi.battery` combines them |
| `security::Posture::firewall` | ipfw only | a pf firewall reads as unknown |
| `host::processes()` | the user's own processes | with `security.bsd.see_other_uids=0` |

Temperatures need `coretemp` or `amdtemp` loaded, and the CPU frequency needs `cpufreq`. The device id's `InstallId` is `kern.hostuuid` (from `/etc/hostid`); the SMBIOS UUID and serials come from `kenv` without privileges.

## Everything else

Any other target compiles and answers honestly: what std knows, and `Unsupported` for the rest.
