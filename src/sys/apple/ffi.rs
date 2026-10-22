//! macOS and iOS declarations: every libSystem, Mach, libproc, CoreFoundation,
//! and IOKit item sys-lib uses, written by hand from the Apple SDK headers.
//!
//! Declared for 64-bit targets (x86_64 and arm64 macOS, arm64 iOS). Items iOS
//! does not offer publicly (libproc, IOKit, `getfsstat`, `gethostuuid`) are
//! gated to macOS. Each struct has a size and alignment assertion, and the
//! tests at the bottom compare it with `libc` field by field.
//!
//! On x86_64 macOS the `statfs` family has two ABIs; the 64-bit-inode one
//! (`statfs$INODE64`) is the one these structs describe and the one linked.
//! arm64 has only that ABI, under the plain name.

#![expect(
    dead_code,
    reason = "the declaration list is locked in 0.2.0; each item is used by the reading that \
              arrives in 0.3 to 0.5 (docs/API.md), and this expectation goes when the last one is"
)]

use core::ffi::{c_char, c_int, c_long, c_uint, c_void};

/// `mach_port_t`.
pub(crate) type MachPort = u32;
/// `kern_return_t`.
pub(crate) type KernReturn = c_int;
/// `uid_t`.
pub(crate) type Uid = u32;
/// `mode_t` (16 bits on Apple platforms).
pub(crate) type Mode = u16;
/// `time_t`.
pub(crate) type Time = i64;
/// `CFTypeRef` and every other CoreFoundation object reference.
pub(crate) type CfRef = *const c_void;
/// `CFIndex`.
pub(crate) type CfIndex = isize;
/// `CFTypeID`.
pub(crate) type CfTypeId = usize;
/// `io_object_t` (and `io_service_t`, `io_iterator_t`, `io_registry_entry_t`).
#[cfg(target_os = "macos")]
pub(crate) type IoObject = MachPort;

/// `KERN_SUCCESS`.
pub(crate) const KERN_SUCCESS: KernReturn = 0;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// `struct statfs` with 64-bit inodes (sys/mount.h).
#[repr(C)]
pub(crate) struct Statfs {
    pub(crate) bsize: u32,
    pub(crate) iosize: i32,
    pub(crate) blocks: u64,
    pub(crate) bfree: u64,
    pub(crate) bavail: u64,
    pub(crate) files: u64,
    pub(crate) ffree: u64,
    pub(crate) fsid: [i32; 2],
    pub(crate) owner: Uid,
    pub(crate) ftype: u32,
    pub(crate) flags: u32,
    pub(crate) fssubtype: u32,
    pub(crate) fstypename: [c_char; 16],
    pub(crate) mntonname: [c_char; 1024],
    pub(crate) mntfromname: [c_char; 1024],
    pub(crate) flags_ext: u32,
    pub(crate) reserved: [u32; 7],
}
layout!(Statfs, size = 2168, align = 8);

// Mount flags and getfsstat modes (sys/mount.h).
pub(crate) const MNT_RDONLY: u32 = 0x0000_0001;
pub(crate) const MNT_LOCAL: u32 = 0x0000_1000;
pub(crate) const MNT_DONTBROWSE: u32 = 0x0010_0000;
pub(crate) const MNT_NOWAIT: c_int = 2;

/// `struct timeval`; `usec` is 32-bit (`suseconds_t`).
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Timeval {
    pub(crate) sec: Time,
    pub(crate) usec: i32,
}
layout!(Timeval, size = 16, align = 8);

/// `struct timespec`.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Timespec {
    pub(crate) sec: Time,
    pub(crate) nsec: c_long,
}
layout!(Timespec, size = 16, align = 8);

/// `struct rusage` (sys/resource.h). `maxrss` is in bytes on Apple platforms.
#[repr(C)]
pub(crate) struct Rusage {
    pub(crate) utime: Timeval,
    pub(crate) stime: Timeval,
    pub(crate) maxrss: c_long,
    pub(crate) ixrss: c_long,
    pub(crate) idrss: c_long,
    pub(crate) isrss: c_long,
    pub(crate) minflt: c_long,
    pub(crate) majflt: c_long,
    pub(crate) nswap: c_long,
    pub(crate) inblock: c_long,
    pub(crate) oublock: c_long,
    pub(crate) msgsnd: c_long,
    pub(crate) msgrcv: c_long,
    pub(crate) nsignals: c_long,
    pub(crate) nvcsw: c_long,
    pub(crate) nivcsw: c_long,
}
layout!(Rusage, size = 144, align = 8);

/// `RUSAGE_SELF`.
pub(crate) const RUSAGE_SELF: c_int = 0;

/// `struct rlimit` (sys/resource.h).
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Rlimit {
    pub(crate) cur: u64,
    pub(crate) max: u64,
}
layout!(Rlimit, size = 16, align = 8);

/// `RLIM_INFINITY`.
pub(crate) const RLIM_INFINITY: u64 = (1 << 63) - 1;
// Resource numbers (sys/resource.h): they differ from Linux.
pub(crate) const RLIMIT_STACK: c_int = 3;
pub(crate) const RLIMIT_CORE: c_int = 4;
pub(crate) const RLIMIT_AS: c_int = 5;
pub(crate) const RLIMIT_NPROC: c_int = 7;
pub(crate) const RLIMIT_NOFILE: c_int = 8;

/// `struct passwd` (pwd.h); Apple's carries the BSD `change`, `class`, and
/// `expire` fields.
#[repr(C)]
pub(crate) struct Passwd {
    pub(crate) name: *mut c_char,
    pub(crate) passwd: *mut c_char,
    pub(crate) uid: Uid,
    pub(crate) gid: u32,
    pub(crate) change: Time,
    pub(crate) class: *mut c_char,
    pub(crate) gecos: *mut c_char,
    pub(crate) dir: *mut c_char,
    pub(crate) shell: *mut c_char,
    pub(crate) expire: Time,
}
layout!(Passwd, size = 72, align = 8);

/// `struct tm` (time.h).
#[repr(C)]
pub(crate) struct Tm {
    pub(crate) sec: c_int,
    pub(crate) min: c_int,
    pub(crate) hour: c_int,
    pub(crate) mday: c_int,
    pub(crate) mon: c_int,
    pub(crate) year: c_int,
    pub(crate) wday: c_int,
    pub(crate) yday: c_int,
    pub(crate) isdst: c_int,
    pub(crate) gmtoff: c_long,
    pub(crate) zone: *mut c_char,
}
layout!(Tm, size = 56, align = 8);

/// `struct sockaddr` (sys/socket.h), with the BSD length byte.
#[repr(C)]
pub(crate) struct Sockaddr {
    pub(crate) len: u8,
    pub(crate) family: u8,
    pub(crate) data: [c_char; 14],
}
layout!(Sockaddr, size = 16, align = 1);

/// `struct sockaddr_in` (netinet/in.h).
#[repr(C)]
pub(crate) struct SockaddrIn {
    pub(crate) len: u8,
    pub(crate) family: u8,
    pub(crate) port: u16,
    pub(crate) addr: u32,
    pub(crate) zero: [c_char; 8],
}
layout!(SockaddrIn, size = 16, align = 4);

/// `struct sockaddr_in6` (netinet6/in6.h).
#[repr(C)]
pub(crate) struct SockaddrIn6 {
    pub(crate) len: u8,
    pub(crate) family: u8,
    pub(crate) port: u16,
    pub(crate) flowinfo: u32,
    pub(crate) addr: [u8; 16],
    pub(crate) scope_id: u32,
}
layout!(SockaddrIn6, size = 28, align = 4);

/// `struct sockaddr_dl` (net/if_dl.h): the link-level address follows the
/// `nlen` bytes of the interface name inside `data`.
#[repr(C)]
pub(crate) struct SockaddrDl {
    pub(crate) len: u8,
    pub(crate) family: u8,
    pub(crate) index: u16,
    pub(crate) ftype: u8,
    pub(crate) nlen: u8,
    pub(crate) alen: u8,
    pub(crate) slen: u8,
    pub(crate) data: [c_char; 12],
}
layout!(SockaddrDl, size = 20, align = 2);

/// `struct ifaddrs` (ifaddrs.h).
#[repr(C)]
pub(crate) struct Ifaddrs {
    pub(crate) next: *mut Ifaddrs,
    pub(crate) name: *mut c_char,
    pub(crate) flags: c_uint,
    pub(crate) addr: *mut Sockaddr,
    pub(crate) netmask: *mut Sockaddr,
    pub(crate) dstaddr: *mut Sockaddr,
    pub(crate) data: *mut c_void,
}
layout!(Ifaddrs, size = 56, align = 8);

// Address families (sys/socket.h) and interface flags (net/if.h).
pub(crate) const AF_INET: u8 = 2;
pub(crate) const AF_LINK: u8 = 18;
pub(crate) const AF_INET6: u8 = 30;
pub(crate) const IFF_UP: c_uint = 0x1;
pub(crate) const IFF_LOOPBACK: c_uint = 0x8;
pub(crate) const IFF_RUNNING: c_uint = 0x40;

/// `struct timeval32` (net/if_var.h).
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Timeval32 {
    pub(crate) sec: i32,
    pub(crate) usec: i32,
}
layout!(Timeval32, size = 8, align = 4);

/// `struct if_data64` (net/if_var.h), packed to 4 bytes. Fields of a packed
/// struct are read by copy, never by reference.
#[repr(C, packed(4))]
pub(crate) struct IfData64 {
    pub(crate) ftype: u8,
    pub(crate) typelen: u8,
    pub(crate) physical: u8,
    pub(crate) addrlen: u8,
    pub(crate) hdrlen: u8,
    pub(crate) recvquota: u8,
    pub(crate) xmitquota: u8,
    pub(crate) unused1: u8,
    pub(crate) mtu: u32,
    pub(crate) metric: u32,
    pub(crate) baudrate: u64,
    pub(crate) ipackets: u64,
    pub(crate) ierrors: u64,
    pub(crate) opackets: u64,
    pub(crate) oerrors: u64,
    pub(crate) collisions: u64,
    pub(crate) ibytes: u64,
    pub(crate) obytes: u64,
    pub(crate) imcasts: u64,
    pub(crate) omcasts: u64,
    pub(crate) iqdrops: u64,
    pub(crate) noproto: u64,
    pub(crate) recvtiming: u32,
    pub(crate) xmittiming: u32,
    pub(crate) lastchange: Timeval32,
}
layout!(IfData64, size = 128, align = 4);

/// `struct if_msghdr2` (net/if.h): one routing message per interface from
/// `sysctl(CTL_NET, PF_ROUTE, 0, 0, NET_RT_IFLIST2, 0)`, packed to 4 bytes.
#[repr(C, packed(4))]
pub(crate) struct IfMsghdr2 {
    pub(crate) msglen: u16,
    pub(crate) version: u8,
    pub(crate) mtype: u8,
    pub(crate) addrs: c_int,
    pub(crate) flags: c_int,
    pub(crate) index: u16,
    pub(crate) snd_len: c_int,
    pub(crate) snd_maxlen: c_int,
    pub(crate) snd_drops: c_int,
    pub(crate) timer: c_int,
    pub(crate) data: IfData64,
}
layout!(IfMsghdr2, size = 160, align = 4);

// Routing sysctl (sys/sysctl.h, net/route.h).
pub(crate) const CTL_NET: c_int = 4;
pub(crate) const PF_ROUTE: c_int = 17;
pub(crate) const NET_RT_IFLIST2: c_int = 6;
pub(crate) const RTM_IFINFO2: u8 = 0x12;

/// `struct xsw_usage` (sys/sysctl.h), from `vm.swapusage`.
#[repr(C)]
pub(crate) struct XswUsage {
    pub(crate) total: u64,
    pub(crate) avail: u64,
    pub(crate) used: u64,
    pub(crate) pagesize: u32,
    pub(crate) encrypted: i32,
}
layout!(XswUsage, size = 32, align = 8);

/// `struct vm_statistics64` (mach/vm_statistics.h), the long-standing fields
/// through `total_uncompressed_pages_in_compressor`. Newer SDKs append fields;
/// the kernel fills at most the count passed, which is this struct's size.
#[repr(C)]
pub(crate) struct VmStatistics64 {
    pub(crate) free_count: u32,
    pub(crate) active_count: u32,
    pub(crate) inactive_count: u32,
    pub(crate) wire_count: u32,
    pub(crate) zero_fill_count: u64,
    pub(crate) reactivations: u64,
    pub(crate) pageins: u64,
    pub(crate) pageouts: u64,
    pub(crate) faults: u64,
    pub(crate) cow_faults: u64,
    pub(crate) lookups: u64,
    pub(crate) hits: u64,
    pub(crate) purges: u64,
    pub(crate) purgeable_count: u32,
    pub(crate) speculative_count: u32,
    pub(crate) decompressions: u64,
    pub(crate) compressions: u64,
    pub(crate) swapins: u64,
    pub(crate) swapouts: u64,
    pub(crate) compressor_page_count: u32,
    pub(crate) throttled_count: u32,
    pub(crate) external_page_count: u32,
    pub(crate) internal_page_count: u32,
    pub(crate) total_uncompressed_pages_in_compressor: u64,
}
layout!(VmStatistics64, size = 152, align = 8);

/// `HOST_VM_INFO64_COUNT` for [`VmStatistics64`], in `natural_t` units.
pub(crate) const VM_STATISTICS64_COUNT: u32 =
    (core::mem::size_of::<VmStatistics64>() / core::mem::size_of::<u32>()) as u32;

/// `host_cpu_load_info` and `processor_cpu_load_info` (mach/host_info.h,
/// mach/processor_info.h): ticks per CPU state.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct CpuLoadInfo {
    pub(crate) ticks: [u32; CPU_STATE_MAX],
}
layout!(CpuLoadInfo, size = 16, align = 4);

/// `HOST_CPU_LOAD_INFO_COUNT`.
pub(crate) const CPU_LOAD_INFO_COUNT: u32 = CPU_STATE_MAX as u32;

// CPU states (mach/machine.h).
pub(crate) const CPU_STATE_USER: usize = 0;
pub(crate) const CPU_STATE_SYSTEM: usize = 1;
pub(crate) const CPU_STATE_IDLE: usize = 2;
pub(crate) const CPU_STATE_NICE: usize = 3;
pub(crate) const CPU_STATE_MAX: usize = 4;

// Flavors (mach/host_info.h, mach/processor_info.h, mach/task_info.h).
pub(crate) const HOST_CPU_LOAD_INFO: c_int = 3;
pub(crate) const HOST_VM_INFO64: c_int = 4;
pub(crate) const PROCESSOR_CPU_LOAD_INFO: c_int = 2;
pub(crate) const MACH_TASK_BASIC_INFO: u32 = 20;

/// `time_value_t` (mach/time_value.h).
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct TimeValue {
    pub(crate) seconds: i32,
    pub(crate) microseconds: i32,
}
layout!(TimeValue, size = 8, align = 4);

/// `struct mach_task_basic_info` (mach/task_info.h), packed to 4 bytes.
#[repr(C, packed(4))]
pub(crate) struct MachTaskBasicInfo {
    pub(crate) virtual_size: u64,
    pub(crate) resident_size: u64,
    pub(crate) resident_size_max: u64,
    pub(crate) user_time: TimeValue,
    pub(crate) system_time: TimeValue,
    pub(crate) policy: c_int,
    pub(crate) suspend_count: c_int,
}
layout!(MachTaskBasicInfo, size = 48, align = 4);

/// `MACH_TASK_BASIC_INFO_COUNT`, in `natural_t` units.
pub(crate) const MACH_TASK_BASIC_INFO_COUNT: u32 =
    (core::mem::size_of::<MachTaskBasicInfo>() / core::mem::size_of::<u32>()) as u32;

/// `struct proc_taskinfo` (sys/proc_info.h).
#[cfg(target_os = "macos")]
#[repr(C)]
pub(crate) struct ProcTaskinfo {
    pub(crate) virtual_size: u64,
    pub(crate) resident_size: u64,
    pub(crate) total_user: u64,
    pub(crate) total_system: u64,
    pub(crate) threads_user: u64,
    pub(crate) threads_system: u64,
    pub(crate) policy: i32,
    pub(crate) faults: i32,
    pub(crate) pageins: i32,
    pub(crate) cow_faults: i32,
    pub(crate) messages_sent: i32,
    pub(crate) messages_received: i32,
    pub(crate) syscalls_mach: i32,
    pub(crate) syscalls_unix: i32,
    pub(crate) csw: i32,
    pub(crate) threadnum: i32,
    pub(crate) numrunning: i32,
    pub(crate) priority: i32,
}
#[cfg(target_os = "macos")]
layout!(ProcTaskinfo, size = 96, align = 8);

/// `struct proc_bsdinfo` (sys/proc_info.h).
#[cfg(target_os = "macos")]
#[repr(C)]
pub(crate) struct ProcBsdinfo {
    pub(crate) flags: u32,
    pub(crate) status: u32,
    pub(crate) xstatus: u32,
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    pub(crate) uid: Uid,
    pub(crate) gid: u32,
    pub(crate) ruid: Uid,
    pub(crate) rgid: u32,
    pub(crate) svuid: Uid,
    pub(crate) svgid: u32,
    pub(crate) rfu_1: u32,
    pub(crate) comm: [c_char; 16],
    pub(crate) name: [c_char; 32],
    pub(crate) nfiles: u32,
    pub(crate) pgid: u32,
    pub(crate) pjobc: u32,
    pub(crate) e_tdev: u32,
    pub(crate) e_tpgid: u32,
    pub(crate) nice: i32,
    pub(crate) start_tvsec: u64,
    pub(crate) start_tvusec: u64,
}
#[cfg(target_os = "macos")]
layout!(ProcBsdinfo, size = 136, align = 8);

/// `struct proc_fdinfo` (sys/proc_info.h), one per descriptor from `PROC_PIDLISTFDS`.
#[cfg(target_os = "macos")]
#[repr(C)]
pub(crate) struct ProcFdinfo {
    pub(crate) fd: i32,
    pub(crate) fdtype: u32,
}
#[cfg(target_os = "macos")]
layout!(ProcFdinfo, size = 8, align = 4);

/// `struct rusage_info_v2` (sys/resource.h). `phys_footprint` is what Activity
/// Monitor shows as a process's memory.
#[cfg(target_os = "macos")]
#[repr(C)]
pub(crate) struct RusageInfoV2 {
    pub(crate) uuid: [u8; 16],
    pub(crate) user_time: u64,
    pub(crate) system_time: u64,
    pub(crate) pkg_idle_wkups: u64,
    pub(crate) interrupt_wkups: u64,
    pub(crate) pageins: u64,
    pub(crate) wired_size: u64,
    pub(crate) resident_size: u64,
    pub(crate) phys_footprint: u64,
    pub(crate) proc_start_abstime: u64,
    pub(crate) proc_exit_abstime: u64,
    pub(crate) child_user_time: u64,
    pub(crate) child_system_time: u64,
    pub(crate) child_pkg_idle_wkups: u64,
    pub(crate) child_interrupt_wkups: u64,
    pub(crate) child_pageins: u64,
    pub(crate) child_elapsed_abstime: u64,
    pub(crate) diskio_bytesread: u64,
    pub(crate) diskio_byteswritten: u64,
}
#[cfg(target_os = "macos")]
layout!(RusageInfoV2, size = 160, align = 8);

// libproc flavors (sys/proc_info.h, sys/resource.h).
#[cfg(target_os = "macos")]
pub(crate) const PROC_PIDLISTFDS: c_int = 1;
#[cfg(target_os = "macos")]
pub(crate) const PROC_PIDTBSDINFO: c_int = 3;
#[cfg(target_os = "macos")]
pub(crate) const PROC_PIDTASKINFO: c_int = 4;
#[cfg(target_os = "macos")]
pub(crate) const RUSAGE_INFO_V2: c_int = 2;

// CoreFoundation (CFString.h, CFNumber.h).
pub(crate) const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
pub(crate) const CF_NUMBER_SINT32_TYPE: CfIndex = 3;
pub(crate) const CF_NUMBER_SINT64_TYPE: CfIndex = 4;

/// `kIOMainPortDefault`: documented as `MACH_PORT_NULL`, so the constant is
/// used instead of the symbol (whose name changed in macOS 12).
#[cfg(target_os = "macos")]
pub(crate) const IO_MAIN_PORT_DEFAULT: MachPort = 0;

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

unsafe extern "C" {
    // sys/sysctl.h
    pub(crate) fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
    pub(crate) fn sysctl(
        name: *mut c_int,
        namelen: c_uint,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
    // mach/mach_init.h, mach/mach_host.h, mach/task.h, mach/vm_map.h, mach/mach_port.h
    pub(crate) safe static mach_task_self_: MachPort;
    pub(crate) safe fn mach_host_self() -> MachPort;
    pub(crate) fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    pub(crate) fn host_statistics(
        host: MachPort,
        flavor: c_int,
        info: *mut c_int,
        count: *mut u32,
    ) -> KernReturn;
    pub(crate) fn host_statistics64(
        host: MachPort,
        flavor: c_int,
        info: *mut c_int,
        count: *mut u32,
    ) -> KernReturn;
    pub(crate) fn host_processor_info(
        host: MachPort,
        flavor: c_int,
        processor_count: *mut u32,
        info: *mut *mut c_int,
        info_count: *mut u32,
    ) -> KernReturn;
    pub(crate) fn task_info(
        task: MachPort,
        flavor: u32,
        info: *mut c_int,
        count: *mut u32,
    ) -> KernReturn;
    pub(crate) fn vm_deallocate(task: MachPort, address: usize, size: usize) -> KernReturn;
    // unistd.h, sys/stat.h, sys/resource.h, pwd.h, time.h, stdlib.h
    pub(crate) safe fn geteuid() -> Uid;
    pub(crate) safe fn umask(mask: Mode) -> Mode;
    pub(crate) fn getrlimit(resource: c_int, rlim: *mut Rlimit) -> c_int;
    pub(crate) fn setrlimit(resource: c_int, rlim: *const Rlimit) -> c_int;
    pub(crate) fn getrusage(who: c_int, usage: *mut Rusage) -> c_int;
    pub(crate) fn getpwuid_r(
        uid: Uid,
        pwd: *mut Passwd,
        buf: *mut c_char,
        buflen: usize,
        result: *mut *mut Passwd,
    ) -> c_int;
    pub(crate) fn localtime_r(time: *const Time, result: *mut Tm) -> *mut Tm;
    pub(crate) fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
    pub(crate) fn gethostname(name: *mut c_char, len: usize) -> c_int;
    // ifaddrs.h
    pub(crate) fn getifaddrs(ifap: *mut *mut Ifaddrs) -> c_int;
    pub(crate) fn freeifaddrs(ifa: *mut Ifaddrs);
    // sys/mount.h
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "statfs$INODE64"
    )]
    pub(crate) fn statfs(path: *const c_char, buf: *mut Statfs) -> c_int;
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    // sys/mount.h
    #[cfg_attr(target_arch = "x86_64", link_name = "getfsstat$INODE64")]
    pub(crate) fn getfsstat(buf: *mut Statfs, bufsize: c_int, mode: c_int) -> c_int;
    // unistd.h
    pub(crate) fn gethostuuid(id: *mut u8, wait: *const Timespec) -> c_int;
    // libproc.h
    pub(crate) fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        size: c_int,
    ) -> c_int;
    pub(crate) fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut *mut c_void) -> c_int;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CFRelease(cf: CfRef);
    pub(crate) fn CFGetTypeID(cf: CfRef) -> CfTypeId;
    pub(crate) safe fn CFStringGetTypeID() -> CfTypeId;
    pub(crate) safe fn CFNumberGetTypeID() -> CfTypeId;
    pub(crate) safe fn CFBooleanGetTypeID() -> CfTypeId;
    pub(crate) safe fn CFDictionaryGetTypeID() -> CfTypeId;
    pub(crate) safe fn CFArrayGetTypeID() -> CfTypeId;
    pub(crate) fn CFStringCreateWithBytes(
        allocator: CfRef,
        bytes: *const u8,
        length: CfIndex,
        encoding: u32,
        external_representation: u8,
    ) -> CfRef;
    pub(crate) fn CFStringGetLength(string: CfRef) -> CfIndex;
    pub(crate) safe fn CFStringGetMaximumSizeForEncoding(length: CfIndex, encoding: u32)
    -> CfIndex;
    pub(crate) fn CFStringGetCString(
        string: CfRef,
        buffer: *mut c_char,
        size: CfIndex,
        encoding: u32,
    ) -> u8;
    pub(crate) fn CFDictionaryGetValue(dictionary: CfRef, key: *const c_void) -> *const c_void;
    pub(crate) fn CFArrayGetCount(array: CfRef) -> CfIndex;
    pub(crate) fn CFArrayGetValueAtIndex(array: CfRef, index: CfIndex) -> *const c_void;
    pub(crate) fn CFNumberGetValue(number: CfRef, kind: CfIndex, value: *mut c_void) -> u8;
    pub(crate) fn CFBooleanGetValue(boolean: CfRef) -> u8;
    pub(crate) fn CFLocaleCopyCurrent() -> CfRef;
    pub(crate) fn CFLocaleGetIdentifier(locale: CfRef) -> CfRef;
}

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    // IOKit/IOKitLib.h
    pub(crate) fn IOServiceMatching(name: *const c_char) -> CfRef;
    pub(crate) fn IOServiceGetMatchingService(main_port: MachPort, matching: CfRef) -> IoObject;
    pub(crate) fn IOServiceGetMatchingServices(
        main_port: MachPort,
        matching: CfRef,
        iterator: *mut IoObject,
    ) -> KernReturn;
    pub(crate) fn IOIteratorNext(iterator: IoObject) -> IoObject;
    pub(crate) fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CfRef,
        allocator: CfRef,
        options: u32,
    ) -> CfRef;
    pub(crate) fn IORegistryEntryGetParentEntry(
        entry: IoObject,
        plane: *const c_char,
        parent: *mut IoObject,
    ) -> KernReturn;
    pub(crate) fn IOObjectRelease(object: IoObject) -> KernReturn;
    // IOKit/ps/IOPowerSources.h
    pub(crate) fn IOPSCopyPowerSourcesInfo() -> CfRef;
    pub(crate) fn IOPSCopyPowerSourcesList(blob: CfRef) -> CfRef;
    pub(crate) fn IOPSGetPowerSourceDescription(blob: CfRef, source: CfRef) -> CfRef;
    pub(crate) fn IOPSGetProvidingPowerSourceType(snapshot: CfRef) -> CfRef;
}

#[cfg(test)]
mod tests {
    //! Every struct and constant above against `libc` (a dev-dependency used as
    //! an oracle only): size, alignment, and the offset of every field.

    use super::*;

    same_layout!(Statfs, libc::statfs,
        bsize => f_bsize, iosize => f_iosize, blocks => f_blocks, bfree => f_bfree,
        bavail => f_bavail, files => f_files, ffree => f_ffree, fsid => f_fsid,
        owner => f_owner, ftype => f_type, flags => f_flags, fssubtype => f_fssubtype,
        fstypename => f_fstypename, mntonname => f_mntonname,
        mntfromname => f_mntfromname, flags_ext => f_flags_ext, reserved => f_reserved);
    same_layout!(Timeval, libc::timeval, sec => tv_sec, usec => tv_usec);
    same_layout!(Timespec, libc::timespec, sec => tv_sec, nsec => tv_nsec);
    same_layout!(Rusage, libc::rusage,
        utime => ru_utime, stime => ru_stime, maxrss => ru_maxrss, minflt => ru_minflt,
        majflt => ru_majflt, nvcsw => ru_nvcsw, nivcsw => ru_nivcsw);
    same_layout!(Rlimit, libc::rlimit, cur => rlim_cur, max => rlim_max);
    same_layout!(Passwd, libc::passwd,
        name => pw_name, passwd => pw_passwd, uid => pw_uid, gid => pw_gid,
        change => pw_change, class => pw_class, gecos => pw_gecos, dir => pw_dir,
        shell => pw_shell, expire => pw_expire);
    same_layout!(Tm, libc::tm,
        sec => tm_sec, isdst => tm_isdst, gmtoff => tm_gmtoff, zone => tm_zone);
    same_layout!(Sockaddr, libc::sockaddr, len => sa_len, family => sa_family, data => sa_data);
    same_layout!(SockaddrIn, libc::sockaddr_in,
        len => sin_len, family => sin_family, port => sin_port, addr => sin_addr, zero => sin_zero);
    same_layout!(SockaddrIn6, libc::sockaddr_in6,
        len => sin6_len, family => sin6_family, port => sin6_port,
        flowinfo => sin6_flowinfo, addr => sin6_addr, scope_id => sin6_scope_id);
    same_layout!(SockaddrDl, libc::sockaddr_dl,
        len => sdl_len, family => sdl_family, index => sdl_index, ftype => sdl_type,
        nlen => sdl_nlen, alen => sdl_alen, slen => sdl_slen, data => sdl_data);
    same_layout!(Ifaddrs, libc::ifaddrs,
        next => ifa_next, name => ifa_name, flags => ifa_flags, addr => ifa_addr,
        netmask => ifa_netmask, dstaddr => ifa_dstaddr, data => ifa_data);
    same_layout!(IfData64, libc::if_data64,
        ftype => ifi_type, mtu => ifi_mtu, baudrate => ifi_baudrate,
        ipackets => ifi_ipackets, ierrors => ifi_ierrors, opackets => ifi_opackets,
        oerrors => ifi_oerrors, ibytes => ifi_ibytes, obytes => ifi_obytes,
        iqdrops => ifi_iqdrops, lastchange => ifi_lastchange);
    same_layout!(IfMsghdr2, libc::if_msghdr2,
        msglen => ifm_msglen, mtype => ifm_type, addrs => ifm_addrs, flags => ifm_flags,
        index => ifm_index, snd_len => ifm_snd_len, timer => ifm_timer, data => ifm_data);
    same_layout!(XswUsage, libc::xsw_usage,
        total => xsu_total, avail => xsu_avail, used => xsu_used,
        pagesize => xsu_pagesize, encrypted => xsu_encrypted);
    same_layout!(prefix VmStatistics64, libc::vm_statistics64,
        free_count => free_count, active_count => active_count,
        inactive_count => inactive_count, wire_count => wire_count,
        zero_fill_count => zero_fill_count, pageins => pageins, pageouts => pageouts,
        purgeable_count => purgeable_count, speculative_count => speculative_count,
        compressions => compressions, swapins => swapins, swapouts => swapouts,
        compressor_page_count => compressor_page_count, throttled_count => throttled_count,
        external_page_count => external_page_count, internal_page_count => internal_page_count,
        total_uncompressed_pages_in_compressor => total_uncompressed_pages_in_compressor);
    same_layout!(CpuLoadInfo, libc::host_cpu_load_info, ticks => cpu_ticks);
    same_layout!(CpuLoadInfo, libc::processor_cpu_load_info, ticks => cpu_ticks);
    same_layout!(TimeValue, libc::time_value_t, seconds => seconds, microseconds => microseconds);
    same_layout!(MachTaskBasicInfo, libc::mach_task_basic_info,
        virtual_size => virtual_size, resident_size => resident_size,
        resident_size_max => resident_size_max, user_time => user_time,
        system_time => system_time, policy => policy, suspend_count => suspend_count);
    #[cfg(target_os = "macos")]
    same_layout!(ProcTaskinfo, libc::proc_taskinfo,
        virtual_size => pti_virtual_size, resident_size => pti_resident_size,
        total_user => pti_total_user, total_system => pti_total_system,
        threadnum => pti_threadnum, priority => pti_priority);
    #[cfg(target_os = "macos")]
    same_layout!(ProcBsdinfo, libc::proc_bsdinfo,
        pid => pbi_pid, ppid => pbi_ppid, uid => pbi_uid, comm => pbi_comm, name => pbi_name,
        nfiles => pbi_nfiles, nice => pbi_nice, start_tvsec => pbi_start_tvsec,
        start_tvusec => pbi_start_tvusec);
    #[cfg(target_os = "macos")]
    same_layout!(ProcFdinfo, libc::proc_fdinfo, fd => proc_fd, fdtype => proc_fdtype);
    #[cfg(target_os = "macos")]
    same_layout!(RusageInfoV2, libc::rusage_info_v2,
        uuid => ri_uuid, user_time => ri_user_time, system_time => ri_system_time,
        resident_size => ri_resident_size, phys_footprint => ri_phys_footprint,
        diskio_byteswritten => ri_diskio_byteswritten);

    #[allow(
        clippy::unnecessary_cast,
        reason = "libc's constant types differ between macOS and iOS"
    )]
    const _: () = {
        assert!(KERN_SUCCESS == libc::KERN_SUCCESS);
        assert!(MNT_RDONLY as i64 == libc::MNT_RDONLY as i64);
        assert!(MNT_LOCAL as i64 == libc::MNT_LOCAL as i64);
        assert!(MNT_NOWAIT == libc::MNT_NOWAIT);
        assert!(RUSAGE_SELF == libc::RUSAGE_SELF);
        assert!(RLIM_INFINITY == libc::RLIM_INFINITY as u64);
        assert!(RLIMIT_STACK == libc::RLIMIT_STACK);
        assert!(RLIMIT_CORE == libc::RLIMIT_CORE);
        assert!(RLIMIT_AS == libc::RLIMIT_AS);
        assert!(RLIMIT_NPROC == libc::RLIMIT_NPROC);
        assert!(RLIMIT_NOFILE == libc::RLIMIT_NOFILE);
        assert!(AF_INET as c_int == libc::AF_INET);
        assert!(AF_INET6 as c_int == libc::AF_INET6);
        assert!(AF_LINK as c_int == libc::AF_LINK);
        assert!(IFF_UP as c_int == libc::IFF_UP);
        assert!(IFF_LOOPBACK as c_int == libc::IFF_LOOPBACK);
        assert!(IFF_RUNNING as c_int == libc::IFF_RUNNING);
        assert!(CTL_NET == libc::CTL_NET);
        assert!(PF_ROUTE == libc::PF_ROUTE);
        assert!(NET_RT_IFLIST2 == libc::NET_RT_IFLIST2);
        assert!(HOST_CPU_LOAD_INFO == libc::HOST_CPU_LOAD_INFO);
        assert!(HOST_VM_INFO64 == libc::HOST_VM_INFO64);
        assert!(PROCESSOR_CPU_LOAD_INFO == libc::PROCESSOR_CPU_LOAD_INFO);
        assert!(MACH_TASK_BASIC_INFO == libc::MACH_TASK_BASIC_INFO);
        assert!(MACH_TASK_BASIC_INFO_COUNT == libc::MACH_TASK_BASIC_INFO_COUNT);
        assert!(CPU_STATE_USER as c_int == libc::CPU_STATE_USER);
        assert!(CPU_STATE_SYSTEM as c_int == libc::CPU_STATE_SYSTEM);
        assert!(CPU_STATE_IDLE as c_int == libc::CPU_STATE_IDLE);
        assert!(CPU_STATE_NICE as c_int == libc::CPU_STATE_NICE);
        assert!(CPU_STATE_MAX as c_int == libc::CPU_STATE_MAX);
    };

    #[cfg(target_os = "macos")]
    const _: () = {
        assert!(PROC_PIDLISTFDS == libc::PROC_PIDLISTFDS);
        assert!(PROC_PIDTBSDINFO == libc::PROC_PIDTBSDINFO);
        assert!(PROC_PIDTASKINFO == libc::PROC_PIDTASKINFO);
        assert!(RUSAGE_INFO_V2 == libc::RUSAGE_INFO_V2);
        assert!(RTM_IFINFO2 as c_int == libc::RTM_IFINFO2);
    };

    #[test]
    fn test_ffi_apple_layouts_match_libc() {
        // The checks above run at compile time; this test also confirms the
        // Mach host port is reachable on the machine running it.
        let host = mach_host_self();
        assert_ne!(host, 0);
        // SAFETY: `host` is a send right this test owns; returning it once is correct.
        let released = unsafe { mach_port_deallocate(mach_task_self_, host) };
        assert_eq!(released, KERN_SUCCESS);
    }
}
