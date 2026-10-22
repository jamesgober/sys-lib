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

Review a capture before committing it.

## Captured

| Fixture | Machine | Why |
|---|---|---|
| `wsl2` | WSL 2, Ubuntu 24.04, kernel 6.6 (Microsoft), 32 logical CPUs, cgroup v2, systemd | WSL quirks (E-40 to E-46): no DMI, the WSL kernel release, the utility VM's memory |
| `wsl2-systemd-scope-limited` | the same machine, inside `systemd-run --user --scope -p MemoryMax=256M -p MemoryHigh=200M -p CPUQuota=150%` | a cgroup v2 limit without a cgroup namespace (E-5), nested under ancestors whose limits are `max` (E-2), a fractional CPU quota (E-12) |

## To capture

| Fixture | How |
|---|---|
| `docker-cgroup2-limited` | the `Fixtures` workflow (`.github/workflows/fixtures.yml`, run by hand) on a GitHub runner |
| `minimal-scratch` | the same workflow: a container with the os-release files and machine id removed, emulating a scratch or distroless image |
| `docker-cgroup1-limited` | a host still on cgroup v1 (GitHub runners are cgroup v2 only) |
| `ubuntu-24.04-desktop-amd` | a bare-metal Linux desktop |

The full planned set, with the reason for each machine, is in [`../../dev/PLAN.md`](../../dev/PLAN.md) section 19.2.

Windows fixtures are raw SMBIOS tables and registry captures; macOS fixtures are sysctl and IOKit property dumps. Both arrive with the readings that parse them and run through the same code paths as live data.
