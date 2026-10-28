# Fixture machines

File trees captured from real machines, run against the Linux backend on any host. The backend reads `/proc`, `/sys`, `/etc`, `/usr/lib`, `/run`, and `/var` through a root prefix, so a fixture is a directory holding exactly the files it reads:

```
tests/fixtures/<name>/
  proc/  sys/  etc/  usr/  run/  var/   the files, at their real paths
  CAPTURED.txt                          when, on which kernel, as which user, how many files
  TOOLS.txt                             what nproc, getconf, free, df, systemd-detect-virt said
```

## Capture

```
tests/fixtures/capture.sh <name> [<output-dir>]
```

Run by hand on a real Linux machine, inside a container, or inside a limited systemd scope. The script:

- copies only the files the Linux backend reads (a fixed list in the script), each capped at 1 MiB, plus the cgroup files of the capturing process's cgroup and every ancestor;
- never copies DMI serial numbers or UUIDs (root-only files), keys, tokens, or Kubernetes service-account files (only that the directory exists);
- replaces every identifier with a fixed fake value: the host name with `fixture-host`, a valid machine id with `5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e`, MAC addresses with `00:00:5e:00:53:NN` (the RFC 7042 documentation range), an ARM serial with zeros, user names in paths with `user`, and the time zone with `Europe/London`;
- records symbolic links as `<path>.link` files holding the target (so fixtures contain no real links and survive any checkout);
- writes `TOOLS.txt` with the output of the OS's own tools, the answers the real machine gave.

Review a capture before committing it. `CAPTURED.txt` records the script version; version 3 (0.4) adds the live-reading files (`/proc/stat`, `/proc/vmstat`, `/proc/diskstats`, `/proc/net/dev`, pressure, and the memory and I/O cgroup files), and every fixture above was recaptured with it.

## Captured

| Fixture | Machine | Why |
|---|---|---|
| `wsl2` | WSL 2, Ubuntu 24.04, kernel 6.6 (Microsoft), 32 logical CPUs, cgroup v2, systemd | WSL quirks (E-40 to E-46): no DMI, the WSL kernel release, the utility VM's memory |
| `wsl2-systemd-scope-limited` | the same machine, inside `systemd-run --user --scope -p MemoryMax=256M -p MemoryHigh=200M -p CPUQuota=150%` | a cgroup v2 limit without a cgroup namespace (E-5), nested under ancestors whose limits are `max` (E-2), a fractional CPU quota (E-12) |
| `docker-cgroup2-limited` | Ubuntu 24.04 container on a GitHub runner (kernel 7.0, Azure), `docker run --memory=256m --memory-reservation=200m --cpus=1.5 --cpuset-cpus=0-1`, captured by the `Fixtures` workflow | a cgroup namespace (path `/`, E-5), `memory.max` 256 MiB, `cpu.max` 1.5 CPUs, a cpuset of 2 CPUs (E-12), an overlay root, `/.dockerenv` |
| `minimal-scratch` | the same runner, a container with the os-release files and machine id removed first | emulates a scratch or distroless image: no os-release, no machine id (E-47, E-69) |
| `docker-cgroup1-limited` | Ubuntu 24.04 container on Ubuntu 20.04 (kernel 5.4, cgroup v1 hybrid layout) in a 4-vCPU KVM virtual machine on a GitHub runner, the same `docker run` limits, captured by `capture_cgroup1_vm.sh` through the `Fixtures` workflow (D-038) | cgroup v1 hierarchies with the container's path as the mount root, `cpu.cfs_quota_us` 1.5 CPUs, a cpuset of 2 of 4 CPUs (E-13) |

## Distribution corpus

`distros/<image>/` holds the release files of 64 container images (os-release, `lsb-release`, and the legacy files), copied out of unstarted containers by `capture_distros.sh` from the list in `distros.list`, and `EXPECTED.txt`: the image and digest, and what a clean shell reads when it sources the image's os-release (`ID`, `NAME`, `PRETTY_NAME`, `VERSION_ID`, `ID_LIKE`, `VERSION_CODENAME`), or `LEGACY=1` for an image with no os-release. The tests in `src/linux/os.rs` check every answer against it, and check the legacy files against os-release where an image has both (D-022, D-038). Run the `Fixtures` workflow with `job=distros` to refresh it.

## To capture

| Fixture | How |
|---|---|
| `ubuntu-24.04-desktop-amd` | a bare-metal Linux desktop (moved to 0.5, D-051) |

The full planned set, with the reason for each machine, is in [`../../dev/PLAN.md`](../../dev/PLAN.md) section 19.2.

Windows fixtures are raw SMBIOS tables and registry captures; macOS fixtures are sysctl and IOKit property dumps. Both arrive with the readings that parse them and run through the same code paths as live data.
