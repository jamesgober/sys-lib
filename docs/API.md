<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br><b>sys-lib</b><br>
    <sub><sup>API REFERENCE</sup></sub>
</h1>
<div align="center">
    <sup>
        <a href="../README.md" title="Project Home"><b>HOME</b></a>
        <span>&nbsp;│&nbsp;</span>
        <span>API</span>
        <span>&nbsp;│&nbsp;</span>
        <a href="./SOURCES.md" title="Sources"><b>SOURCES</b></a>
        <span>&nbsp;│&nbsp;</span>
        <a href="../dev/PLAN.md" title="Plan"><b>PLAN</b></a>
    </sup>
</div>
<br>

The complete public API of sys-lib 1.0, locked at `0.2.0`. Every item carries the version it arrives in (**Since**); the published crate exposes only what is implemented, so an item marked `0.4` does not exist in `0.3`. Signatures here are the contract: a change after `0.2.0` is recorded in the decision log of [`../dev/PLAN.md`](../dev/PLAN.md). Where an operating system has no such fact, the item says so and returns `Error::Unsupported`, `None`, or an `Unknown` variant; it never guesses. The exact file, system call, or API behind every reading, per OS, is in [`SOURCES.md`](./SOURCES.md).

Costs are estimates (marked [est.]) until they are measured in `0.6.0`; measured numbers then replace them, labelled as measured.

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Table of contents

- [Setup](#setup)
- [Conventions](#conventions)
  - [Tiers](#tiers)
  - [Three kinds of answers](#three-kinds-of-answers)
  - [Platforms](#platforms)
- [Crate root](#crate-root)
  - [`VERSION`](#version)
  - [`Error` and `Result`](#error-and-result)
  - [`Bytes` and `Units`](#bytes-and-units)
  - [`Percent`](#percent)
  - [`Mhz`](#mhz)
  - [`summary()`](#summary)
- [`build`: build info](#build-build-info)
- [`instance`: instance info](#instance-instance-info)
- [`process`: this process](#process-this-process)
  - [Readings](#readings)
  - [`CpuSampler`](#cpusampler)
  - [The process guard](#the-process-guard)
- [`os`: operating system](#os-operating-system)
  - [Kind and identity](#kind-and-identity)
  - [Form factor](#form-factor)
- [`cpu`: processor](#cpu-processor)
  - [Identity and counts](#identity-and-counts)
  - [Usage](#usage)
  - [Features](#features)
  - [Topology, caches, NUMA](#topology-caches-numa)
- [`mem`: memory](#mem-memory)
- [`disk`: disks and filesystems](#disk-disks-and-filesystems)
- [`net`: network interfaces](#net-network-interfaces)
- [`host`: the machine and its environment](#host-the-machine-and-its-environment)
- [`virt`: virtualization and containers](#virt-virtualization-and-containers)
- [`power`: battery and AC](#power-battery-and-ac)
- [`device`: device identity](#device-device-identity)
  - [Tier 1](#tier-1)
  - [`Builder`](#builder)
  - [Stability report](#stability-report)
  - [Fingerprint](#fingerprint)
  - [When not to use it](#when-not-to-use-it)
- [After 1.0](#after-10)
- [Errors by variant](#errors-by-variant)

## Setup

```toml
[dependencies]
sys-lib = "0.2"
# Without instance info and the process guard (no dependencies at all):
# sys-lib = { version = "0.2", default-features = false }
# With device identity:
# sys-lib = { version = "0.2", features = ["device-id"] }
```

| Feature | Default | Enables | Brings |
|---|---|---|---|
| `instance` | yes | `Instance`, `InstanceId`, `instance::current()` | clock-lib, id-forge |
| `guard` | yes | `process::guard`, `Guard`, `GuardBuilder`, `GuardReport` | path-lib |
| `device-id` | no | the `device` module | crypt-io (keyed BLAKE3, zeroize) |

Everything else is always compiled and has no dependencies. No feature brings a third-party crate except `device-id`, whose cryptography comes through crypt-io.

The convention in every example:

```rust
use sys_lib as sys;
```

## Conventions

### Tiers

| Tier | What | Examples |
|---|---|---|
| 1 | free functions, no configuration, a plain value or `Result<plain value>` | `sys::os::name()`, `sys::mem::total()`, `sys::process::guard("site")` |
| 2 | snapshots, samplers, builders, options | `sys::mem::snapshot()`, `sys::cpu::Sampler`, `Guard::builder` |
| 3 | caller buffers and raw counters | `Sampler::sample_per_core_into`, `sys::cpu::times()` |

### Three kinds of answers

| Kind | Cached | Cost after the first call | Examples |
|---|---|---|---|
| compile-time constant | in the binary | 0 | `os::kind()`, `cpu::arch()` |
| static fact | once per process, in a `OnceLock` | about 1 ns [est.] | `os::name()`, `cpu::name()`, `cpu::physical()`, `virt::detect()` |
| live reading | never | one file read or system call, 0.2 to 20 µs [est.] | `mem::used()`, `host::uptime()`, `disk::space()` |
| sampled value | in a caller-owned sampler | one reading and a subtraction | `cpu::Sampler`, `net::Sampler` |

A static fact that failed is cached as failed (the reason is kept, not retried). Live readings that look static but are not: `mem::total()` (cgroup resize, ballooning), `cpu::available()` and `cpu::online()` (affinity, hot-plug, quota), `host::name()`, `host::timezone()`, `net::interfaces()`.

Every public function is safe to call from any thread at any time. All public types are `Send + Sync`; samplers are `Send` and take `&mut self`. sys-lib never starts a thread, never runs another program, and holds no file descriptor or handle after a call returns (the guard's lock file excepted).

### Platforms

Linux, macOS, and Windows are first class (x86_64 and aarch64). Android and iOS are build-checked and best effort: the Linux or Apple code path where it applies unchanged, `Error::Unsupported` elsewhere, as listed per item. Other targets compile and return `Error::Unsupported` from every reading.

Unless an item says otherwise, a function returning `Result` can fail with `Unsupported`, `PermissionDenied`, `NotFound`, `Malformed`, `TooLarge`, or `Os`; see [Errors by variant](#errors-by-variant).

---

## Crate root

### `VERSION`

**Since** 0.1 · ROOT-01

```rust
pub const VERSION: &str;
```

The crate version, populated by Cargo at build time.

```rust
use sys_lib as sys;

println!("sys-lib {}", sys::VERSION);
```

### `Error` and `Result`

**Since** 0.3 · ROOT-02

```rust
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Unsupported { what: &'static str, platform: &'static str },
    PermissionDenied { what: &'static str, hint: &'static str },
    NotFound { what: &'static str, source: &'static str },
    Malformed { what: &'static str, source: &'static str, line: Option<u32> },
    TooLarge { what: &'static str, limit: u64 },
    TooSoon { elapsed: Duration, minimum: Duration },
    Os { what: &'static str, call: &'static str, code: i32 },
    Io { what: &'static str, source: std::io::Error },
    InvalidInput { what: &'static str, reason: &'static str },
    AlreadyRunning { pid: Option<u32>, lock_file: PathBuf },
    RunDirUnavailable { path: PathBuf, source: std::io::Error },
    OpenFilesTooLow { have: u64, need: u64 },
    #[cfg(feature = "device-id")]
    NoIdentifierSource { basis: device::Basis },
}

pub type Result<T, E = Error> = core::result::Result<T, E>;

impl Error {
    /// True for errors that mean "this fact is not available here", which a caller
    /// usually treats as unknown: Unsupported, PermissionDenied, NotFound.
    pub fn is_unavailable(&self) -> bool;
}
```

`Error` is `Send + Sync + 'static` and implements `std::error::Error` (`source()` returns the I/O error for `Io` and `RunDirUnavailable`). `Display` is one sentence saying what failed and what to do. No variant ever carries file content, registry content, or identifier values: `Malformed` names the source (a constant path) and the line number only.

```rust
use sys_lib as sys;

match sys::mem::pressure() {
    Ok(Some(p)) => println!("memory pressure {:?}", p.level),
    Ok(None) => println!("this kernel reports no memory pressure"),
    Err(e) if e.is_unavailable() => println!("unknown here: {e}"),
    Err(e) => return Err(e),
}
# Ok::<(), sys_lib::Error>(())
```

Every variant, its meaning, and what the message tells the caller to do: [Errors by variant](#errors-by-variant).

### `Bytes` and `Units`

**Since** 0.3 · ROOT-03

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bytes(/* u64 */);

impl Bytes {
    pub const ZERO: Bytes;
    pub const MAX: Bytes;
    pub const fn new(bytes: u64) -> Bytes;
    pub const fn raw(self) -> u64;
    pub const fn from_kib(n: u64) -> Bytes;           // saturating
    pub const fn from_mib(n: u64) -> Bytes;           // saturating
    pub const fn from_gib(n: u64) -> Bytes;           // saturating
    pub const fn saturating_add(self, other: Bytes) -> Bytes;
    pub const fn saturating_sub(self, other: Bytes) -> Bytes;
    pub const fn checked_add(self, other: Bytes) -> Option<Bytes>;
    pub const fn checked_sub(self, other: Bytes) -> Option<Bytes>;
    pub fn percent_of(self, total: Bytes) -> Percent; // 0% when total is zero; clamped to 100%
    pub fn display(self, units: Units) -> impl fmt::Display;
    pub fn binary(self) -> impl fmt::Display;         // "125.6 GiB"
    pub fn decimal(self) -> impl fmt::Display;        // "134.9 GB"
    pub fn jedec(self) -> impl fmt::Display;          // "125.6 GB" (1024-based, JEDEC labels)
}
impl From<u64> for Bytes;
impl From<Bytes> for u64;
impl fmt::Display for Bytes;  // binary by default, decimal with {:#}
impl fmt::Debug for Bytes;    // Bytes(134876545024)

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Units { Binary, Decimal, Jedec }
```

A byte count with an exact `u64` inside and a `Display` that picks the unit. Formatting is deterministic, allocation-free, and done in integer arithmetic, so it never depends on floating-point printing:

| Rule | Detail |
|---|---|
| `{}` | binary values, IEC labels: `B`, `KiB`, `MiB`, `GiB`, `TiB`, `PiB`, `EiB` |
| `{:#}` | decimal SI: `B`, `kB`, `MB`, `GB`, `TB`, `PB`, `EB` |
| `.jedec()` | binary values with JEDEC labels: `B`, `KB`, `MB`, `GB`, `TB`, `PB`, `EB` (what Windows Explorer shows) |
| unit | the largest unit for which the value is at least 1 |
| precision | one decimal place, trailing `.0` dropped (`128 GiB`, `127.9 GiB`); `{:.0}`, `{:.2}` honoured |
| under 1 KiB | an integer: `0 B`, `1 B`, `512 B` |
| rounding | half away from zero |
| carry | a value that rounds up to 1024.0 of a unit prints as 1 of the next (`1 MiB`, never `1024 KiB`) |
| width, fill, alignment | honoured (`{:>10}`) |
| maximum | `u64::MAX` prints `16 EiB` and, with `{:#}`, `18.4 EB` |

```rust
use sys_lib::{Bytes, Units};

let ram = Bytes::new(134_876_545_024);
assert_eq!(ram.to_string(), "125.6 GiB");
assert_eq!(format!("{ram:#}"), "134.9 GB");
assert_eq!(ram.jedec().to_string(), "125.6 GB");
assert_eq!(format!("{ram:.2}"), "125.61 GiB");
assert_eq!(ram.display(Units::Decimal).to_string(), "134.9 GB");
assert_eq!(ram.raw(), 134_876_545_024);

assert_eq!(Bytes::new(512).to_string(), "512 B");
assert_eq!(Bytes::from_gib(128).to_string(), "128 GiB");
assert_eq!(Bytes::new(1_048_575).to_string(), "1 MiB");      // 1023.999 KiB carries
assert_eq!(Bytes::MAX.to_string(), "16 EiB");
assert_eq!(Bytes::from_gib(u64::MAX), Bytes::MAX);          // saturating
```

```rust
use sys_lib as sys;

let total = sys::mem::total()?;
let used = sys::mem::used()?;
println!("{used} of {total} ({})", used.percent_of(total));  // "41.2 GiB of 125.6 GiB (32.8%)"
# Ok::<(), sys_lib::Error>(())
```

### `Percent`

**Since** 0.3 · ROOT-04

```rust
#[derive(Clone, Copy, Default)]
pub struct Percent(/* f32 */);   // always 0.0..=100.0, never NaN

impl Percent {
    pub const ZERO: Percent;
    pub const FULL: Percent;
    pub fn from_ratio(part: u64, whole: u64) -> Percent;  // clamped; whole == 0 gives ZERO
    pub fn from_value(value: f32) -> Percent;             // clamped; NaN gives ZERO
    pub fn value(self) -> f32;                            // 0.0..=100.0
    pub fn fraction(self) -> f32;                         // 0.0..=1.0
    pub fn complement(self) -> Percent;                   // 100 - value
}
impl PartialEq, Eq, PartialOrd, Ord, Hash for Percent;  // total order: the value is never NaN
impl fmt::Display for Percent;   // "32.8%"; precision honoured
impl fmt::Debug for Percent;     // Percent(32.8)
```

A share that is always within 0 to 100 and never NaN, so it can be sorted, hashed, and compared without surprises. `from_ratio` computes `part * 1000 / whole` in `u128` and converts once, so a percentage of two integers is exact to 0.1%.

```rust
use sys_lib::Percent;

let p = Percent::from_ratio(41, 125);
assert_eq!(p.to_string(), "32.8%");
assert_eq!(format!("{p:.0}"), "33%");
assert_eq!(p.complement().to_string(), "67.2%");
assert_eq!(Percent::from_ratio(5, 0), Percent::ZERO);
assert_eq!(Percent::from_ratio(9, 4), Percent::FULL);    // clamped
assert_eq!(Percent::from_value(f32::NAN), Percent::ZERO);
assert!(Percent::FULL > p);
```

### `Mhz`

**Since** 0.4 · ROOT-05

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Mhz(pub u32);
impl fmt::Display for Mhz;   // "3.0 GHz" at or above 1000 MHz, else "800 MHz"
```

```rust
use sys_lib::Mhz;

assert_eq!(Mhz(5_700).to_string(), "5.7 GHz");
assert_eq!(Mhz(800).to_string(), "800 MHz");
```

### `summary()`

**Since** 0.5 · ROOT-06

```rust
pub fn summary() -> Summary;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Summary {
    pub os: &'static str, pub kernel: &'static str,
    pub cpu: &'static str, pub cpu_counts: cpu::Counts,
    pub memory: Option<mem::Memory>, pub cpu_quota: Option<cpu::CpuQuota>,
    pub virtualization: &'static virt::Virtualization,
    pub form_factor: os::FormFactor,
    pub uptime: Option<Duration>,
}
impl Summary {
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result;
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result;
}
impl fmt::Display for Summary;   // several lines, for the first log lines and support bundles
```

Every static fact plus one memory snapshot and the uptime, for the start-up log line, a health endpoint, or a support bundle. It never fails: a reading that fails becomes `None` in its field. It deliberately carries **no** host name, user name, MAC address, or device id, because it is meant for output that leaves the machine (threat T13). Cost: the first call reads every static fact; later calls 5 to 20 µs [est.] (memory and uptime are live).

```rust
use sys_lib as sys;

let summary = sys::summary();
println!("{summary}");
// os=Ubuntu 24.04.1 LTS kernel=6.8.0-45-generic cpu="AMD Ryzen 9 7950X" cores=16 threads=32
// available=2 memory.total=256 MiB memory.used=41 MiB limit=cgroup2 virt=docker form=unknown

let mut line = String::new();
summary.write_json(&mut line)?;
# Ok::<(), std::fmt::Error>(())
```

---

## `build`: build info

**Since** 0.3 (BLD-01 to BLD-05), 0.4 (BLD-06)

Facts fixed when the application was compiled, answering "exactly which build is running?" in logs, crash reports, health checks, and metrics. Zero run-time cost: every field is a `&'static str` in the binary.

```rust
pub mod build {
    pub fn emit();                          // call from the application's build.rs
    pub fn emitter() -> Emitter;            // Tier 2, since 0.4
    pub struct Emitter { /* private */ }
    impl Emitter {
        pub fn git(self, on: bool) -> Self;        // default true
        pub fn rustc(self, on: bool) -> Self;      // default true
        pub fn features(self, on: bool) -> Self;   // default true
        pub fn emit(self);
    }
}

#[macro_export]
macro_rules! build_info { () => { /* &'static BuildInfo, expanded in the calling crate */ } }

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BuildInfo {
    pub name: &'static str,          // the application's CARGO_PKG_NAME
    pub version: &'static str,       // the application's CARGO_PKG_VERSION
    pub commit: &'static str,        // full hash, or "unknown"
    pub commit_short: &'static str,  // 7 characters, or "unknown"
    pub dirty: Option<bool>,         // None when unknown
    pub built_at: &'static str,      // RFC 3339 UTC, or "unknown"
    pub profile: &'static str,       // "release", "debug"
    pub target: &'static str,        // "x86_64-unknown-linux-gnu"
    pub rustc: &'static str,         // "1.89.0"
    pub features: &'static str,      // comma-separated, sorted
}
impl BuildInfo {
    pub fn public(&self) -> PublicBuild<'_>;                            // name and version only
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result;   // build.version=0.3.0 ...
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result;
}
impl fmt::Display for BuildInfo;   // site 0.3.0 (9db9e36, release, built 2026-10-21T18:02:11Z)

#[derive(Clone, Copy, Debug)]
pub struct PublicBuild<'a> { /* private */ }
impl fmt::Display for PublicBuild<'_>;   // site 0.3.0
```

| Id | Item | Tier | Cost |
|---|---|---|---|
| BLD-01 | `build::emit()` | 1 | build time only: runs `git` and `$RUSTC`, 20 to 60 ms [est.] |
| BLD-02 | `build_info!()` | 1 | 0 at run time |
| BLD-03 | `BuildInfo` fields, `Display` | 1 | `Display` writes about 80 bytes |
| BLD-04 | `write_kv`, `write_json` | 2 | no allocation |
| BLD-05 | `public()` | 1 | 0 |
| BLD-06 | `emitter()` | 2 | build time only |

**How it works.** The application adds sys-lib as a build-dependency and calls `sys_lib::build::emit()` from its own `build.rs`. At build time the helper runs `git -C $CARGO_MANIFEST_DIR rev-parse HEAD`, `git status --porcelain --untracked-files=no`, and `$RUSTC --version`, then hands the values to the compiler with `cargo:rustc-env=SYS_LIB_BUILD_*`. It prints `cargo:rerun-if-changed` for the git `HEAD` file and the ref it points to (followed through a `.git` file in worktrees) and `cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH`, so a new commit refreshes the values without rebuilding every time. The macro expands in the application crate, so it reads the **application's** name and version; it reads the helper's values with `option_env!`, so an application that never called `emit()` still compiles and shows `unknown`.

**Never fails the build.** `git` missing, a crates.io tarball, a shallow clone, a detached HEAD: the value becomes `unknown`. `SOURCE_DATE_EPOCH`, when set, wins over the clock (reproducible builds); a value that is not a non-negative integer is ignored with a `cargo:warning`. Every value is checked (printable ASCII, at most 128 bytes) before it reaches `rustc-env`, because a branch name can contain anything (threat T11). The helper records no build host, user name, or source path.

**At run time sys-lib starts no program.** The helper is the only code in the crate that runs one, and it runs in the application's build script.

```rust,ignore
// The application's build.rs (sys-lib in [build-dependencies] as well as [dependencies])
fn main() {
    sys_lib::build::emit();
}
```

```rust
use sys_lib as sys;

let build: &'static sys::BuildInfo = sys::build_info!();
println!("{build}");                         // site 0.3.0 (9db9e36, release, built 2026-10-21T18:02:11Z)
println!("{}", build.public());              // site 0.3.0: safe for a public health endpoint
if build.dirty == Some(true) {
    println!("warning: built from uncommitted changes");
}
```

```rust,ignore
// Only the commit, no rustc version or feature list.
fn main() {
    sys_lib::build::emitter().rustc(false).features(false).emit();
}
```

---

## `instance`: instance info

**Since** 0.3 · feature `instance` (default)

Facts fixed when the process **started**, answering "which running copy is this?" when several run at once (several servers, or the old and new process during a hand-off restart). Put the instance id in every log line, audit record, and metric so events from two processes are never mixed up.

```rust
pub struct Instance { /* private */ }
impl Instance {
    pub fn start() -> &'static Instance;       // the first call creates it; later calls return the same
    pub fn id(&self) -> InstanceId;
    pub fn started_at(&self) -> SystemTime;
    pub fn uptime(&self) -> Duration;          // monotonic: wall-clock steps do not affect it
    pub fn host(&self) -> &str;                // host name at start, "unknown" if unreadable
    pub fn pid(&self) -> u32;
    pub fn user_at_start(&self) -> &str;       // effective user at start, "unknown" if unreadable
    pub fn is_current_process(&self) -> bool;  // false in a fork()ed child
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result;
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result;
}
impl fmt::Display for Instance;   // 01J9ZQ4M8T6Y2K5V3W1X7R0B9C web-01 pid 48213 up 3h12m04s

pub mod instance {
    pub fn current() -> Option<&'static Instance>;   // without creating it
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(/* [u8; 16] */);
impl InstanceId {
    pub fn as_bytes(&self) -> &[u8; 16];
    pub fn parse(text: &str) -> Result<InstanceId>;   // exactly 26 Crockford base32 characters
}
impl fmt::Display for InstanceId;   // 26-character ULID
impl fmt::Debug for InstanceId;     // InstanceId(01J9ZQ4M8T6Y2K5V3W1X7R0B9C)
impl FromStr for InstanceId;
```

| Id | Item | Tier | Cost [est.] |
|---|---|---|---|
| INS-01 | `Instance::start()` | 1 | first call: a ULID (about 50 ns), the host name, and the user (5 to 20 µs); later about 1 ns |
| INS-02 | `instance::current()` | 1 | about 1 ns |
| INS-03 | `id`, `started_at`, `uptime`, `host`, `pid`, `user_at_start` | 1 | about 1 ns; `uptime` one monotonic clock read (about 20 ns) |
| INS-04 | `InstanceId` | 1 | `Display` about 30 ns |
| INS-05 | `write_kv`, `write_json`, `Display` | 2 | no allocation |
| INS-06 | `is_current_process()` | 1 | about 20 ns |

`start()` never fails and is safe to call from many threads at once: exactly one instance is created. The user is recorded **at start**, before a privilege drop; the current effective user is always `process::user()`. A process created with `fork()` and no `exec` inherits the parent's instance; `is_current_process()` says so, and forked workers must not log under the parent's id. `InstanceId::parse` rejects anything but exactly 26 valid characters with `InvalidInput`.

The host name and user name are identifying values. They are in the instance by design (the instance names the running copy); keep them out of output that leaves your control.

```rust
use sys_lib::Instance;

let me = Instance::start();
println!("{me}");                                  // 01J9ZQ4M8T6Y2K5V3W1X7R0B9C web-01 pid 48213 up 0s
let id = me.id();
assert_eq!(id.to_string().len(), 26);
assert_eq!(id.to_string().parse::<sys_lib::InstanceId>()?, id);
assert!(std::ptr::eq(me, Instance::start()));     // one instance per process
# Ok::<(), sys_lib::Error>(())
```

```rust
// Tag every log line.
let instance = sys_lib::Instance::start().id();
eprintln!("instance={instance} event=boot phase=0");
```

---

## `process`: this process

### Readings

**Since** 0.3 (PRC-01, PRC-11, PRC-12), 0.4 (PRC-02 to PRC-04, PRC-06 to PRC-10, PRC-13), 0.5 (PRC-05)

Facts about **this** process. sys-lib does not list or inspect other processes.

```rust
pub mod process {
    pub fn id() -> u32;
    pub fn parent_id() -> Option<u32>;
    pub fn memory() -> Result<Bytes>;                  // resident set size
    pub fn peak_memory() -> Result<Bytes>;             // high-water resident set size
    pub fn memory_detail() -> Result<ProcessMemory>;
    pub fn cpu_time() -> Result<CpuTime>;              // user and system time used so far
    pub fn threads() -> Result<usize>;
    pub fn open_files() -> Result<usize>;              // descriptors (Unix) or handles (Windows)
    pub fn limits() -> Result<Limits>;
    pub fn started_at() -> Result<SystemTime>;
    pub fn user() -> Result<User>;                     // the effective user now
    pub fn is_elevated() -> Result<bool>;              // root, or an elevated Administrator token
    pub fn info() -> Result<ProcessInfo>;              // everything above, in the fewest reads
    pub fn info_with(options: InfoOptions) -> Result<ProcessInfo>;
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ProcessMemory {
    pub resident: Bytes, pub peak_resident: Bytes, pub virtual_size: Bytes,
    pub private: Option<Bytes>,      // anonymous / private bytes (commit charge on Windows)
    pub shared: Option<Bytes>,
    pub swapped: Option<Bytes>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CpuTime { pub user: Duration, pub system: Duration }
impl CpuTime { pub fn total(&self) -> Duration; }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    pub open_files: Limit, pub processes: Limit, pub address_space: Limit,
    pub core_dump: Limit, pub stack: Limit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limit { pub soft: Option<u64>, pub hard: Option<u64> }   // None = unlimited or not applicable

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct User { pub name: String, pub id: Option<u32>, pub is_admin: bool }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProcessInfo {
    pub id: u32, pub parent_id: Option<u32>,
    pub memory: ProcessMemory, pub cpu_time: CpuTime,
    pub threads: Option<usize>, pub open_files: Option<usize>,
    pub limits: Limits, pub started_at: Option<SystemTime>,
    pub user: Option<User>, pub is_elevated: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct InfoOptions { pub with_threads: bool }   // Windows: the thread count walks a system-wide snapshot
```

| Id | Item | Since | Linux | macOS | Windows | Cost [est.] |
|---|---|---|---|---|---|---|
| PRC-01 | `id()` | 0.3 | std | std | std | 20 ns |
| PRC-02 | `parent_id()` | 0.4 | `PPid` in `/proc/self/status` | `getppid` | toolhelp snapshot entry for this pid, cached after the first call | 2 µs / 0.1 µs / 1 ms first, then 1 ns |
| PRC-03 | `memory()` | 0.4 | `/proc/self/statm` field 2 × page size | `task_info(MACH_TASK_BASIC_INFO)` | `K32GetProcessMemoryInfo` working set | 2 µs / 0.5 µs / 0.5 µs |
| PRC-04 | `peak_memory()` | 0.4 | `VmHWM` | `getrusage` `ru_maxrss` (bytes on macOS, KiB on Linux) | `PeakWorkingSetSize` | 3 µs / 0.5 µs / 0.5 µs |
| PRC-05 | `memory_detail()` | 0.5 | `/proc/self/status` | `task_info` + `proc_pid_rusage` | `PROCESS_MEMORY_COUNTERS_EX` | 4 µs / 2 µs / 1 µs |
| PRC-06 | `cpu_time()` | 0.4 | `getrusage` | `getrusage` | `GetProcessTimes` | 0.3 µs |
| PRC-07 | `threads()` | 0.4 | `Threads` in `/proc/self/status` | `proc_pidinfo(PROC_PIDTASKINFO)` | toolhelp thread snapshot, filtered | 3 µs / 2 µs / 0.2 to 5 ms |
| PRC-08 | `open_files()` | 0.4 | entries of `/proc/self/fd` | `proc_pidinfo(PROC_PIDLISTFDS)` | `GetProcessHandleCount` | 1 µs per 100 / 2 µs / 0.3 µs |
| PRC-09 | `limits()` | 0.4 | `getrlimit` × 5 | `getrlimit` × 5 | job-object limits when present; otherwise `None` | 1 µs |
| PRC-10 | `started_at()` | 0.4 | `/proc/self/stat` field 22 + boot time | `proc_pidinfo(PROC_PIDTBSDINFO)` | `GetProcessTimes` creation time | 4 µs / 2 µs / 0.3 µs |
| PRC-11 | `user()` | 0.3 | `geteuid` + `getpwuid_r` | same | `GetUserNameW`; `is_admin` from the token | 5 to 50 µs |
| PRC-12 | `is_elevated()` | 0.3 | `geteuid() == 0` | same | `GetTokenInformation(TokenElevation)` | 0.1 µs / 0.1 µs / 2 µs |
| PRC-13 | `info()`, `info_with()` | 0.4 | `/proc/self/status` read once | | | the sum, minus shared reads |

`memory()`, `peak_memory()`, `cpu_time()`, `threads()` (Linux, macOS), `open_files()`, and `limits()` allocate nothing. `user()` allocates the name. On Linux and macOS, `user()` goes through the system's user database (NSS), which can read `/etc/passwd` or, with LDAP users, wait on the network; that is stated so callers do not put it on a hot path. On Windows, `threads()` walks a system-wide thread snapshot whose cost grows with the machine's total thread count; `info()` skips it unless `InfoOptions { with_threads: true }` is passed.

`is_elevated()` is a check, not a control: privilege drops belong to shield-sec, which calls `is_elevated()` afterwards to verify.

```rust
use sys_lib as sys;

println!("pid {} rss {} peak {}", sys::process::id(), sys::process::memory()?, sys::process::peak_memory()?);
let t = sys::process::cpu_time()?;
println!("cpu {:?} (user {:?}, system {:?})", t.total(), t.user, t.system);
let files = sys::process::limits()?.open_files;
println!("open files {} of {:?}", sys::process::open_files()?, files.soft);
# Ok::<(), sys_lib::Error>(())
```

```rust
use sys_lib as sys;

// After shield-sec dropped privileges:
if sys::process::is_elevated()? {
    return Err("still running elevated; refusing to open the door".into());
}
println!("running as {}", sys::process::user()?.name);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `CpuSampler`

**Since** 0.4 · PRC-14 · Tier 2

```rust
pub mod process {
    pub struct CpuSampler { /* previous CpuTime and Instant */ }
    impl CpuSampler {
        pub fn new() -> Result<CpuSampler>;
        pub fn sample(&mut self) -> Result<ProcessUsage>;   // never blocks; no allocation
    }
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ProcessUsage {
    pub of_one_core: f32,     // 0.0..=100.0 × logical CPUs; 250.0 = two and a half cores busy
    pub of_machine: Percent,  // share of every logical CPU
    pub interval: Duration,
}
```

The share of CPU this process used since the previous sample. Owned by the caller: two monitors keep two samplers and never disturb each other. Returns `TooSoon` when no time passed on the OS clock since the previous sample. Cost: about 0.4 µs per sample [est.].

```rust
use sys_lib as sys;
use std::time::Duration;

let mut cpu = sys::process::CpuSampler::new()?;
std::thread::sleep(Duration::from_millis(200));
let u = cpu.sample()?;
println!("this process: {:.1}% of one core, {} of the machine", u.of_one_core, u.of_machine);
# Ok::<(), sys_lib::Error>(())
```

### The process guard

**Since** 0.3 · PRC-20 to PRC-23 · feature `guard` (default)

What a service does before it touches its first file: make sure it is the only copy running, make every file it creates private by default, and make sure it will not run out of file descriptors under load. One call, secure defaults, refusal with an actionable error when a requirement cannot be met. Hold the returned `Guard` for the life of the process.

```rust
pub mod process {
    pub fn guard(name: &str) -> Result<Guard>;
}

#[must_use = "dropping the guard releases the single-instance lock"]
pub struct Guard { /* lock file, report */ }
impl Guard {
    pub fn builder(name: &str) -> GuardBuilder;
    pub fn report(&self) -> &GuardReport;
    pub fn lock_file(&self) -> Option<&Path>;
}
impl Drop for Guard;   // closes the lock file, which releases the lock; never deletes the file

pub struct GuardBuilder { /* private */ }
impl GuardBuilder {
    pub fn lock_dir(self, dir: impl Into<PathBuf>) -> Self;   // default: path-lib's runtime directory for `name`
    pub fn lock(self, on: bool) -> Self;                      // default true; false when a supervisor holds the lock
    pub fn allow_network_lock(self, on: bool) -> Self;        // default false
    pub fn umask(self, mask: u32) -> Self;                    // default 0o027; Unix only
    pub fn keep_umask(self) -> Self;                          // leave the umask alone
    pub fn open_files_target(self, n: u64) -> Self;           // default 65_536
    pub fn open_files_minimum(self, n: u64) -> Self;          // default 1_024
    pub fn build(self) -> Result<Guard>;
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct GuardReport {
    pub lock_file: Option<PathBuf>,
    pub lock_filesystem: Option<&'static str>,      // "ext4", "apfs", "NTFS", ...
    pub umask_before: Option<u32>, pub umask_after: Option<u32>,   // None on Windows
    pub open_files_before: Limit, pub open_files_after: Limit,
    pub warnings: Vec<GuardWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardWarning {
    /// Other threads were running when the umask changed; files they created
    /// meanwhile may have the old permissions. Call the guard before starting threads.
    ThreadsAlreadyRunning { threads: usize },
    /// The lock directory is on a filesystem whose locks may not hold across machines,
    /// accepted because `allow_network_lock(true)` was set.
    NetworkLockAllowed { filesystem: &'static str },
    /// `lock(false)`: no single-instance lock was taken.
    LockDisabled,
}
```

| Id | Item | Tier | Cost [est.] |
|---|---|---|---|
| PRC-20 | `process::guard(name)` | 1 | 10 to 100 µs, once |
| PRC-21 | `Guard::builder` and options | 2 | same |
| PRC-22 | `Guard::report`, `GuardReport` | 1 | 0 |
| PRC-23 | lock directory hardening | 1 | one `stat` and one `statfs` |

What the guard does, in order:

| Step | Linux and macOS | Windows |
|---|---|---|
| 1. lock directory | created owner-only (0700) through path-lib if missing; refused if it is a symlink, group- or world-writable, or on a network filesystem (`nfs`, `cifs`, `smb2`, `fuse.sshfs`, WSL's `drvfs` and `9p`) unless `allow_network_lock(true)` | created through path-lib; refused if it is a reparse point or on a remote drive unless allowed |
| 2. single-instance lock | `<dir>/<name>.lock` opened with `O_NOFOLLOW`, locked with `File::try_lock`; the OS releases the lock when the process dies, even after `kill -9` | the same file, `File::try_lock`, reparse points refused |
| 3. pid file | `<dir>/<name>.pid` written after the lock is held, with `path_lib::write_atomic` | same (the locked file cannot be read by another process, so the pid lives beside it) |
| 4. umask | `umask(0o027)`: owner read and write, group read, others nothing | not applicable; `umask_after` is `None` |
| 5. open-file limit | soft `RLIMIT_NOFILE` raised to the target, capped at the hard limit (and on macOS at `kern.maxfilesperproc`); `OpenFilesTooLow` if the hard limit is below the minimum, with nothing changed | not applicable (no per-process descriptor count for sockets); reported as unlimited |

The lock file is **never deleted**, not even on a clean exit: deleting it would let a waiting process lock the unlinked file while a third creates a new one, and two copies would run. The name must be 1 to 64 characters of ASCII letters, digits, `-`, `_`, `.`, not starting with `.`. The umask is process-wide, so call the guard before starting threads; if other threads already run, the report says so. Privileges are not dropped here: that is shield-sec's job, after the listening sockets are bound.

Errors, each saying what to do:

| Error | When | The message tells the operator |
|---|---|---|
| `AlreadyRunning { pid, lock_file }` | another copy holds the lock | the pid when the pid file is readable, and the lock file |
| `RunDirUnavailable { path, source }` | the directory cannot be created or written, is a symlink, is writable by others, or is on a network filesystem | the path, the reason, and the fix (`chmod 700`, a local directory, `allow_network_lock`) |
| `OpenFilesTooLow { have, need }` | the hard limit is below the minimum | what to raise: `LimitNOFILE=` in the systemd unit, `ulimit -Hn`, or launchd's limits |
| `InvalidInput` | the name failed validation | the rule |

```rust,no_run
use sys_lib as sys;

let guard = sys::process::guard("site")?;      // before any file is opened
println!("lock {:?}, open files now {:?}", guard.lock_file(), guard.report().open_files_after.soft);
// ... run; the lock is held while `guard` lives.
# Ok::<(), sys_lib::Error>(())
```

```rust,no_run
use sys_lib::process::Guard;

// A database locks beside its data, so two copies under different users or run
// directories still exclude each other.
let guard = Guard::builder("hive")
    .lock_dir("/var/lib/hive")
    .open_files_target(1_048_576)
    .open_files_minimum(65_536)
    .build()?;
# Ok::<(), sys_lib::Error>(())
```

```rust,no_run
use sys_lib::{process::Guard, Error};

match Guard::builder("site").build() {
    Ok(guard) => { /* run */ drop(guard); }
    Err(Error::AlreadyRunning { pid: Some(pid), .. }) => eprintln!("already running as pid {pid}"),
    Err(e) => eprintln!("cannot start: {e}"),
}
```

---

## `os`: operating system

### Kind and identity

**Since** 0.3 (OS-01 to OS-07), 0.5 (OS-08 to OS-10)

```rust
pub mod os {
    pub const fn kind() -> Kind;
    pub const fn is_windows() -> bool;
    pub const fn is_linux() -> bool;          // the Linux kernel, not Android
    pub const fn is_macos() -> bool;
    pub const fn is_mac() -> bool;            // the same as is_macos
    pub const fn is_android() -> bool;
    pub const fn is_ios() -> bool;
    pub const fn is_unix() -> bool;
    pub const fn is_bsd() -> bool;

    pub fn name() -> &'static str;                  // "Ubuntu 24.04.1 LTS", "macOS 15.1 Sequoia", "Windows 11 Pro"
    pub fn version() -> Option<&'static Version>;
    pub fn kernel() -> &'static str;                // "6.8.0-45-generic", "24.1.0", "10.0.26200.6899"
    pub fn distro() -> Option<&'static Distro>;     // Linux only
    pub fn edition() -> Option<&'static str>;       // "Professional", "ServerDatacenter"; Linux VARIANT_ID
    pub fn codename() -> Option<&'static str>;      // "Sequoia", "noble"
    pub fn is_server_edition() -> Option<bool>;     // Windows Server; None where it has no meaning
    pub fn is_64bit() -> bool;                      // the OS, not the process (WOW64, ARM64 emulation)
    pub fn info() -> &'static OsInfo;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Kind { Linux, Windows, MacOs, Android, Ios, FreeBsd, OpenBsd, NetBsd, Other }
impl Kind { pub const fn name(self) -> &'static str; }   // "linux", "windows", "macos", ...

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub struct Version {
    pub major: u32, pub minor: u32, pub patch: u32, pub build: Option<u32>,
    pub text: &'static str,      // the original text: "24.04" stays "24.04"
}
impl fmt::Display for Version;   // prints `text`

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Distro {
    pub id: &'static str, pub id_like: &'static [&'static str], pub name: &'static str,
    pub pretty_name: &'static str, pub version: Option<&'static str>,
    pub version_id: Option<&'static str>, pub codename: Option<&'static str>,
    pub variant_id: Option<&'static str>, pub build_id: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OsInfo {
    pub kind: Kind, pub name: &'static str, pub version: Option<&'static Version>,
    pub kernel: &'static str, pub distro: Option<&'static Distro>,
    pub edition: Option<&'static str>, pub codename: Option<&'static str>,
    pub is_64bit: bool,
    pub name_source: Source,     // where `name` came from
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Source { OsRelease, UsrLibOsRelease, Sysctl, Registry, SystemProperty, Fallback }
```

| Id | Item | Since | Cost | Android / iOS |
|---|---|---|---|---|
| OS-01 | `kind()` | 0.3 | 0 (const) | yes / yes |
| OS-02 | `is_*` constants | 0.3 | 0 (const) | yes / yes |
| OS-03 | `name()` | 0.3 | about 1 ns; first call 10 to 50 µs [est.] | yes / yes |
| OS-04 | `version()` | 0.3 | about 1 ns | yes / yes |
| OS-05 | `kernel()` | 0.3 | about 1 ns | yes / yes |
| OS-06 | `distro()` | 0.3 | about 1 ns | `None` / `None` |
| OS-07 | `info()` | 0.3 | about 1 ns | yes / yes |
| OS-08 | `edition()`, `is_server_edition()` | 0.5 | about 1 ns | `None` |
| OS-09 | `codename()` | 0.5 | about 1 ns | `None` |
| OS-10 | `is_64bit()` | 0.5 | about 1 ns | yes |

`name()` never fails: a minimal container without os-release (`scratch`, distroless) gives `"Linux"`, and `info().name_source` is `Fallback`. On Windows the product comes from the build number, because the registry's `ProductName` still says "Windows 10" on Windows 11; the version comes from `RtlGetVersion`, which reports the real version whatever the executable's manifest declares. On macOS the version comes from `kern.osproductversion`, unaffected by `SYSTEM_VERSION_COMPAT`; a future major without a known codename prints without one. `kind()` maps Android to `Android` (path-lib's `Os` maps it to `OtherUnix`).

```rust
use sys_lib as sys;

println!("{} (kernel {})", sys::os::name(), sys::os::kernel());
if let Some(v) = sys::os::version() {
    if v.major >= 24 { println!("recent enough: {v}"); }
}
if sys::os::is_linux() {
    if let Some(d) = sys::os::distro() {
        let debian_family = d.id == "debian" || d.id_like.contains(&"debian");
        println!("{} ({}), debian family: {debian_family}", d.pretty_name, d.id);
    }
}
const ON_WINDOWS: bool = sys::os::is_windows();   // usable in const context
```

### Form factor

**Since** 0.5 · OS-11 to OS-13

```rust
pub mod os {
    pub fn form_factor() -> FormFactor;
    pub fn detect_form_factor() -> &'static Detection<FormFactor>;
    pub fn is_desktop() -> bool;       // Desktop
    pub fn is_laptop() -> bool;        // Laptop or Convertible
    pub fn is_mobile() -> bool;        // Phone, Tablet, Handheld, Wearable
    pub fn is_phone() -> bool;
    pub fn is_tablet() -> bool;
    pub fn is_server() -> bool;        // the chassis or the platform role says server
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum FormFactor {
    Desktop, Laptop, Convertible, Tablet, Phone, Handheld,
    Server, Embedded, Wearable, Tv, Unknown,
}
impl FormFactor { pub const fn name(self) -> &'static str; }   // "laptop"

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Detection<T> { pub value: T, pub source: &'static str, pub confidence: Confidence }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum Confidence { None, Low, Medium, High }
```

What kind of machine this is, from the firmware chassis type, the ACPI power-management profile, the platform role, the model identifier, or a battery, first answer wins (the order per OS is in [`SOURCES.md`](./SOURCES.md)). On Linux, `CHASSIS=` in `/etc/machine-info` overrides the firmware, as systemd honours it. Inside a VM the chassis describes virtual hardware, so the answer is `Unknown` unless an administrator said otherwise: an honest unknown, not a guess. Cost: about 1 ns after the first call; the first call 5 to 500 µs [est.] (an SMBIOS read on Windows).

Detection is advisory. Do not make security decisions on it.

```rust
use sys_lib as sys;

if sys::os::is_laptop() && sys::power::on_ac()? == Some(false) {
    // on battery: back off background work
}
let d = sys::os::detect_form_factor();
println!("{} (from {}, confidence {:?})", d.value.name(), d.source, d.confidence);
# Ok::<(), sys_lib::Error>(())
```

---

## `cpu`: processor

### Identity and counts

**Since** 0.3 (CPU-01 to CPU-06), 0.4 (CPU-07 to CPU-12), 0.5 (CPU-13)

```rust
pub mod cpu {
    pub const fn arch() -> Arch;                    // the compile target
    pub const fn is_64() -> bool;
    pub const fn is_32() -> bool;
    pub fn native_arch() -> Arch;                   // the hardware (under Rosetta 2, WOW64, ARM64 emulation)
    pub fn is_translated() -> bool;                 // this process runs under emulation

    pub fn name() -> &'static str;                  // cleaned: "AMD Ryzen 9 7950X", "Apple M2 Pro"
    pub fn raw_name() -> Option<&'static str>;      // as reported, for support output
    pub fn vendor() -> Vendor;

    pub fn logical() -> usize;                      // configured hardware threads (static)
    pub fn physical() -> usize;                     // cores (static); logical() if unknown
    pub fn packages() -> usize;                     // sockets (static)
    pub fn available() -> usize;                    // what this process may use now
    pub fn online() -> usize;                       // online now (hot-plug)
    pub fn count() -> Counts;
    pub fn quota() -> Result<Option<CpuQuota>>;     // cgroup cpu.max / job object rate

    pub fn frequency() -> &'static Frequency;        // base and maximum (static)
    pub fn current_frequency() -> Result<Option<Mhz>>;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Arch { X86_64, X86, Aarch64, Arm, Riscv64, Powerpc64, S390x, Loongarch64, Wasm32, Other }
impl Arch { pub const fn name(self) -> &'static str; }   // "x86_64", "aarch64"

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Vendor { Intel, Amd, Apple, Arm, Qualcomm, Ampere, Nvidia, Samsung, Mediatek, Hygon, Zhaoxin, Unknown }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Counts {
    pub logical: usize, pub physical: usize, pub packages: usize,
    pub available: usize, pub online: usize,
    pub physical_exact: bool,     // false when the topology was hidden and physical fell back to logical
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct CpuQuota { pub cpus: f32, pub source: LimitSource }   // 1.5 = one and a half CPUs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Frequency { pub base: Option<Mhz>, pub max: Option<Mhz> }
```

| Id | Item | Since | Cost |
|---|---|---|---|
| CPU-01 | `arch()`, `is_64()`, `is_32()` | 0.3 | 0 (const) |
| CPU-02 | `logical()` | 0.3 | about 1 ns |
| CPU-03 | `physical()` | 0.3 | about 1 ns; first call about 200 µs on 32 CPUs (Linux) [est.] |
| CPU-04 | `available()` | 0.3 | Linux 3 to 10 µs, others 0.2 µs [est.] |
| CPU-05 | `name()`, `raw_name()` | 0.3 | about 1 ns |
| CPU-06 | `vendor()` | 0.3 | about 1 ns |
| CPU-07 | `packages()` | 0.4 | about 1 ns |
| CPU-08 | `count()` | 0.4 | `available` and `online` are live |
| CPU-09 | `online()` | 0.4 | 2 µs / 0.2 µs [est.] |
| CPU-10 | `quota()` | 0.4 | 5 to 15 µs [est.] |
| CPU-11 | `frequency()` | 0.4 | about 1 ns |
| CPU-12 | `current_frequency()` | 0.4 | 2 µs per CPU (Linux) / 20 µs (Windows) [est.] |
| CPU-13 | `native_arch()`, `is_translated()` | 0.5 | about 1 ns |

**Three counts, three meanings.** `physical()` is cores; `logical()` is hardware threads, the right size for per-CPU arrays (it counts **present** CPUs, not the `possible` count VMs inflate); `available()` is what **this process** may use right now: the affinity mask, intersected with the cpuset, then limited by the cgroup CPU quota (`ceil(quota / period)`, at least 1), or on Windows the group affinity across every processor group and the job object's CPU rate. Size worker pools by `available()`. On Windows with more than 64 logical processors, every count spans all processor groups. `physical <= logical`, `available <= logical`, `available >= 1`, and `online >= 1` always hold.

**The name is cleaned.** "(R)", "(TM)", trailing frequencies (" CPU @ 3.00GHz"), "CPU" as a word, core-count suffixes ("16-Core Processor"), and repeated spaces are removed; `raw_name()` keeps the original. ARM big.LITTLE systems list their core types largest first ("Cortex-X1 + Cortex-A78 + Cortex-A55"). The name is never empty: it falls back to the vendor and architecture ("ARM aarch64"), then "unknown".

There is no "total frequency": a sum of per-core frequencies is not a meaningful quantity. `frequency()` gives base and maximum; Apple silicon reports neither (`None`).

```rust
use sys_lib as sys;

println!("{} ({:?}, {})", sys::cpu::name(), sys::cpu::vendor(), sys::cpu::arch().name());
println!("{} cores, {} threads, {} usable here", sys::cpu::physical(), sys::cpu::logical(), sys::cpu::available());

// Size a worker pool by what the container allows, not by the host.
let workers = sys::cpu::available();
if let Some(q) = sys::cpu::quota()? {
    println!("CPU quota {:.1} from {:?}", q.cpus, q.source);
}
let f = sys::cpu::frequency();
if let Some(max) = f.max { println!("up to {max}"); }
# Ok::<(), sys_lib::Error>(())
```

### Usage

**Since** 0.4 · CPU-20 to CPU-25

```rust
pub mod cpu {
    pub const DEFAULT_WINDOW: Duration;                     // 100 ms
    pub fn usage() -> Result<Usage>;                        // BLOCKS for DEFAULT_WINDOW
    pub fn usage_over(window: Duration) -> Result<Usage>;   // BLOCKS for `window`, clamped to 10 ms ..= 10 s
    pub fn used() -> Result<Percent>;                       // BLOCKS: usage()?.used()
    pub fn free() -> Result<Percent>;                       // BLOCKS: usage()?.free()
    pub fn times() -> Result<Times>;                        // Tier 3: raw cumulative counters

    pub struct Sampler { /* previous Times and Instant */ }
    impl Sampler {
        pub fn new() -> Result<Sampler>;                                              // the first reading
        pub fn sample(&mut self) -> Result<Usage>;                                    // never blocks; no allocation
        pub fn sample_per_core(&mut self) -> Result<(Usage, Vec<Percent>)>;           // allocates the Vec
        pub fn sample_per_core_into(&mut self, out: &mut [Percent]) -> Result<Usage>; // Tier 3; no allocation
    }
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Usage {
    pub user: Percent, pub system: Percent, pub idle: Percent,
    pub iowait: Option<Percent>,   // Linux
    pub steal: Option<Percent>,    // Linux VMs: time the hypervisor gave to others
    pub irq: Option<Percent>,
    pub interval: Duration,
}
impl Usage { pub fn used(&self) -> Percent; pub fn free(&self) -> Percent; }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Times {
    pub user: Duration, pub nice: Duration, pub system: Duration, pub idle: Duration,
    pub iowait: Duration, pub irq: Duration, pub softirq: Duration, pub steal: Duration,
}
```

| Id | Item | Tier | Cost [est.] |
|---|---|---|---|
| CPU-20 | `usage()`, `used()`, `free()` | 1 | 100 ms of wall time; two readings |
| CPU-21 | `usage_over(window)` | 1 | `window` of wall time |
| CPU-22 | `Sampler::new`, `sample` | 2 | Linux 2 to 4 µs (the first line of `/proc/stat`), macOS 1 to 2 µs, Windows 0.3 µs |
| CPU-23 | `sample_per_core`, `sample_per_core_into` | 2 / 3 | Linux 10 to 60 µs (scales with CPUs), macOS 5 µs, Windows 5 to 20 µs |
| CPU-24 | `Usage` fields | 2 | 0 |
| CPU-25 | `times()` | 3 | one reading |

CPU usage is the share of non-idle time **between two readings** of the OS's cumulative counters; one reading only gives the average since boot. `usage()` takes both readings itself and blocks for 100 ms, which its name and documentation say. A service that reports usage periodically keeps a `Sampler` instead: it never sleeps, each `sample()` measures the time since the previous one, and two samplers never disturb each other (there is no hidden global baseline).

- Steal counts as busy (the CPU was not available to us) and iowait as idle (the CPU was free to run other work); both are also reported separately.
- `TooSoon` when no tick passed on the OS clock since the previous sample (about 10 ms on Linux): never a meaningless 0% or 100%.
- Counters that go backwards (a CPU taken offline and back, a VM migration, a wrap) re-base the sampler: `TooSoon` for that core, never a negative or above-100 value.
- `sample_per_core_into` returns `TooLarge` without writing anything when `out` is shorter than `cpu::logical()`.
- Windows with more than 64 logical processors: the total is correct (system-wide counters); per-core values return `Unsupported` until the per-group call is verified.

```rust
use sys_lib as sys;

println!("CPU {}", sys::cpu::used()?);   // blocks 100 ms: "12.4%"
# Ok::<(), sys_lib::Error>(())
```

```rust,no_run
use sys_lib as sys;
use std::time::Duration;

// A monitor loop: non-blocking, caller-owned.
let mut cpu = sys::cpu::Sampler::new()?;
let mut per_core = vec![sys::Percent::ZERO; sys::cpu::logical()];
loop {
    std::thread::sleep(Duration::from_secs(5));
    let u = cpu.sample_per_core_into(&mut per_core)?;
    println!("used {} (user {}, system {}, steal {:?})", u.used(), u.user, u.system, u.steal);
}
# Ok::<(), sys_lib::Error>(())
```

### Features

**Since** 0.4 (CPU-30, CPU-31), 0.5 (CPU-32)

```rust
pub mod cpu {
    pub fn has(feature: Feature) -> bool;          // about 1 ns
    pub fn features() -> &'static Features;
    pub fn x86_level() -> Option<X86Level>;        // x86-64-v1 ..= v4; None off x86
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Feature {
    // x86
    Sse2, Sse3, Ssse3, Sse41, Sse42, Popcnt, Avx, Avx2, Fma, F16c, Bmi1, Bmi2, Lzcnt, Movbe,
    Avx512f, Avx512bw, Avx512cd, Avx512dq, Avx512vl, Avx512vnni, Avx512vbmi, AvxVnni,
    Aes, Pclmulqdq, Vaes, Vpclmulqdq, Sha, Rdrand, Rdseed, Adx, Gfni,
    // AArch64
    Neon, Crc32, AesArm, Pmull, Sha2, Sha3, Dotprod, Fp16, Lse, Sve, Sve2, Rcpc, Bf16, I8mm,
}
impl Feature { pub const fn name(self) -> &'static str; }   // rustc's target-feature name: "avx2", "neon"

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Features { /* bitset */ }
impl Features {
    pub fn contains(&self, feature: Feature) -> bool;
    pub fn iter(&self) -> impl Iterator<Item = Feature> + '_;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
impl fmt::Display for Features;   // "sse2 sse3 ... avx2 aes": for a log line

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum X86Level { V1, V2, V3, V4 }
```

Detection goes through std (`is_x86_feature_detected!`, `is_aarch64_feature_detected!`), which checks operating-system support as well as the CPU: a CPU with AVX-512 on an OS that disabled its state reports `false`. That is the same detection the compiler's `#[target_feature]` dispatch trusts, so the two always agree. A feature std cannot detect at the MSRV is left out rather than detected differently; `Sha3` on AArch64 is std's `sha3`, which covers both SHA-3 and SHA-512. Under Rosetta 2, the x86 features are what Rosetta emulates (no AVX).

```rust
use sys_lib::{self as sys, cpu::Feature};

if sys::cpu::has(Feature::Avx2) {
    // pick the AVX2 kernel
}
println!("features: {}", sys::cpu::features());
if let Some(level) = sys::cpu::x86_level() { println!("x86-64-{level:?}"); }
```

### Topology, caches, NUMA

**Since** 0.5 · CPU-40 to CPU-44

```rust
pub mod cpu {
    pub fn topology() -> &'static Topology;
    pub fn caches() -> &'static [Cache];
    pub fn cache_line() -> usize;                 // bytes; 64 when unknown on x86, 128 on Apple silicon
    pub fn numa_nodes() -> &'static [NumaNode];
    pub fn is_hybrid() -> bool;                   // more than one core kind
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Topology { pub packages: Vec<Package>, pub cores: Vec<Core> }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Package { pub id: u32, pub cores: Vec<u32> }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Core {
    pub id: u32, pub package: u32,
    pub logical: Vec<u32>,              // logical CPU indexes
    pub kind: CoreKind,
    pub capacity: Option<u16>,          // Linux cpu_capacity, 0..=1024
    pub numa_node: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CoreKind { Performance, Efficiency, Uniform, Unknown }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Cache {
    pub level: u8, pub kind: CacheKind, pub size: Bytes, pub line: u32,
    pub shared_by: Vec<u32>,            // logical CPUs sharing this cache instance
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CacheKind { Data, Instruction, Unified }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NumaNode { pub id: u32, pub cpus: Vec<u32>, pub memory: Option<Bytes> }
```

Static facts read on first use: about 1 ns afterwards; the first call reads up to a few hundred small files on large Linux hosts (1 to 5 ms on a 256-CPU machine [est.]), which is why they are separate from `cpu::name()`. CPU hot-plug after the first call is not reflected; `cpu::online()` is the live count. Lists are sorted by id. macOS reports one NUMA node with every CPU.

```rust
use sys_lib as sys;

let t = sys::cpu::topology();
let fast = t.cores.iter().filter(|c| c.kind == sys::cpu::CoreKind::Performance).count();
println!("{} cores, {fast} performance cores, hybrid: {}", t.cores.len(), sys::cpu::is_hybrid());
for c in sys::cpu::caches() {
    println!("L{} {:?} {} shared by {}", c.level, c.kind, c.size, c.shared_by.len());
}
println!("cache line {} bytes, {} NUMA nodes", sys::cpu::cache_line(), sys::cpu::numa_nodes().len());
```

---

## `mem`: memory

**Since** 0.4 (MEM-01 to MEM-08), 0.5 (MEM-09)

```rust
pub mod mem {
    pub fn total() -> Result<Bytes>;              // effective: min(host, limit)
    pub fn used() -> Result<Bytes>;
    pub fn free() -> Result<Bytes>;               // available to new allocations
    pub fn used_percent() -> Result<Percent>;
    pub fn free_percent() -> Result<Percent>;
    pub fn snapshot() -> Result<Memory>;          // every value from one set of reads
    pub fn host_total() -> Result<Bytes>;         // ignores limits
    pub fn host_free() -> Result<Bytes>;
    pub fn limit() -> Result<Option<MemoryLimit>>;
    pub fn swap() -> Result<Swap>;
    pub fn page_size() -> usize;                  // static
    pub fn pressure() -> Result<Option<Pressure>>;
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Memory {
    pub total: Bytes, pub used: Bytes, pub free: Bytes,
    pub host_total: Bytes, pub host_free: Bytes,
    pub unused: Option<Bytes>,       // pages doing nothing (the kernel's MemFree)
    pub cached: Option<Bytes>,
    pub limit: Option<MemoryLimit>, pub swap: Swap,
    pub source: MemorySource,
}
impl Memory { pub fn used_percent(&self) -> Percent; pub fn free_percent(&self) -> Percent; }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum MemorySource { Host, Cgroup1, Cgroup2, JobObject, Lxcfs }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct MemoryLimit { pub hard: Bytes, pub soft: Option<Bytes>, pub swap: Option<Bytes>, pub source: LimitSource }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum LimitSource { Cgroup1, Cgroup2, JobObject }

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Swap { pub total: Bytes, pub used: Bytes, pub free: Bytes }

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Pressure { pub level: PressureLevel, pub some_avg10: Option<Percent>, pub full_avg10: Option<Percent> }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum PressureLevel { Normal, Warning, Critical }
```

| Id | Item | Since | Cost per call [est.] |
|---|---|---|---|
| MEM-01 | `total()` | 0.4 | Linux 2 to 4 µs bare, 6 to 15 µs in a cgroup; macOS 0.5 µs; Windows 0.5 to 1 µs |
| MEM-02 | `used()`, `free()` | 0.4 | same |
| MEM-03 | `used_percent()`, `free_percent()` | 0.4 | same |
| MEM-04 | `snapshot()` | 0.4 | the same reads, every value |
| MEM-05 | `host_total()`, `host_free()` | 0.4 | Linux 2 to 4 µs |
| MEM-06 | `limit()` | 0.4 | Linux 4 to 10 µs; Windows 1 µs; macOS 0 (`None`) |
| MEM-07 | `swap()` | 0.4 | Linux 2 to 4 µs; macOS 0.5 µs; Windows 2 µs |
| MEM-08 | `page_size()` | 0.4 | about 1 ns |
| MEM-09 | `pressure()` | 0.5 | Linux 2 µs (`None` on kernels without PSI); macOS 0.5 µs; Windows 1 µs |

**Container-aware by default.** Inside a container or a systemd unit with a memory limit, `total()` is the limit: what this process can use and what the OOM killer enforces. sysinfo and psutil report the host instead; sys-lib gives the host through `host_total()`. The limit is the smallest `memory.max` over the cgroup and its ancestors (cgroup v2), `hierarchical_memory_limit` (cgroup v1), or the job object's process or job memory limit (Windows). "Used" inside a limit is the cgroup's working set: current usage minus inactive file pages, the definition the kubelet uses for eviction. With LXCFS the limit is not applied twice. The limit can change while a process runs (Kubernetes in-place resize, VM ballooning), so `total()` is a live reading.

**"free" means available**: what new allocations can get without swapping, counting reclaimable cache (`MemAvailable` on Linux). The kernel's idle pages (`MemFree`) are `Memory::unused`. On every platform `used + free == total`, `used <= total`, and percentages are within 0 to 100.

Each Tier 1 function performs only the reads it needs. A caller who wants several values calls `snapshot()` and pays for one set of reads. Every scalar reading allocates nothing.

```rust
use sys_lib as sys;

println!("{} of {} used ({})", sys::mem::used()?, sys::mem::total()?, sys::mem::used_percent()?);

let m = sys::mem::snapshot()?;
println!("{} free of {} (host {} of {}), source {:?}", m.free, m.total, m.host_free, m.host_total, m.source);
if let Some(limit) = m.limit {
    println!("limit {} from {:?}, throttles at {:?}", limit.hard, limit.source, limit.soft);
}
# Ok::<(), sys_lib::Error>(())
```

```rust
use sys_lib as sys;

// Size a cache to a quarter of what this process may use.
let cache_bytes = sys::mem::total()?.raw() / 4;
// Shed load before the OOM killer does.
if sys::mem::free_percent()?.value() < 5.0 { /* refuse new work */ }
if let Some(p) = sys::mem::pressure()? {
    if p.level >= sys::mem::PressureLevel::Warning { /* trim caches */ }
}
# Ok::<(), sys_lib::Error>(())
```

---

## `disk`: disks and filesystems

**Since** 0.4 (DSK-01 to DSK-03), 0.5 (DSK-04, DSK-05)

```rust
pub mod disk {
    pub fn space(path: impl AsRef<Path>) -> Result<Space>;       // the filesystem holding `path`
    pub fn list() -> Result<Vec<Disk>>;                           // local, real filesystems
    pub fn list_with(options: ListOptions) -> Result<Vec<Disk>>;
    pub fn of(path: impl AsRef<Path>) -> Result<Disk>;            // the mount holding `path`
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Space {
    pub total: Bytes,
    pub free: Bytes,        // every free block
    pub available: Bytes,   // what an unprivileged user can use (excludes the root reserve)
}
impl Space { pub fn used(&self) -> Bytes; pub fn used_percent(&self) -> Percent; }

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Disk {
    pub mount_point: PathBuf,
    pub device: String,          // "/dev/nvme0n1p2", "C:", "\\?\Volume{...}\"
    pub label: Option<String>,   // volume label (Windows, macOS)
    pub filesystem: String,      // "ext4", "btrfs", "apfs", "NTFS", "overlay"
    pub kind: DiskKind,
    pub space: Option<Space>,    // None for network mounts unless included, or media not present
    pub read_only: bool,
    pub removable: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum DiskKind { Ssd, Hdd, Removable, Network, Optical, Ram, Virtual, Unknown }

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ListOptions {
    pub include_network: bool,      // default false: statvfs on a dead NFS mount can hang
    pub include_pseudo: bool,       // proc, sysfs, cgroup, tmpfs, devtmpfs, squashfs; default false
    pub include_duplicates: bool,   // bind mounts and subvolumes of one filesystem; default false
    pub max_mounts: usize,          // default 4_096
}
impl Default for ListOptions;
```

| Id | Item | Since | Cost [est.] |
|---|---|---|---|
| DSK-01 | `space(path)` | 0.4 | 1 to 3 µs; no allocation |
| DSK-02 | `list()` | 0.4 | Linux 20 µs + 3 µs per mount; Windows 50 to 500 µs |
| DSK-03 | `list_with(options)` | 0.4 | as `list()` |
| DSK-04 | `DiskKind` SSD/HDD | 0.5 | 2 to 10 µs per disk |
| DSK-05 | `of(path)` | 0.5 | 5 to 30 µs |

`list()` never blocks on a dead network mount: network filesystems are skipped unless `include_network` is set, and Windows volume calls run with critical-error dialogs disabled, so an empty card reader never pops up "insert a disk" (threat T8). Mount points appear once per filesystem (deduplicated by device number); escaped characters in Linux mount points (`\040` and friends) are decoded. Inside a container, `list()` shows the container's mounts (the overlay root, volumes): that is correct for the process, and the host's disks are neither visible nor guessed. `space(path)` blocks if the caller points it at a dead network mount; that is the caller's choice and documented. More mounts than `max_mounts` is `TooLarge`, checked before allocation. The list is sorted by mount point.

```rust
use sys_lib as sys;

let s = sys::disk::space("/var/lib/hive")?;
println!("{} available of {} ({} used)", s.available, s.total, s.used_percent());
if s.available < sys::Bytes::from_gib(10) { /* refuse compaction */ }

for d in sys::disk::list()? {
    println!("{:<20} {:<8} {:?} {:?}", d.mount_point.display(), d.filesystem, d.kind, d.space.map(|s| s.available));
}
# Ok::<(), sys_lib::Error>(())
```

```rust
use sys_lib::disk::{self, ListOptions};

let mut options = ListOptions::default();
options.include_network = true;      // accept the risk of a hang on a dead mount
let all = disk::list_with(options)?;
# Ok::<(), sys_lib::Error>(())
```

---

## `net`: network interfaces

**Since** 0.4 (NET-01 to NET-03), 0.5 (NET-04 to NET-06)

```rust
pub mod net {
    pub fn interfaces() -> Result<Vec<Interface>>;
    pub fn addresses() -> Result<Vec<IpAddr>>;        // non-loopback, non-link-local addresses of up interfaces
    pub fn primary() -> Result<Option<Interface>>;    // the one holding the default route
    pub fn counters() -> Result<Vec<Counters>>;

    pub struct Sampler { /* previous counters */ }
    impl Sampler {
        pub fn new() -> Result<Sampler>;
        pub fn sample(&mut self) -> Result<Vec<Rates>>;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Interface {
    pub name: String,                  // "eth0", "en0", "Ethernet 2"
    pub index: u32,
    pub description: Option<String>,   // Windows friendly description
    pub kind: InterfaceKind,
    pub mac: Option<Mac>,
    pub addresses: Vec<IpNet>,
    pub up: bool,                      // administratively up
    pub running: bool,                 // link up / operational
    pub mtu: Option<u32>,
    pub speed_mbps: Option<u64>,       // None when the driver does not report it
    pub is_physical: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum InterfaceKind { Ethernet, Wifi, Loopback, Tunnel, Bridge, Virtual, Cellular, Other }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Mac { pub bytes: [u8; 6], pub kind: MacKind }
impl fmt::Display for Mac;   // "aa:bb:cc:dd:ee:ff"

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum MacKind { Permanent, Random, Unknown }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IpNet { pub addr: IpAddr, pub prefix: u8 }
impl fmt::Display for IpNet;   // "192.168.1.20/24"

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Counters {
    pub interface: String,
    pub rx_bytes: u64, pub tx_bytes: u64, pub rx_packets: u64, pub tx_packets: u64,
    pub rx_errors: u64, pub tx_errors: u64, pub rx_dropped: u64, pub tx_dropped: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Rates {
    pub interface: String,
    pub rx_bytes_per_sec: f64, pub tx_bytes_per_sec: f64,
    pub rx_packets_per_sec: f64, pub tx_packets_per_sec: f64,
    pub interval: Duration,
}
```

| Id | Item | Since | Cost [est.] |
|---|---|---|---|
| NET-01 | `interfaces()` | 0.4 | Linux 30 to 150 µs, macOS 20 to 80 µs, Windows 1 to 5 ms |
| NET-02 | `Mac`, `MacKind` | 0.4 | 0 |
| NET-03 | `addresses()` | 0.4 | Linux 10 to 30 µs |
| NET-04 | `counters()` | 0.5 | 5 to 20 µs / 10 µs / 200 µs |
| NET-05 | `Sampler` | 0.5 | as `counters()` |
| NET-06 | `primary()` | 0.5 | 10 to 50 µs |

**Permanent or random MAC.** Phones, Apple devices, Windows, and NetworkManager randomize Wi-Fi addresses. `MacKind::Random` marks them: on Linux from the kernel's `addr_assign_type`, elsewhere from the locally administered bit. A MAC address is an identifier: for device identity use [`device`](#device-device-identity), never a raw MAC.

Interfaces are sorted by index, then name. More than 4 096 interfaces is `TooLarge`. Names from the OS are validated before they are used to build a path. Counters are 64-bit on every platform (macOS's 32-bit counters wrap and are not used). A counter that goes backwards between two samples (an interface re-created) re-bases that interface instead of reporting a negative rate.

```rust
use sys_lib as sys;

for i in sys::net::interfaces()? {
    let mac = i.mac.map(|m| format!("{m} ({:?})", m.kind)).unwrap_or_default();
    println!("{} {:?} up={} speed={:?} {mac}", i.name, i.kind, i.running, i.speed_mbps);
    for a in &i.addresses { println!("    {a}"); }
}
println!("addresses: {:?}", sys::net::addresses()?);
# Ok::<(), sys_lib::Error>(())
```

---

## `host`: the machine and its environment

**Since** 0.3 (HST-01), 0.4 (HST-02 to HST-04), 0.5 (HST-05 to HST-07)

```rust
pub mod host {
    pub fn name() -> Result<String>;                      // live
    pub fn uptime() -> Result<Duration>;                  // includes time suspended
    pub fn boot_time() -> Result<SystemTime>;
    pub fn load_average() -> Result<Option<LoadAverage>>; // Ok(None) on Windows
    pub fn timezone() -> Result<TimeZone>;
    pub fn locale() -> Option<&'static Locale>;          // static: the environment at the first call
    pub fn model() -> Option<&'static Model>;            // static
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct LoadAverage { pub one: f32, pub five: f32, pub fifteen: f32 }

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TimeZone {
    pub iana: Option<String>,           // "Europe/London"
    pub windows: Option<String>,        // "GMT Standard Time"
    pub utc_offset_seconds: i32,        // now, including daylight saving
    pub abbreviation: Option<String>,   // "BST"
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Locale { pub tag: &'static str, pub raw: &'static str }   // "en-US", "en_US.UTF-8"

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Model { pub vendor: Option<&'static str>, pub name: &'static str, pub family: Option<&'static str> }
```

| Id | Item | Since | Cost [est.] |
|---|---|---|---|
| HST-01 | `name()` | 0.3 | 2 µs / 0.5 µs / 1 µs |
| HST-02 | `uptime()` | 0.4 | 0.05 to 2 µs |
| HST-03 | `boot_time()` | 0.4 | 2 to 4 µs / 0.5 µs / 0.1 µs |
| HST-04 | `load_average()` | 0.4 | 2 µs / 0.3 µs / 0 |
| HST-05 | `timezone()` | 0.5 | 5 to 15 µs |
| HST-06 | `locale()` | 0.5 | about 1 ns after the first call |
| HST-07 | `model()` | 0.5 | about 1 ns after the first call |

- `name()` is live: DHCP and `hostnamectl` change it while processes run. A name that is not UTF-8 is converted lossily; longer than 255 bytes is truncated.
- `uptime()` is monotonic; `boot_time()` is stable when the wall clock steps.
- `load_average()` is `Ok(None)` on Windows, which keeps no load average; nothing is simulated.
- `timezone()` returns the zone **name** and the current offset, not zone arithmetic (that is a calendar crate's job). `TZ` is used only when it is a valid name (at most 64 bytes, no `..`, no control characters, a leading `:` stripped); it is never opened as a path. When `/etc/localtime` is a copy rather than a link, the name comes from `/etc/timezone` or is `None`: zone files are not compared to guess. On Windows the IANA name comes from an embedded copy of the CLDR Windows-zone table.
- `locale()` is `None` for `C`, `POSIX`, and `C.UTF-8` (no language preference expressed).
- `model()` is the hardware model from DMI, SMBIOS, the device tree ("Raspberry Pi 5"), or `hw.model` on macOS ("Mac14,9"; no marketing-name table is shipped).

The host name, time zone, and locale are identifying values; keep them out of output that leaves the machine unless that is intended.

```rust
use sys_lib as sys;

println!("{} up {:?}", sys::host::name()?, sys::host::uptime()?);
match sys::host::load_average()? {
    Some(l) => println!("load {:.2} {:.2} {:.2}", l.one, l.five, l.fifteen),
    None => println!("no load average on this OS"),
}
let tz = sys::host::timezone()?;
println!("zone {:?}, offset {} s", tz.iana, tz.utc_offset_seconds);
# Ok::<(), sys_lib::Error>(())
```

---

## `virt`: virtualization and containers

**Since** 0.5 · VRT-01 to VRT-04

```rust
pub mod virt {
    pub fn detect() -> &'static Virtualization;
    pub fn is_vm() -> bool;
    pub fn is_container() -> bool;
    pub fn is_wsl() -> bool;
    pub fn is_kubernetes() -> bool;
    pub fn hypervisor() -> Option<Hypervisor>;
    pub fn container() -> Option<Container>;
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Virtualization {
    pub hypervisor: Option<Hypervisor>,
    pub container: Option<Container>,
    pub wsl: Option<u8>,           // 1 or 2
    pub kubernetes: bool,
    pub evidence: Vec<Evidence>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Hypervisor {
    Kvm, Qemu, Vmware, HyperV, Xen, VirtualBox, Parallels, Bhyve, Acrn, Qnx,
    AppleVirtualization, AmazonNitro, GoogleCompute, Bochs, Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Container { Docker, Podman, Containerd, Lxc, SystemdNspawn, OpenVz, Other }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Evidence { pub check: &'static str, pub strength: Strength }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Strength { Strong, Weak }
```

| Id | Item | Cost |
|---|---|---|
| VRT-01 | `detect()` | about 1 ns; the first call 20 to 200 µs [est.] |
| VRT-02 | `is_vm`, `is_container`, `is_wsl`, `is_kubernetes` | about 1 ns |
| VRT-03 | `hypervisor()`, `container()` | about 1 ns |
| VRT-04 | `evidence` | 0 |

Layered checks (CPUID hypervisor leaves, DMI strings, hypervisor files, container marker files and cgroup names, the overlay root, the Kubernetes service account, WSL's kernel release) with every check that fired named in `evidence` (constant strings, never raw values). A Windows **host** with Hyper-V, WSL 2, or virtualization-based security enabled sees "Microsoft Hv" in CPUID too; with the root-partition privilege it is not reported as a VM. Inside WSL 2 both `is_vm()` and `is_wsl()` are true. A container inside a VM sets both `container` and `hypervisor`. Evidence from environment variables (`KUBERNETES_SERVICE_HOST`) is `Weak`: any process can set it.

Detection is advisory. Do not make security decisions on it (threat T14).

```rust
use sys_lib as sys;

let v = sys::virt::detect();
if sys::virt::is_container() {
    println!("in {:?}; kubernetes: {}", v.container, v.kubernetes);
}
for e in &v.evidence { println!("  {} ({:?})", e.check, e.strength); }
```

---

## `power`: battery and AC

**Since** 0.5 · PWR-01 to PWR-03

```rust
pub mod power {
    pub fn has_battery() -> bool;                     // static
    pub fn battery() -> Result<Option<Battery>>;      // None: no system battery
    pub fn on_ac() -> Result<Option<bool>>;           // None: unknown
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Battery {
    pub charge: Percent,
    pub state: BatteryState,
    pub time_to_empty: Option<Duration>,
    pub time_to_full: Option<Duration>,
    pub health: Option<Percent>,     // full capacity / design capacity
    pub cycles: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum BatteryState { Charging, Discharging, Full, NotCharging, Unknown }
```

| Id | Item | Cost [est.] |
|---|---|---|
| PWR-01 | `has_battery()` | about 1 ns after the first call |
| PWR-02 | `battery()` | 10 to 40 µs / 50 to 200 µs / 0.3 µs |
| PWR-03 | `on_ac()` | 5 to 20 µs / 20 µs / 0.3 µs |

The system battery only: wireless mice, keyboards, and headsets report batteries too and are excluded. Several internal batteries are combined by energy (the sum of energy now over the sum of energy when full), not by averaging percentages. A desktop without power information gives `on_ac() == Ok(None)`, not a guess. Windows reports no health or cycle count through `GetSystemPowerStatus` (`None`). Android: best effort through the Linux path (SELinux often denies it: `PermissionDenied`). iOS: `Unsupported`.

```rust
use sys_lib as sys;

if let Some(b) = sys::power::battery()? {
    println!("battery {} {:?}, empty in {:?}", b.charge, b.state, b.time_to_empty);
}
if sys::power::on_ac()? == Some(false) { /* on battery */ }
# Ok::<(), sys_lib::Error>(())
```

---

## `device`: device identity

**Since** 0.5 · DEV-01 to DEV-07 · feature `device-id` (off by default)

> **Not authentication.** A device id is a claim, not proof: root or Administrator can set the machine id to anything, and hypervisors set firmware UUIDs freely. Never use it as the only check for anything that matters. It is personal data in client software. Read [When not to use it](#when-not-to-use-it) first.

### Tier 1

```rust
pub mod device {
    /// An application-specific device id. `app` is a stable, application-unique string
    /// such as a reverse domain name ("com.hivedb.server"), 1 to 255 bytes.
    pub fn id(app: &str) -> Result<DeviceId>;
    pub fn fingerprint(app: &str) -> Result<Fingerprint>;
    pub fn report() -> StabilityReport;               // sources only, no application needed
    pub fn builder(app: &str) -> Builder;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(/* [u8; 16] */);
impl DeviceId {
    pub fn as_bytes(&self) -> &[u8; 16];
    pub fn parse(text: &str) -> Result<DeviceId>;     // accepts its own Display format only
    pub fn ct_eq(&self, other: &DeviceId) -> bool;    // constant time
}
impl fmt::Display for DeviceId;   // RFC 9562 UUID text, version 8: "6f1c2a4e-9b3d-8e21-a7c4-..."
impl fmt::Debug for DeviceId;     // DeviceId(6f1c2a4e-...): already a keyed hash, safe to show
impl FromStr for DeviceId;
```

The id is a keyed BLAKE3 hash of **one** identifier the OS keeps (by default the OS installation id: `/etc/machine-id`, the Windows `MachineGuid`, or on macOS the platform UUID), keyed by a key derived from your application id. Two applications on the same machine get unrelated ids, and nobody can turn an id back into the machine id or the serial numbers behind it. The raw identifier is read into a fixed buffer, hashed, and wiped; it never appears in a return value, an error, `Debug`, `Display`, or a log. sys-lib does not cache the id; store it if you need it often.

| Id | Item | Tier | Cost [est.] |
|---|---|---|---|
| DEV-01 | `device::id(app)` | 1 | Linux 5 to 15 µs; macOS 50 to 200 µs (IOKit); Windows 20 to 50 µs (registry); the hash under 1 µs |
| DEV-02 | `DeviceId` | 1 | `Display` about 40 ns |
| DEV-03 | `device::report()` | 1 | every source probed once, 0.1 to 2 ms |
| DEV-04 | `Builder` | 2 | as `id` |
| DEV-05 | `device::fingerprint(app)`, `Fingerprint` | 2 | 0.2 to 3 ms (SMBIOS, interfaces) |
| DEV-06 | `Fingerprint::compare` | 2 | under 1 µs |
| DEV-07 | `to_bytes`, `from_bytes` | 2 | under 1 µs |

```rust
use sys_lib as sys;

let id = sys::device::id("com.hivedb.server")?;
println!("device {id}");                                   // 6f1c2a4e-9b3d-8e21-a7c4-...
let again: sys::device::DeviceId = id.to_string().parse()?;
assert!(id.ct_eq(&again));
# Ok::<(), sys_lib::Error>(())
```

### `Builder`

```rust
pub struct Builder { /* private */ }
impl Builder {
    pub fn basis(self, basis: Basis) -> Self;        // default Basis::Install
    pub fn secret(self, key: [u8; 32]) -> Self;      // an application secret; wiped on drop
    pub fn fallback(self, on: bool) -> Self;         // default false: fail rather than change basis silently
    pub fn id(&self) -> Result<DeviceId>;
    pub fn fingerprint(&self) -> Result<Fingerprint>;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Basis {
    /// This OS installation. Unprivileged everywhere; changes on reinstall.
    /// macOS has no installation id and uses the platform UUID (the report says so).
    Install,
    /// The hardware: the firmware UUID, then the board serial. Survives reinstalls.
    /// Linux needs root (the DMI identifier files are readable by root only).
    Hardware,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Anchor { InstallId, PlatformUuid, BoardSerial, ProductSerial, CpuSignature, PermanentMac }
```

**With a secret**, nobody without it can compute or correlate the id, even with root on the machine: use one when ids are sent to a server. **Without one**, anyone who knows the raw machine id and your (public) application id can compute the device id, the same property as systemd's application-specific machine id. `fallback(true)` lets `Basis::Hardware` fall back to `Install` when hardware sources are denied; the id is then different from the hardware one, which is why it is off by default.

The derivation is versioned (`sys-lib/device/v1`) and **frozen for all of 1.x**: a given machine and application id give the same device id in every 1.x release. On a dual-boot machine, `Basis::Hardware` gives the same id under Linux (as root) and Windows.

```rust,no_run
use sys_lib::device::{self, Basis};

let secret: [u8; 32] = load_secret();   // the application's own secret, never shipped in clients
let id = device::builder("com.example.licensing")
    .basis(Basis::Hardware)
    .secret(secret)
    .id()?;
# fn load_secret() -> [u8; 32] { [7; 32] }
# Ok::<(), sys_lib::Error>(())
```

### Stability report

```rust
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct StabilityReport {
    pub sources: Vec<SourceStatus>,       // one per Anchor, in Anchor order
    pub default_basis_available: bool,
    pub clone_risk: Vec<CloneRisk>,       // empty when none detected
    pub confidence: Confidence,           // for Basis::Install
    pub changes_on: Vec<ChangeEvent>,     // for the default basis on this OS
}
impl fmt::Display for StabilityReport;   // a short table for support output; contains no values

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct SourceStatus { pub anchor: Anchor, pub state: SourceState }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum SourceState { Present, Absent, Placeholder, PermissionDenied, Unsupported }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CloneRisk { Container, VirtualMachine, MachineIdUninitialized, SharedImageLikely }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ChangeEvent { OsReinstall, BoardReplacement, NicReplacement, CpuUpgrade, VmRecreated, ContainerRecreated, AdminReset }
```

How far to trust the id on this machine, without revealing any value: which sources exist, which are placeholders that manufacturers and hypervisors ship ("To be filled by O.E.M.", all-zero UUIDs, `uninitialized` machine ids) and are treated as absent, which need privileges, whether the machine looks cloned (a container image or VM template sharing one machine id), and what events would change the id.

```rust
use sys_lib::device;

let report = device::report();
println!("{report}");
if !report.clone_risk.is_empty() {
    println!("this id may be shared with other machines: {:?}", report.clone_risk);
}
```

### Fingerprint

```rust
#[derive(Clone, PartialEq, Eq)]
pub struct Fingerprint { /* an app-keyed 16-byte hash per anchor, or absent */ }
impl Fingerprint {
    pub fn compare(&self, stored: &Fingerprint) -> Comparison;
    pub fn to_bytes(&self) -> [u8; FINGERPRINT_LEN];
    pub fn from_bytes(bytes: &[u8]) -> Result<Fingerprint>;
    pub fn anchors(&self) -> impl Iterator<Item = Anchor> + '_;   // which anchors are present
}
impl fmt::Debug for Fingerprint;   // the present anchors only
pub const FINGERPRINT_LEN: usize = 112;

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Comparison { pub verdict: Verdict, pub matched: Vec<Anchor>, pub changed: Vec<Anchor>, pub missing: Vec<Anchor> }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Verdict {
    Same,         // every anchor present in both matches
    LikelySame,   // matched weight >= 5 and at most one strong anchor changed
    Different,
}
```

A fingerprint hashes **every** available anchor separately, so a stored one can tell "this machine, one part changed" (a NIC swap, a reinstall) from "another machine". Concatenating anchors into one id would do the opposite: any one change would change everything. Weights: install id 3, platform UUID 3, board serial 2, product serial 2, permanent MAC 1, CPU signature 1 (identical CPUs share a signature, so it only confirms). Each hash is compared in constant time.

Stored format (112 bytes): magic `SLFP`, format version 1, an anchor bitmap, two reserved zero bytes, six 16-byte anchor hashes, and an 8-byte BLAKE3 checksum. `from_bytes` checks length, magic, version, reserved bytes, bitmap consistency, and checksum, in that order, and returns `Malformed` for anything wrong; the checksum catches storage corruption, not tampering.

```rust
use sys_lib::device::{self, Fingerprint, Verdict};

let now = device::fingerprint("com.example.app")?;
let stored = Fingerprint::from_bytes(&load_stored())?;
match now.compare(&stored).verdict {
    Verdict::Same => {}
    Verdict::LikelySame => { save(now.to_bytes()); }     // one part changed: update the record
    Verdict::Different => { /* treat as a new device */ }
    _ => {}
}
# fn load_stored() -> Vec<u8> { device::fingerprint("com.example.app").unwrap().to_bytes().to_vec() }
# fn save(_: [u8; 112]) {}
# Ok::<(), sys_lib::Error>(())
```

### When not to use it

- **Not authentication.** A device id is a claim, not proof. For proof of a device, use a key held in a TPM or Secure Enclave.
- **Not for tracking across applications or for advertising.** The application-specific key prevents correlation by design; using device identifiers that way breaks platform policies and, in the EU, likely needs consent under the ePrivacy Directive, Article 5(3).
- **Personal data.** Under the GDPR (Recital 30), an identifier from a user's device can identify a person: an application storing a device id needs a lawful basis, a retention period, and a way to delete it. This is a statement of the issue, not legal advice.
- **Not stable enough for licensing alone.** Reinstalls and board replacements happen; licensing should allow re-activation.

---

## After 1.0

Room is left in the design (`#[non_exhaustive]` everywhere); these arrive as minor releases. No module appears before it is implemented.

| Version | Item | What |
|---|---|---|
| 1.1 | SNS-01 | `sensors::temperatures()`: Linux hwmon and thermal zones; macOS and Windows `Unsupported` (no documented API) |
| 1.1 | MEM-10 | huge pages and large-page minimum |
| 1.1 | DSK-06 | disk I/O counters |
| 1.1 | HST-08 | `host::users()`: logged-in sessions |
| 1.1 | PWR-04 | `power::batteries()`: each battery separately |
| 1.1 | NET-07 | `net::interfaces_with(options)` |
| 1.1 | VRT-05 | `virt::cloud()`: from DMI vendor strings only; never a metadata-service network call |
| 1.2 | GPU-01 | `gpu::list()`: adapters, without vendor libraries |
| 1.2 | PRB-01 | `probe::Probe::with_root(root)` (Linux): read `/proc` and `/sys` under an explicit root, for an agent with the host's trees mounted; explicit in code, never taken from the environment |

---

## Errors by variant

| Variant | Meaning | What the message tells the caller to do |
|---|---|---|
| `Unsupported { what, platform }` | this reading does not exist on this platform (load average on Windows is `Ok(None)` instead; temperatures on macOS are `Unsupported`) | treat the value as unknown |
| `PermissionDenied { what, hint }` | the OS refused (root-only DMI files, a sandbox, SELinux, seccomp) | run with the permission named in `hint`, or accept the value as unknown |
| `NotFound { what, source }` | a file or key that should exist does not (a container with a reduced `/proc`) | accept the value as unknown; `docs/PLATFORM-NOTES.md` lists known causes |
| `Malformed { what, source, line }` | the OS returned data that does not parse; the source and line, never the content | report it with the output of `sys_lib::summary()` |
| `TooLarge { what, limit }` | a file or a list exceeded its budget (1 MiB per file; 4 096 mounts or interfaces; 16 384 CPUs), or a caller buffer was too short | raise the budget with the Tier 2 option when the machine is genuinely that large, or pass a longer buffer |
| `TooSoon { elapsed, minimum }` | a sampler was asked again before any time passed on the OS clock | sample at least `minimum` apart |
| `Os { what, call, code }` | a system call failed, with its name and OS error code | the code says why; report it if unexpected |
| `Io { what, source }` | file I/O failed outside the cases above | see `source` |
| `InvalidInput { what, reason }` | invalid input from the caller (a guard name with a slash, an empty app id, an id string of the wrong length) | fix the input as `reason` says |
| `AlreadyRunning { pid, lock_file }` | the process guard found another copy holding the lock | stop the other copy (its pid when known) or use another lock directory |
| `RunDirUnavailable { path, source }` | the guard's lock directory cannot be created, written, or trusted | the message names the reason and the fix (`chmod 700`, a local directory, `allow_network_lock`) |
| `OpenFilesTooLow { have, need }` | the hard open-file limit is below the guard's minimum | raise `LimitNOFILE=` in the systemd unit, `ulimit -Hn`, or launchd's limits |
| `NoIdentifierSource { basis }` (feature `device-id`) | no usable identifier for the requested basis | see `device::report()` for the state of each source |

`Error::is_unavailable()` is true for `Unsupported`, `PermissionDenied`, and `NotFound`: the three that mean "this fact is not available here".
