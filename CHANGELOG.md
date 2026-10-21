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

The design lock (0.2.0, in progress). No public API beyond `VERSION` yet.

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
  GitHub runner.

### Changed

- CI's cross-target job compiles the unit tests too, so the layout checks run
  for musl, 32-bit Windows, Android, and iOS.

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

[Unreleased]: https://github.com/jamesgober/sys-lib/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jamesgober/sys-lib/releases/tag/v0.1.0
