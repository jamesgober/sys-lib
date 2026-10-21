# Fixture machines

Captured file trees from real machines, run against the Linux backend on any host. The backend reads `/proc`, `/sys`, `/etc`, and `/run` through a root prefix, so a fixture is a directory holding the files it reads:

```
tests/fixtures/<name>/
  proc/   sys/   etc/   run/
  EXPECTED.toml        the answers the fixture must produce
```

## Capture

`capture.sh` arrives in `0.2.0`. It is run by hand on a real machine, copies only the files the backend reads (a fixed list, nothing else), and replaces every identifier (machine id, DMI serials and UUIDs, MAC addresses, host name, user names) with a fake value before anything is committed. Fixtures never contain real identifiers.

## Planned set

The full list, with the reason for each machine, is in [`../../dev/PLAN.md`](../../dev/PLAN.md) section 19.2. The first five, in `0.2.0`: `ubuntu-24.04-desktop-amd`, `docker-cgroup2-limited`, `docker-cgroup1-limited`, `wsl2`, `minimal-scratch`.

Windows fixtures are raw SMBIOS tables and registry captures; macOS fixtures are sysctl and IOKit property dumps. Both are parsed by the same code paths as live data, through a test seam.
