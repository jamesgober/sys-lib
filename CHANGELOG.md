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

## [0.4.0] - 2026-10-28

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

[Unreleased]: https://github.com/jamesgober/sys-lib/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/jamesgober/sys-lib/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jamesgober/sys-lib/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/sys-lib/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/sys-lib/releases/tag/v0.1.0
