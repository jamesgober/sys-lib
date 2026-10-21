# sys-lib - Project Plan

> The design, strategy, and decision record for `sys-lib`. A living document: every design decision, idea, or open question about sys-lib is written here or in [`ROADMAP.md`](./ROADMAP.md) before it is acted on.
>
> **Read order:** [`../REPS.md`](../REPS.md) &rarr; [`DIRECTIVES.md`](./DIRECTIVES.md) &rarr; **this file** &rarr; [`ROADMAP.md`](./ROADMAP.md).
>
> **Status:** scaffold, `0.1.0`. Design lock is `0.2.0`; the public API is then written out in full in [`../docs/API.md`](../docs/API.md).

---

## Contents

1. [Purpose](#1-purpose)
2. [Where it sits](#2-where-it-sits)
3. [Scope](#3-scope)
4. [Requirements](#4-requirements)
5. [Answers: constants, static facts, live readings, samplers](#5-answers-constants-static-facts-live-readings-samplers)
6. [Units and errors](#6-units-and-errors)
7. [Containers and limits](#7-containers-and-limits)
8. [CPU usage](#8-cpu-usage)
9. [The boot kit: build info, instance info, process guard](#9-the-boot-kit-build-info-instance-info-process-guard)
10. [Detection: OS identity, form factor, virtualization](#10-detection-os-identity-form-factor-virtualization)
11. [Memory definitions](#11-memory-definitions)
12. [Device identity](#12-device-identity)
13. [API outline](#13-api-outline)
14. [Threats](#14-threats)
15. [Architecture](#15-architecture)
16. [Ecosystem and dependencies](#16-ecosystem-and-dependencies)
17. [Competitors and parity](#17-competitors-and-parity)
18. [Performance](#18-performance)
19. [Testing](#19-testing)
20. [Documentation deliverables](#20-documentation-deliverables)
21. [Open decisions](#21-open-decisions)
22. [Ideas parking lot](#22-ideas-parking-lot)
23. [Decision log](#23-decision-log)

---

## 1. Purpose

sys-lib answers the questions a program asks about the machine it runs on, with one plain function call each, on Linux, macOS, and Windows:

```rust
use sys_lib as sys;

println!("{}", sys::os::name());                 // "Ubuntu 24.04.1 LTS", "macOS 15.1 Sequoia", "Windows 11 Pro"
println!("{}", sys::cpu::name());                // "AMD Ryzen 9 7950X"
println!("{} cores, {} threads", sys::cpu::physical(), sys::cpu::logical());
println!("{} of {}", sys::mem::used()?, sys::mem::total()?);    // "41.2 GiB of 125.6 GiB"
println!("{}", sys::mem::used_percent()?);       // "32.8%"
println!("{}", sys::cpu::used()?);               // "12.4%" (blocks for 100 ms)
if sys::os::is_laptop() && sys::power::on_ac()? == Some(false) { /* save energy */ }
```

It is built in four layers, each usable alone:

1. **Static facts**, read once and cached for the life of the process: OS name and version, CPU model, core counts, CPU features, cache sizes, virtualization, form factor. After the first call each costs about a nanosecond.
2. **Live readings**, read fresh on every call, with no allocation for single values: memory in use, swap, uptime, load, this process's memory and CPU time, disk space.
3. **Samplers**, for values that need two readings over time (CPU usage, network rates), held by the caller, never by the library.
4. **Boot kit**, what a service does in its first milliseconds: build info, instance info, and the process guard (single-instance lock, safe file permissions, open-file limit).

And one opt-in module, **device identity** (feature `device-id`), which turns identifiers that rarely change into an application-specific, privacy-preserving device id with an honest stability report.

The engineering promises:

- **Container-aware.** Inside Docker or Kubernetes, `mem::total()` is the container's memory limit, not the host's, and `cpu::available()` respects the CPU quota. The host values are one call away (`mem::host_total()`).
- **Honest.** When a fact cannot be known (a laptop's chassis inside a VM, a CPU temperature on Windows without Administrator), the answer is `Unknown`, `None`, or `Error::Unsupported`. Never a guess presented as a fact, never a simulated number.
- **No subprocesses.** sys-lib never runs `lsb_release`, `ioreg`, `wmic`, `sysctl`, `powershell`, or any other program at run time. Every reading is a file read or a system call.
- **Zero third-party runtime dependencies.** The OS is called directly through small, reviewed declarations (`extern "system"` / `extern "C"`), confined to one `sys` module per platform.

### 1.1 What it is not

- **Not a monitoring agent.** It never starts a thread or a timer. Periodic collection is the caller's loop (metrics-lib, tower, a health job).
- **Not a process table.** It reports on **this** process and on the machine, not on other processes.
- **Not a system configuration tool.** It never changes system settings. The only things it changes are this process's own umask and open-file soft limit, and only inside the process guard, which the caller asks for explicitly.
- **Not an authentication mechanism.** The device id identifies a machine for an application's own bookkeeping (licensing seats, "new device" notices, de-duplicating telemetry). An administrator can spoof it trivially; it must never be the only proof of anything (section 12.6).
- **Not a time, path, environment, or id library.** Those exist: `clock-lib`, `path-lib`, `env-lib`, `id-forge`. sys-lib uses them and never duplicates them.

---

## 2. Where it sits

### 2.1 Who uses it

| User | Uses | When |
|---|---|---|
| WASP kernel (service-suite) | build info, instance info, process guard, `process::is_elevated`, `os::name`, `cpu::available` | boot phase 0 |
| WASP runtime and task-io | `cpu::available()` to size worker pools, `cpu::topology()` for placement | phases 1 to 3 |
| tower (watchdog) | `mem::snapshot()`, `process::memory()`, `mem::pressure()`, `host::uptime()` | memory watermark and "memory critical" signal |
| metrics-lib exporters | live readings and samplers | each scrape |
| Hive DB | process guard (lock beside the data directory), `disk::space()`, `mem::total()` for cache sizing | start-up and before compaction |
| shield-sec | `process::is_elevated()`, `process::user()` | verifying the privilege drop |
| CLI tools, installers, support bundles | `summary()`, `os::info()`, `device::report()` | diagnostics |
| licensing or "new device" logic | `device::id(app)` | sign-in and activation (as a hint, not proof) |

### 2.2 Boot order

```
 PHASE 0
   1 build info          sys_lib::build_info!()       compiled in; the first log line
   2 instance info       sys_lib::Instance::start()   id, start time, host, pid, user
   3 process guard       sys_lib::process::guard()    lock -> umask -> open-file limit
                                                       (fails: exit; nothing was opened)
   4 env, paths, config, logger, audit, metrics, ...   (files now created with safe permissions)
   5 facts logged once   sys_lib::summary()            OS, CPU, memory, container, limits
 PHASES 1-3
   6 worker pools sized  sys_lib::cpu::available()
 PHASE 4
   7 listeners bound
   8 shield-sec drops privileges, then verifies with sys_lib::process::is_elevated() == false
   9 door opens
 RUNNING
   tower and metrics read live values on their own schedule; sys-lib does nothing in between
```

The lock comes before any data file is touched, the umask before any file is created, and privileges are dropped only after the sockets that need them are bound.

### 2.3 Layering

```
                 ┌──────────── service-suite (foundation) ─────────────┐
                 │ kernel · runtime · config · logger · audit · metrics │
                 │        │                       │                    │
                 │        ▼                       ▼                    │
                 │   ┌─────────┐   uses    ┌───────────┐               │
                 │   │ sys-lib │──────────▶│ path-lib  │               │
                 │   └─────────┘──────────▶│ clock-lib │               │
                 │        │ (opt-in)       │ id-forge  │               │
                 │        └───────────────▶│ crypt-io  │               │
                 └──────────────────────────────────────────────────────┘
                          │ system calls and file reads only
                          ▼
     Linux: /proc, /sys, sysconf    macOS: sysctl, mach, IOKit    Windows: kernel32, advapi32,
                                                                   iphlpapi, powrprof, ntdll
```

---

## 3. Scope

### 3.1 In scope

| Area | Module | Summary |
|---|---|---|
| Build | `build` | compile-time facts of the application (name, version, commit, dirty, build time, profile, target, rustc, features) |
| Instance | `instance` | start-time facts of this run (ULID, start time, host, pid, user at start) |
| Process | `process` | this process: memory, CPU time, threads, descriptors, limits, elevation, user; the process guard |
| Operating system | `os` | kind, name, version, kernel, distribution, edition, form factor |
| CPU | `cpu` | name, vendor, architecture, counts, frequency, usage, features, topology, caches, NUMA, quota |
| Memory | `mem` | total, used, free, percentages, swap, page size, limits (cgroup, job object), pressure |
| Disks | `disk` | space for a path, mounted filesystems, kind (SSD, HDD, network, removable) |
| Network | `net` | interfaces, MAC addresses (permanent or random), addresses, state, speed, counters |
| Host | `host` | host name, uptime, boot time, load average, time zone name, locale, hardware model |
| Virtualization | `virt` | VM, hypervisor vendor, container runtime, Kubernetes, WSL |
| Power | `power` | battery presence, charge, state, AC power |
| Device identity | `device` (feature `device-id`) | application-scoped device id, fingerprint, stability report |
| Sensors | `sensors` | temperatures, best effort (after 1.0) |
| GPU | `gpu` | adapters, best effort (after 1.0) |
| Units | crate root | `Bytes`, `Percent`, `Mhz` |

### 3.2 Out of scope, and where it goes

| Not here | Why | Where |
|---|---|---|
| dropping privileges, disabling core dumps | a security control that must be verified | `shield-sec` (sys-lib only reports `is_elevated`) |
| signal handling | lifecycle | `signal-mod`, the WASP kernel |
| application and OS directories, `exe()`, `temp()` | already solved | `path-lib` |
| environment variables, `.env` files | already solved | `env-lib` |
| time readings, calendars, time zone arithmetic | already solved; arithmetic belongs to calendar crates | `clock-lib`; sys-lib returns the zone **name** only |
| generating ids, random numbers | already solved | `id-forge`, `mod-rand` |
| listing, inspecting, or signalling other processes | cost (a full `/proc` walk), privacy (other users' command lines), a different threat model | not planned |
| background collection, history, alerting, dashboards | a `-lib` has no threads; that is a service | metrics-lib, tower, health jobs |
| DNS lookups (fully qualified host name) | can block on the network | `dns-net` |
| disk SMART health, fan control, CPU governors | privileged, vendor-specific, and changes the system | not planned |
| TPM, Secure Enclave, hardware attestation, device keys | cryptographic proof of a device: the right tool for authentication | `key-vault` / `shield-sec` (future) |
| kernel tuning (`sysctl` writes, registry writes) | changes the system | not planned |
| per-process network connections (`netstat`) | other processes, privacy | not planned |
| package lists, installed software, drivers | inventory, not runtime facts | not planned |

---

## 4. Requirements

### 4.1 Functional

| Id | Requirement |
|---|---|
| FR-1 | Every Tier 1 function is a free function in its module (`sys_lib::os::name()`), takes no configuration, and returns a plain value or `Result<plain value>`. |
| FR-2 | Static facts are read at most once per process and cached in `OnceLock`; every later call returns the cached value. |
| FR-3 | Live readings are read fresh on every call; no live reading is ever cached by the library. |
| FR-4 | Values that need two readings over time come through caller-owned samplers (`cpu::Sampler`, `process::CpuSampler`, `net::Sampler`) plus one documented blocking convenience (`cpu::usage()`, 100 ms). |
| FR-5 | Memory totals and CPU availability respect the limits imposed on the process: cgroup v1 and v2 on Linux (nested hierarchies included), job objects on Windows. Host values are available separately. |
| FR-6 | Byte quantities are returned as `Bytes` (raw `u64` through `raw()`), whose `Display` chooses the unit automatically; binary (IEC), decimal (SI), and JEDEC renderings are available. |
| FR-7 | Percentages are returned as `Percent`, always within 0.0 to 100.0, never NaN. |
| FR-8 | Unknown facts are reported as `None`, an `Unknown` variant, or `Error::Unsupported`, never as invented values. |
| FR-9 | The OS kind and target architecture are compile-time constants (`const fn`). |
| FR-10 | `os::form_factor()` reports desktop, laptop, convertible, tablet, phone, handheld, server, embedded, wearable, TV, or Unknown; `os::detect_form_factor()` adds the detection source and a confidence. |
| FR-11 | `cpu::name()` returns a cleaned product name (no "(R)", "(TM)", "CPU @ 3.00GHz", core-count suffixes, or repeated spaces). |
| FR-12 | `cpu::physical()`, `cpu::logical()`, and `cpu::available()` are distinct and documented: cores, hardware threads, and what this process may use. Windows systems with more than 64 logical processors are counted across all processor groups. |
| FR-13 | CPU features are reported for x86 (SSE through AVX-512, AES, SHA, and more) and AArch64 (NEON, AES, SHA2, SHA3, CRC32, SVE, and more) and reflect operating-system support, not only CPUID bits. |
| FR-14 | Disk space is available for any path (`disk::space(path)`) and for every mounted filesystem (`disk::list()`); listing never blocks on a dead network mount by default. |
| FR-15 | Network interfaces report name, kind, state, MTU, speed when known, addresses with prefix lengths, and the MAC address with whether it is permanent or randomized. |
| FR-16 | Virtualization detection reports VM and hypervisor vendor, container runtime, Kubernetes, and WSL (1 or 2), with the evidence used. A Windows host with Hyper-V or virtualization-based security enabled is not reported as a VM. |
| FR-17 | Battery and AC state are reported where a battery exists; peripheral batteries (mice, keyboards) are excluded. |
| FR-18 | The process guard acquires an exclusive lock that the OS releases on process death, sets the umask (Unix), raises the open-file soft limit, and refuses to start with an actionable error when a requirement cannot be met. |
| FR-19 | Build info reads the **application's** name and version (macro expansion in the application crate) and never fails the build; missing values become `unknown`. `SOURCE_DATE_EPOCH` wins over the clock. |
| FR-20 | Instance info is created once per process and is identical for every caller. |
| FR-21 | The device id is derived with a keyed hash from an application identifier (and an optional application secret); raw machine identifiers are never returned, logged, displayed, stored, or put in errors. |
| FR-22 | The device module reports which identifier sources were used, which were missing and why, the clone risk (VM, container, image without machine id), and what events change the id. |
| FR-23 | A device fingerprint of per-source hashes can be compared with a stored one to tell "same device, one part changed" from "different device". |
| FR-24 | Every public type implements `Debug`; every value a person reads implements `Display`; summaries can be written as key-value pairs and as JSON without third-party crates. |
| FR-25 | Android and iOS: build-checked, best effort for OS, CPU, memory, form factor, and battery; everything else returns `Error::Unsupported`, documented per function. |

### 4.2 Non-functional

| Id | Requirement | Measured by |
|---|---|---|
| NFR-1 | No panic reachable from any public function on any input the OS can produce (malformed `/proc`, `/sys`, SMBIOS, registry content included). | fuzzing, proptest, `clippy::unwrap_used` and `clippy::panic` denied |
| NFR-2 | Cached static facts cost at most 2 ns after the first call [est. target]. | criterion |
| NFR-3 | Scalar live readings allocate nothing. | counting-allocator tests |
| NFR-4 | Every live reading at least 20% faster than the best Rust competitor offering the same reading, measured. | criterion, published in `docs/PERFORMANCE.md` |
| NFR-5 | No background threads, no timers, zero CPU between calls. | code review; a test that counts threads before and after every public call |
| NFR-6 | No subprocess at run time. | a CI grep for `Command::new` outside the build helper and tests; a fake-`PATH` test |
| NFR-7 | Zero third-party runtime dependencies with default features; `device-id` adds only the crypt-io chain. | `cargo tree`, `cargo deny` |
| NFR-8 | Every file read is bounded (default 1 MiB per file, raised only through Tier 2/3 options) and every list is bounded (mounts, interfaces, CPUs). | attack tests (T7) |
| NFR-9 | `unsafe` only in `src/sys/<platform>/`; every block has a `SAFETY:` comment. | `#![deny(unsafe_code)]` at the root, `clippy::undocumented_unsafe_blocks` |
| NFR-10 | Linux, macOS, Windows first class in CI on stable and MSRV, x86_64 and aarch64; musl, 32-bit Windows, Android, iOS build-checked. | CI matrix |
| NFR-11 | Deterministic: for the same OS state, the same output, including formatting and ordering of lists. | fixture tests |
| NFR-12 | No file descriptors, handles, or OS references held after a call returns (except the guard's lock file). | soak test counting descriptors and handles |
| NFR-13 | Clean build adds less than 2 seconds of compile time on a current desktop and less than 300 KiB to a release binary using every module [est. targets]. | measured per release |
| NFR-14 | All public types `Send + Sync` (samplers `Send`). | compile-time assertions |
| NFR-15 | Every error variant says what to do. | review checklist; a test per variant |

---

## 5. Answers: constants, static facts, live readings, samplers

| Kind | Cached? | Example | Cost after first call | Why |
|---|---|---|---|---|
| Compile-time constant | in the binary | `os::kind()`, `cpu::arch()`, `cpu::is_64()` | 0 (folded by the compiler) | fixed when compiled |
| Static fact | `OnceLock`, written once | `os::name()`, `cpu::name()`, `cpu::physical()`, `cpu::features()`, `virt::detect()` | about 1 ns (one atomic load and a pointer) [est.] | cannot change while the process runs, or changes so rarely (OS upgrade) that the process restarts anyway |
| Live reading | never | `mem::used()`, `host::uptime()`, `disk::space()`, `host::name()` | one file read or system call, 0.2 to 20 µs [est.] | changes at any moment |
| Sampled value | caller-owned state | `cpu::Sampler`, `net::Sampler` | one reading plus a subtraction | needs two readings |

Things that look static but are live, each an edge case with a test:

| Value | Why it is live |
|---|---|
| `mem::total()` | the cgroup limit can change at run time (Kubernetes in-place pod resize); VM memory can be hot-added or ballooned (Hyper-V dynamic memory) |
| `cpu::available()`, `cpu::online()` | affinity can change; CPUs can be hot-plugged or taken offline; the cgroup quota can change |
| `host::name()` | DHCP and `hostnamectl` change it while processes run |
| `host::timezone()` | the user can change it |
| `net::interfaces()` | interfaces come and go (VPN, Wi-Fi, USB tethering, containers) |

`cpu::logical()` is static: it is the number of **configured** logical processors, which is the right size for per-CPU arrays. `cpu::online()` is the live count.

### 5.1 Static fact caching

Static facts from the same source are read together: the first call to any of `cpu::name`, `cpu::vendor`, `cpu::physical` reads `/proc/cpuinfo` and the topology once into one `CpuStatic` struct behind one `OnceLock`.

A failed static read is cached as failed (the reason is cached, not retried), because static facts do not change and retrying on every call would turn a cheap call into an expensive one. Exception: `PermissionDenied` sources in the device module are not cached, because a privileged process may call later, after setup.

### 5.2 Reading files without allocation (Linux)

All Linux readings go through one bounded reader:

```rust
// Sketch.
pub(crate) fn read_into<'b>(path: &CStr, buf: &'b mut [u8]) -> Result<&'b [u8]>;
```

- One `open`, `read` until EOF or the buffer is full, one `close`, through `std::fs::File` (no unsafe).
- A buffer that fills before EOF is `Error::TooLarge { what, limit }`, except for files where only a prefix is needed (the first line of `/proc/stat` for total CPU usage), which use `read_prefix`.
- Buffer sizes: 4 KiB on the stack for single-value files and `/proc/meminfo` (about 1.5 KiB on current kernels [est.]); heap buffers bounded at 1 MiB by default for files that scale with the machine (`/proc/stat` per core, `/proc/self/mountinfo`, `/proc/cpuinfo` on many-core hosts).
- Every path is a compile-time constant or built from a validated component (an interface name from `/sys/class/net` that passed a name check, a CPU index that is a number). Paths are never built from environment variables or caller strings (threat T4).
- `/proc` and `/sys` files are produced by the kernel on each read, so one read of one file is internally consistent. Values combined from several files are not atomic; section 5.3 says how invariants are kept.

### 5.3 Consistency and invariants

A value built from several reads is not atomic. sys-lib guarantees, by clamping and checked arithmetic:

- `used <= total` and `free <= total` in every `Memory`, `Swap`, `Space`.
- `free + used == total` in `Memory` (free is derived from used, saturating).
- `Percent` within 0..=100.
- `physical <= logical`, `available <= logical`, `available >= 1`, `online >= 1`.
- `boot_time + uptime` within one second of the wall clock at the time of reading.
- Lists sorted deterministically (mounts by mount point, interfaces by index then name, cores by id).

Each invariant has a property test over fixture data with values changed between reads.

### 5.4 Thread safety

- Every public function is safe to call from any thread at any time.
- `OnceLock::get_or_init` runs the initialiser once; concurrent first callers wait.
- Samplers are `Send` and used through `&mut self`; sharing one across threads takes the caller's own `Mutex`, which is the caller's decision.
- The guard's `umask` and `setrlimit` are process-wide side effects, documented and done once.
- Nothing in sys-lib calls `std::env::set_var` (unsafe in edition 2024) or changes the working directory.

---

## 6. Units and errors

### 6.1 `Bytes`

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bytes(u64);

impl Bytes {
    pub const ZERO: Bytes;
    pub const fn new(bytes: u64) -> Bytes;
    pub const fn raw(self) -> u64;                   // the exact number of bytes
    pub const fn from_kib(n: u64) -> Bytes;          // saturating
    pub const fn from_mib(n: u64) -> Bytes;
    pub const fn from_gib(n: u64) -> Bytes;
    pub const fn saturating_sub(self, other: Bytes) -> Bytes;
    pub const fn saturating_add(self, other: Bytes) -> Bytes;
    pub const fn checked_sub(self, other: Bytes) -> Option<Bytes>;
    pub fn percent_of(self, total: Bytes) -> Percent;  // 0% when total is zero
    pub fn display(self, units: Units) -> impl fmt::Display;
    pub fn binary(self) -> impl fmt::Display;        // "125.6 GiB"
    pub fn decimal(self) -> impl fmt::Display;       // "134.9 GB"
    pub fn jedec(self) -> impl fmt::Display;         // "125.6 GB" (1024-based, JEDEC labels)
}

#[non_exhaustive]
pub enum Units { Binary, Decimal, Jedec }
```

Display rules (deterministic, allocation-free, written straight into the formatter):

| Rule | Detail |
|---|---|
| Default `{}` | binary values with IEC labels: `B`, `KiB`, `MiB`, `GiB`, `TiB`, `PiB`, `EiB` |
| Alternate `{:#}` | decimal SI: `B`, `kB`, `MB`, `GB`, `TB`, `PB`, `EB` |
| Unit choice | the largest unit for which the value is at least 1 |
| Precision | one decimal place by default, trailing `.0` dropped (`"128 GiB"`, `"127.9 GiB"`); `{:.0}` and `{:.2}` honoured |
| Below 1 KiB | integer, no decimals: `"512 B"`, `"1 B"`, `"0 B"` |
| Rounding | half away from zero, in integer arithmetic (`u128` intermediate), so the output never depends on floating-point formatting |
| Carry | a value that rounds up to 1024.0 of a unit prints as `1 MiB`, not `1024 KiB` |
| Maximum | `u64::MAX` prints `"16 EiB"` (binary) and `"18.4 EB"` (decimal) |
| `Debug` | `Bytes(134_876_545_024)`: the raw value, so tests and logs are exact |

### 6.2 `Percent`

```rust
#[derive(Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Percent(f32);   // invariant: 0.0 <= value <= 100.0, never NaN

impl Percent {
    pub const ZERO: Percent;
    pub const FULL: Percent;
    pub fn from_ratio(part: u64, whole: u64) -> Percent;   // clamps; whole == 0 gives ZERO
    pub fn value(self) -> f32;                             // 0.0..=100.0
    pub fn fraction(self) -> f32;                          // 0.0..=1.0
    pub fn complement(self) -> Percent;                    // 100 - value
}
// Display: one decimal place and "%": "32.8%"; precision honoured.
```

Ratios are computed as `part * 1000 / whole` in `u128` and converted once, so a `Percent` from integers is exact to 0.1% and never NaN.

### 6.3 `Error`

```rust
/// Every error says what failed and what to do.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// This reading does not exist on this platform. Do: treat the value as unknown.
    Unsupported { what: &'static str, platform: &'static str },
    /// The OS refused (root-only DMI files, a sandbox). Do: run with the permission
    /// named in `hint`, or accept the value as unknown.
    PermissionDenied { what: &'static str, hint: &'static str },
    /// A file or key that should exist does not (a container with a reduced /proc).
    NotFound { what: &'static str, source: &'static str },
    /// The OS returned data that does not parse. Carries the source name and line,
    /// never the content.
    Malformed { what: &'static str, source: &'static str, line: Option<u32> },
    /// A file or list exceeded its budget. Do: raise the budget with the Tier 2 options.
    TooLarge { what: &'static str, limit: u64 },
    /// A sampler was asked again before any time passed on the OS clock.
    TooSoon { elapsed: Duration, minimum: Duration },
    /// A system call failed. Carries the call name and the OS error code.
    Os { what: &'static str, call: &'static str, code: i32 },
    /// File I/O failed outside the cases above.
    Io { what: &'static str, source: std::io::Error },
    /// Invalid input from the caller (a guard name with a slash, an empty app id).
    InvalidInput { what: &'static str, reason: &'static str },
    /// Process guard: another copy holds the lock.
    AlreadyRunning { pid: Option<u32>, lock_file: PathBuf },
    /// Process guard: the lock directory cannot be created, written, or trusted.
    RunDirUnavailable { path: PathBuf, source: std::io::Error },
    /// Process guard: the hard open-file limit is below the configured minimum.
    OpenFilesTooLow { have: u64, need: u64 },
    /// Device identity: no usable identifier source for the requested basis.
    NoIdentifierSource { basis: device::Basis },
}
pub type Result<T, E = Error> = core::result::Result<T, E>;
```

- `Display` for each variant is one sentence plus the "do" part; tested per variant (NFR-15).
- No variant ever contains file content, registry content, or identifier values. `Malformed` carries the source path (a constant) and a line number only (threat T1).
- `Error` is `Send + Sync + 'static` and implements `std::error::Error` with `source()` for `Io` and `RunDirUnavailable`.

---

## 7. Containers and limits

The rule (FR-5): **the default answer is what this process can actually use**; the host answer is a separate call.

### 7.1 Finding the cgroup (Linux)

1. Read `/proc/self/cgroup`. A line `0::/<path>` means the unified (v2) hierarchy. Lines `N:<controllers>:/<path>` are v1; the one listing `memory` (or `cpu,cpuacct`) names the v1 controller path. Hybrid systems have both; the v1 `memory` controller wins for memory when present, because that is where the kernel enforces the limit.
2. Find the mount point from `/proc/self/mountinfo` (filesystem type `cgroup2`, or `cgroup` with the controller in the super options). `/sys/fs/cgroup` is never assumed.
3. Resolve the cgroup directory as `<mount>/<path>`. Inside a cgroup namespace (Docker, Kubernetes) the path is `/` and the container's limits are at the mount root.
4. Validate the path: no `..` component, no NUL, total length under 4096 bytes, ancestor walk capped at 64 levels (threats T4, T6).

### 7.2 Memory limit

| Hierarchy | Limit | "Used" | Notes |
|---|---|---|---|
| v2 | minimum of `memory.max` over the cgroup and every ancestor up to the mount root (`max` means none) | `memory.current` minus `inactive_file` from `memory.stat` | `memory.high` (throttle point) reported as `soft`, `memory.swap.max` as the swap limit |
| v1 | `hierarchical_memory_limit` from `memory.stat` (already the minimum over ancestors); values at or above 2^63 minus one page mean unlimited | `memory.usage_in_bytes` minus `total_inactive_file` | `memory.soft_limit_in_bytes` as `soft` |

Subtracting inactive file pages follows the kubelet's working-set definition (Kubernetes node-pressure eviction documentation): what a container user means by "used", and close to what the OOM killer compares against.

```
 effective total = min(host MemTotal, cgroup limit)          (mem::total)
 effective used  = cgroup working set   if a limit applies    (mem::used)
                 = MemTotal - MemAvailable otherwise
 effective free  = total - used, saturating                   (mem::free)
```

### 7.3 CPU quota

v2 `cpu.max` (`"max 100000"` or `"150000 100000"`: quota and period in microseconds; minimum over ancestors), v1 `cpu.cfs_quota_us` and `cpu.cfs_period_us` (`-1` means none). `cpu::available()` = min(affinity count, ceil(quota / period)), at least 1. `cpuset.cpus.effective` (v2) or `cpuset.cpus` (v1) further limits the CPU set and is intersected with `Cpus_allowed_list`.

### 7.4 Windows job objects

`QueryInformationJobObject(NULL, JobObjectExtendedLimitInformation)` reports the limits of the job the process is in: `ProcessMemoryLimit` and `JobMemoryLimit` when the matching limit flags are set. Process-isolated Windows containers run inside job objects. CPU rate limits come from `JobObjectCpuRateControlInformation`. Nested jobs: whether the query returns the effective nested minimum is **unverified**; a nested-job test settles it before 0.4 ships.

### 7.5 macOS, iOS, LXCFS

- macOS has no cgroup equivalent: `mem::limit()` is `Ok(None)`. iOS has a per-app memory budget; `os_proc_available_memory()` reports it on iOS 13 and later [unverified], used for `mem::free()` on iOS (after 1.0).
- Some LXC and Kubernetes setups bind-mount LXCFS over `/proc/meminfo`, `/proc/stat`, and `/proc/cpuinfo` so they already show container values. sys-lib detects an LXCFS mount (`fuse.lxcfs` in mountinfo for those paths) and does not apply the cgroup limit a second time. Recorded in `Memory::source`.

---

## 8. CPU usage

The OS keeps counters of time spent per CPU state, growing since boot. Usage is the share of non-idle time **between two readings**; a single reading gives only the average since boot.

| OS | Source | States |
|---|---|---|
| Linux | `/proc/stat`, line `cpu` (total) and `cpuN` (per core), in USER_HZ ticks | user, nice, system, idle, iowait, irq, softirq, steal, guest, guest_nice; `guest` and `guest_nice` are already in `user` and `nice` and are not added again |
| macOS | `host_statistics(HOST_CPU_LOAD_INFO)` total; `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` per core | user, system, idle, nice |
| Windows | `GetSystemTimes` total (**kernel includes idle**, so busy = kernel + user - idle); per core `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` | idle, kernel, user, dpc, interrupt |

```rust
// Tier 2: non-blocking, caller-owned.
let mut sampler = sys::cpu::Sampler::new()?;    // takes the first reading
let usage = sampler.sample()?;                   // difference since the previous reading

// Tier 1: blocking convenience.
let usage = sys::cpu::usage()?;                  // reads, sleeps 100 ms, reads again
let used = sys::cpu::used()?;                    // same, returns Percent
```

- `Sampler::sample` never blocks and never sleeps.
- If no tick has elapsed since the previous reading, it returns `Error::TooSoon { elapsed, minimum: 10 ms }` instead of a meaningless 0% or 100%.
- Counters that go backwards (a CPU offlined, a VM migration, a counter wrap) are treated as a reset: the sampler re-bases and returns `TooSoon` for that core, never a negative or above-100 value.
- `cpu::usage()` sleeps 100 ms (`cpu::DEFAULT_WINDOW`); the function's name and the first line of its documentation say it blocks. 100 ms gives 1% resolution per core at Linux's 100 Hz tick. `cpu::usage_over(Duration)` takes another window, clamped to 10 ms ..= 10 s.
- No process-wide hidden baseline (the psutil `cpu_percent(interval=None)` style): that would be global mutable state, and two callers would reset each other's window (D-007).
- Steal counts as busy (reported separately); iowait counts as idle (reported separately).

---

## 9. The boot kit: build info, instance info, process guard

### 9.1 Build info

Facts fixed when the binary is compiled: "exactly which build is running?"

| Field | Example | Source |
|---|---|---|
| name | `site` | `CARGO_PKG_NAME` of the application (the macro expands in the application crate) |
| version | `0.3.0` | `CARGO_PKG_VERSION` |
| commit | `9db9e36` and the full hash | `git rev-parse HEAD`, run by the build helper |
| dirty | `true` if uncommitted changes were built | `git status --porcelain --untracked-files=no` is non-empty |
| built at | `2026-10-21T18:02:11Z` | `SOURCE_DATE_EPOCH` if set, else the clock at build time |
| profile | `release` | `PROFILE` in the build script |
| target | `x86_64-unknown-linux-gnu` | `TARGET` in the build script |
| rustc | `1.89.0` | `$RUSTC --version` |
| features | `default,guard` | `CARGO_FEATURE_*` of the application's build script |

1. **A build helper** the application calls from its own `build.rs` (sys-lib as a build-dependency). It runs `git -C $CARGO_MANIFEST_DIR` and `$RUSTC --version` at **build** time and passes the values on with `cargo:rustc-env=SYS_LIB_BUILD_*`. It prints `cargo:rerun-if-changed` for the git `HEAD` file and the ref it points to (resolved through a `.git` file in worktrees) and `cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH`. If anything fails (`git` missing, a crates.io tarball, a shallow clone), the values become `unknown`. **The build never fails because of build info.**
2. **A macro** expands inside the application's crate, so `env!("CARGO_PKG_VERSION")` reads the application's version. Helper values are read with `option_env!`, so an application that forgot `emit()` still compiles and shows `unknown`.

Every field is a `&'static str` in the binary; the macro expands to a reference to a `static`: zero run-time cost. Values are validated (printable ASCII, at most 128 bytes) before being passed to `rustc-env`. The helper does **not** record the build machine's host name, user name, or source path (threat T11). `BuildInfo::public()` exposes only name and version, for public endpoints.

### 9.2 Instance info

Facts fixed when the process **starts**: "which running copy is this?"

| Field | Source |
|---|---|
| instance id | id-forge ULID, created once |
| started at | `SystemTime::now()` at start; displayed in RFC 3339 through clock-lib |
| uptime | clock-lib monotonic start, so NTP steps do not affect it |
| host | `host::name()` at start |
| pid | `std::process::id()` |
| user at start | `process::user()` at start, before any privilege drop (OD-12, section 21) |

`Instance::start()` creates the instance once and returns `&'static Instance`; later calls return the same value. `instance::current()` returns it without creating it. `start()` never fails: unreadable host or user become `"unknown"`. A `fork()`ed child inherits the instance; `is_current_process()` tells, and the documentation says forked workers must not log under the parent's instance id.

### 9.3 Process guard

| Protection | Stops | Linux and macOS | Windows |
|---|---|---|---|
| single-instance lock | two copies running against the same data | `<lock dir>/<name>.lock`, opened with `O_NOFOLLOW`, locked with `File::try_lock` | the same file, `File::try_lock`, reparse points refused |
| file-permission default | log, audit, and data files readable by every user | `umask(0o027)` before any file is created | no umask; reported as not applicable; owner-only directories come from path-lib |
| open-file limit | running out of descriptors under load | raise the soft `RLIMIT_NOFILE` to the target, capped at the hard limit (and on macOS at `kern.maxfilesperproc`); refuse to start below the minimum | not applicable; reported as such |
| privilege drop | a compromised process running as root | **shield-sec**, after listeners are bound | **shield-sec** |

Why a locked file and not "a pid file plus a liveness check": the OS releases the lock when the process dies, even after `kill -9`; pid numbers are reused.

Details that are easy to get wrong, each with a test:

1. **Never delete the lock file**, not even on clean shutdown. If A deletes it while B waits to open it, B locks the unlinked file and C locks a new one: two copies run. Only the lock is released (`Guard::drop` closes the file).
2. **The pid lives in a separate file** (`<name>.pid`), written after the lock is held, with `path_lib::write_atomic`. On Windows the locked byte range cannot be read by another process. `AlreadyRunning { pid }` reads the `.pid` file (at most 128 bytes, parsed strictly); missing or malformed gives `pid: None`.
3. **No symbolic links** at the lock path (`O_NOFOLLOW` on Unix; on Windows `FILE_FLAG_OPEN_REPARSE_POINT` and a check that the opened file is not a reparse point). A symlink is an error, not followed (threat T9).
4. **The lock directory must be owner-only** (0700 on Unix). Group- or world-writable: refused (`RunDirUnavailable` with the reason).
5. **The name is validated**: 1 to 64 characters, ASCII letters, digits, `-`, `_`, `.`, not starting with `.` (threat T4).
6. **umask is process-wide.** The guard must run before threads start; more than one thread in `/proc/self/status` gives a report warning (some runtimes start helper threads early).
7. **Supervised restarts**: under tower, tower holds the lock (old and new process briefly overlap); `.lock(false)` covers that case.
8. **Network filesystems**: `flock` over NFS is not reliable across clients. The guard reports the lock directory's filesystem type and refuses `nfs`, `cifs`, `smb2`, `fuse.sshfs`, and WSL host mounts (`drvfs`, `9p`) unless `.allow_network_lock(true)`.

Tier 1 is `process::guard(name)` (lock + umask 027 + open-file limit 65 536 target, 1 024 minimum). Tier 2 is `Guard::builder(name)` with `lock_dir`, `lock`, `allow_network_lock`, `umask`, `keep_umask`, `open_files_target`, `open_files_minimum`. The default lock directory is path-lib's runtime directory; databases should pass `.lock_dir(data_dir)`.

---

## 10. Detection: OS identity, form factor, virtualization

All detection is **advisory**. It reports `Unknown` rather than "no" when unsure, and the documentation forbids using it for security decisions.

### 10.1 OS name and version

| OS | `name()` | `version()` | `kernel()` | Notes |
|---|---|---|---|---|
| Linux | `PRETTY_NAME` from `/etc/os-release`, then `/usr/lib/os-release`, then `"Linux"` | `VERSION_ID` as dotted numbers; rolling releases have none | `/proc/sys/kernel/osrelease` | shell-style quoting per os-release(5); unknown keys ignored |
| macOS | `"macOS " + version + " " + codename` when known (11 Big Sur to 26 Tahoe); unknown future majors print without a codename | `kern.osproductversion` | `kern.osrelease` | the sysctl, not `SystemVersion.plist`, because `SYSTEM_VERSION_COMPAT=1` makes the plist report 10.16 [verify on CI] |
| Windows | `"Windows 11 Pro"`: product from the build number (22000 and later is Windows 11; the registry `ProductName` still says "Windows 10"), edition from `EditionID`, server from `wProductType` | `RtlGetVersion`: major, minor, build; `UBR` from the registry as revision | `"10.0.<build>.<UBR>"` | `GetVersionEx` is not used: it reports the version the manifest declares |
| Android | `"Android " + ro.build.version.release` | same, parsed | `/proc/sys/kernel/osrelease` | `__system_property_get` |
| iOS | `"iOS " + kern.osproductversion` (iPadOS on iPads) | same | `kern.osrelease` | |

`name()` never fails. `os::info()` records the source of each field. `kind()` maps `target_os = "android"` to `Android` (path-lib maps it to `OtherUnix`; the difference is documented).

### 10.2 Form factor

```rust
#[non_exhaustive]
pub enum FormFactor { Desktop, Laptop, Convertible, Tablet, Phone, Handheld,
                      Server, Embedded, Wearable, Tv, Unknown }
```

Sources in order, first answer wins; each result carries `source` and `confidence`:

| OS | 1 | 2 | 3 | 4 | Fallback |
|---|---|---|---|---|---|
| Linux | `CHASSIS=` in `/etc/machine-info` (admin override) | `/sys/class/dmi/id/chassis_type` | `/sys/firmware/acpi/pm_profile` | device tree `chassis-type` | battery present: `Laptop` (low confidence), else `Unknown` |
| Windows | SMBIOS type 3 chassis from `GetSystemFirmwareTable('RSMB')` | `PowerDeterminePlatformRoleEx` | `GetSystemPowerStatus` battery flag | | `Unknown` |
| macOS | internal battery (IOKit power sources): Laptop | `hw.model`: `MacBook*` Laptop, `iMac*`, `Macmini*`, `MacPro*` Desktop | `VirtualMac*`: `Unknown` | | `Unknown` |
| Android | `ro.build.characteristics`: `tablet`, `watch`, `tv` | | | | Phone (medium confidence) |
| iOS | `uname().machine` prefix; `SIMULATOR_MODEL_IDENTIFIER` on the simulator | | | | `Unknown` |

SMBIOS chassis mapping (systemd's, from SMBIOS 3.5.0 section 7.4.1, plus Server for rack mount and multi-system): 0x03, 0x04, 0x06, 0x07, 0x0D, 0x23, 0x24 Desktop; 0x08, 0x09, 0x0A, 0x0E Laptop; 0x0B Handheld; 0x11, 0x17, 0x19, 0x1C, 0x1D Server; 0x1E Tablet; 0x1F, 0x20 Convertible; 0x21, 0x22 Embedded; anything else goes to the next source. ACPI PM profile: 1, 3, 6 Desktop; 2 Laptop; 4, 5, 7 Server; 8 Tablet. Inside a VM the chassis describes virtual hardware; the answer is `Unknown` unless the admin override says otherwise.

`os::is_laptop()` is Laptop or Convertible; `os::is_mobile()` is Phone, Tablet, Handheld, Wearable; `os::is_desktop()` is Desktop only.

### 10.3 Virtualization

| Check | Platform | Reads | Detects |
|---|---|---|---|
| CPUID leaf 1 ECX bit 31, leaf 0x40000000 vendor | x86 (all OS) | `__cpuid` | KVM, QEMU TCG, VMware, Hyper-V, Xen, bhyve, QNX, ACRN, Apple VZ, VirtualBox |
| Hyper-V root partition | Windows x86 | CPUID 0x40000003 EBX bit 0 [verify against the Hyper-V TLFS before 0.5] | a Windows **host** with Hyper-V, WSL 2, or VBS is not a VM |
| DMI strings | Linux (0444 files), Windows (SMBIOS 0, 1, 2) | product, system vendor, board vendor, BIOS vendor | KVM, OpenStack, KubeVirt, EC2, QEMU, VMware, VirtualBox, Xen, Bochs, Parallels, bhyve, Hyper-V, Apple Virtualization, GCE, Alibaba ECS |
| Hypervisor files | Linux | `/sys/hypervisor/type`, `/proc/xen/capabilities` (`control_d` is Dom0), device tree | Xen, ARM hypervisors |
| sysctl | macOS | `kern.hv_vmm_present`, `hw.model` `VirtualMac*` | Apple Virtualization, Parallels, VMware Fusion |
| Container files | Linux | `/run/systemd/container`, `/run/.containerenv`, `/.dockerenv`, `/proc/1/cgroup` v1 names, overlay root in mountinfo | Docker, Podman, LXC, systemd-nspawn, containerd |
| Kubernetes | Linux | `KUBERNETES_SERVICE_HOST` (weak), service-account directory | Kubernetes |
| WSL | Linux | `osrelease` contains `Microsoft` (WSL 1) or `microsoft-standard-WSL2` (WSL 2); `WSL_DISTRO_NAME`; `WSLInterop` | WSL 1 or 2 |
| Emulation | macOS, Windows | `sysctl.proc_translated`; `IsWow64Process2` | not virtualization: `cpu::native_arch()`, `cpu::is_translated()` |

The result is a cached static fact with an `evidence` list naming each check that fired (constant strings, no raw values). Environment-based evidence is marked `Weak`: any process can set environment variables (threat T14).

---

## 11. Memory definitions

| Value | Linux | macOS | Windows |
|---|---|---|---|
| host total | `MemTotal` | `hw.memsize` | `ullTotalPhys` (`GlobalMemoryStatusEx`) |
| host free | `MemAvailable` (3.14 and later; older: `MemFree + Buffers + Cached`) | `(free_count + inactive_count) * page size` from `host_statistics64(HOST_VM_INFO64)` [est.: approximates Activity Monitor; validated against `vm_stat` in CI] | `ullAvailPhys` |
| unused (truly idle) | `MemFree` | `free_count - speculative_count` | not exposed separately |
| cached | `Cached + SReclaimable - Shmem` | `external_page_count * page size` | `CacheBytes` (`GetPerformanceInfo`) |
| swap | `SwapTotal`, `SwapFree` | `vm.swapusage` | pagefile from `GetPerformanceInfo` [est.: documented approximation] |
| page size | `sysconf(_SC_PAGESIZE)` (or auxv) | `vm.pagesize` | `GetSystemInfo().dwPageSize` |
| pressure | PSI `/proc/pressure/memory` (4.20 and later) | `kern.memorystatus_vm_pressure_level` | `QueryMemoryResourceNotification` |

"free" in sys-lib means **available**: what new allocations can get without swapping, counting reclaimable cache. The kernel's `MemFree` (pages doing nothing) is `Memory::unused`. The definition is stated in every doc comment.

---

## 12. Device identity

### 12.1 The goal

An identifier for "this device" that an application can store and recognise later; stable across reboots, network changes, and application reinstalls; impossible to use for correlating the same machine across unrelated applications or for recovering the serial numbers behind it.

Platform guidance: systemd says the machine id "should be considered confidential" and that applications should hash it with a keyed, application-specific key (`machine-id(5)`). Android's `ANDROID_ID` is per signing key, user, and device since 8.0. MAC addresses are randomized by default on Android 10+, iOS 14+, and optionally on macOS and Windows: a weak, often changing anchor.

### 12.2 Anchors

| Anchor | Linux | macOS | Windows | Survives reinstall | Privilege |
|---|---|---|---|---|---|
| `InstallId` | `/etc/machine-id` (fallback `/var/lib/dbus/machine-id`) | none (the platform UUID stands in) | `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` (`KEY_WOW64_64KEY`) | no | none |
| `PlatformUuid` | `/sys/class/dmi/id/product_uuid` (0400) | `IOPlatformUUID` | SMBIOS type 1 UUID | yes | Linux: root |
| `BoardSerial` | `/sys/class/dmi/id/board_serial` (0400) | `IOPlatformSerialNumber` | SMBIOS type 2 serial | yes | Linux: root |
| `ProductSerial` | `/sys/class/dmi/id/product_serial` (0400) | (same as board on Macs; not used twice) | SMBIOS type 1 serial | yes | Linux: root |
| `CpuSignature` | vendor, family, model, stepping, brand | same | same | yes | none; **not unique** |
| `PermanentMac` | lowest MAC with `addr_assign_type == 0` on a physical, non-removable interface | lowest built-in interface MAC | lowest physical adapter MAC | yes | none |

Placeholder values that OEMs and hypervisors ship are treated as absent (each a test): all-zero and all-`F` UUIDs; serials `To be filled by O.E.M.`, `Default string`, `System Serial Number`, `Not Specified`, `Not Applicable`, `None`, `0`, `0123456789`, `123456789`, `Chassis Serial Number`, `Base Board Serial Number`, empty or whitespace; machine-id `uninitialized`, empty, all zeros; MAC all zeros, broadcast, multicast, or locally administered. One table, one source comment per entry.

### 12.3 One anchor per id, every anchor in the fingerprint

Concatenating anchors makes an id **less** stable: any one change (a NIC swap) changes the whole id. So:

- `device::id(app)` hashes **one** anchor chosen by the basis (default `Basis::Install`; on macOS the platform UUID).
- `device::fingerprint(app)` hashes **each** available anchor separately, so a stored fingerprint can tell "one part changed" from "another machine". Weights: `InstallId` 3, `PlatformUuid` 3, `BoardSerial` 2, `ProductSerial` 2, `PermanentMac` 1, `CpuSignature` 1. Verdict `Same` (all present anchors match), `LikelySame` (matched weight at least 5 and at most one strong anchor changed), `Different`. Per-anchor comparison in constant time.

### 12.4 Derivation (keyed BLAKE3 through crypt-io)

```
 app_key  = secret                                        if the caller supplied a 32-byte secret
          = BLAKE3("sys-lib/device/v1/app-key" || 0x00 || u16_be(len(app)) || app)   otherwise
 message  = "sys-lib/device/v1" || 0x00 || basis tag (1 byte) || anchor tag (1 byte)
            || u16_be(len(canonical)) || canonical
 tag      = BLAKE3-keyed(app_key, message)                 (crypt-io, 32 bytes)
 DeviceId = first 16 bytes of tag, with RFC 9562 version 8 and variant bits set   (122 bits)
```

- **Canonical form**: UUIDs as 16 bytes in RFC 9562 byte order (the Windows raw SMBIOS table stores the first three fields little-endian and is converted); machine-id and MachineGuid as 16 bytes parsed from hex; serials trimmed and upper-cased; MAC as 6 bytes; CPU signature as a fixed struct. On a dual-boot machine, `Basis::Hardware` gives the same id under Linux (as root) and Windows.
- **Domain separation**: version, basis tag, and anchor tag are inside the message; ids for different bases and fingerprint entries never collide by construction.
- **Without a secret**, anyone who knows the raw machine id and the (public) app id can compute the device id: the same property as systemd's app-specific id. **With a secret**, nobody without it can compute or correlate the id, even with root.
- **Raw values** are read into a fixed buffer, hashed, and wiped. They never cross the public API (threat T1).
- **Frozen**: the derivation `sys-lib/device/v1` is frozen for all of 1.x. A change would silently move every stored id, so it can only arrive as a new `Basis` or a new version tag.

### 12.5 Stored fingerprint format

```
 offset  size  field
 0       4     magic "SLFP"
 4       1     format version (1)
 5       1     anchor-present bitmap (bit n = Anchor n)
 6       2     reserved, zero
 8       96    6 x 16-byte anchor hashes (zeros where absent)
 104     8     first 8 bytes of BLAKE3(bytes 0..104): checksum against storage corruption
```

`from_bytes` checks length, magic, version, reserved bytes, bitmap consistency, and checksum, in that order; it is a fuzz target. The checksum detects corruption, not tampering.

### 12.6 Stability report and when not to use it

`device::report()` returns per anchor whether it is present, absent, placeholder, denied, or unsupported (states only, never values), plus `clone_risk` (container, VM, uninitialised machine id, shared image), `confidence`, and `changes_on` (the events that change the id for the default basis).

When not to use it (first paragraph of `device::id`'s rustdoc, and `docs/DEVICE-ID.md`):

- **Not authentication.** Root or Administrator can set `/etc/machine-id` or `MachineGuid` to anything; hypervisors set SMBIOS UUIDs freely. For proof, use a key held in a TPM or Secure Enclave.
- **Not cross-application tracking or advertising.** The application-specific key prevents it; it is against platform policies and, in the EU, likely needs consent under ePrivacy Article 5(3).
- **Personal data.** Under GDPR Recital 30 a device id in a client application can identify a person: the application needs a lawful basis, a retention period, and a way to delete it. sys-lib states this; it is not legal advice.
- **Not stable enough for licensing alone.** Reinstalls and board swaps happen; licensing should allow re-activation.

---

## 13. API outline

The full API with signatures, errors, and examples is written in `docs/API.md` at the 0.2.0 design lock. This section fixes the item ids the roadmap refers to. Grades: P0 must ship in the listed version and never moves; P1 and P2 ship by 0.5; P3 after 1.0. Tier 1 is free functions with no configuration; Tier 2 snapshots, samplers, builders; Tier 3 caller buffers, raw counters, explicit roots.

Convention in every example: `use sys_lib as sys;`, then `sys::os::name()`.

| Module | Ids | Items | Ver |
|---|---|---|---|
| root | ROOT-01 | `VERSION` | 0.1 |
| root | ROOT-02 to ROOT-04 | `Error`, `Result`, `Bytes`, `Percent` | 0.3 |
| root | ROOT-05 | `Mhz` | 0.4 |
| root | ROOT-06 | `summary()`, `Summary` (no identifiers, no host or user name) | 0.5 |
| build | BLD-01 to BLD-05 | `build::emit()`, `build_info!()`, `BuildInfo` and `Display`, `write_kv`/`write_json`, `public()` | 0.3 |
| build | BLD-06 | `build::emitter()` builder | 0.4 |
| instance | INS-01 to INS-06 | `Instance::start()`, `instance::current()`, accessors, `InstanceId`, writers, `is_current_process` | 0.3 |
| process | PRC-01, PRC-11, PRC-12 | `id()`, `user()`, `is_elevated()` | 0.3 |
| process | PRC-20 to PRC-23 | `process::guard(name)`, `Guard::builder`, `GuardReport`, lock directory hardening | 0.3 |
| process | PRC-02 to PRC-04, PRC-06 to PRC-10, PRC-13, PRC-14 | `parent_id`, `memory`, `peak_memory`, `cpu_time`, `threads`, `open_files`, `limits`, `started_at`, `info`, `CpuSampler` | 0.4 |
| process | PRC-05 | `memory_detail()` | 0.5 |
| os | OS-01 to OS-07 | `kind`, `is_*` constants, `name`, `version`, `kernel`, `distro`, `info` | 0.3 |
| os | OS-08 to OS-13 | `edition`, `is_server_edition`, `codename`, `is_64bit`, `form_factor`, `detect_form_factor`, `is_desktop`/`is_laptop`/`is_mobile`/`is_phone`/`is_tablet`/`is_server` | 0.5 |
| cpu | CPU-01 to CPU-06 | `arch`/`is_64`/`is_32`, `logical`, `physical`, `available` (affinity, cpuset, quota), `name`/`raw_name`, `vendor` | 0.3 |
| cpu | CPU-07 to CPU-12 | `packages`, `count`, `online`, `quota`, `frequency`, `current_frequency` | 0.4 |
| cpu | CPU-20 to CPU-25 | `usage`, `usage_over`, `used`, `free`, `Sampler` (total, per core, into a buffer), `Usage`, `times` | 0.4 |
| cpu | CPU-30, CPU-31 | `has(feature)`, `features()` | 0.4 |
| cpu | CPU-13, CPU-32, CPU-40 to CPU-44 | `native_arch`/`is_translated`, `x86_level`, `topology`, `caches`, `cache_line`, `numa_nodes`, `is_hybrid` | 0.5 |
| mem | MEM-01 to MEM-08 | `total`, `used`, `free`, percentages, `snapshot`, `host_total`/`host_free`, `limit`, `swap`, `page_size` | 0.4 |
| mem | MEM-09 | `pressure()` | 0.5 |
| disk | DSK-01 to DSK-03 | `space(path)`, `list()`, `list_with(options)` | 0.4 |
| disk | DSK-04, DSK-05 | `DiskKind` SSD/HDD, `of(path)` | 0.5 |
| net | NET-01 to NET-03 | `interfaces()`, `Mac`/`MacKind`, `addresses()` | 0.4 |
| net | NET-04 to NET-06 | `counters()`, `Sampler`, `primary()` | 0.5 |
| host | HST-01 | `name()` | 0.3 |
| host | HST-02 to HST-04 | `uptime`, `boot_time`, `load_average` | 0.4 |
| host | HST-05 to HST-07 | `timezone`, `locale`, `model` | 0.5 |
| virt | VRT-01 to VRT-04 | `detect`, `is_vm`/`is_container`/`is_wsl`/`is_kubernetes`, `hypervisor`/`container`, evidence | 0.5 |
| power | PWR-01 to PWR-03 | `has_battery`, `battery`, `on_ac` | 0.5 |
| device | DEV-01 to DEV-07 | `id(app)`, `DeviceId`, `report()`, `Builder`, `fingerprint(app)`, `compare`, `to_bytes`/`from_bytes` | 0.5 |
| after 1.0 | MEM-10, DSK-06, NET-07, HST-08, VRT-05, PWR-04, SNS-01, GPU-01, PRB-01 | huge pages, disk I/O counters, interface options, sessions, cloud vendor, several batteries, temperatures, GPU adapters, `Probe::with_root` | 1.1 / 1.2 |

Counts by grade: P0 33, P1 45, P2 38, P3 10 (by id row). A P3 module appears only when implemented; no stub modules returning `Unsupported` are shipped.

---

## 14. Threats

Assets: raw hardware identifiers; the integrity of reported values (an application sizes memory or refuses to start on them); the guard's guarantee that only one copy runs; the availability of the calling process (sys-lib must never hang or crash it).

Attackers: a local unprivileged user; a hostile container image or orchestrator that controls `/proc`, `/sys`, and the environment visible to the process; a remote party receiving values the application sends (logs, telemetry, health endpoints); an application vendor wanting to correlate users across applications.

| Id | Threat | Defence | Attack test (must fail to get through) | Cost |
|---|---|---|---|---|
| T1 | disclosure of raw hardware identifiers through logs, errors, `Debug`, `Display`, or the API | raw values exist only inside `device` internals, in a fixed buffer wiped after hashing; `Error` carries no values; `summary()` has no identifiers | fixture with known fake identifiers: no public output (every function, `Debug`, `Display`, JSON) contains them in hex, text, or bytes; the buffer is zero after `device::id` | wipe about 20 ns [est.] |
| T2 | cross-application correlation of device ids | keyed hash with an application-derived key; optional secret | two app ids on one fixture: Hamming distance over many pairs averages about 61 of 122 bits; secret changes the id | one key derivation, under 0.5 µs [est.] |
| T3 | device id used as authentication, then spoofed | documentation; no API implying verification; `AdminReset` as a change event | doc test asserts the warning paragraph; API review checklist | none |
| T4 | path or value injection through the environment (`TZ=../../etc/shadow`, hostile `LANG`, a `HOST_PROC`-style redirect) | no root overrides from the environment; `TZ`, `LC_*`, `LANG` validated and never used to open files; guard and interface names validated | hostile values set: no file outside the fixed list is opened (the fixture root records every path), values rejected or ignored | under 50 ns per validation [est.] |
| T5 | TOCTOU on `/proc` (pid reuse, files changing between reads) | only `/proc/self` and `/proc/1`; one open per file, read to EOF; combined values clamped to invariants | fixture backend changing values between reads; invariants hold for every interleaving | clamping under 5 ns |
| T6 | untrusted `/proc` and `/sys` in containers (huge or negative numbers, binary junk, enormous files) | safe, bounded, checked parsers, no recursion, fuzzed | fuzz target per parser; proptest "never panics, never exceeds budget"; fixtures with `u64::MAX`, negatives, NUL bytes, 10 MB files | none measurable |
| T7 | resource exhaustion through huge files or lists (100 000 mounts, 4 096 CPUs) | per-file budget 1 MiB; list budgets (mounts 4 096, interfaces 4 096, CPUs 16 384) checked before allocation; `TooLarge` | fixtures one over each budget: `TooLarge`, allocation within budget (counting allocator) | none in the normal case |
| T8 | hangs on dead network filesystems or removable drives | network filesystems skipped by default; `SEM_FAILCRITICALERRORS` around Windows volume calls | Linux CI with an unreachable NFS mount: `disk::list()` within 100 ms; Windows empty removable drive: no dialog | none |
| T9 | lock-file attacks (symlink at the lock path, pre-created lock file, shared writable directory) | owner-only directory required; no-follow open; writable directories refused; lock file never deleted | symlink planted: refused; directory 0777: refused; second process: `AlreadyRunning`; `kill -9` holder: lock free immediately | one `stat` + one `fstat`, about 2 µs |
| T10 | umask race with other threads | the guard runs before threads; warns when more are running; umask set once | extra thread started: report has `ThreadsAlreadyRunning` | one file read at guard time |
| T11 | disclosure through build and instance info in public endpoints | `BuildInfo::public()`; no build host, user, or path recorded; values validated before `rustc-env` | `public()` output only name and version; hostile branch name (newlines, quotes, 10 KB) gives a clean, bounded value | none |
| T12 | FFI memory-safety bugs (wrong layouts, overruns in size-then-fill loops, leaked CF references, unterminated wide strings) | unsafe confined to `sys/<platform>`; layout assertions per target; bounded retries (at most 3); lengths checked before slicing; RAII release wrappers | Miri on parsers; layout tests on every CI target; soak counting handles, descriptors, CF retain counts; AddressSanitizer job | none |
| T13 | identifying values (host name, user, MAC, time zone) leaving the machine in logs | documented as identifying; excluded from `summary()` | `summary()` checked against the fixture's host name, user, MAC | none |
| T14 | spoofed detection environment (`KUBERNETES_SERVICE_HOST`, planted `/.dockerenv`, fake DMI) | detection advisory; evidence strength recorded; docs forbid security decisions on it | fixture with only weak evidence reports `Strength::Weak` | none |
| T15 | malformed firmware tables (bad lengths, missing terminators, structure loops) | bounded SMBIOS parser: each structure checked against the table end; at most 4 096 structures; no recursion | fuzz target seeded with real tables; fixtures from real machines and hypervisors | none |
| T16 | subprocess hijacking (`PATH` with a fake `lsb_release`, as competitors' fallbacks allow) | no subprocess at run time; the build helper runs `git` and `$RUSTC` only at build time | CI grep for `Command::new` outside the build helper; fake-`PATH` marker test: the marker never appears | none |

---

## 15. Architecture

```
src/
  lib.rs              crate docs, VERSION, re-exports, lint attributes
  error.rs            Error, Result
  units.rs            Bytes, Percent, Units, Mhz
  build.rs            BuildInfo, emit(), Emitter, build_info! macro
  instance.rs         Instance, InstanceId                                  (feature "instance")
  process/
    mod.rs            this process: memory, cpu time, threads, files, limits, user, elevation
    guard.rs          Guard, GuardBuilder, GuardReport                      (feature "guard")
    sampler.rs        CpuSampler
  os/                 kind, name, version, kernel, distro, edition, form factor
  cpu/                name, vendor, arch, counts, frequency, usage, features, topology, quota
  mem.rs              totals, limits, swap, page size, pressure
  disk.rs             space, list, kinds
  net.rs              interfaces, MAC, addresses, counters, sampler
  host.rs             host name, uptime, boot time, load, time zone, locale, model
  virt.rs             VM, hypervisor, container, WSL, Kubernetes
  power.rs            battery, AC
  device/             DeviceId, Fingerprint, StabilityReport                (feature "device-id")
  summary.rs          Summary (Display, key-value, JSON)
  fmt/                hand-written key-value and JSON writers (no serde)
  parse/              safe parsers over &[u8]: os_release, meminfo, stat, mountinfo, cgroup,
                      cpuinfo, uevent, smbios, kv, numbers (all fuzzed, all run under Miri)
  read.rs             bounded file reader: one open, one read into a caller or stack buffer
  sys/
    mod.rs            the platform boundary (pub(crate) functions, no unsafe here)
    linux/            /proc and /sys files; a few libc symbols (sysconf, statvfs, getifaddrs,
                      getrlimit/setrlimit, umask, getrusage, geteuid, getpwuid_r, localtime_r)
    android/          linux/ plus __system_property_get
    macos/            sysctlbyname, mach, IOKit and CoreFoundation, getifaddrs, getfsstat, proc_pidinfo
    ios/              a macos/ subset plus uname
    windows/          kernel32, advapi32, iphlpapi, powrprof, ntdll (RtlGetVersion only)
    unsupported.rs    every function returns Error::Unsupported
```

**The platform boundary.** A public function never contains `#[cfg(target_os)]` branches. It calls `crate::sys::<function>()`, and each platform provides that function. The public layer is identical on every OS, and the "every Unix path has a Windows counterpart or a documented reason" rule is checkable by reading one file per platform.

**Calling the OS without `libc` or `windows-sys`** (D-003):

1. Declare only what is called: about 25 functions on Linux, 30 on macOS, 45 on Windows [est.], in `src/sys/<platform>/ffi.rs`, reviewed line by line against the platform headers.
2. Structs mirror the C layout with `#[repr(C)]` and carry a size assertion per supported target (`const _: () = assert!(size_of::<Statvfs>() == 112);`). A target without a verified layout gets no implementation for that function (it falls back to `unsupported.rs`) and is listed in `docs/PLATFORM-NOTES.md`.
3. Where Linux offers a file with the same data, the file wins over the call: `uname` becomes `/proc/sys/kernel/{ostype,osrelease,version}`, `gethostname` becomes `/proc/sys/kernel/hostname`, `sched_getaffinity` becomes `Cpus_allowed_list`. The calls with layout risk left are `statvfs`, `getifaddrs`, `getrusage`, `getpwuid_r`, `localtime_r`, each with per-target layout tests (glibc and musl differ for some).
4. Windows: `#[link(name = "kernel32")] unsafe extern "system"` blocks, UTF-16 strings with explicit lengths, and "call for the size, allocate, call again, retry at most 3 times" for `GetAdaptersAddresses`, `GetLogicalProcessorInformationEx`, and `GetSystemFirmwareTable`.
5. macOS: CoreFoundation and IOKit objects in small RAII wrappers (`CfOwned` calls `CFRelease`, `IoObject` calls `IOObjectRelease`); Mach memory from `host_processor_info` released with `vm_deallocate` in a drop guard.
6. No callbacks from the OS into Rust, so no unwinding across FFI.
7. x86 CPUID: `core::arch::x86_64::__cpuid` is an `unsafe fn` at 1.89 and safe in later std; the call sits in `sys::x86` with `#[allow(unused_unsafe)]` so it compiles cleanly on both.

**Unsafe code.** The crate root carries `#![deny(unsafe_code)]` (not `forbid`, which cannot be lifted locally), with `#[allow(unsafe_code)]` only on the `mod sys` items for each platform: the arrangement path-lib uses for `SHGetKnownFolderPath`. Also denied: `unsafe_op_in_unsafe_fn`, `clippy::undocumented_unsafe_blocks`, `clippy::multiple_unsafe_ops_per_block`, `clippy::ptr_as_ptr`, `clippy::cast_ptr_alignment`. Every parser is safe Rust over `&[u8]`, so Miri runs the entire parsing layer; unsafe code only moves bytes from the OS into a buffer.

**Features.**

| Feature | Default | Enables | Brings |
|---|---|---|---|
| `instance` | yes | `Instance`, `InstanceId` | clock-lib, id-forge |
| `guard` | yes | `process::guard`, `Guard` | path-lib |
| `device-id` | no | `device` module | crypt-io (and its blake3, subtle) |

Everything else (os, cpu, mem, disk, net, host, virt, power, process readings, build, summary) is always compiled: no dependencies and small. With `default-features = false`, sys-lib has no dependencies at all. A `serde` feature is not planned for 1.0 (section 21, OD-13).

---

## 16. Ecosystem and dependencies

### 16.1 Uses (first-party, by published version, never `path`)

| Crate | Version | For | Feature |
|---|---|---|---|
| `clock-lib` | 1.1 | monotonic start for instance uptime, RFC 3339 formatting | `instance` (default) |
| `id-forge` | 1.0 | ULID instance id | `instance` (default) |
| `path-lib` | 1.0 | runtime directory for the lock file, `write_atomic` for the pid file, name validation | `guard` (default) |
| `crypt-io` | 1.1, `default-features = false`, features `std`, `mac-blake3`, `zeroize` | keyed BLAKE3 for device ids, wiping of raw identifiers | `device-id` (off by default) |

Each dependency is added with the first code that needs it. `crypt-io` brings `blake3` and `subtle`: the portfolio's accepted cryptography exception, and opt-in here. If something is missing in a first-party crate, it is added there in its own PR, never worked around here.

### 16.2 Needs from neighbours

- **path-lib: owner-only directory ACL on Windows** (OD-14). path-lib owns directory creation and already sets 0700 on Unix; the Windows owner-only access list belongs there. Until it exists, the guard reports `umask_after: None` on Windows and documents the gap.

### 16.3 Talks to

shield-sec (privilege drop verification), tower (memory watermark), metrics-lib (system collector), the WASP kernel (boot phase 0), Hive DB (guard and cache sizing).

---

## 17. Competitors and parity

Checked 2026-10-21 against the crates.io API.

| Crate | Latest | Runtime dependencies | MSRV | Notes |
|---|---|---|---|---|
| sysinfo | 0.39.6 | `libc`; on each OS by default `windows` 0.62, `ntapi`, `objc2-*`; optional `rayon`, `memchr`, `serde` | 1.95 | the reference crate. CPU usage needs a previous measurement (internal minimum 200 ms). Keeps file descriptors open to speed process refreshes. Reports host memory in `total_memory()`; cgroup limits through a separate call. |
| heim | 0.0.11 | nine sub-crates and a runtime | none | async-only, unmaintained since 2020 |
| os_info | 3.15.0 | `log`, `nix`, `windows-sys`, `objc2*` | 1.60 | runs the `lsb_release -a` subprocess first on Linux |
| num_cpus | 1.17.0 | `libc`, `hermit-abi` | none | mostly superseded by `std::thread::available_parallelism` for logical counts |
| raw-cpuid | 11.6.0 | `bitflags` | none | x86 CPUID decoding only; no OS-support check for AVX state by itself |
| machine-uid | 0.6.0 | `windows-registry`, `windows-sys`, `libc` | none | returns the **raw** machine id; runs `kenv` on FreeBSD |
| sys-info | 0.9.1 | `libc`, builds C with `cc` | none | unmaintained since 2021 |
| hardware-query | 0.3.0 | sysinfo, num_cpus, regex, serde, serde_json, WMI, ... | 1.95 | wraps sysinfo plus WMI and GPU vendor libraries |
| machineid-rs | 1.2.4 | sysinfo 0.29, hmac, sha2, sha-1, md-5, ... | none | hashed hardware ids with MD5 and SHA-1 options |
| psutil (Python), gopsutil (Go) | reference | | | feature references; gopsutil reads `HOST_PROC`/`HOST_SYS` from the environment |

Where sys-lib is ahead at 1.0 (target): container limits by default, Windows job-object limits, form factor, cleaned CPU names, CPU features with OS support, topology and hybrid cores, memory pressure, permanent versus random MACs, time zone and locale, virtualization with evidence, elevation check, build and instance info, the process guard, an application-scoped keyed device id with fingerprint, no subprocess, zero third-party dependencies, synchronous with no runtime.

Where it deliberately does less: other processes (out of scope), temperatures on macOS and Windows (no documented API), GPU details (no vendor libraries).

---

## 18. Performance

All figures are targets [est.] until measured in 0.6; `docs/PERFORMANCE.md` publishes measured numbers with hardware and method.

| Reading | Target | Compared with | Bar |
|---|---|---|---|
| static fact after first call | 1-2 ns | sysinfo `System::name()` (re-reads), `num_cpus::get_physical()` (not cached) | at least 20% faster |
| first `os::name()` in a fresh process | Linux 15 µs, Windows 20 µs, macOS 10 µs | os_info `get()` (spawns `lsb_release` on Linux) | at least 20% faster |
| `mem::snapshot()` | Linux 4 µs bare / 15 µs in a cgroup; macOS 1 µs; Windows 1 µs | sysinfo `refresh_memory()` + totals + `cgroup_limits()` | at least 20% faster at equal container correctness |
| `cpu::Sampler::sample()` | Linux 3 µs, macOS 1.5 µs, Windows 0.3 µs | sysinfo `refresh_cpu_usage()` + `global_cpu_usage()` | at least 20% faster |
| `cpu::physical()` first call | Linux 200 µs on 32 CPUs | `num_cpus::get_physical()` | at least 20% faster |
| `cpu::has(Feature::Avx2)` | 1 ns | raw-cpuid | at least 20% faster |
| `process::memory()` | Linux 2 µs, macOS 0.5 µs, Windows 0.5 µs | sysinfo refresh of this pid | at least 20% faster |
| `disk::list()` (10 mounts) | Linux 50 µs | sysinfo `Disks::new_with_refreshed_list()` | at least 20% faster |
| `net::interfaces()` | Linux 100 µs | sysinfo `Networks::new_with_refreshed_list()` | at least 20% faster at equal data |
| `host::uptime()` | Linux 0.1 µs | sysinfo `System::uptime()` | at least 20% faster |
| `device::id(app)` Linux | 10 µs | machine-uid `get()` plus a keyed BLAKE3 hash in the benchmark | at least 20% faster |
| allocations per scalar live reading | 0 | | 0 |
| threads started | 0 | | 0 |

Footprint targets: 0 third-party runtime dependencies (3 first-party with default features, 0 with none); clean release build of the dependency under 2 s [est.]; under 300 KiB added to a stripped binary using every module [est.]; 0 descriptors held between calls (the guard holds 1).

Method: criterion groups per module with competitors as dev-dependencies; warm numbers from criterion's loop; cold numbers from a harness that spawns a fresh process per sample (200 samples); hardware recorded with each run; Linux on bare metal and in Docker (cgroup v2). A faster number that reports host memory inside a container does not count. A 5% slowdown against the stored baseline fails the change.

Why sys-lib can be faster: static facts cached in `OnceLock` (competitors re-read or need a long-lived object the caller refreshes); each scalar reading reads exactly one source; stack buffers and in-place parsing, no `String` per line; files instead of calls where both exist, and the call where it is cheaper (`clock_gettime(CLOCK_BOOTTIME)` beats parsing `/proc/uptime`); no thread pool to wake.

---

## 19. Testing

### 19.1 Layers

| Layer | What | Where | Runs |
|---|---|---|---|
| Unit | every function, every `Ok` and `Err` path | bottom of each file | every CI job |
| Fixture conformance | the Linux backend against captured machine trees; Windows SMBIOS and registry parsers against captured tables; macOS sysctl parsing against captured values | `tests/fixtures/`, `tests/linux_fixtures.rs`, `tests/smbios.rs` | every CI job (pure data) |
| Live comparison | answers compared with the OS's own tools on the runner (`nproc`, `getconf`, `free -b`, `df -B1`, `ip -j addr`, `systemd-detect-virt`; `sysctl -n`, `vm_stat`; `Get-CimInstance`, `Get-NetAdapter`, `Get-Volume`). Tests may spawn tools; the library never does. Exact for counts and names; memory within 2% plus 16 MiB | `tests/live_*.rs` | every CI job on its OS |
| Property | invariants (5.3), `Bytes` formatting and ordering, `Percent` bounds, parsers never panic or exceed budget, round trips | `tests/prop_*.rs` | every CI job |
| Fuzz | every parser (19.4) | `fuzz/` | nightly job 10 minutes per target; 24 hours per target before 0.7 |
| Miri | the whole `parse/` module and `units.rs` | CI job (from 0.3) | every push |
| Counting allocator | every scalar live reading allocates 0 times after warm-up; list functions at most their documented count | `tests/alloc.rs` | every CI job |
| Concurrency | 64 threads call every static getter at once at first use and see the same value; 16 threads call live readings for 10 s | `tests/concurrency.rs` | every CI job |
| Container | the suite inside Docker with `--memory=256m --cpus=1.5 --cpuset-cpus=0-1` on cgroup v2: `mem::total()` 256 MiB, `cpu::available()` 2, `is_container()` true | CI job (from 0.4) | every push |
| Process guard | the test binary re-executes itself as a second instance; `kill -9` tests | `tests/guard.rs` | every CI job |
| Attack | one test per threat T1 to T16 | `tests/attack_*.rs` | every CI job |
| Soak | 1 hour weekly, 8 hours before 1.0: descriptors, handles, CF retain counts, RSS sampled every minute; any growth fails | `examples/soak.rs` | weekly, before 0.9 and 1.0 |
| Cross-target | `cargo check` for musl, i686 Windows, Android, iOS | CI | every push |
| WSL | the full suite in WSL 2 Ubuntu plus WSL assertions | local gate | before every push |

Test names: `test_<subject>_<condition>_<expected>`, for example `test_mem_total_cgroup_v2_nested_limit_returns_smallest`.

### 19.2 Fixture machines (Linux trees)

Each fixture is a directory with the files the Linux backend reads (`proc/`, `sys/`, `etc/`, `run/`), captured by `tests/fixtures/capture.sh` on real machines, with identifiers replaced by fake ones before commit. The backend reads through a root prefix, so fixtures run on any host.

Planned: `ubuntu-24.04-desktop-amd`, `fedora-laptop-intel-hybrid`, `debian-server-2socket`, `docker-cgroup2-limited`, `docker-cgroup1-limited`, `k8s-pod-cgroup2`, `lxc-lxcfs`, `wsl1`, `wsl2`, `raspberry-pi-5`, `arm-biglittle-phone-like`, `gvisor`, `minimal-scratch`, `vm-kvm`, `vm-vmware`, `vm-hyperv-guest`, `xen-domu`, `xen-dom0`, `huge-1024-cpus` (synthetic: 1 024 CPUs, 20 000 mounts, 5 000 interfaces), `hostile` (synthetic: malformed, oversized, binary, negative, and overflowing values in every file). Windows: raw SMBIOS tables and registry captures. macOS: sysctl and IOKit property dumps through a test seam.

### 19.3 Edge cases

Every entry has a test. The ids are stable; the roadmap and the tests refer to them.

**Containers and limits**

- E-1 cgroup v2 `memory.max` is `max`: no limit; host values used; `limit()` is `None`.
- E-2 cgroup v2 nested: a parent's `memory.max` is lower than the leaf's: the minimum over ancestors wins.
- E-3 cgroup v1 unlimited sentinel (2^63 minus one page, `9223372036854771712` on 4 KiB pages) and any value at or above host RAM: treated as no limit.
- E-4 hybrid hierarchy (v1 controllers plus a v2 unified mount at `/sys/fs/cgroup/unified`): memory from the v1 `memory` controller.
- E-5 cgroup namespace (path `/`) versus no namespace (path `/system.slice/site.service` from a systemd unit with `MemoryMax=`): both resolve correctly.
- E-6 cgroupfs mounted at a non-standard path: found through mountinfo.
- E-7 no cgroupfs at all (gVisor, minimal sandboxes): host values, `source: Host`, no error.
- E-8 `/proc/self/cgroup` unreadable: host values, no error.
- E-9 a limit larger than host RAM (1 TiB on a 64 GiB host): `total()` is the host RAM.
- E-10 LXCFS present: the limit is not applied twice; `source: Lxcfs`.
- E-11 `inactive_file` larger than `memory.current` (values read at different instants): used saturates at 0.
- E-12 CPU quota fractional (`150000 100000`: available 2), below one CPU (`50000 100000`: available 1), and cpuset limited to 2 of 64 CPUs (available 2).

**CPU counts and topology**

- E-13 Windows with more than 64 logical processors: `logical()` counts all groups; `available()` follows the Windows 11 / Server 2022 default all-group affinity, and on older Windows the single primary group.
- E-14 Linux affinity list with ranges and gaps (`0-3,8-11,64`), and more than 1 024 CPUs: parsed from `Cpus_allowed_list` without a fixed-size mask.
- E-15 `possible` greater than `present` greater than `online` (VMs often report `possible` 0-127 with 4 present): `logical()` counts **present** CPUs, `online()` counts online ones; per-CPU arrays are sized by `logical()`.
- E-16 hybrid cores: Intel 8 P-cores with SMT plus 16 E-cores without (24 cores, 32 threads); ARM three-tier clusters: counts and `CoreKind` correct; the name lists core types.
- E-17 SMT disabled in firmware or through `/sys/devices/system/cpu/smt/control`: `physical == logical`.
- E-18 topology hidden in a container: `physical()` falls back to `logical()` with `physical_exact: false`.
- E-19 Apple silicon: no SMT (`physical == logical`), performance levels reported; a Rosetta 2 process sees `arch() == X86_64`, `native_arch() == Aarch64`, `is_translated() == true`.

**CPU usage**

- E-20 counters go backwards (a CPU offlined and back, VM live migration, counter wrap): the sampler re-bases and returns `TooSoon` for that core; never negative, never above 100%.
- E-21 two samples within one tick: `TooSoon` with `minimum` 10 ms (Linux).
- E-22 the set of CPU lines changes between samples (hot-plug adds `cpu17`): per-core output resized (or `TooLarge` for a short caller buffer); total unaffected.
- E-23 CPU hot-plug after `topology()` was first called: topology stays as first read (documented static fact); `online()` is live.
- E-24 high steal time on an overcommitted VM: steal counts as busy, reported separately in `Usage::steal`, matching psutil's convention.
- E-25 iowait counts as idle in `used()` (the CPU was free to run other work), reported separately.
- E-26 Windows `GetSystemTimes`: kernel time includes idle; a reading where idle exceeds kernel saturates instead of underflowing.
- E-27 `/proc/stat` larger than 4 KiB (hundreds of CPUs): total usage reads only the first line; per-core uses the bounded heap buffer.

**Memory**

- E-28 kernels before 3.14 without `MemAvailable`: fallback `MemFree + Buffers + Cached` (documented approximation).
- E-29 Hyper-V dynamic memory or a balloon driver changes `MemTotal` while running: `total()` is live and follows it.
- E-30 Kubernetes in-place resize changes `memory.max`: the next `total()` reflects it.
- E-31 no swap (`SwapTotal: 0`): `Swap` all zero, percentages 0%, never NaN.
- E-32 zram and zswap: zram devices count as swap; zswap is invisible to these counters (documented).
- E-33 macOS compressed memory: counted in used, consistent with Activity Monitor's "memory used" [est.; validated in CI against `vm_stat` arithmetic].
- E-34 32-bit process on 64-bit Windows: memory totals are the machine's; the process's address-space limit is in `process::limits().address_space`.
- E-35 ZFS ARC on Linux appears as used memory in `MemAvailable` (the kernel does not count it as reclaimable): documented; no correction in 1.x.

**Disks**

- E-36 an unreachable NFS mount: `disk::list()` skips it by default and returns promptly; `include_network: true` documents the risk.
- E-37 mountinfo escapes (`\040` space, `\011` tab, `\012` newline, `\134` backslash), bind mounts, btrfs subvolumes, overlay root in containers: decoded and deduplicated by device number.
- E-38 Windows: an empty card reader or optical drive, mounted-folder volumes without drive letters, a BitLocker-locked volume: no dialog; space `None`; listed with the right kind.
- E-39 macOS APFS volumes sharing one container (System, Data, Preboot, VM): deduplicated so free space is not counted several times; firmlinks not listed twice.

**WSL**

- E-40 WSL 1 (`osrelease` contains `Microsoft`) versus WSL 2 (`microsoft-standard-WSL2`): `virt::detect().wsl` is 1 or 2.
- E-41 WSL 2 has no DMI: `form_factor()` Unknown, `host::model()` None, `Basis::Hardware` unavailable (the report says Unsupported).
- E-42 WSL 2 memory is the utility VM's (limited by `.wslconfig`, by default a share of host RAM): reported as is; documented as correct for processes inside WSL.
- E-43 WSL 2's `eth0` MAC changes per boot: the `PermanentMac` anchor is not used when `is_wsl()`.
- E-44 WSL file systems: a lock directory on `/mnt/c` (drvfs in WSL 1, 9p in WSL 2) is treated like a network filesystem by the guard.
- E-45 the WSL host name equals the Windows host name; `/etc/machine-id` may be missing when systemd is off: `device::id` returns `NoIdentifierSource` and the report says why.
- E-46 inside WSL 2, CPUID reports `Microsoft Hv`: `is_vm()` and `is_wsl()` are both true, documented; on the Windows side, a host with WSL 2, Hyper-V, or VBS enabled is **not** a VM (root-partition check).

**OS identity**

- E-47 no os-release (scratch, distroless): `name()` is `"Linux"`, `distro()` None, `info().name_source` is fallback.
- E-48 os-release with single and double quotes, backslash escapes, comments, blank lines, invalid lines, and the `/etc/os-release` symlink to `/usr/lib/os-release`: parsed per os-release(5); invalid lines skipped.
- E-49 Windows 11 whose registry `ProductName` says "Windows 10": named Windows 11 from the build number (22000 and later); Windows Server named from `wProductType` and build (17763 is 2019, 20348 is 2022, 26100 is 2025).
- E-50 an executable without a Windows 10 compatibility manifest: `RtlGetVersion` still reports the real version (unlike `GetVersionEx`).
- E-51 macOS with `SYSTEM_VERSION_COMPAT=1`: the real version is reported.
- E-52 a future macOS major with no known codename, and the jump from 15 to 26: name printed without a codename; version parsed.

**Network**

- E-53 interfaces without a MAC (loopback, `tun`, WireGuard): `mac` None.
- E-54 randomized MACs (Android, iOS private addresses, Windows random hardware addresses, NetworkManager): `MacKind::Random`.
- E-55 Windows friendly names with non-ASCII characters; Linux alternative names: names are UTF-8 (lossy conversion documented), deterministic.
- E-56 thousands of `veth` interfaces on a Kubernetes node: bounded by the interface budget; linear cost.
- E-57 IPv6 link-local addresses with scope ids; addresses on down interfaces: reported with prefix; `addresses()` excludes link-local by default.
- E-58 link speed unknown (Linux `-1` or an error, Windows maximum values): `None`.

**Build, instance, guard**

- E-59 build helper: `git` missing, not a repository, shallow clone, detached HEAD, a worktree (`.git` is a file pointing at the git dir): values `unknown` or correct; never a build failure; `rerun-if-changed` paths resolved through the `.git` file.
- E-60 the application forgot `build::emit()`: `build_info!()` compiles and shows `unknown`.
- E-61 `SOURCE_DATE_EPOCH` invalid or negative: ignored with a `cargo:warning`.
- E-62 `Instance::start()` called from 64 threads at once: one instance.
- E-63 a stale `.pid` file from a crashed run while the lock is free: lock acquired, pid file replaced atomically.
- E-64 lock directory is a symlink, group- or world-writable, or on NFS; the lock file is a symlink: refused with the reason.
- E-65 the guard called after threads started: warning in the report; umask applied.
- E-66 macOS hard limit `RLIM_INFINITY`: the target is capped at `kern.maxfilesperproc` so `setrlimit` succeeds.
- E-67 hard limit below the minimum: `OpenFilesTooLow { have, need }`; nothing else changed.
- E-68 Windows: a second process cannot read the locked file; the pid comes from the `.pid` sidecar.

**Device identity**

- E-69 `/etc/machine-id` empty, `uninitialized`, all zeros, or missing: the anchor is `Placeholder` or `Absent`; clone risk `MachineIdUninitialized` for empty and `uninitialized`.
- E-70 SMBIOS placeholder UUIDs and serials (section 12.2 list): `Placeholder`.
- E-71 VM clones sharing MachineGuid or SMBIOS UUID: clone risk `VirtualMachine`; documentation explains sysprep `/generalize` and `systemd-machine-id-setup`.
- E-72 containers started from one image with a baked-in machine id: clone risk `Container` (and `SharedImageLikely`, section 22).
- E-73 Linux without root: `Basis::Hardware` returns `NoIdentifierSource`; the report shows `PermissionDenied` for the DMI anchors.
- E-74 a 32-bit process on 64-bit Windows: `MachineGuid` read with `KEY_WOW64_64KEY`.
- E-75 SMBIOS UUID byte order: the Windows raw table (first three fields little-endian since SMBIOS 2.6) and Linux sysfs text canonicalise to the same 16 bytes.
- E-76 dual boot: `Basis::Hardware` gives the same id under Linux (root) and Windows for the same app id.
- E-77 randomized MACs never become the `PermanentMac` anchor.
- E-78 `Fingerprint::from_bytes` with truncated, extended, wrong-magic, future-version, non-zero-reserved, inconsistent-bitmap, or bad-checksum input: rejected with `Malformed`, no panic.

**Form factor, power, virtualization, host**

- E-79 a VM reporting chassis "Other": `Unknown`.
- E-80 wrong firmware chassis (a desktop BIOS value on a laptop): `CHASSIS=laptop` in `/etc/machine-info` overrides on Linux.
- E-81 peripheral batteries (`scope=Device`: wireless mouse, keyboard, headset): excluded from `has_battery()` and `battery()`.
- E-82 two internal batteries: combined by energy (`ENERGY_NOW` sum over `ENERGY_FULL` sum), not averaged by percentage.
- E-83 battery present but status unknown or capacity missing: `BatteryState::Unknown`, `charge` from energy if available.
- E-84 Windows host with Hyper-V, WSL 2, or VBS enabled: not a VM (E-46).
- E-85 nested virtualization (KVM inside Hyper-V): the innermost hypervisor visible to the guest is reported.
- E-86 a container inside a VM: both `container` and `hypervisor` set.
- E-87 `KUBERNETES_SERVICE_HOST` set outside Kubernetes: `kubernetes: true` with weak evidence only; documented.
- E-88 Rosetta 2: x86 feature detection reports what Rosetta emulates (no AVX); documented.
- E-89 Windows on ARM running an x64 process: `native_arch()` is `Aarch64`, `is_translated()` true.
- E-90 Android SELinux denying `/sys/class/power_supply`: `PermissionDenied`.
- E-91 iOS simulator: form factor from `SIMULATOR_MODEL_IDENTIFIER`.
- E-92 `/etc/localtime` is a copy, not a symlink: IANA name from `/etc/timezone` if present, else `None` (no guessing by comparing zone files).
- E-93 `TZ` empty, `:Europe/London` (leading colon), a path, or hostile: leading colon stripped; paths and hostile values ignored.
- E-94 locale `C`, `POSIX`, or `C.UTF-8`: `locale()` is `None`.
- E-95 host name not valid UTF-8 or longer than 255 bytes: lossy UTF-8 conversion, truncated at 255 bytes, documented.
- E-96 `/proc` mounted with `hidepid=2`: `/proc/1/*` unreadable; container detection uses the remaining checks.
- E-97 seccomp or a sandbox blocking a call (`statvfs` returns `EPERM`, `sysctl` denied in the macOS App Sandbox): `PermissionDenied` naming the call.
- E-98 gVisor and Firecracker: different `/proc` content; fixtures pass.
- E-99 the wall clock stepping backwards: `boot_time()` from `btime` is stable; `uptime()` is monotonic.
- E-100 no cpufreq directory (many VMs, some ARM boards): `frequency()` fields `None`.

Every FR and NFR in section 4 maps to at least one named test in `tests/requirements.rs`, with a comment naming the requirement id; the 1.0 audit lists the mapping.

### 19.4 Fuzz targets

`os_release`, `meminfo`, `proc_stat`, `proc_status`, `mountinfo`, `cgroup`, `cpuinfo`, `cpu_list`, `uevent`, `smbios`, `pid_file`, `fingerprint`, `instance_id`, `bytes_display`, `windows_zones`. Each asserts no panic, no allocation beyond its budget, and no result violating the type invariants. Seeds are the fixtures; the corpus is kept outside the repository.

---

## 20. Documentation deliverables

| File | Content | By |
|---|---|---|
| `README.md` | header image, badges (crates.io, downloads, docs.rs, CI, MSRV 1.89), pitch, features, install, quick start (Tier 1 first), status, links, license, footer | 0.1 slim, 0.3 quick start, 1.0 full |
| `docs/API.md` | every public item: signature, description, parameters, errors, platform table, several examples per use case | 0.2, updated each version |
| `docs/SOURCES.md` | for every reading, per OS: the file, system call, or API; units; privileges; quirks | 0.2 first version, complete by 0.5 |
| `docs/PLATFORM-NOTES.md` | per OS and environment: what is supported, what is `Unsupported`, and why | 0.5 |
| `docs/DEVICE-ID.md` | derivation, anchors, stability, clone risks, privacy and legal notes, when not to use it | 0.5 |
| `docs/ARCHITECTURE.md` | module tree, platform boundary, static versus live, FFI rules, parser layer | 0.5 |
| `docs/PERFORMANCE.md` | measured numbers, method, hardware, competitors, cold and warm, container and bare metal | 0.6 |
| `docs/SECURITY.md` | the threat model, each defence with its test and cost | 0.7 |
| `docs/STABILITY-1.0.md` | the 1.0 audit, the requirement-to-test map, and the 1.x guarantees (API, output formats, device id derivation) | 0.9 draft, 1.0 |
| `CHANGELOG.md` | Keep a Changelog 1.1.0, in the same change as the code | every version |
| `docs/release/vX.Y.Z.md` | release notes, past tense | every version |
| rustdoc | every public item with a runnable example; Tier 1 first; `doc(cfg)` badges on platform-specific items | every version |
| `examples/` | `quick_start`, `summary`, `monitor` (sampler loop), `guard`, `device_id`, `container_limits`, `soak` | grows per version |

---

## 21. Open decisions

Each has a recommendation. The recommendation stands unless James overrides it; every outcome goes into the decision log. All are settled by the 0.2.0 design lock. OD-1, OD-2, OD-3, OD-7, OD-8, and OD-10 are already recorded as D-002, D-003, D-006, D-007, and D-005.

| Id | Decision | Recommendation and reason | Status |
|---|---|---|---|
| OD-1 | MSRV: 1.85 with in-house lock calls, or 1.89 | **1.89.** `File::lock` and `File::try_lock` are stable since 1.89.0, removing lock FFI; a new crate has no users to break; clock-lib, path-lib, id-forge, and crypt-io all build on 1.89 | D-002 |
| OD-2 | How to call the OS: in-house declarations; `libc` + `windows-sys`; build on sysinfo | **In-house**, with `libc` per target as the recorded fallback. sysinfo is rejected: MSRV 1.95, host memory by default, the large `windows` crate | D-003 |
| OD-3 | Device id hash: keyed BLAKE3 (crypt-io); HMAC-SHA256 like systemd; in-house | **Keyed BLAKE3 through crypt-io.** Cryptography is never written in-house; it is the portfolio's MAC already; faster; compatibility with systemd's value is not a goal | D-006 |
| OD-4 | Default device-id basis: `Install`, `Hardware`, or a composite | **`Install`.** Unprivileged on Linux, macOS (platform UUID), and Windows; systemd's recommended source; reads no serial numbers; a reinstall is often the right moment for a new device. A composite is rejected: any part changing changes the whole id | open |
| OD-5 | `Bytes` default `Display` units: IEC ("125.6 GiB"), JEDEC ("125.6 GB", 1024-based, what Windows Explorer shows), SI ("134.9 GB") | **IEC by default**, `{:#}` for SI, `.jedec()` for the "127 GB" style. IEC labels are the only unambiguous choice. Switching the default to JEDEC is a one-line change before 0.3 and a breaking change after 1.0 | open (James) |
| OD-6 | `os::is_device()`: keep or rename | **Rename to `is_mobile()`.** "Device" is every computer; the intent is phones, tablets, handhelds, wearables. No alias | open |
| OD-7 | CPU usage Tier 1: blocking 100 ms, or a psutil-style hidden baseline | **Blocking, 100 ms, named and documented.** A hidden baseline is global mutable state and two callers reset each other's window | D-007 |
| OD-8 | `mem::total()` in containers: the limit or the host | **The limit.** That is what the process can use and what the OOM killer enforces; `host_total()` gives the host. Differs from sysinfo and psutil; the README says so in its first feature line | D-005 |
| OD-9 | Default lock location for `process::guard(name)` | **path-lib's runtime directory**, with `.lock_dir(data_dir)` documented as the right choice for databases (two copies under different users still exclude each other) | open |
| OD-10 | `device-id` default | **Off.** It brings crypt-io and BLAKE3, and device identifiers carry privacy obligations | D-006 |
| OD-11 | Android and iOS support level | **Build-checked best effort for 1.x.** No portfolio product targets them; emulator CI costs time on every push | open |
| OD-12 | Instance user field: effective user after the privilege drop, or the user at start | **`user_at_start()` in the instance, `process::user()` for the current user.** The instance is created before the drop | open |
| OD-13 | `serde` now; a separate units crate | **Neither for 1.0.** Hand-written key-value and JSON writers cover logs and health endpoints; add `serde` behind a non-default feature only when a caller asks | open |
| OD-14 | Owner-only directories on Windows: sys-lib, path-lib, or shield-sec | **path-lib**, which owns directory creation; until then the guard reports the gap | open |
| OD-15 | Time types in the public API: std or clock-lib | **std `Duration` and `SystemTime`.** clock-lib types would tie sys-lib's major version to clock-lib's | open |
| OD-16 | Crate name and import convention | **`sys-lib`, `use sys_lib as sys;` in every example.** Matches the `-lib` taxonomy; the alias gives the `sys::os::name()` shape | open |
| OD-17 | Collection and visibility | **Public, in `lib-collection`.** Nothing proprietary; a general utility | open |

---

## 22. Ideas parking lot

At least one new defence idea per version, as hypothesis, method, and expected cost:

| Version | Hypothesis | Method | Expected cost |
|---|---|---|---|
| 0.3 | a lock directory on a filesystem without reliable locking (NFS, drvfs, 9p) silently allows two copies | refuse network and WSL host filesystems by `statfs` magic or type name | one `statfs` at start |
| 0.4 | a hostile cgroup tree can make sys-lib walk forever (a bind-mounted fake tree with deep nesting) | cap the ancestor walk at 64 levels and the path at 4 096 bytes | none |
| 0.5 | a device id derived from a shared container image identifies every container as the same device | `CloneRisk::SharedImageLikely` when the machine id file's modification time predates the container's start and the container has no private `/etc` mount | one `stat` |
| 0.6 | timing of `Fingerprint::compare` could reveal which anchor changed to a co-located observer | constant-time per-anchor comparison and a fixed iteration count | about 10 ns |
| 0.7 | a fuzzer-found class of parser bug repeats across parsers | one shared, fuzzed number and key-value tokenizer used by every parser | none |

Other ideas:

- A fixture recorder mode in the Linux backend's test seam that lists every path opened, so attack test T4 can assert the exact set and a new, unreviewed file read shows up as a test failure.
- `cpu::available()` as the input to a worker-pool sizing helper in the runtime crate, not here (a `-lib` decides nothing for the caller).

---

## 23. Decision log

| Id | Date | Decision | Reason |
|---|---|---|---|
| D-001 | 2026-10-21 | sys-lib supersedes the earlier SYS-LIB plan and merges build info, instance info, and the process guard into one crate | the boot kit and the system facts share sources (host name, user, CPU counts) and one platform layer; one crate keeps phase 0 to one dependency |
| D-002 | 2026-10-21 | MSRV 1.89, edition 2024 | `std::fs::File::lock` and `try_lock` are stable since 1.89.0, so the guard needs no lock FFI; a new crate breaks no users; every first-party dependency builds on 1.89 |
| D-003 | 2026-10-21 | OS calls through in-house declarations in `src/sys/<platform>/ffi.rs`, no `libc` or `windows-sys`; `libc` for a single target is the recorded fallback when a struct layout cannot be verified, argued here first | zero third-party runtime dependencies is portfolio law; path-lib already does this for Windows; the layout risk is held down by preferring Linux files over calls, size assertions per target, and a short target list |
| D-004 | 2026-10-21 | No subprocess at run time; only the build helper runs `git` and `$RUSTC`, at build time | removes `PATH` hijacking (T16) and millisecond costs; enforced by a CI grep and a fake-`PATH` test |
| D-005 | 2026-10-21 | Memory totals and CPU availability respect cgroup v1/v2 and job-object limits by default; host values are separate calls | what the process can use is what callers size caches and pools by; the OOM killer enforces the limit, not the host total |
| D-006 | 2026-10-21 | Device id by keyed BLAKE3 through crypt-io; feature `device-id` off by default; derivation `sys-lib/device/v1` frozen for 1.x | no in-house cryptography; the portfolio's MAC; opt-in because of the dependency chain and privacy duties; freezing protects every stored id |
| D-007 | 2026-10-21 | CPU usage Tier 1 blocks 100 ms (`cpu::usage()`); no hidden global baseline; the non-blocking path is the caller-owned `Sampler` | a hidden baseline is global mutable state and concurrent callers would corrupt each other's window |
| D-008 | 2026-10-21 | No async runtime; the REPS clause that requires tokio is an approved exception for this crate | a `-lib` has no threads or tasks; every call is a file read or a system call measured in microseconds; the portfolio does not use tokio |
| D-009 | 2026-10-21 | CI covers macOS on both architectures: `macos-latest` (aarch64) and `macos-15-intel` (x86_64), next to `ubuntu-24.04-arm` and `windows-11-arm` | the definition of done requires x86_64 and aarch64 on all three OSes, and `macos-latest` alone is aarch64 only |
| D-010 | 2026-10-21 | CI builds the documentation with default features and with all features, both under `-D warnings` | a doc link to a feature-gated item breaks only when that feature is off; checking one mode misses it |
| D-011 | 2026-10-21 | The fuzz crate (`fuzz/`) is its own workspace with no targets until the first parser lands in 0.3; corpora stay outside the repository | no placeholder targets in shipped files; a corpus written into the tree once swept fuzzer output into a commit in another repository |
