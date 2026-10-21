# sys-lib &mdash; Engineering Directives

> Engineering standards and the definition of done for this project. Read alongside `REPS.md` (root, authoritative), [`PLAN.md`](./PLAN.md) (design and decisions), and [`ROADMAP.md`](./ROADMAP.md) (current phase). If anything here conflicts with `REPS.md`, `REPS.md` wins, except where `PLAN.md` records an approved exception (D-008: no async runtime).

---

## 0. Philosophy

This library is built and maintained to a production standard and treated as a flagship piece of work. Plan the full path, then build one verified step at a time. "Good enough" is treated as a defect. It is a `-lib`: a toolbox of functions and plain values, with no background threads, no timers, and no daemons. It is the foundation the service platform and Hive DB stand on in their first milliseconds, so every answer it gives must be true, cheap, and safe to ask from any thread at any time. A security component that cannot name the attack it stops, and prove it with a test, does not ship.

---

## 1. What this is

sys-lib reports facts about the machine and this process (OS, CPU, memory, disks, network, host, virtualization, power, device identity) and provides the boot kit every long-running service needs (build info, instance info, process guard). It reads; it never configures the system. The only process-wide changes it makes (umask, open-file soft limit) happen inside the process guard, which the caller invokes explicitly. See `PLAN.md` sections 1 to 3.

---

## 2. Engineering law (non-negotiable)

- **Honest values.** A fact that cannot be known is `None`, an `Unknown` variant, or `Error::Unsupported`. Never a guess presented as a fact, never a simulated number, never a placeholder in shipped code.
- **Ours, not rented.** No third-party runtime dependency. First-party crates (clock-lib, id-forge, path-lib; crypt-io behind `device-id` only, the audited-cryptography exception) are wired when first used, at published versions; never a `path` dependency to a sibling repository. If something is missing in a first-party crate, it is added there in its own PR. Dev-only tooling (`criterion`, `proptest`, competitor crates for benchmarks) is allowed.
- **No subprocess at run time.** Every reading is a file read or a system call. Only the build helper runs `git` and `$RUSTC`, at build time. CI fails on `Command::new` anywhere else in `src/`.
- **No threads, no timers, no async runtime.** Zero CPU between calls. Static facts in `OnceLock`; live readings read fresh; samplers owned by the caller.
- **Container-aware by default.** Memory totals and CPU availability respect cgroup v1/v2 and Windows job-object limits; host values are separate calls.
- **Unsafe.** `#![deny(unsafe_code)]` at the crate root; `#[allow(unsafe_code)]` only on the `mod sys` item for each platform. Every `unsafe` block has a `// SAFETY:` comment, every `unsafe fn` a `# Safety` section; `unsafe_op_in_unsafe_fn`, `clippy::undocumented_unsafe_blocks`, `clippy::multiple_unsafe_ops_per_block`, `clippy::ptr_as_ptr`, `clippy::cast_ptr_alignment` denied. Every `#[repr(C)]` struct has a size assertion per supported target. Every parser is safe Rust over `&[u8]` and runs under Miri.
- **Hostile-input hardening.** Everything the OS returns is untrusted input (`/proc`, `/sys`, SMBIOS, registry, sysctl, environment). Explicit budgets for bytes and counts, checked before allocation; checked or saturating arithmetic on every value; no recursion; no reachable panics; no quadratic paths.
- **Fail closed where it guards; stay advisory where it detects.** The process guard fails closed: no lock, no start. Detection (`virt`, `form_factor`) reports `Unknown` rather than "no", and the documentation forbids using it for security decisions.
- **Secrets.** Raw hardware identifiers are treated as secrets: read into a fixed buffer, hashed, wiped, never returned, logged, displayed, or put in errors or `Debug` output. Comparisons of identifier hashes run in constant time.
- **Performance.** Peak is the baseline: no allocation on scalar live readings (proven by a counting allocator), one source read per reading, stack buffers and in-place parsing, no locks on hot paths. No "faster" claim without `criterion` numbers; the bar is at least 20% faster than the best Rust competitor at equal or stronger correctness.
- **Cross-platform.** Linux, macOS, and Windows are first-class on x86_64 and aarch64, verified by CI on stable and MSRV 1.89. Every Unix path has a Windows counterpart or a documented, tested reason why not. musl, i686 Windows, Android, and iOS are build-checked.
- **Error handling.** Every fallible path returns `Result`; `Error` is `#[non_exhaustive]` and every variant says what to do.
- **Production-ready.** `#![deny(missing_docs)]`; no `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`unreachable!`/`dbg!`/printing in library code; every public item has rustdoc with a runnable example; every claim in the docs maps to a test.

---

## 3. Definition of done

1. Compiles clean on Linux, macOS, and Windows (x86_64 and aarch64), stable and MSRV 1.89; cross-target checks pass.
2. `fmt`, `clippy -D warnings` (all targets; default, all, and no default features), tests in the same three modes, `cargo doc -D warnings` with default and all features, `cargo package` in a fresh target directory: clean on Windows and in WSL Ubuntu, stable and 1.89.
3. `cargo audit` and `cargo deny check` pass; `cargo tree` shows no third-party runtime dependency with default features.
4. No `unwrap`/`expect`/`todo!`/`dbg!` in shipping code; no subprocess outside the build helper.
5. A Tier-1 API exists and headlines the docs (`use sys_lib as sys;`).
6. Every parser has a fuzz target; every threat in `PLAN.md` section 14 has an attack test that must fail to get through; every edge case in section 19.3 that the change touches has a test.
7. Hot-path changes carry benchmarks; no regression over 5%.
8. `README.md`, `docs/API.md`, and `CHANGELOG.md` updated in the same change; the matching `docs/release/vX.Y.Z.md` written before the tag; every version string bumped.
9. `PLAN.md` decision log updated for every design decision made during the work; at least one new defence idea per version in `PLAN.md` section 22.
10. v1.0.0 only after the full audit (API, docs, tests, benchmarks, security) is recorded in `docs/STABILITY-1.0.md` and the 1.0 checklist in `ROADMAP.md` holds.

---

## 4. Project-specific invariants

- `used <= total`, `free <= total`, and `used + free == total` in every memory and space value; `Percent` always within 0..=100 and never NaN.
- `physical <= logical`, `available <= logical`, `available >= 1`, `online >= 1`.
- A static fact is read at most once per process; a live reading is never cached.
- No public function contains a `#[cfg(target_os)]` branch; it calls `crate::sys`, and each platform provides the function.
- No file descriptor, handle, or OS reference is held after a call returns, except the guard's lock file.
- The guard never deletes its lock file; it refuses symlinked, group- or world-writable, and network lock directories unless explicitly allowed.
- No path is built from the environment or a caller string without validation; no root-path override is read from the environment.
- Raw identifiers never cross the public API; the device id derivation `sys-lib/device/v1` never changes in 1.x.
- Nothing panics on any input the OS can produce.

---

## 5. Writing style

- Plain, specific, human prose in every shipping file. No marketing voice.
- Banned in shipping files: "comprehensive", "robust", "seamless", "leverage", the em-dash character (U+2014) in prose, and emoji.
- Every security measure states its cost (latency, memory); estimates are labelled as estimates, measurements as measured.
- Release notes in the past tense.
- Commits: imperative present tense, descriptive subject line.
