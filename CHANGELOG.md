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
