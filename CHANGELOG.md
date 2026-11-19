<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>sys-lib</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

---

## [0.7.0] - 2026-11-11

Optimisation: every reading measured against the crates people use for it, the
slow ones made faster, and the numbers published in `docs/PERFORMANCE.md`. The
fixes from the independent review of 0.6.0 are in this release too.

### Added

- `docs/PERFORMANCE.md`: warm and cold numbers against sysinfo, num_cpus,
  os_info, raw-cpuid, and machine-uid on Linux (x86-64, Arm64, and a container
  with limits), Windows, and macOS (Apple silicon and Intel), the footprint,
  and the reason for each reading that is not 20% ahead.
- `bench/`: the benchmarks (criterion warm, a fresh-process cold harness,
  footprint binaries) and a 5% regression gate that runs on every pull request.
- `Guard::is_held()`: whether the lock file at its path is still the one the
  guard locked, for a long-running service to check.
- A test that no reading keeps a file descriptor or handle open.

### Changed

- **The guard's default lock directory depends only on the user**, never on
  `$XDG_RUNTIME_DIR`, `$XDG_STATE_HOME`, or `$HOME`: `/run/<name>` for root on
  Linux (`/var/run/<name>` on macOS and FreeBSD), and under the account
  database's home directory for other users. Before, a copy started by a
  service manager and one started from a shell could take different locks and
  both run. A user without a home directory now gets `RunDirUnavailable`
  unless `lock_dir` is passed (D-106).
- The guard checks the lock file again after locking it, so a file replaced in
  between is never the one held, and on Windows opens it without delete
  sharing, so the held file cannot be deleted (D-107).
- `cpu::logical()`, `cpu::physical()`, and the CPU identity are cached apart:
  the first `cpu::logical()` on Linux reads one small file (271 µs to 13 µs
  cold on 32 CPUs) (D-101).
- Windows `mem::snapshot()` takes swap from `GlobalMemoryStatusEx` (53 µs to
  1.6 µs); **`Memory::cached` is now `None` on Windows**, whose only source
  counted every handle on the machine (D-102).
- Windows remembers each fixed volume's SSD or HDD kind (D-104), and reads the
  host name once (a new name takes effect at restart): `host::name()` from
  about 100 µs to a copy.
- The Linux file helpers share one non-generic read loop, and the kernel
  release comes from `uname` (D-101, D-103).
- The readiness report shows the data directory's filesystem without its mount
  source, which can name a server, an account, or a volume, and prints control
  characters in found values as `?` (D-110).
- The documented Windows costs of `process::threads()` and `host::name()`, and
  the dependency claim: no third-party dependencies by default, while
  `device-id` brings crypt-io's BLAKE3 stack.

### Fixed

- 32-bit Linux read the utmp database with the 64-bit record layout, so
  `host::sessions()` lost users and login times.
- `cpu::is_32bit()` was false on 32-bit processors running 32-bit Linux.
- Past 4 096 mounts, `disk::of()`, the readiness data-directory checks, and the
  posture's disk encryption failed; `disk::of()` now streams the mount table
  and keeps only the best match (D-109).
- A mountinfo line longer than 8 KiB was skipped, so `disk::of()` reported the
  parent mount; lines up to 64 KiB are read whole, longer ones are `TooLarge`
  (D-108).
- A FIFO or device at a file path such as `/etc/os-release` blocked the caller,
  and every later `os::*` caller, forever (D-108).
- A hostile CPU list of many wide ranges took seconds to parse; ranges are now
  set a word at a time.
- A cgroup membership matched a mount root by byte prefix, so `/docker/abc`
  read limits under `/docker/ab`.
- The Linux directory walk formed a reference to a whole `dirent` from a record
  that may be shorter.
- macOS `host::processes()` and `process::open_files()` returned a full buffer
  as the count when the table outgrew three reads; they are now `TooLarge`.
- Windows `disk::of()` described a disk for device-namespace paths
  (`\\.\NUL`); they are now `NotFound`.
- `AlreadyRunning`'s message calls the pid from the pid file a hint to check,
  not a process to stop.

---

## [0.6.0] - 2026-11-08

Environment and reports: feature complete. The settings that decide whether a
server performs, whether a benchmark can be trusted, and how the machine is
secured; temperatures and graphics adapters; and two more platforms.

### Added

- `readiness`: `report(profile)` and `report_for(profile, data_dir)` for the
  General, Database, WebServer, and Benchmark profiles. Every check says what
  it found, what the profile wants, why, and the command that changes it; it is
  left out where it does not apply and `Unknown` with the reason where it cannot
  be read. Read-only. `Report` prints as a table, key-value lines, or JSON.
- `bench::environment()`: governor or power plan, turbo, SMT, invariant TSC,
  clock source, throttling, isolated CPUs, ASLR, power source, load, and
  whether it is a VM, with the warnings that make results noisy and
  `is_reproducible()`.
- `security::posture()`: Secure Boot, TPM, disk encryption, SELinux or
  AppArmor, ASLR, kernel lockdown or VBS, code integrity, firewall, and the
  kernel's CPU vulnerability list.
- `sensors::temperatures()`: Linux hwmon and thermal zones, Windows drives,
  FreeBSD CPU and ACPI zones.
- `gpu::list()`: Linux DRM adapters named from `pci.ids`, macOS PCI display
  controllers and Apple silicon, Windows display adapters with their memory and
  driver version.
- `probe::Probe::with_root` (Linux): a monitoring agent reads a host mounted
  elsewhere through the same samplers, given in code, never through the
  environment.
- A FreeBSD backend (64-bit, FreeBSD 12 and later) for every reading FreeBSD
  answers without privileges, with a CI job that runs the suite in a FreeBSD 14
  virtual machine.
- 32-bit Linux on glibc (i686, armv7): verified layouts and a CI job that runs
  the suite on both.
- iOS `mem::free()` from `os_proc_available_memory()`.
- Example `reports`; a fuzz target for the `pci.ids` parser.

### Changed

- Windows `process::threads()` reads a process snapshot instead of a thread
  snapshot: about four times faster on a busy machine.
- Every `[verify]` mark in `docs/SOURCES.md` confirmed or replaced, among them
  macOS `kern.osproductversion`, which does answer 10.16 in compatibility mode.

### Fixed

- `cpu::Sampler`'s per-core reading grew its buffer once when the OS listed
  more CPUs than expected (CPUs brought online since it was made) and then
  failed with `TooLarge`; it now grows to the 16 384 CPU budget.

### Known gaps

- The bare-metal Linux desktop fixture is still to be captured (it needs a
  bare-metal Linux desktop).

---

## [0.5.1] - 2026-11-07

### Fixed

- docs.rs built only the Linux documentation for 0.5.0: the Windows and macOS
  builds failed in blake3's build script (feature `device-id`), which compiles
  C for the target and found no compiler for it. docs.rs now builds blake3's
  portable Rust code, so every platform's items are documented again. Nothing
  changes outside docs.rs.
- The 0.5.0 date below corrected to the day it was published.

---

## [0.5.0] - 2026-11-07

Detection and identity: what this machine is and what it runs inside, how the
program was started, power, DNS, and an application-scoped device id.

### Added

- `virt`: `detect()`, `is_vm()`, `is_container()`, `is_wsl()`,
  `is_kubernetes()`, `hypervisor()`, `container()`, and `cloud()`, every answer
  with the checks that fired, and no call to a metadata service.
- `os`: `edition()`, `codename()`, `is_server_edition()`, `is_64bit()`,
  `form_factor()`, `detect_form_factor()`, and the `is_desktop()`-style checks.
- `cpu`: `native_arch()`, `is_translated()`, `topology()`, `caches()`,
  `cache_line()`, `numa_nodes()`, and `is_hybrid()`.
- `mem::modules()`; `disk::of()`, `DiskKind::Ssd` and `Hdd`, and volume labels
  on macOS; `net::primary()` and `net::dns()`.
- `host`: `timezone()`, `locale()`, `model()`, `firmware()`, `board()`,
  `clock_sync()`, and `sessions()`.
- `power`: `has_battery()`, `battery()`, `batteries()`, and `on_ac()`, leaving
  out peripheral batteries and combining packs by energy.
- `process`: `launch()`, `is_service()`, and `is_interactive()`.
- `summary()`: every fact for a log line or a support bundle, as `Display`,
  key-value pairs, or JSON, with no host name, user, MAC, or device id.
- `device` (feature `device-id`, bringing crypt-io): `id()`, `fingerprint()`,
  `report()`, `builder()`, `DeviceId`, `Fingerprint`, and `StabilityReport`.
  The derivation `sys-lib/device/v1` is frozen for all of 1.x.
- `docs/DEVICE-ID.md`, `docs/ARCHITECTURE.md`, and `docs/PLATFORM-NOTES.md`;
  examples `summary` and `device_id`.
- Parsers for SMBIOS, `uevent`, `resolv.conf`, utmp, time zone names, and
  locales, with fuzz targets, and one for stored fingerprints; attack tests T1,
  T2, T3, T13, T14, and T15; a CI job that runs a real binary on Nano Server.

### Fixed

- WSL 2 with systemd writes `wsl` to `/run/systemd/container`, and its root is
  an overlay: neither makes it a container (D-061). Found by the WSL fixture.
- The macOS CoreFoundation and IOKit owners, and the Windows registry and
  wtsapi32 owners, were built with `bool::then_some`, which makes its value
  before testing: a missing IOKit property released NULL and stopped the
  process. Found by the macOS CI runners, with a backtrace from lldb.

### Changed

- `capture.sh` version 4 records the remaining world-readable DMI files,
  `/sys/firmware/efi`, and `/proc/sys/kernel/arch`.
- The `ubuntu-24.04-desktop-amd` fixture and iOS `os_proc_available_memory()`
  move to 0.6 (D-062).
- The 0.4.0 date below corrected to the day it was published.

---

## [0.4.0] - 2026-10-30

Live readings: memory and swap with container limits, CPU usage and load,
disks and disk I/O rates, network interfaces, traffic and connections,
pressure and out-of-memory events, and this process's resources.

### Added

- `mem`: `total()`, `used()`, `free()`, `used_percent()`, `free_percent()`,
  `snapshot()`, `host_total()`, `host_free()`, `limit()` (cgroup v1 and v2,
  the Windows job object), `swap()`, `page_size()`, `pressure()`, `events()`
  (out-of-memory kills), `activity()` and `Sampler` (swap and paging rates),
  and `huge_pages()`. Inside a container the totals are the container's.
- `cpu`: `usage()`, `usage_over()`, `used()`, `free()`, `times()`, `Sampler`
  (total and per core, with `sample_per_core_into` for a caller's buffer),
  `load()`, `count()`, `online()`, `packages()`, `quota()`, `frequency()`,
  `current_frequency()`, `pressure()`, `features()`, `has()`, and
  `x86_level()` (planned for 0.5, D-052).
- `disk`: `space()`, `list()`, `list_with()`, `io()`, `io_over()`,
  `io_totals()`, `io_counters()`, `IoSampler`, and `pressure()`.
- `net`: `interfaces()`, `interfaces_with()`, `addresses()`, `Mac`,
  `totals()`, `counters()`, `traffic()`, `traffic_over()`, `Sampler`, and
  `connections()`.
- `process`: `parent_id()`, `memory()`, `peak_memory()`, `memory_detail()`
  (planned for 0.5, D-044), `cpu_time()`, `threads()`, `open_files()`,
  `limits()`, `started_at()`, `io()`, `info()`, `info_with()`, and
  `CpuSampler`.
- `host`: `uptime()`, `boot_time()`, `processes()`, and `open_files()`.
- `Pressure`, `PressureLevel`, and `Stall`; `Rate` and `Mhz`.
- Examples `monitor` (a live view built on the samplers) and an extended
  `facts`.
- Tests: a counting-allocator test that every scalar reading and every
  sampler step allocates nothing after warm-up; attack tests T5 to T8 over a
  hostile `/proc` and `/sys` tree; live comparisons against the OS's own
  tools; a `container` CI job that runs the tests under `docker run --memory
  --cpus --cpuset-cpus` and checks the limits are read; the `procfs` fuzz
  target for the new parsers.
- Fixtures recaptured with `capture.sh` v3, which records the live-reading
  files.

### Fixed

- A hybrid cgroup host (v1 controllers with a unified hierarchy mounted
  beside them) read the empty unified hierarchy for memory and CPU limits.
  Each controller now comes from the v1 hierarchy that carries it first
  (D-046). Found by the recaptured fixtures.
- An enormous uptime value in `/proc/uptime` panicked in the duration
  conversion; it is now reported as malformed. Found by the hostile tree.
- macOS CPU usage read `host_statistics`, which the kernel rate-limits: a
  reading taken soon after another could show no time passing. The totals
  now come from `host_processor_info`, summed over every processor. Found
  by the macOS CI runners.
- `cpu::name()` cleaning ran its rules once, so removing one suffix could
  leave another ("8-core 16-Cores"); the rules now run until nothing
  changes. Found by fuzzing.
- WSL's `rootfs` mount at `/init` was listed as a disk; `rootfs` is now a
  pseudo filesystem unless it is the root (D-050).

### Changed

- `docs/API.md` and `docs/SOURCES.md` corrected to the shipped code
  (D-047 to D-049, D-053): Windows per-core usage past 64 processors or
  several processor groups is `Unsupported` with the total correct; Windows
  paging activity is `Unsupported`; macOS threads and connection counts come
  from the kernel calls the code uses; `Disk::label` is Windows only until
  0.5.
- The `ubuntu-24.04-desktop-amd` fixture moves to 0.5 (D-051).

---

## [0.3.0] - 2026-10-23

The boot kit: OS identity and the distribution, CPU counts and names, the
host, the user, build info, instance info, and the process guard.

### Added

- `os`: `kind()` and the `const` `is_*()` checks, `name()`, `version()`,
  `kernel()`, `distro()`, `distro_id()`, `is_like()`, `os_release()`, and
  `info()`. On Linux: os-release, then the legacy release files, then "Linux".
- `cpu`: `arch()`, `is_64bit()`, `is_32bit()`, `logical()`, `physical()`,
  `available()` (affinity, cgroup cpuset, cgroup v1 and v2 CPU quota, the
  Windows job object's CPU rate cap), `name()`, `raw_name()`, and `vendor()`.
- `host::name()`; `process::id()`, `process::user()`, `process::is_elevated()`.
- The process guard: `process::guard()`, `Guard`, `GuardBuilder`,
  `GuardReport`, `GuardWarning`, and `process::Limit`.
- Build info: `build::emit()`, `build::emitter()` (planned for 0.4, D-032),
  `build_info!()`, `BuildInfo`, and `PublicBuild`.
- Instance info: `Instance`, `InstanceId`, and `instance::current()`.
- `Error` and `Result`, `Bytes` and `Units`, and `Percent`.
- `cpu::Vendor` variants `Broadcom`, `Marvell`, `Fujitsu`, `HiSilicon`, and
  `Microsoft`, beyond the planned list (D-033).
- Examples `facts` and `boot`.
- A distribution corpus of 64 systems in `tests/fixtures/distros`, captured
  from container images by the `Fixtures` workflow and checked against a
  shell's reading of each os-release; the `docker-cgroup1-limited` fixture,
  captured on a cgroup v1 host in a virtual machine (D-038).
- Live comparison with the OS's tools, a 64-thread first-use test, the guard
  across processes with `kill -9` recovery, and attack tests T4, T9, T10,
  T11, and T16.
- Fuzz targets for all 12 parsers; CI runs Miri over the parsers on every
  push, each fuzz target for 20 seconds on every push, and 10 minutes per
  target nightly (D-040).

### Changed

- `docs/API.md` corrected to the shipped code after an item-by-item audit
  (D-034); `GuardWarning::ThreadsAlreadyRunning` is Unix only (D-035).
- The `ubuntu-24.04-desktop-amd` fixture moves to 0.4: it needs a bare-metal
  Linux desktop (D-039).

- The plan and `docs/API.md` cover the complete library: nothing is scheduled
  after 1.0. Added to the API: disk read and write rates and this process's
  disk I/O; CPU, memory, and I/O pressure with out-of-memory event counts; swap
  and paging activity; huge pages; memory modules; system-wide process, thread,
  and open-file counts; firmware and motherboard; how the program was started;
  DNS servers; clock synchronization; sessions; the cloud vendor; each battery;
  the server readiness, benchmark environment, and security posture reports;
  temperatures, GPU adapters, and `probe::Probe`. FreeBSD and 32-bit Linux get
  real implementations. The roadmap gains 0.6 (environment and reports);
  optimisation moves to 0.7, hardening to 0.8, alpha and beta to 0.9.

---

## [0.2.0] - 2026-10-22

The design lock: the complete 1.0 API, written down and approved. No public
API beyond `VERSION` yet; the boot kit arrives in 0.3.0.

### Added

- `docs/API.md`: the complete 1.0 public API, every item with its signature,
  errors, platform behaviour, cost, the version it arrives in, and examples.
- `docs/SOURCES.md`: for every reading and every OS, the file, system call, or
  API it comes from, with units, privileges, and quirks.
- The OS declaration lists in `src/sys/{linux,apple,windows}/ffi.rs` (crate
  private): every system call, struct, and constant the readings will use,
  with size and alignment assertions per target and compile-time checks of
  every field offset against `libc` and `windows-sys` (dev-dependencies only).
  Windows functions are imported with `raw-dylib`. The checks already caught
  one difference: musl's `struct rusage` is 128 bytes longer than glibc's.
- `tests/fixtures/capture.sh` and two fixture machines captured with it:
  `wsl2` and `wsl2-systemd-scope-limited` (a nested cgroup v2 limit of 256 MiB
  and 1.5 CPUs). A manual `Fixtures` workflow captures container fixtures on a
  GitHub runner, which captured `docker-cgroup2-limited` (256 MiB, 1.5 CPUs,
  cpuset of two) and `minimal-scratch` (no os-release, no machine id).
- API additions at the design review: `os::distro()` and `os::distro_id()`
  answer on every OS ("Ubuntu", "Windows", "macOS"), with `os::is_like()` and
  `os::os_release()`; `cpu::load()` with `Load::per_cpu()`; `cpu::is_64bit()`
  and `cpu::is_32bit()` for the processor; network download and upload
  (`net::traffic()`, `net::Sampler`, `net::totals()`), connection counts
  (`net::connections()`), and the `Rate` type.
- The documented install line is Cargo's rename,
  `sys = { package = "sys-lib", version = "0.2" }`, so code writes `sys::`.

### Changed

- CI's cross-target job compiles the unit tests too, so the layout checks run
  for musl, 32-bit Windows, Android, and iOS.
- `host::load_average()` became `cpu::load()`; `cpu::is_64()`/`is_32()` became
  `cpu::is_64bit()`/`is_32bit()` and now answer for the processor; network
  traffic moved from 0.5 to 0.4.
- `tests/fixtures/` and `fuzz/` are left out of the published package.

### Fixed

- `capture.sh` found no cgroup files in Docker, whose mountinfo lines have no
  optional fields.

---

## [0.1.0] - 2026-10-21

The scaffold and the plan. No functional code yet.

### Added

- Crate manifest (edition 2024, MSRV 1.89, no dependencies; features
  `instance` and `guard` on by default, `device-id` off, all declared empty
  until their code lands), `Apache-2.0 OR MIT` license, README, REPS,
  `deny.toml`, `clippy.toml`, `rustfmt.toml`, `.gitattributes`.
- `VERSION` constant and the crate-root lint set: `unsafe_code` denied (the
  per-platform OS modules will lift it locally), `missing_docs`,
  undocumented unsafe blocks, and `unwrap`/`expect`/`panic` in library code
  denied.
- CI on Linux, macOS, and Windows, each on x86_64 and aarch64, on stable and
  1.89; `cargo check` for `x86_64-unknown-linux-musl`, `i686-pc-windows-msvc`,
  `aarch64-linux-android`, and `aarch64-apple-ios`; documentation built with
  default and all features; `cargo package`; `cargo audit` and `cargo deny`;
  a job that fails if `src/` can start a subprocess outside the build helper.
- `dev/PLAN.md` (purpose, scope, requirements, design, API outline with item
  ids, threats T1 to T16, architecture, dependencies, competitors,
  performance targets, testing with edge cases E-1 to E-100, open decisions,
  ideas, decision log D-001 to D-011), `dev/DIRECTIVES.md`, and
  `dev/ROADMAP.md`.
- `tests/fixtures/` and `fuzz/` skeletons.

[Unreleased]: https://github.com/jamesgober/sys-lib/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/jamesgober/sys-lib/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/jamesgober/sys-lib/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/jamesgober/sys-lib/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/jamesgober/sys-lib/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jamesgober/sys-lib/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jamesgober/sys-lib/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/sys-lib/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/sys-lib/releases/tag/v0.1.0
