<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br><b>sys-lib</b><br>
    <sub><sup>SOURCES</sup></sub>
</h1>
<div align="center">
    <sup>
        <a href="../README.md" title="Project Home"><b>HOME</b></a>
        <span>&nbsp;│&nbsp;</span>
        <a href="./API.md" title="API Reference"><b>API</b></a>
        <span>&nbsp;│&nbsp;</span>
        <span>SOURCES</span>
        <span>&nbsp;│&nbsp;</span>
        <a href="../dev/PLAN.md" title="Plan"><b>PLAN</b></a>
    </sup>
</div>
<br>

Where every value comes from. For each reading and each operating system: the file, system call, or API sys-lib reads, the unit it arrives in, the privilege it needs, and the quirks that shaped the code. Every reading is a file read or a system call; sys-lib never runs another program to get a value.

First version, written at the `0.2.0` design lock; it grows with each version and is complete at `0.5.0`. A source marked [verify] is planned but not yet confirmed on a real machine or in CI; the version that ships the reading confirms it or changes it, and this file changes with it.

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Contents

- [General rules](#general-rules)
- [OS identity](#os-identity)
- [Form factor](#form-factor)
- [CPU](#cpu)
- [CPU usage](#cpu-usage)
- [Memory](#memory)
- [Container and job limits](#container-and-job-limits)
- [Disks](#disks)
- [Network](#network)
- [Host](#host)
- [This process](#this-process)
- [Process guard](#process-guard)
- [Virtualization](#virtualization)
- [Power](#power)
- [Device identity anchors](#device-identity-anchors)
- [Build info](#build-info)
- [Android and iOS](#android-and-ios)

## General rules

- **Linux: files first.** Where `/proc` or `/sys` has the same data as a system call, the file wins: no struct layout to get wrong across glibc and musl. `uname` becomes `/proc/sys/kernel/{ostype,osrelease,version}`, `gethostname` becomes `/proc/sys/kernel/hostname`, `sched_getaffinity` becomes `Cpus_allowed_list` in `/proc/self/status`. A system call wins where it is clearly cheaper (`clock_gettime(CLOCK_BOOTTIME)` for uptime).
- **Bounded reads.** Every file is read with one `open`, reads to EOF, and one `close`, into a stack buffer (4 KiB) or a heap buffer bounded at 1 MiB. Larger is `TooLarge`. Only `/proc/self/...`, `/proc/1/...` (the namespace's init), `/proc/sys`, `/proc/stat`, `/proc/meminfo`, `/proc/loadavg`, `/proc/pressure`, `/sys`, and fixed files under `/etc` and `/run` are opened; never a path from the environment.
- **Windows: `raw-dylib` imports and wide strings with explicit lengths.** Functions come from kernel32, advapi32, iphlpapi, powrprof, and ntdll (`RtlGetVersion`, `NtQuerySystemInformation`), imported without import libraries. "Ask for the size, allocate, ask again" calls retry at most 3 times. Volume calls run with critical-error dialogs disabled.
- **macOS: every CoreFoundation and IOKit object is released** by an RAII wrapper; Mach memory is returned with `vm_deallocate`.
- **Units.** Linux `/proc/meminfo` is in KiB (`kB` there means 1024), `/proc/stat` in `USER_HZ` ticks (from `sysconf(_SC_CLK_TCK)`, normally 100), cpufreq in kHz, hwmon in millidegrees; Windows `FILETIME` in 100 ns intervals; Mach CPU ticks in `CLK_TCK` units.

## OS identity

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| `os::name()` | `PRETTY_NAME` in `/etc/os-release`, then `/usr/lib/os-release` (os-release(5) quoting and escapes); fallback `"Linux"` | `"macOS " + kern.osproductversion + codename` (codename table: 11 Big Sur, 12 Monterey, 13 Ventura, 14 Sonoma, 15 Sequoia, 26 Tahoe) | product from the build number (22000 and later: Windows 11; Server: `wProductType` from `RtlGetVersion`, 17763 2019, 20348 2022, 26100 2025), edition from `EditionID` under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` |
| `os::version()` | `VERSION_ID` (dotted numbers; rolling releases have none) | `sysctlbyname("kern.osproductversion")` | `RtlGetVersion` (ntdll): major, minor, build; `UBR` from the same registry key as the revision |
| `os::kernel()` | `/proc/sys/kernel/osrelease` | `kern.osrelease` (Darwin) | `"10.0.<build>.<UBR>"` |
| `os::distro()`, `distro_id()` | os-release `NAME` and `ID` from `/etc/os-release`, then `/usr/lib/os-release`; then the legacy files `/etc/lsb-release` (`DISTRIB_ID`, `DISTRIB_DESCRIPTION`), `/etc/redhat-release` and its CentOS, Fedora, Rocky, AlmaLinux variants, `/etc/SuSE-release`, `/etc/alpine-release`, `/etc/arch-release`, `/etc/gentoo-release`, `/etc/slackware-version`, `/etc/debian_version`; then `"Linux"` / `linux` | `"macOS"` / `macos` (iOS: `"iOS"` / `ios`) | `"Windows"` / `windows` |
| `os::os_release()`, `is_like()` | os-release `ID`, `ID_LIKE`, `NAME`, `PRETTY_NAME`, `VERSION`, `VERSION_ID`, `VERSION_CODENAME`, `VARIANT_ID`, `BUILD_ID` | `None`; `is_like` matches `macos` | `None`; `is_like` matches `windows` |
| `os::edition()` | os-release `VARIANT_ID` | `None` | `EditionID` ("Professional", "ServerDatacenter") |
| `os::is_64bit()` | compile target, or the kernel's machine (`/proc/sys/kernel/arch` [verify]) for a 32-bit process | `hw.optional.x86_64` / `hw.optional.arm64` | `IsWow64Process2` native machine |

Quirks:

- Windows' registry `ProductName` says "Windows 10" on Windows 11; the build number is the truth.
- `GetVersionEx` reports the version the executable's manifest declares, not the real one; it is never used.
- macOS `SystemVersion.plist` reports 10.16 to processes with `SYSTEM_VERSION_COMPAT=1`; the sysctl is unaffected [verify on CI].
- `/etc/os-release` is often a symlink to `/usr/lib/os-release`; both are fixed paths.

## Form factor

First answer wins, in this order.

| OS | 1 | 2 | 3 | 4 | Fallback |
|---|---|---|---|---|---|
| Linux | `CHASSIS=` in `/etc/machine-info` (administrator override) | `/sys/class/dmi/id/chassis_type` (mode 0444) | `/sys/firmware/acpi/pm_profile` | `/proc/device-tree/chassis-type` | a system battery: `Laptop` (low confidence); else `Unknown` |
| Windows | SMBIOS type 3 chassis type, from `GetSystemFirmwareTable('RSMB', 0)` | `PowerDeterminePlatformRoleEx(POWER_PLATFORM_ROLE_V2)` (powrprof) | `GetSystemPowerStatus` battery flag | | `Unknown` |
| macOS | an internal battery (IOKit power sources): `Laptop` | `hw.model`: `MacBook*` laptop; `iMac*`, `Macmini*`, `MacPro*`, `Mac*` without a battery desktop | `VirtualMac*`: `Unknown` | | `Unknown` |
| Android | `ro.build.characteristics` (`tablet`, `watch`, `tv`) via `__system_property_get` | | | | `Phone` (medium confidence) |
| iOS | `uname().machine` prefix (`iPhone`, `iPad`, `Watch`, `AppleTV`); `SIMULATOR_MODEL_IDENTIFIER` on the simulator | | | | `Unknown` |

SMBIOS chassis types (SMBIOS 3.5.0, 7.4.1, as systemd maps them, plus servers): 0x03, 0x04, 0x06, 0x07, 0x0D, 0x23, 0x24 Desktop; 0x08, 0x09, 0x0A, 0x0E Laptop; 0x0B Handheld; 0x11, 0x17, 0x19, 0x1C, 0x1D Server; 0x1E Tablet; 0x1F, 0x20 Convertible; 0x21, 0x22 Embedded; anything else goes to the next source. ACPI PM profiles: 1, 3, 6 Desktop; 2 Laptop; 4, 5, 7 Server; 8 Tablet. Windows platform roles: Slate Tablet, Mobile Laptop, Desktop, Workstation, and AppliancePC Desktop, the server roles Server.

## CPU

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| `cpu::logical()` | CPUs in `/sys/devices/system/cpu/present` (not `possible`, which VMs inflate); fallback `sysconf(_SC_NPROCESSORS_CONF)` | `hw.logicalcpu_max` | `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` |
| `cpu::physical()` | unique `(physical_package_id, core_id)` pairs in `/sys/devices/system/cpu/cpu*/topology/` | `hw.physicalcpu_max` | `GetLogicalProcessorInformationEx(RelationProcessorCore)` entries |
| `cpu::packages()` | unique `physical_package_id` | `hw.packages` | `RelationProcessorPackage` entries |
| `cpu::available()` | `Cpus_allowed_list` in `/proc/self/status`, intersected with `cpuset.cpus.effective` (v2) or `cpuset.cpus` (v1), then the CPU quota (see [limits](#container-and-job-limits)) | `hw.activecpu` (no affinity API) | `GetProcessGroupAffinity` + per-group masks (Windows 11 and Server 2022 default to every group); job object CPU rate |
| `cpu::online()` | `/sys/devices/system/cpu/online` | `hw.activecpu` | `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` |
| `cpu::name()` | x86: `model name` in `/proc/cpuinfo`; arm64: `midr_el1` under `/sys/devices/system/cpu/cpu*/regs/identification/` mapped through a part-number table, or the device tree `compatible` | `machdep.cpu.brand_string` ("Apple M1" on Apple silicon) | `ProcessorNameString` under `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0` |
| `cpu::vendor()` | CPUID vendor string (x86) or MIDR implementer (arm64) | `machdep.cpu.vendor`, or Apple | CPUID, or the registry `VendorIdentifier` |
| `cpu::frequency()` | `cpuinfo_max_freq`, `base_frequency` (kHz) in `/sys/devices/system/cpu/cpu0/cpufreq/` | `hw.cpufrequency`, `hw.cpufrequency_max` (Intel Macs); Apple silicon: `None` | `CallNtPowerInformation(ProcessorInformation)` `MaxMhz`; registry `~MHz` as base |
| `cpu::current_frequency()` | mean `scaling_cur_freq` over online CPUs | `None` | mean `CurrentMhz` from `CallNtPowerInformation` |
| `cpu::native_arch()` | the compile target (qemu-user is not detected) | `sysctl.proc_translated` | `IsWow64Process2` native machine |
| `cpu::load()` | `/proc/loadavg` | `getloadavg` | `Ok(None)`: Windows keeps no load average |
| `cpu::is_64bit()` | a 64-bit build implies a 64-bit processor (32-bit Linux has no implementation yet) | every supported Mac is 64-bit | `IsWow64Process2` native machine for a 32-bit process |
| `cpu::has()`, `features()` | `is_x86_feature_detected!` / `is_aarch64_feature_detected!` (std; checks OS support such as XCR0 for AVX state) | same | same |
| `cpu::topology()` | `topology/{core_id,physical_package_id,core_cpus_list}`; Intel hybrid: `/sys/devices/cpu_core/cpus`, `/sys/devices/cpu_atom/cpus`; ARM: `cpu_capacity` | `hw.nperflevels`, `hw.perflevel0.physicalcpu` (performance), `hw.perflevel1.physicalcpu` (efficiency) | `RelationProcessorCore` with `EfficiencyClass` |
| `cpu::caches()` | `cache/index*/{level,type,size,coherency_line_size,shared_cpu_list}` per CPU | `hw.l1dcachesize`, `hw.l1icachesize`, `hw.l2cachesize`, `hw.l3cachesize`, `hw.perflevelN.l2cachesize`, `hw.cachelinesize` | `RelationCache` |
| `cpu::numa_nodes()` | `/sys/devices/system/node/node*/{cpulist,meminfo}` | one node with every CPU | `RelationNumaNodeEx`, `GetNumaAvailableMemoryNodeEx` |

Quirks:

- x86 CPUID: `core::arch::x86_64::__cpuid` is an `unsafe fn` at Rust 1.89 and safe in later std; the call compiles cleanly on both.
- Windows with more than 64 logical processors: processor groups; every count spans all groups.
- CPU lists (`0-3,8-11,64`) are parsed without a fixed-size mask, so hosts with more than 1 024 CPUs work.

## CPU usage

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| total | line `cpu` of `/proc/stat` (only the first line is read) | `host_statistics(HOST_CPU_LOAD_INFO)` | `GetSystemTimes` (idle, kernel, user) |
| per core | lines `cpuN` of `/proc/stat` | `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` | `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` |
| states | user, nice, system, idle, iowait, irq, softirq, steal (guest and guest_nice are already in user and nice) | user, system, idle, nice | idle, kernel (includes idle), user, dpc, interrupt |

Busy on Windows is `kernel + user - idle`, because kernel time includes idle; a reading where idle exceeds kernel saturates. Per-core values on Windows with more than 64 logical processors need the per-group variant of the call [verify]; until then they are `Unsupported` while the total stays correct.

## Memory

| Value | Linux | macOS | Windows |
|---|---|---|---|
| host total | `MemTotal` in `/proc/meminfo` | `hw.memsize` | `ullTotalPhys` (`GlobalMemoryStatusEx`) |
| host free (available) | `MemAvailable` (kernel 3.14 and later; older: `MemFree + Buffers + Cached`) | `(free_count + inactive_count) × page size` from `host_statistics64(HOST_VM_INFO64)` [verify against `vm_stat` and Activity Monitor] | `ullAvailPhys` |
| unused | `MemFree` | `free_count - speculative_count` | not exposed separately |
| cached | `Cached + SReclaimable - Shmem` | `external_page_count × page size` | `CacheBytes` (`GetPerformanceInfo`) |
| swap | `SwapTotal`, `SwapFree` | `vm.swapusage` (`xsw_usage`) | `CommitLimit - PhysicalTotal` (`GetPerformanceInfo`), an approximation: the commit limit covers every pagefile |
| page size | `sysconf(_SC_PAGESIZE)` | `vm.pagesize` | `GetSystemInfo().dwPageSize` |
| pressure | PSI `/proc/pressure/memory` (kernel 4.20 and later) | `kern.memorystatus_vm_pressure_level` | `QueryMemoryResourceNotification` on a low-memory notification handle |

Quirks: zram devices count as swap, zswap is invisible to these counters; ZFS ARC counts as used on Linux (the kernel does not count it as reclaimable); WSL 2 reports its utility VM's memory, which is correct for processes inside WSL.

## Container and job limits

**Finding the cgroup (Linux):** `/proc/self/cgroup` (`0::/<path>` is v2; `N:<controllers>:/<path>` is v1), the mount point from `/proc/self/mountinfo` (type `cgroup2`, or `cgroup` with the controller in the super options; `/sys/fs/cgroup` is never assumed), then `<mount>/<path>`, validated (no `..`, no NUL, under 4 096 bytes, at most 64 ancestor levels).

| Limit | cgroup v2 | cgroup v1 | Windows |
|---|---|---|---|
| memory hard | minimum `memory.max` over the cgroup and its ancestors (`max` is none) | `hierarchical_memory_limit` in `memory.stat` (2^63 minus one page and anything at or above host RAM mean none) | `QueryInformationJobObject(NULL, JobObjectExtendedLimitInformation)`: `ProcessMemoryLimit`, `JobMemoryLimit` when their flags are set |
| memory soft | `memory.high` | `memory.soft_limit_in_bytes` | none |
| swap | `memory.swap.max` | `memory.memsw.limit_in_bytes` minus the memory limit [verify] | none |
| memory used | `memory.current - inactive_file` (`memory.stat`) | `memory.usage_in_bytes - total_inactive_file` | `PeakProcessMemoryUsed`/job accounting [verify] |
| CPU quota | `cpu.max` (`max 100000` or `150000 100000`), minimum over ancestors | `cpu.cfs_quota_us` and `cpu.cfs_period_us` (`-1` is none) | `JobObjectCpuRateControlInformation` |
| CPU set | `cpuset.cpus.effective` | `cpuset.cpus` | job group affinity [verify] |

LXCFS: a `fuse.lxcfs` mount over `/proc/meminfo`, `/proc/stat`, or `/proc/cpuinfo` (seen in mountinfo) already shows container values; the cgroup limit is not applied a second time. Nested job objects: whether the query returns the effective minimum of nested jobs is [verify], settled by a nested-job test before 0.4. macOS has no equivalent: `mem::limit()` is `Ok(None)`.

## Disks

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| `disk::space(path)` | `statvfs`: `f_blocks`, `f_bfree`, `f_bavail` × `f_frsize` | `statfs` | `GetDiskFreeSpaceExW` |
| `disk::list()` | `/proc/self/mountinfo` (octal escapes `\040`, `\011`, `\012`, `\134` decoded), then `statvfs` per mount | `getfsstat(MNT_NOWAIT)` | `GetLogicalDriveStringsW`, `GetDriveTypeW`, `GetVolumeInformationW`, `GetDiskFreeSpaceExW`; mounted folders via `FindFirstVolumeW` and `GetVolumePathNamesForVolumeNameW` |
| `DiskKind` | `/sys/block/<dev>/queue/rotational` (a partition resolved to its parent device), `removable` | IOKit device characteristics `Medium Type` [verify] | `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceSeekPenaltyProperty` |
| `disk::of(path)` | the longest matching mount point after canonicalising `path`, checked by device number | `statfs` `f_mntonname` | `GetVolumePathNameW` |

Network filesystems (`nfs`, `nfs4`, `cifs`, `smb3`, `smbfs`, `fuse.sshfs`, `9p`, `afpfs`, `DRIVE_REMOTE` on Windows) are skipped by default because `statvfs` on a dead hard mount can hang. Pseudo filesystems (`proc`, `sysfs`, `cgroup`, `cgroup2`, `tmpfs`, `devtmpfs`, `squashfs`, `overlay` when not the root, `autofs`, `debugfs`, `tracefs`, `securityfs`, `pstore`, `bpf`, `mqueue`, `hugetlbfs`, `configfs`, `fusectl`, `binfmt_misc`) are skipped by default. Windows volume calls run under `SetThreadErrorMode(SEM_FAILCRITICALERRORS)`. APFS volumes sharing a container are reported once.

## Network

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| addresses and prefixes | `getifaddrs` | `getifaddrs` | `GetAdaptersAddresses(AF_UNSPEC, GAA_FLAG_INCLUDE_PREFIX)` |
| MAC | `/sys/class/net/<if>/address` | `AF_LINK` entries (`sockaddr_dl`) of `getifaddrs` | `PhysicalAddress` |
| MAC kind | `addr_assign_type`: 0 permanent, 1 random; 2 (stolen) and 3 (set) unknown | locally administered bit | locally administered bit |
| state | `operstate`, `flags` | `if_flags` | `OperStatus` |
| kind | `type` (ARPHRD), `wireless/` or `phy80211/` present for Wi-Fi | `if_data.ifi_type` | `IfType` |
| physical | `/sys/class/net/<if>/device` exists | built-in interfaces from IOKit [verify] | `IfType` Ethernet or Wi-Fi and not virtual (`ConnectionType`, the tunnel types) |
| MTU, speed | `mtu`, `speed` (Mb/s; `-1` or an error means unknown) | `if_data.ifi_mtu`, `ifi_baudrate` | `Mtu`, `TransmitLinkSpeed` (maximum values mean unknown) |
| `net::totals()`, `counters()`, `traffic()`, `Sampler` | `/proc/net/dev` (one read; loopback left out of totals) | `sysctl` `NET_RT_IFLIST2` (`if_data64`; the 32-bit counters wrap) | `GetIfTable2` (`MIB_IF_ROW2`, 64-bit; software loopback left out) |
| `net::connections()` | `/proc/net/snmp` `Tcp:` `CurrEstab`; `/proc/net/sockstat` and `/proc/net/sockstat6` `TCP inuse`, `tw`, `UDP inuse` | `net.inet.tcp.pcbcount`, `net.inet.udp.pcbcount` [verify]; established and time-wait `None` | `GetTcpStatisticsEx` (`dwCurrEstab`, `dwNumConns`) and `GetUdpStatisticsEx` (`dwNumAddrs`), IPv4 and IPv6 |
| default route | `/proc/net/route` (destination 0) and `/proc/net/ipv6_route` | routing socket dump via `sysctl(NET_RT_DUMP)` [verify] | `GetBestInterfaceEx` towards 0.0.0.0 |

Interface names from the OS are validated before they are used in a `/sys` path: no `/`, no `..`, at most 15 bytes on Linux (`IFNAMSIZ` 16 with the NUL).

## Host

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| `host::name()` | `/proc/sys/kernel/hostname` | `gethostname` | `GetComputerNameExW(ComputerNameDnsHostname)` |
| `host::uptime()` | `clock_gettime(CLOCK_BOOTTIME)` (includes suspend); fallback `/proc/uptime` | now minus `kern.boottime` | `GetTickCount64` |
| `host::boot_time()` | `btime` in `/proc/stat` | `kern.boottime` | now minus `GetTickCount64` |
| `host::timezone()` | `TZ` when it is a valid name; else the `/etc/localtime` link target after `zoneinfo/`; else `/etc/timezone`; offset and abbreviation from `localtime_r` (`tm_gmtoff`, `tm_zone`) | same; link target under `/var/db/timezone/zoneinfo/` | `GetDynamicTimeZoneInformation` (`TimeZoneKeyName`, `Bias`, daylight bias); IANA name from an embedded copy of CLDR `windowsZones.xml` |
| `host::locale()` | `LC_ALL`, then `LC_MESSAGES`, then `LANG`, normalised (`en_US.UTF-8` to `en-US`) | `CFLocaleCopyCurrent` identifier (GUI applications have no `LANG`) | `GetUserDefaultLocaleName` |
| `host::model()` | `/sys/class/dmi/id/{sys_vendor,product_name,product_family}` (0444); `/proc/device-tree/model` on ARM boards | `hw.model` | SMBIOS type 1 manufacturer, product, family |

## This process

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| `id()` | `std::process::id()` | same | same |
| `parent_id()` | `PPid` in `/proc/self/status` | `getppid` | `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)`, the entry for this pid (`th32ParentProcessID`), cached |
| `memory()` | `/proc/self/statm` field 2 × page size | `task_info(MACH_TASK_BASIC_INFO).resident_size` | `K32GetProcessMemoryInfo().WorkingSetSize` |
| `peak_memory()` | `VmHWM` in `/proc/self/status` | `getrusage(RUSAGE_SELF).ru_maxrss` (bytes) | `PeakWorkingSetSize` |
| `memory_detail()` | `VmRSS`, `VmSize`, `RssAnon`, `RssFile + RssShmem`, `VmSwap` | `task_info` + `proc_pid_rusage` (`ri_phys_footprint` as private) | `PROCESS_MEMORY_COUNTERS_EX` (`PrivateUsage`) |
| `cpu_time()` | `getrusage(RUSAGE_SELF)` | `getrusage` | `GetProcessTimes` |
| `threads()` | `Threads` in `/proc/self/status` | `proc_pidinfo(PROC_PIDTASKINFO).pti_threadnum` | `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)` filtered to this pid (system-wide snapshot) |
| `open_files()` | entries of `/proc/self/fd` | `proc_pidinfo(PROC_PIDLISTFDS)` size ÷ entry size | `GetProcessHandleCount` |
| `limits()` | `getrlimit` for `RLIMIT_NOFILE`, `NPROC`, `AS`, `CORE`, `STACK` | same | job-object limits when present; otherwise `None` |
| `started_at()` | field 22 of `/proc/self/stat` (ticks since boot) + boot time | `proc_pidinfo(PROC_PIDTBSDINFO).pbi_start_tvsec` | `GetProcessTimes` creation time |
| `user()` | `geteuid` + `getpwuid_r` (NSS) | same | `GetUserNameW`; `is_admin` from the token |
| `is_elevated()` | `geteuid() == 0` | same | `OpenProcessToken` + `GetTokenInformation(TokenElevation)` |

`ru_maxrss` is in KiB on Linux and in bytes on macOS; sys-lib reads `VmHWM` on Linux and never mixes the two.

## Process guard

| Step | Linux | macOS | Windows |
|---|---|---|---|
| lock directory | path-lib's runtime directory (or `lock_dir`); `lstat` for symlinks and mode | same | path-lib; `GetFileAttributesW` for reparse points |
| filesystem type | `statfs` `f_type` magic (NFS `0x6969`, SMB `0x517B`, CIFS `0xFF534D42`, SMB2 `0xFE534D42`, FUSE `0x65735546`, 9p `0x01021997`, drvfs/WSL [verify]) | `statfs` `f_fstypename` | `GetVolumeInformationW` file system name; `GetDriveTypeW` `DRIVE_REMOTE` |
| lock | `open(O_NOFOLLOW)` through `OpenOptionsExt::custom_flags`, `File::try_lock` (std, `flock`) | same | `FILE_FLAG_OPEN_REPARSE_POINT`, `File::try_lock` (std, `LockFileEx`) |
| pid file | `path_lib::write_atomic` | same | same |
| umask | `umask` | `umask` | not applicable |
| open-file limit | `getrlimit`/`setrlimit(RLIMIT_NOFILE)` | same, capped at `kern.maxfilesperproc` | not applicable |
| threads check | `Threads` in `/proc/self/status` | `proc_pidinfo(PROC_PIDTASKINFO)` | toolhelp snapshot [verify cost] |

## Virtualization

| Check | Platform | Reads | Detects |
|---|---|---|---|
| CPUID leaf 1 ECX bit 31, leaf 0x40000000 vendor | x86 | `__cpuid` | `KVMKVMKVM`, `Linux KVM Hv`, `TCGTCGTCGTCG`, `VMwareVMware`, `Microsoft Hv`, `XenVMMXenVMM`, `bhyve bhyve `, `QNXQVMBSQG`, `ACRNACRNACRN`, `Apple VZ`, `VBoxVBoxVBox` |
| Hyper-V root partition | Windows x86 | CPUID 0x40000003 EBX bit 0 (`CreatePartitions`) [verify against the Hyper-V TLFS] | a host with Hyper-V, WSL 2, or VBS is not a VM |
| DMI strings | Linux (0444 files), Windows (SMBIOS types 0, 1, 2) | `product_name`, `sys_vendor`, `board_vendor`, `bios_vendor`, `product_version` | KVM, OpenStack, KubeVirt, Amazon EC2, QEMU, VMware, innotek GmbH, VirtualBox, Oracle, Xen, Bochs, Parallels, BHYVE, Hyper-V ("Microsoft Corporation" + "Virtual Machine"), Apple Virtualization, Google Compute Engine, Alibaba Cloud ECS |
| hypervisor files | Linux | `/sys/hypervisor/type`; `/proc/xen/capabilities` (`control_d` is Dom0, not a guest); `/proc/device-tree/hypervisor/compatible` | Xen, ARM hypervisors |
| sysctl | macOS | `kern.hv_vmm_present`; `hw.model` `VirtualMac*` | Apple Virtualization, Parallels, VMware Fusion |
| container files | Linux | `/run/systemd/container`, `/run/.containerenv` (Podman), `/.dockerenv`, `/proc/1/cgroup` v1 names (`docker`, `kubepods`, `lxc`), an overlay root in `/proc/self/mountinfo` | Docker, Podman, LXC, systemd-nspawn, containerd |
| Kubernetes | Linux | `/var/run/secrets/kubernetes.io/serviceaccount` (strong); `KUBERNETES_SERVICE_HOST` (weak) | Kubernetes |
| WSL | Linux | `/proc/sys/kernel/osrelease` containing `Microsoft` (WSL 1) or `microsoft-standard-WSL2` (WSL 2); `/proc/sys/fs/binfmt_misc/WSLInterop`; `WSL_DISTRO_NAME` (weak) | WSL 1 or 2 |

## Power

| Reading | Linux | macOS | Windows |
|---|---|---|---|
| `has_battery()` | any `/sys/class/power_supply/*` with `type` `Battery` and `scope` not `Device` | `IOPSCopyPowerSourcesInfo` lists an internal battery | `GetSystemPowerStatus().BatteryFlag` not 128 (no battery) and not 255 (unknown) |
| `battery()` | `uevent` per battery (one read each): `POWER_SUPPLY_CAPACITY`, `STATUS`, `ENERGY_NOW`/`ENERGY_FULL`/`ENERGY_FULL_DESIGN` (or `CHARGE_*`), `CYCLE_COUNT` | the IOKit power source dictionary (current and max capacity, charging, time to empty and to full); health and cycles from `AppleSmartBattery` | `GetSystemPowerStatus` (`BatteryLifePercent`, `BatteryLifeTime`, the charging flag) |
| `on_ac()` | any `Mains` or `USB` supply with `online` = 1 | `IOPSGetProvidingPowerSourceType` | `ACLineStatus` (255 is unknown) |

## Device identity anchors

Feature `device-id`. Values are read into a fixed buffer, hashed, and wiped; none leaves the module.

| Anchor | Linux | macOS | Windows | Privilege |
|---|---|---|---|---|
| `InstallId` | `/etc/machine-id`, fallback `/var/lib/dbus/machine-id` (32 hex characters) | none (the platform UUID stands in) | `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`, read with `KEY_WOW64_64KEY` | none |
| `PlatformUuid` | `/sys/class/dmi/id/product_uuid` (mode 0400) | `IOPlatformUUID` of `IOPlatformExpertDevice`, or `gethostuuid()` | SMBIOS type 1 UUID (`GetSystemFirmwareTable('RSMB')`; the first three fields little-endian since SMBIOS 2.6, converted to RFC 9562 order) | Linux: root |
| `BoardSerial` | `/sys/class/dmi/id/board_serial` (0400) | `IOPlatformSerialNumber` | SMBIOS type 2 serial | Linux: root |
| `ProductSerial` | `/sys/class/dmi/id/product_serial` (0400) | not used twice (the same as the board on Macs) | SMBIOS type 1 serial | Linux: root |
| `CpuSignature` | CPUID vendor, family, model, stepping, brand; or MIDR | `machdep.cpu.*` | CPUID or registry | none |
| `PermanentMac` | lowest MAC with `addr_assign_type == 0` on a physical, non-removable interface (not used under WSL, whose MAC changes per boot) | lowest built-in interface MAC (`IOPrimaryInterface`) | lowest `PhysicalAddress` of a physical Ethernet or Wi-Fi adapter | none |

The DMI file permissions are the kernel's (`drivers/firmware/dmi-id.c`): `product_uuid`, `product_serial`, `board_serial`, and `chassis_serial` are 0400; `chassis_type`, `sys_vendor`, `product_name`, and `board_name` are 0444.

Placeholders treated as absent: all-zero and all-`F` UUIDs; serials `To be filled by O.E.M.`, `Default string`, `System Serial Number`, `Not Specified`, `Not Applicable`, `None`, `0`, `0123456789`, `123456789`, `Chassis Serial Number`, `Base Board Serial Number`, empty or whitespace; machine ids `uninitialized`, empty, all zeros; MACs all zeros, broadcast, multicast, or locally administered.

## Build info

At build time only, in the application's build script: `git -C $CARGO_MANIFEST_DIR rev-parse HEAD`, `git -C $CARGO_MANIFEST_DIR status --porcelain --untracked-files=no`, `$RUSTC --version`, and the Cargo variables `CARGO_PKG_NAME`, `CARGO_PKG_VERSION`, `PROFILE`, `TARGET`, `CARGO_FEATURE_*`, `SOURCE_DATE_EPOCH`. Re-run triggers: the git `HEAD` file and the ref it names (through a `.git` file in worktrees) and `SOURCE_DATE_EPOCH`. Nothing at run time.

## Android and iOS

Best effort, build-checked in CI. Android uses the Linux paths where SELinux allows them (`/proc`, `/sys`), plus `__system_property_get` for `ro.build.version.release` and `ro.build.characteristics`; denied reads are `PermissionDenied`. iOS uses the Apple sysctls that exist on iOS (`kern.osproductversion`, `kern.osrelease`, `hw.memsize`, `hw.logicalcpu_max`, `hw.physicalcpu_max`, `machdep.cpu.brand_string` [verify]) and `uname`; everything that needs IOKit power sources, `getfsstat` outside the sandbox, or UIKit is `Unsupported`. `os_proc_available_memory()` (iOS 13 and later) is planned for `mem::free()` on iOS after 1.0.
