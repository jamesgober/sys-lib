# sys-lib - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria and ends with the release routine (bump, CHANGELOG, release notes, local gate, push, green CI, tag, GitHub release, `cargo doc`, `cargo publish`).
> Design, API item ids (ROOT-, BLD-, INS-, PRC-, OS-, CPU-, MEM-, DSK-, NET-, HST-, VRT-, PWR-, DEV-), threats (T1 to T16), and edge cases (E-1 to E-100): [`PLAN.md`](./PLAN.md).
>
> **Anti-deferral rule:** no listed task moves to a later phase unless this file records the move and the reason, with a decision-log entry in `PLAN.md`. P0 items ship in the version listed and never move.

## v0.1.0 - Scaffold (DONE, 2026-10-21)
Exit: CI green on every job; the crate published to claim the name; decision log D-001 to D-008 written.
- [x] Repository `jamesgober/sys-lib` created and cloned to `lib-collection/sys-lib`.
- [x] Manifest (edition 2024, MSRV 1.89, features `instance`, `guard`, `device-id` declared with no dependencies yet), README, CHANGELOG, REPS, dual license, `deny.toml`, `clippy.toml`, `rustfmt.toml`, `.editorconfig`, `.gitattributes`.
- [x] `src/lib.rs`: crate docs, lint attributes, `VERSION` (ROOT-01), one test.
- [x] `dev/PLAN.md`, `dev/DIRECTIVES.md`, `dev/ROADMAP.md`; `tests/fixtures/` and `fuzz/` skeletons.
- [x] CI: Linux, macOS, Windows on x86_64 and aarch64, stable and 1.89; cross-target `cargo check` (musl, i686 Windows, Android, iOS); security job (audit, deny); no-subprocess job.

## v0.2.0 - Design lock (DONE, 2026-10-22)
Exit: James approves `docs/API.md`; layout tests pass on every target; every open decision answered.
- [x] `docs/API.md` written for every item in `PLAN.md` section 13, with signatures, errors, platform tables, and examples; each item marked with the version it arrives in (the crate exposes only what is implemented). Approved by James, 2026-10-22.
- [x] `docs/SOURCES.md` first version: every reading, every OS, the exact file, call, or API.
- [x] `src/sys/<platform>/ffi.rs` declaration lists written and reviewed against the platform headers; `#[repr(C)]` layout assertions for every struct, and compile-time oracles against `libc` and `windows-sys` on all ten CI targets (D-012).
- [x] Fixture capture script `tests/fixtures/capture.sh`.
- [x] Fixtures: `wsl2`, `wsl2-systemd-scope-limited`, `docker-cgroup2-limited`, and `minimal-scratch` captured (the last two by the `Fixtures` workflow on a GitHub runner, D-019).
- Moved to 0.3 (D-027): `docker-cgroup1-limited` and `ubuntu-24.04-desktop-amd`, which need a cgroup v1 host and a bare-metal Linux desktop. 0.3 reads the CPU side of cgroups and the desktop's os-release and topology, so both are needed by 0.3's exit; nothing in 0.2 reads them.
- [x] Open decisions OD-4 to OD-6, OD-9, OD-11 to OD-17 accepted by James as recommended (D-021); his API additions recorded (D-022 to D-026).
- [x] One new defence idea recorded (`PLAN.md` section 22: the layout oracle).

## v0.3.0 - Core: the boot kit (WASP boot phase 0)
API: ROOT-02, ROOT-03, ROOT-04 · BLD-01 to BLD-05 · INS-01 to INS-06 · PRC-01, PRC-11, PRC-12, PRC-20 to PRC-23 · OS-01 to OS-07 · CPU-01 to CPU-06 · HST-01.
Exit: WASP's phase 0 can be written against 0.3 (build info first line, instance id in every log line, guard before any file is opened, worker pool sized by `cpu::available()`); every 0.3 item has unit, fixture, live, and edge-case tests; the guard tests pass on all three OSes including `kill -9` recovery.
- [ ] Bounded reader (`read.rs`); parser layer: `os_release` (and the legacy release files), `cpu_list`, `cpuinfo`, `proc_status`, `cgroup` (CPU side).
- [ ] Fixtures moved from 0.2 (D-027): `docker-cgroup1-limited` (a cgroup v1 host) and `ubuntu-24.04-desktop-amd` (a bare-metal Linux desktop).
- [ ] Distribution corpus: real os-release and legacy release files from at least 40 distributions, each with its expected `distro()`, `distro_id()`, `version()`, and `is_like()` answers (D-022).
- [ ] `Error` with every variant and its "do" sentence tested; `Bytes` and `Percent` with the display rules of `PLAN.md` section 6.
- [ ] `cpu::available()` complete: affinity, cpuset, and the cgroup v1/v2 CPU quota, with E-12 to E-15.
- [ ] Process guard complete with every protection in `PLAN.md` section 9.3 and E-63 to E-68; first-party dependency path-lib wired.
- [ ] Build helper and macro with E-59 to E-61; instance info with id-forge and clock-lib wired, E-62.
- [ ] Fuzz targets for every parser introduced; Miri job active in CI.
- [ ] Attack tests T4, T9, T10, T11, T16.
- [ ] README quick start (Tier 1 first).

## v0.4.0 - Extended: live readings
API: ROOT-05 · BLD-06 · PRC-02 to PRC-04, PRC-06 to PRC-10, PRC-13, PRC-14 · CPU-07 to CPU-12, CPU-14, CPU-20 to CPU-25, CPU-30, CPU-31 · MEM-01 to MEM-08 · DSK-01 to DSK-03 · NET-01 to NET-05, NET-08, NET-09 · HST-02, HST-03 · ROOT-07.
Exit: tower's memory watermark and metrics-lib's system collector can be built on 0.4; every live reading allocation-free where promised; all container fixtures pass.
- [ ] cgroup v1 and v2 and job-object memory limits with E-1 to E-12; the container CI job (`--memory`, `--cpus`) asserts real limits; nested job objects verified.
- [ ] CPU sampler with E-20 to E-27; counting-allocator tests for every scalar reading.
- [ ] Disk listing with E-36 to E-39; interfaces with E-53 to E-58.
- [ ] Network traffic (`totals`, `traffic`, `Sampler`) and connection counts, with counter resets and wraps tested; `cpu::load()` (D-023, D-025).
- [ ] Memory with E-28 to E-35; attack tests T5, T6, T7, T8.

## v0.5.0 - Feature complete
API: OS-08 to OS-13 · CPU-13, CPU-32, CPU-40 to CPU-44 · MEM-09 · DSK-04, DSK-05 · NET-06 · HST-05 to HST-07 · VRT-01 to VRT-04 · PWR-01 to PWR-03 · PRC-05 · DEV-01 to DEV-07 · ROOT-06.
Exit: every P0 to P2 item is implemented and tested on Linux, macOS, Windows; `docs/SOURCES.md`, `docs/PLATFORM-NOTES.md`, `docs/ARCHITECTURE.md` complete.
- [ ] Form factor, virtualization (E-79 to E-91), power, time zone and locale (E-92 to E-95), hardware model.
- [ ] Device identity with E-69 to E-78; crypt-io wired behind `device-id`; `docs/DEVICE-ID.md`; attack tests T1, T2, T3, T13, T14, T15.
- [ ] Android and iOS build-checked; best-effort paths implemented and documented in `docs/PLATFORM-NOTES.md`.

## v0.6.0 - Optimisation
Exit: every target in `PLAN.md` section 18 measured and published; at least 20% faster than the best competitor for each reading at equal correctness, or a recorded, reviewed reason.
- [ ] Benchmarks for every row against sysinfo, num_cpus, os_info, raw-cpuid, machine-uid; warm and cold; bare metal and container.
- [ ] Footprint measurements (dependencies, build time, binary size, descriptors held).
- [ ] Optimise until every bar is met or the reason it cannot be is recorded.
- [ ] `docs/PERFORMANCE.md` published; criterion baselines stored; the 5% regression gate active.

## v0.7.0 - Hardening
Exit: no open finding; every threat has a passing attack test.
- [ ] Fuzzing: 24 hours per target with no finding open.
- [ ] Attack tests for T1 to T16 complete; `docs/SECURITY.md` with the cost of each defence.
- [ ] Soak: 8 hours, no growth in memory, descriptors, handles, CF references.
- [ ] AddressSanitizer job on Linux and macOS clean; the `hostile` fixture passes every function without panic.

## v0.8.0 - Alpha
Exit: both integrations run on every listed environment; every surprise fixed or recorded in `docs/PLATFORM-NOTES.md`.
- [ ] Integrated in WASP's boot phase 0 and in a Hive DB start-up (guard beside the data directory, cache sizing from `mem::total()`) on Windows 11, WSL 2 Ubuntu, a Linux server, macOS on Apple silicon, a Docker container with limits, and a Kubernetes pod if available.
- [ ] API review against real use; changes made now, not after 1.0.

## v0.9.0 - Beta and release candidates
Exit: two weeks without an API change; every document checked against the build.
- [ ] API frozen; `0.9.x` only fixes.
- [ ] Documentation complete; `docs/STABILITY-1.0.md` drafted with the requirement-to-test map.
- [ ] Release candidates `1.0.0-rc.N` as needed.

## v1.0.0 - Stable
Exit: the audit passes; the definition of done in `DIRECTIVES.md` and the 1.0 checklist below hold.
- [ ] Every P0, P1, and P2 item implemented on Linux, macOS, and Windows, or a documented `Unsupported` listed in `docs/PLATFORM-NOTES.md`.
- [ ] Every FR and NFR has a named, passing test; every edge case E-1 to E-100 has a passing test; every threat T1 to T16 has a passing attack test.
- [ ] Every parser fuzzed 24 hours with no open finding; counting-allocator and 8-hour soak tests pass.
- [ ] Benchmarks published; zero third-party runtime dependencies with default features; `cargo deny` and `cargo audit` clean.
- [ ] `unsafe` only in `src/sys/`, every block documented, layout assertions on every target, Miri clean on the parser layer; no subprocess at run time.
- [ ] CI green on every platform and target, the container job included; every document in `PLAN.md` section 20 complete and matching the build.
- [ ] WASP boot phase 0 and a Hive DB start-up run on sys-lib on every 0.8 environment.
- [ ] Full audit recorded in `docs/STABILITY-1.0.md`; API frozen until 2.0; an MSRV raise is a major change; the device id derivation `sys-lib/device/v1` frozen for 1.x.

## After 1.0 (P3; the design already leaves room)
- 1.1: SNS-01 (Linux temperatures), MEM-10 (huge pages), DSK-06 (disk I/O counters), HST-08 (sessions), PWR-04 (several batteries), NET-07 (interface options), VRT-05 (cloud vendor), per-core CPU usage on Windows with more than 64 CPUs if not verified earlier.
- 1.2: GPU-01 (adapters), PRB-01 (`Probe::with_root`), FreeBSD best effort, a `serde` feature if a caller asks (OD-13).

## Carried items
None.
