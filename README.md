<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>sys-lib</b>
    <br>
    <sub><sup>SYSTEM FACTS</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/sys-lib"><img alt="Crates.io" src="https://img.shields.io/crates/v/sys-lib"></a>
    <a href="https://crates.io/crates/sys-lib"><img alt="Downloads" src="https://img.shields.io/crates/d/sys-lib?color=%230099ff"></a>
    <a href="https://docs.rs/sys-lib"><img alt="docs.rs" src="https://img.shields.io/docsrs/sys-lib"></a>
    <a href="https://github.com/jamesgober/sys-lib/actions"><img alt="CI" src="https://github.com/jamesgober/sys-lib/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.89%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>sys-lib</strong> answers the questions a program asks about the machine it runs on, with one plain function call each, on Linux, macOS, and Windows: the operating system, the CPU, memory, disks, network interfaces, the host, this process, virtualization and containers, power, and an application-scoped device id. It also carries the boot kit a long-running service needs in its first milliseconds: build info, instance info, and a process guard (single-instance lock, safe file permissions, open-file limit).
    </p>
    <p>
        It is container-aware: inside Docker or Kubernetes, the memory total is the container's limit and the CPU count respects the quota, with the host values one call away. It is honest: a fact that cannot be known is reported as unknown, never guessed. Every reading is a file read or a system call; sys-lib never runs another program, never starts a thread, and has no third-party runtime dependencies.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.89+</strong> (Rust 2024 edition). <code>unsafe</code> is denied at the crate root and allowed only in one OS module per platform.
    </p>
    <blockquote>
        <strong>Status: <code>0.3.0</code>, the boot kit.</strong> OS identity and the distribution, CPU counts and names (with <code>cpu::available()</code> honouring affinity, cpusets, and container CPU quotas), the host name, the user, build info, instance info, and the process guard. Memory, disks, network, and the rest arrive through <code>0.6</code>; the complete approved API, each item with its version, is in <a href="./docs/API.md"><code>docs/API.md</code></a>. Design: <a href="./dev/PLAN.md"><code>dev/PLAN.md</code></a>; schedule: <a href="./dev/ROADMAP.md"><code>dev/ROADMAP.md</code></a>.
    </blockquote>
</div>

<hr>
<br>

## Scope

Static facts read once and cached (OS name and version, CPU model, core counts, CPU features, caches, topology, virtualization, form factor); live readings read fresh on every call with no allocation for single values (memory, swap, uptime, load, disk space, this process's memory and CPU time); caller-owned samplers for values that need two readings (CPU usage, network rates); the boot kit (build info, instance info, process guard); and an opt-in device id derived with a keyed hash, so raw hardware identifiers never leave the library. It does not inspect other processes, change system settings, or run in the background.

## Quick start

Cargo's rename makes the crate `sys` in your code:

```toml
[dependencies]
sys = { package = "sys-lib", version = "0.3" }
```

Tier 1 is one plain call per fact. Static facts are read once and cached, so
every later call costs about a nanosecond:

```rust
println!("{}", sys::os::name());          // "Ubuntu 24.04.1 LTS", "macOS 15.1 Sequoia", "Windows 11 Pro"
println!("{}", sys::os::distro());        // "Ubuntu"; "Windows" and "macOS" elsewhere
println!("{}", sys::cpu::name());         // "AMD Ryzen 9 7950X"
println!("{} cores, {} threads", sys::cpu::physical(), sys::cpu::logical());
println!("{} usable here", sys::cpu::available());   // the container's quota, not the host's count
println!("{}", sys::host::name()?);       // "build-07"
if sys::cpu::is_64bit() && sys::os::is_like("debian") { /* ... */ }
```

The boot kit, for a long-running service's first milliseconds:

```rust
fn main() -> Result<(), sys::Error> {
    // One copy only, private files by default, enough file descriptors.
    // Before any file is opened and before threads start.
    let _guard = sys::process::guard("site")?;

    let build = sys::build_info!();       // needs `sys::build::emit()` in build.rs
    let instance = sys::Instance::start();
    println!("{build}");                  // site 1.4.0 (9db9e36, release, built 2026-10-21T18:02:11Z)
    println!("{instance}");               // 01J9ZQ4M8T6Y2K5V3W1X7R0B9C build-07 pid 4821 up 0s
    println!("{}", build.public());       // site 1.4.0: safe for a public health endpoint

    let workers = sys::cpu::available();  // size the pool to what the cgroup allows
    // ...
    Ok(())
}
```

```rust
// build.rs
fn main() {
    sys::build::emit();
}
```

Tier 2 adds detail behind the same modules (`sys::os::info()`, `sys::os::os_release()`,
`sys::process::Guard::builder(..)`); errors say what failed and what to do,
and a fact the OS does not expose is `Error::Unsupported`, never a guess.

### Planned through 0.6

```rust
println!("{} of {}", sys::mem::used()?, sys::mem::total()?);    // 0.4: "41.2 GiB of 125.6 GiB"
println!("{}", sys::cpu::used()?);                              // 0.4: "12.4%" (blocks for 100 ms)
let t = sys::net::traffic()?;                                   // 0.4: download and upload
let r = sys::readiness::report(sys::readiness::Profile::Database); // 0.6: is this host fit to serve?
```

The complete planned API, with the version each item arrives in, is in [`docs/API.md`](./docs/API.md); where every value comes from on each operating system is in [`docs/SOURCES.md`](./docs/SOURCES.md). See [`dev/PLAN.md`](./dev/PLAN.md) for the full scope, what is deliberately left to other crates, and the decision log.

<hr>
<br>

## Contributing

See <a href="./dev/DIRECTIVES.md"><code>dev/DIRECTIVES.md</code></a> for engineering standards and the definition of done. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b> &mdash; <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober.</strong></sup>
</div>
