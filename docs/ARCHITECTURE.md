# Architecture

How sys-lib is put together, for someone changing it. The design and its reasons are in [`dev/PLAN.md`](../dev/PLAN.md); the public contract is in [`API.md`](./API.md); where each value comes from is in [`SOURCES.md`](./SOURCES.md).

## Module tree

```
src/
  lib.rs            crate docs, VERSION, re-exports, lint attributes
  error.rs          Error, Result
  units.rs          Bytes, Percent, Rate, Mhz, Units
  pressure.rs       Pressure, the shared stall thresholds
  summary.rs        summary(): the facts for a log line (no identifiers)
  build.rs          build info (feature-free; the helper runs in build.rs)
  instance.rs       instance info                         (feature `instance`)
  os.rs             identity, distribution, form factor
  cpu/              names, counts, frequency, usage, features, topology
  mem.rs            totals, limits, swap, paging, modules
  disk.rs           space, mounts, I/O rates
  net.rs            interfaces, traffic, connections, DNS
  host.rs           name, uptime, time zone, locale, model, firmware, clock, sessions
  process/          this process, how it was started, the guard (feature `guard`)
  virt.rs           hypervisor, container, WSL, Kubernetes, cloud
  power.rs          batteries and mains power
  device/           device id, fingerprint, stability report (feature `device-id`)
  parse/            parsers over &[u8]: safe, bounded, fuzzed, run under Miri
  linux/            the Linux backend's file-tree logic, read through `Root`
  sys/
    mod.rs          the platform boundary, the layout macros
    linux/          the libc calls no file answers
    apple/          sysctl, Mach, libproc, CoreFoundation, IOKit
    windows/        kernel32, advapi32, iphlpapi, and the rest, by raw-dylib
    x86.rs          CPUID, shared by the x86 backends
    pending.rs      live readings, answered `Unsupported`, for targets with no backend
    detection.rs    detection and identity, answered "unknown", for the same targets
    unsupported.rs  everything else on those targets
```

## The platform boundary

A public function never branches on `target_os`. It calls `crate::sys::<function>()`, each platform provides that function, and the public layer does the arithmetic every OS shares: samplers, rates, percentages, limits, invariants. Linux, macOS, and Windows define every item themselves, so a missing item is a build error. Only `unsupported.rs` imports the stubs.

The public layer takes raw values from a platform and enforces the type invariants, so a reading cannot report `used > total` whatever the OS said (threat T5).

Detection that is the same logic on two OSes lives in the public module with its inputs passed in: `virt::hypervisor_from_dmi` takes strings, whether they came from sysfs on Linux or SMBIOS on Windows; `os::form_from_chassis` takes the chassis code.

## Static facts, live readings, samplers

| Kind | Examples | Cost | How |
|---|---|---|---|
| Constant | `os::kind()` | 0 | `const fn` |
| Static fact | `cpu::name()`, `virt::detect()` | about 1 ns after the first call | `OnceLock`, kept for the life of the process |
| Live reading | `mem::used()`, `host::uptime()` | one read, no allocation | a fixed buffer on the stack |
| Sampler | `cpu::Sampler` | two readings | caller-owned, no background thread |

sys-lib never starts a thread and never runs a program.

## Linux: everything through `Root`

The Linux backend's logic lives in `src/linux/` and reads through `Root`, which is either the real `/` or a directory. That is how a captured fixture machine runs on any host, and how the attack tests build hostile trees. There is no unsafe code in this tree. `Root` is never pointed anywhere by the environment (threat T4); a test records every path asked for and fails on one outside the reviewed list.

Files are read through bounded helpers: `with` uses an 8 KiB stack buffer, `for_each_line` streams through the stack with a byte budget, and anything past 1 MiB is `TooLarge`.

## Calling the OS

No `libc` and no `windows-sys` at run time. Each platform has a hand-written `ffi.rs` with only what is called.

- Every `#[repr(C)]` struct has a `layout!` size and alignment assertion, and a `same_layout!` test comparing size, alignment, and every field offset with `libc` or `windows-sys`, which are dev-dependencies only. CI compiles those tests for every target, so a layout that differs from the headers does not build.
- `unsafe` is allowed only inside `sys::{linux,apple,windows}` and `sys::x86`. The crate root denies it. One operation per block, each with a `// SAFETY:` comment.
- Unsafe code moves bytes from the OS into a buffer. Everything that interprets those bytes is safe Rust in `parse/`.
- Size-then-fill calls retry at most three times; lengths are checked before slicing; handles and CF references are released through RAII wrappers.

## Parsers

Each parser takes `&[u8]`, uses no recursion, bounds what it reads and keeps, and never panics on any input. Each has a fuzz target (nightly and on every push) and runs under Miri. A parser fed by firmware or a container is treated as fed by an attacker (threats T6, T15).

SMBIOS serial numbers are reachable only through `parse::smbios::anchors`, which borrows from the table so the caller wipes one buffer. The types the rest of the crate sees never carry one.

## Device identity

`device` reads each anchor into a `Raw`, a fixed buffer wiped on drop, canonicalises it, and hashes it with crypt-io's keyed BLAKE3. On Linux, `Root::read_into` reads into that buffer directly, because the ordinary helpers use a stack buffer that is not wiped. [`DEVICE-ID.md`](./DEVICE-ID.md) has the derivation.

## Testing

Unit tests sit with the code. On top of them: fixture machines (`tests/fixtures/`), a corpus of 64 distributions, live comparison with the OS's own tools on every CI runner (`tests/live.rs`), real limits in the CI `container` job, a counting allocator (`tests/alloc.rs`), a test per threat (`tests/attack_*.rs`, `src/linux/hostile.rs`), and the layout oracles above.
