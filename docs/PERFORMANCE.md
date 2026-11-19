# Performance

Measured for 0.7.0 on GitHub's runners, the same machines for every change, by the `Bench` workflow (`.github/workflows/bench.yml`) on the release pull request. The targets and the bar are in `dev/PLAN.md` section 18: at least 20% faster than the best competitor for each reading, at equal correctness, or a recorded reason.

## Machines

| Runner | Processor | CPUs | Memory |
|---|---|---|---|
| Linux x86-64 (`ubuntu-24.04`) | AMD EPYC 7763 | 4 | 16 GB |
| Linux Arm64 (`ubuntu-24.04-arm`) | Arm64 (the runner reports no model name) | 4 | 16 GB |
| Docker (`ubuntu:24.04` on the x86-64 runner) | the same | `--cpus=2` | `--memory=1g` |
| Windows (`windows-latest`) | AMD EPYC 7763 | 4 | 16 GB |
| macOS Apple M1 (`macos-latest`) | Apple M1, virtual | 3 | 7 GB |
| macOS Intel (`macos-15-intel`) | Intel Core i7-8700B | 4 | 14 GB |

Rust 1.99.1, release profile, `strip = true`. The competitors: sysinfo 0.40, num_cpus 1.17, os_info 3.15 (default features off), raw-cpuid 11.6, machine-uid 0.6, with blake3 1.8 for the keyed hash a caller of machine-uid would add.

## Method

- **Warm:** criterion 0.8, the median of 100 samples (10 for the CPU samplers, which wait between readings). A static fact after its first call measures the cache.
- **Cold:** the first call in a fresh process, the cost a service pays at start-up: 200 processes per case, each timing one call (`bench/src/bin/cold.rs`). The median is shown; the workflow log has p90 and the minimum.
- **Equal work:** sysinfo skips a CPU refresh within 200 ms of the last one, so its CPU benchmark sleeps past that interval outside the timed part; the competitor's device id derives its per-application key inside the loop, as `device::id` does on every call.
- **Container:** the same binaries inside Docker with cgroup v2 limits, where sys-lib reports the container's memory and CPUs.
- **Regression gate:** on every pull request, the base branch and the change are built apart and run alternately on one runner for 3 rounds; a sys-lib benchmark more than 5% slower in every round fails the change (`bench/gate.sh`, D-105). The base is measured in the same job instead of stored, because a stored baseline from another runner is not comparable.

## Warm

Each cell is sys-lib / the competitor (how many times faster sys-lib is; below 1.0 it is slower).

| Reading | Compared with | Linux x86-64 | Linux Arm64 | Docker 1 GiB, 2 CPUs | Windows | macOS Apple M1 | macOS Intel |
|---|---|---|---|---|---|---|---|
| `os::name()`, warm | sysinfo `System::name` | 6.5 ns / 5.17 µs (795.4x) | 0.9 ns / 2.59 µs (2877.8x) | 3.9 ns / 9.14 µs (2343.6x) | 7.3 ns / 50 ns (6.8x) | 1.2 ns / 1.03 µs (858.3x) | 1.2 ns / 1.72 µs (1433.3x) |
| `cpu::physical()`, warm | num_cpus `get_physical` | 1.9 ns / 70 µs (36847.4x) | 0.8 ns / 12.4 µs (15487.5x) | 1.2 ns / 72.3 µs (60216.7x) | 2.2 ns / 6.5e+02 ns (294.5x) | 1.4 ns / 1.34 µs (957.1x) | 1 ns / 1.78 µs (1780.0x) |
| `cpu::has(Avx2)` | raw-cpuid | 1.9 ns / 3 µs (1578.9x) | n/a | 1.5 ns / 2.34 µs (1560.0x) | 1.9 ns / 13.6 µs (7168.4x) | n/a | 1.2 ns / 20.4 µs (16958.3x) |
| `mem::snapshot()` | sysinfo `refresh_memory` + `cgroup_limits` | 31 µs / 51.2 µs (1.7x) | 13.6 µs / 24.6 µs (1.8x) | 56.2 µs / 74.1 µs (1.3x) | 1.3 µs / 8.99 µs (6.9x) | 4.98 µs / 1.72 µs (0.3x) | 5.76 µs / 2.23 µs (0.4x) |
| `cpu::Sampler::sample()` | sysinfo `refresh_cpu_usage` + global | 13.8 µs / 54.9 µs (4.0x) | 6.21 µs / 38.8 µs (6.3x) | 13.6 µs / 76.8 µs (5.7x) | 6.95 µs / 61.6 µs (8.9x) | 9.8 µs / 74.9 µs (7.6x) | 12.1 µs / 53 µs (4.4x) |
| `process::memory()` | sysinfo refresh of this pid | 5.85 µs / 45.2 µs (7.7x) | 2.65 µs / 20.4 µs (7.7x) | 6 µs / 45.5 µs (7.6x) | 3.1e+02 ns / 1.6 ms (5124.9x) | 1.35 µs / 30.9 µs (22.9x) | 1.26 µs / 42.2 µs (33.5x) |
| `disk::list()` | sysinfo `Disks::new_with_refreshed_list` | 112 µs / 133 µs (1.2x) | 48.7 µs / 61.7 µs (1.3x) | 76.6 µs / 135 µs (1.8x) | 173 µs / 233 µs (1.3x) | 170 µs / 21.4 ms (126.0x) | 141 µs / 13.2 ms (93.8x) |
| `disk::IoSampler::sample()` | sysinfo `Disks::refresh` (usage) | 35.8 µs / 90.6 µs (2.5x) | 14.3 µs / 41.7 µs (2.9x) | 36.5 µs / 109 µs (3.0x) | 18.5 µs / 194 µs (10.5x) | 239 µs / 13.1 ms (54.8x) | 50.2 µs / 11.1 ms (221.4x) |
| `net::interfaces()` | sysinfo `Networks::new_with_refreshed_list` | 1.09 ms / 1.17 ms (1.1x) | 238 µs / 302 µs (1.3x) | 129 µs / 192 µs (1.5x) | 1.4 ms / 2.06 ms (1.5x) | 73.8 µs / 316 µs (4.3x) | 72 µs / 262 µs (3.6x) |
| `host::uptime()` | sysinfo `System::uptime` | 29 ns / 6.26 µs (213.7x) | 35 ns / 3.09 µs (88.5x) | 22 ns / 6.96 µs (313.5x) | 3.8 ns / 8.2 ns (2.2x) | 40 ns / 6.3e+02 ns (15.8x) | 36 ns / 1.02 µs (28.5x) |
| `host::processes()` | sysinfo process refresh, count | 103 µs / 6.02 ms (58.2x) | 43.7 µs / 3 ms (68.7x) | 29.9 µs / 51.1 µs (1.7x) | 8.95 µs / 1.82 ms (203.4x) | 9.28 µs / 5.32 ms (573.3x) | 10.4 µs / 5.44 ms (522.6x) |
| `device::id(app)` | machine-uid `get` + keyed BLAKE3 | 5.09 µs / 5.43 µs (1.1x) | 2.82 µs / 2.87 µs (1.0x) | n/a | 9.54 µs / 8.03 µs (0.8x) | 11.5 µs / 1.98 µs (0.2x) | 31.7 µs / 3.3 µs (0.1x) |

| Reports, warm | Linux x86-64 | Linux Arm64 | Docker 1 GiB, 2 CPUs | Windows | macOS Apple M1 | macOS Intel |
|---|---|---|---|---|---|---|
| `summary()` | 102 µs | 46.4 µs | 101 µs | 3.93 µs | 6.02 µs | 12.7 µs |
| `readiness::report(Database)` | 178 µs | 81.6 µs | 164 µs | 642 µs | 13.2 µs | 25.6 µs |
| `security::posture()` | 644 µs | 198 µs | 218 µs | 75.8 µs | 1.6e+02 ns | 3.2e+02 ns |

## Cold

| First call, fresh process | Compared with | Linux x86-64 | Linux Arm64 | Docker 1 GiB, 2 CPUs | Windows | macOS Apple M1 | macOS Intel |
|---|---|---|---|---|---|---|---|
| `os::name()` | os_info get | 28.2 µs / 7.67 ms (271.9x) | 34.6 µs / 6.48 ms (187.2x) | 29.2 µs / 541 µs (18.5x) | 47.1 µs / 37.1 µs (0.8x) | 20.6 µs / 126 µs (6.1x) | 31.7 µs / 175 µs (5.5x) |
| `os::name()` | sysinfo System::name | 28.2 µs / 13.8 µs (0.5x) | 34.6 µs / 15.9 µs (0.5x) | 29.2 µs / 16.7 µs (0.6x) | 47.1 µs / 3.9 µs (0.1x) | 20.6 µs / 5.1 µs (0.2x) | 31.7 µs / 5.6 µs (0.2x) |
| `cpu::logical()` | num_cpus get | 23.2 µs / 94.1 µs (4.1x) | 32.1 µs / 70.7 µs (2.2x) | 21.4 µs / 86.6 µs (4.0x) | 7 µs / 4.5 µs (0.6x) | 9.3 µs / 2.5 µs (0.3x) | 11.1 µs / 1.5 µs (0.1x) |
| `cpu::physical()` | num_cpus get_physical | 85.3 µs / 92.3 µs (1.1x) | 96.7 µs / 86.8 µs (0.9x) | 73 µs / 87.7 µs (1.2x) | 13.8 µs / 6.7 µs (0.5x) | 13.4 µs / 7.7 µs (0.6x) | 18.1 µs / 6.7 µs (0.4x) |
| `mem::snapshot()` | sysinfo memory + cgroup_limits | 135 µs / 100 µs (0.7x) | 109 µs / 73.8 µs (0.7x) | 150 µs / 124 µs (0.8x) | 8.6 µs / 53.3 µs (6.2x) | 20.8 µs / 54.5 µs (2.6x) | 21.1 µs / 53.3 µs (2.5x) |
| `device::id(app)` | machine-uid get + keyed blake3 | 21 µs / 18.6 µs (0.9x) | 20.1 µs / 11.9 µs (0.6x) | 19.6 µs / 17.3 µs (0.9x) | 61.8 µs / 54.9 µs (0.9x) | 105 µs / 26.7 µs (0.3x) | 204 µs / 73.3 µs (0.4x) |
| `summary()` | | 568 µs | 362 µs | 524 µs | 167 µs | 66.9 µs | 112 µs |

## Footprint

| Measure | Target | Measured |
|---|---|---|
| Third-party runtime dependencies | 0 | 0 by default (clock-lib, id-forge, path-lib, and mod-rand through them, all first-party); `device-id` brings crypt-io's BLAKE3 stack; `--no-default-features`: none |
| Added to a stripped binary using every module | under 300 KiB [est.] | 337 to 665 KiB, below |
| Clean release build of the crate | under 2 s [est.] | 17 to 27 s on a shared 32-CPU machine (not measured on the runners) |
| Descriptors held between calls | 0 (the guard 1) | 0, checked by `tests/descriptors.rs` |
| Allocations per scalar live reading | 0 | 0, checked by `tests/alloc.rs` |
| Threads started | 0 | 0 |

| Runner | Added | Baseline binary | With every module |
|---|---|---|---|
| Linux x86-64 | 665 KiB | 328 KiB | 994 KiB |
| Linux Arm64 | 512 KiB | 324 KiB | 837 KiB |
| Windows | 527 KiB | 127 KiB | 654 KiB |
| macOS Apple M1 | 337 KiB | 334 KiB | 671 KiB |
| macOS Intel | 403 KiB | 316 KiB | 719 KiB |

## Below the bar

Every reading not at least 20% ahead, with the reason as far as it is known. Those marked **open** are carried into the pre-1.0 pass; none is final.

| Reading | Where | Result | Reason |
|---|---|---|---|
| `mem::snapshot()` warm | macOS | 0.3x, 0.4x | **Open.** sys-lib also reads the swap usage and the memory pressure level, which sysinfo's memory refresh leaves out; not yet confirmed as the whole gap. |
| `disk::list()` warm | Linux x86-64 | 1.2x | **Open.** Each mount's kind (SSD, HDD, removable) comes from two sysfs files; on Arm64 and in Docker the same code is 1.3x and 1.8x ahead. |
| `net::interfaces()` warm | Linux x86-64 | 1.1x | **Open.** sys-lib fills each interface's kind, speed, and MTU from sysfs, which sysinfo does not report, so the data is not equal; on Arm64 it is 1.3x ahead. |
| `device::id(app)` warm and cold | Linux, Windows; macOS | 0.8x to 1.1x; 0.1x to 0.4x | Identifiers are never cached (threat T1), so both sides read the same source on every call and the hash is the same. **Open** on macOS, where sys-lib's IOKit lookup costs 10 to 30 µs against 2 to 3 µs. |
| `os::name()` cold against os_info | Windows | 0.8x | **Open.** The first call fills every OS fact (name, version, build, edition) from the registry in one cache; os_info reads fewer values. |
| `os::name()` cold against sysinfo | everywhere | 0.1x to 0.6x | Not the plan's comparator: sysinfo's `System::name` reads only the distribution name, while the first `os::name()` fills every OS fact. Against os_info, the comparator in the plan, it is 5x to 270x ahead outside Windows. |
| `cpu::logical()`, `cpu::physical()` cold | Windows, macOS | 0.1x to 0.6x (a few µs) | **Open.** Both sides make one or two system calls; the difference is a few microseconds, likely the first execution of a larger binary's code. On Linux, the target platform for this row, `cpu::logical()` is 2x to 4x ahead. |
| `cpu::physical()` cold | Linux | 0.9x to 1.2x | **Open.** sys-lib reads the topology files (one list per package), num_cpus parses `/proc/cpuinfo`; on 4-CPU runners the walk is as costly as the parse. |
| `mem::snapshot()` cold | Linux, Docker | 0.7x, 0.8x | The first call finds the process's cgroup and caches the paths, which makes every later call cheaper (1.3x to 1.8x ahead warm, with the container limit). |
| Binary size | every platform | 337 to 665 KiB | The estimate counted sys-lib's code only; the measurement includes the parts of std that every module brings (formatting, filesystem, errors). D-103 removed 26 KiB of duplicated generic code. |
| Build time | | 17 to 27 s | The estimate was for a crate a tenth of this size (47 000 lines today). |

## Reproducing

```bash
cd bench
cargo bench --bench readings          # warm
cargo run --release --bin cold 200    # cold
bash gate.sh ../base .. 3             # the regression gate against another checkout
```
