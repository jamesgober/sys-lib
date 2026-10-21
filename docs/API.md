# sys-lib &mdash; API Reference

> Complete reference for every public item in `sys-lib`, with examples.
> **Status: scaffold (`0.1.0`).** The public API is written here at `0.2.0` (design lock); see [`../dev/ROADMAP.md`](../dev/ROADMAP.md) and the planned shape in [`../dev/PLAN.md`](../dev/PLAN.md) section 13.

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Table of contents

- [`VERSION`](#version)

## `VERSION`

```rust
pub const VERSION: &str;
```

The crate version, populated by Cargo at build time.

```rust
use sys_lib as sys;

println!("sys-lib {}", sys::VERSION);
```
