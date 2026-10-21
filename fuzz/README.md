# Fuzzing

cargo-fuzz targets for every parser sys-lib runs over operating-system data (`/proc`, `/sys`, SMBIOS tables, registry values, the guard's pid file, stored fingerprints). Targets are added with the parsers they cover, from `0.3.0` on; the list is in [`../dev/PLAN.md`](../dev/PLAN.md) section 19.4.

Each target asserts: no panic, no allocation beyond its budget, and no result that violates the type's invariants.

## Running

Requires nightly and `cargo install cargo-fuzz`. Keep the working corpus **outside** the repository; the first directory is where new inputs are written, the second holds the seeds:

```
cargo +nightly fuzz run <target> ~/fuzz-corpus/sys-lib/<target> tests/fixtures
```

Nothing under `fuzz/corpus/` or `fuzz/artifacts/` is committed. A crash found by fuzzing is fixed test-first: the input becomes a unit test next to the parser before the fix lands.
