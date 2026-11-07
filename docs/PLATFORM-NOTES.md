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
- Nano Server: CI runs a real binary there, so the imports load.

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

## Everything else

FreeBSD and 32-bit Linux arrive in `0.6`. Until then they, and any other target, compile and answer honestly: what std knows, and `Unsupported` for the rest.
