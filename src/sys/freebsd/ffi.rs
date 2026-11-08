//! FreeBSD declarations: every libc call, struct, and constant sys-lib uses,
//! written by hand from the FreeBSD 14 headers.
//!
//! Declared for 64-bit targets (amd64 and arm64), FreeBSD 12 and later: the
//! 64-bit-inode ABI, whose `statfs` and `getfsstat` are the default symbol
//! versions (`@@FBSD_1.5`) that an unversioned reference binds to. Each struct
//! has a size and alignment assertion, and the tests at the bottom compare it
//! with `libc` field by field. Three structs have no `libc` definition to
//! compare with (`struct xswdev`, `struct rt_msghdr`, `struct rt_metrics`);
//! they carry the size assertion and the live tests read through them.
//!
//! Two kernel records are read as bytes at fixed offsets instead of as
//! structs: `struct kinfo_proc` (1 088 bytes, of which five fields are read)
//! and `struct devstat` (288 bytes). Each offset below is compared with the
//! `libc` field's in the tests, and every read is bounds-checked safe code.

#![cfg_attr(not(feature = "guard"), allow(dead_code))]

use core::ffi::{c_char, c_int, c_long, c_short, c_uint, c_void};

/// `uid_t`.
pub(crate) type Uid = u32;
/// `gid_t`.
pub(crate) type Gid = u32;
/// `mode_t` (16 bits on FreeBSD).
pub(crate) type Mode = u16;
/// `time_t`.
pub(crate) type Time = i64;
/// `pid_t`.
pub(crate) type Pid = i32;
/// `rlim_t`: signed on FreeBSD.
pub(crate) type Rlim = i64;

/// `O_NOFOLLOW` (fcntl.h).
pub(crate) const O_NOFOLLOW: c_int = 0x0100;
// errno.h: the codes mapped to sys-lib's own error variants.
pub(crate) const EPERM: c_int = 1;
pub(crate) const ENOENT: c_int = 2;
pub(crate) const ENOMEM: c_int = 12;
pub(crate) const EACCES: c_int = 13;
pub(crate) const EINVAL: c_int = 22;
/// `ERANGE`: a buffer was too small.
pub(crate) const ERANGE: c_int = 34;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// `struct statfs` (sys/mount.h), the FreeBSD 12 layout (`MNAMELEN` 1024).
#[repr(C)]
pub(crate) struct Statfs {
    pub(crate) version: u32,
    pub(crate) ftype: u32,
    pub(crate) flags: u64,
    pub(crate) bsize: u64,
    pub(crate) iosize: u64,
    pub(crate) blocks: u64,
    pub(crate) bfree: u64,
    pub(crate) bavail: i64,
    pub(crate) files: u64,
    pub(crate) ffree: i64,
    pub(crate) syncwrites: u64,
    pub(crate) asyncwrites: u64,
    pub(crate) syncreads: u64,
    pub(crate) asyncreads: u64,
    pub(crate) spare: [u64; 10],
    pub(crate) namemax: u32,
    pub(crate) owner: Uid,
    pub(crate) fsid: [i32; 2],
    pub(crate) charspare: [c_char; 80],
    pub(crate) fstypename: [c_char; 16],
    pub(crate) mntfromname: [c_char; 1024],
    pub(crate) mntonname: [c_char; 1024],
}
layout!(Statfs, size = 2344, align = 8);

// Mount flags and getfsstat modes (sys/mount.h).
pub(crate) const MNT_RDONLY: u64 = 0x0000_0001;
pub(crate) const MNT_LOCAL: u64 = 0x0000_1000;
/// Hidden from `df` and `mount`: autofs triggers, for one.
pub(crate) const MNT_IGNORE: u64 = 0x0080_0000;
pub(crate) const MNT_NOATIME: u64 = 0x1000_0000;
pub(crate) const MNT_NOWAIT: c_int = 2;

/// `struct timeval`; `usec` is a `long` (`suseconds_t`).
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Timeval {
    pub(crate) sec: Time,
    pub(crate) usec: c_long,
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

/// `struct rusage` (sys/resource.h). `maxrss` is in kilobytes on FreeBSD.
#[derive(Default)]
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
pub(crate) const RUSAGE_SELF: c_int = 0;

/// `struct rlimit`.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Rlimit {
    pub(crate) cur: Rlim,
    pub(crate) max: Rlim,
}
layout!(Rlimit, size = 16, align = 8);

// Resource limits (sys/resource.h).
pub(crate) const RLIMIT_STACK: c_int = 3;
pub(crate) const RLIMIT_CORE: c_int = 4;
pub(crate) const RLIMIT_MEMLOCK: c_int = 6;
pub(crate) const RLIMIT_NPROC: c_int = 7;
pub(crate) const RLIMIT_NOFILE: c_int = 8;
/// `RLIMIT_AS`, which FreeBSD names `RLIMIT_VMEM`.
pub(crate) const RLIMIT_AS: c_int = 10;
pub(crate) const RLIM_INFINITY: Rlim = i64::MAX;

/// `struct passwd` (pwd.h).
#[repr(C)]
pub(crate) struct Passwd {
    pub(crate) name: *mut c_char,
    pub(crate) passwd: *mut c_char,
    pub(crate) uid: Uid,
    pub(crate) gid: Gid,
    pub(crate) change: Time,
    pub(crate) class: *mut c_char,
    pub(crate) gecos: *mut c_char,
    pub(crate) dir: *mut c_char,
    pub(crate) shell: *mut c_char,
    pub(crate) expire: Time,
    pub(crate) fields: c_int,
}
layout!(Passwd, size = 80, align = 8);

/// `struct tm` (time.h), with the BSD `tm_gmtoff` and `tm_zone`.
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
    pub(crate) zone: *const c_char,
}
layout!(Tm, size = 56, align = 8);

/// `struct clockinfo` (sys/time.h): `kern.clockrate`.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Clockinfo {
    pub(crate) hz: c_int,
    pub(crate) tick: c_int,
    pub(crate) spare: c_int,
    pub(crate) stathz: c_int,
    pub(crate) profhz: c_int,
}
layout!(Clockinfo, size = 20, align = 4);

/// `struct xswdev` (vm/vm_param.h): one swap device of `vm.swap_info`, in
/// pages. `libc` does not define it.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Xswdev {
    pub(crate) version: c_uint,
    pub(crate) dev: u64,
    pub(crate) flags: c_int,
    pub(crate) nblks: c_int,
    pub(crate) used: c_int,
}
layout!(Xswdev, size = 32, align = 8);
/// `XSWDEV_VERSION`.
pub(crate) const XSWDEV_VERSION: c_uint = 2;

/// `struct timex` (sys/timex.h), for `ntp_adjtime`.
#[derive(Default)]
#[repr(C)]
pub(crate) struct Timex {
    pub(crate) modes: c_uint,
    pub(crate) offset: c_long,
    pub(crate) freq: c_long,
    pub(crate) maxerror: c_long,
    pub(crate) esterror: c_long,
    pub(crate) status: c_int,
    pub(crate) constant: c_long,
    pub(crate) precision: c_long,
    pub(crate) tolerance: c_long,
    pub(crate) ppsfreq: c_long,
    pub(crate) jitter: c_long,
    pub(crate) shift: c_int,
    pub(crate) stabil: c_long,
    pub(crate) jitcnt: c_long,
    pub(crate) calcnt: c_long,
    pub(crate) errcnt: c_long,
    pub(crate) stbcnt: c_long,
}
layout!(Timex, size = 136, align = 8);
/// `TIME_ERROR`: the clock is not synchronised.
pub(crate) const TIME_ERROR: c_int = 5;
/// `STA_UNSYNC`.
pub(crate) const STA_UNSYNC: c_int = 0x0040;

/// `struct utmpx` (utmpx.h).
#[repr(C)]
pub(crate) struct Utmpx {
    pub(crate) utype: c_short,
    pub(crate) tv: Timeval,
    pub(crate) id: [c_char; 8],
    pub(crate) pid: Pid,
    pub(crate) user: [c_char; 32],
    pub(crate) line: [c_char; 16],
    pub(crate) host: [c_char; 128],
    pub(crate) spare: [c_char; 64],
}
layout!(Utmpx, size = 280, align = 8);
pub(crate) const USER_PROCESS: c_short = 4;

/// `struct sockaddr` (sys/socket.h).
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
    /// `struct in_addr`, in network order.
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
/// interface name in `data`.
#[repr(C)]
pub(crate) struct SockaddrDl {
    pub(crate) len: u8,
    pub(crate) family: u8,
    pub(crate) index: u16,
    pub(crate) ftype: u8,
    pub(crate) nlen: u8,
    pub(crate) alen: u8,
    pub(crate) slen: u8,
    pub(crate) data: [c_char; 46],
}
layout!(SockaddrDl, size = 54, align = 2);

/// `struct ifaddrs` (ifaddrs.h).
#[repr(C)]
pub(crate) struct Ifaddrs {
    pub(crate) next: *mut Ifaddrs,
    pub(crate) name: *mut c_char,
    pub(crate) flags: c_uint,
    pub(crate) addr: *mut Sockaddr,
    pub(crate) netmask: *mut Sockaddr,
    pub(crate) dstaddr: *mut Sockaddr,
    /// A `struct if_data` on the `AF_LINK` entry of each interface.
    pub(crate) data: *mut c_void,
}
layout!(Ifaddrs, size = 56, align = 8);

/// `struct if_data` (net/if.h): 64-bit counters since FreeBSD 11.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct IfData {
    pub(crate) ftype: u8,
    pub(crate) physical: u8,
    pub(crate) addrlen: u8,
    pub(crate) hdrlen: u8,
    pub(crate) link_state: u8,
    pub(crate) vhid: u8,
    pub(crate) datalen: u16,
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
    pub(crate) oqdrops: u64,
    pub(crate) noproto: u64,
    pub(crate) hwassist: u64,
    /// `ifi_epoch`, a union of `time_t` and `uint64_t`.
    pub(crate) epoch: u64,
    /// `ifi_lastchange`, a union of `struct timeval` and `struct bintime`.
    pub(crate) lastchange: [u64; 2],
}
layout!(IfData, size = 152, align = 8);

// Interface flags (net/if.h).
pub(crate) const IFF_UP: c_uint = 0x1;
pub(crate) const IFF_LOOPBACK: c_uint = 0x8;
/// `IFF_DRV_RUNNING`, which the kernel reports as `IFF_RUNNING`.
pub(crate) const IFF_RUNNING: c_uint = 0x40;

// Address families (sys/socket.h).
pub(crate) const AF_INET: u8 = 2;
pub(crate) const AF_LINK: u8 = 18;
pub(crate) const AF_INET6: u8 = 28;
/// `PF_ROUTE` (`AF_ROUTE`).
pub(crate) const PF_ROUTE: c_int = 17;

// Interface types (net/if_types.h).
pub(crate) const IFT_ETHER: u8 = 0x06;
pub(crate) const IFT_PPP: u8 = 0x17;
pub(crate) const IFT_LOOP: u8 = 0x18;
pub(crate) const IFT_PROPVIRTUAL: u8 = 0x35;
pub(crate) const IFT_IEEE80211: u8 = 0x47;
pub(crate) const IFT_TUNNEL: u8 = 0x83;
pub(crate) const IFT_L2VLAN: u8 = 0x87;
pub(crate) const IFT_IEEE8023ADLAG: u8 = 0xa1;
pub(crate) const IFT_BRIDGE: u8 = 0xd1;
pub(crate) const IFT_STF: u8 = 0xd7;
pub(crate) const IFT_GIF: u8 = 0xf0;

/// `struct rt_metrics` (net/route.h). `libc` does not define it for FreeBSD.
#[repr(C)]
pub(crate) struct RtMetrics {
    pub(crate) locks: u64,
    pub(crate) mtu: u64,
    pub(crate) hopcount: u64,
    pub(crate) expire: u64,
    pub(crate) recvpipe: u64,
    pub(crate) sendpipe: u64,
    pub(crate) ssthresh: u64,
    pub(crate) rtt: u64,
    pub(crate) rttvar: u64,
    pub(crate) pksent: u64,
    pub(crate) weight: u64,
    pub(crate) nhidx: u64,
    pub(crate) filler: [u64; 2],
}
layout!(RtMetrics, size = 112, align = 8);

/// `struct rt_msghdr` (net/route.h): the header of each `NET_RT_DUMP` message,
/// followed by its `sockaddr`s. `libc` does not define it for FreeBSD.
#[repr(C)]
pub(crate) struct RtMsghdr {
    pub(crate) msglen: u16,
    pub(crate) version: u8,
    pub(crate) mtype: u8,
    pub(crate) index: u16,
    pub(crate) spare1: u16,
    pub(crate) flags: c_int,
    pub(crate) addrs: c_int,
    pub(crate) pid: Pid,
    pub(crate) seq: c_int,
    pub(crate) errno: c_int,
    pub(crate) fmask: c_int,
    pub(crate) inits: u64,
    pub(crate) rmx: RtMetrics,
}
layout!(RtMsghdr, size = 152, align = 8);

// The routing sysctl (net/route.h, sys/socket.h).
pub(crate) const NET_RT_DUMP: c_int = 1;
pub(crate) const RTM_VERSION: u8 = 5;
pub(crate) const RTM_GET: u8 = 0x4;
pub(crate) const RTF_UP: c_int = 0x1;
pub(crate) const RTF_HOST: c_int = 0x4;
pub(crate) const RTA_DST: c_int = 0x1;
pub(crate) const RTA_GATEWAY: c_int = 0x2;
pub(crate) const RTA_NETMASK: c_int = 0x4;

// sysctl MIB names (sys/sysctl.h).
pub(crate) const CTL_KERN: c_int = 1;
pub(crate) const CTL_NET: c_int = 4;
pub(crate) const KERN_PROC: c_int = 14;
pub(crate) const KERN_PROC_PID: c_int = 1;
pub(crate) const KERN_PROC_PROC: c_int = 8;
/// `KERN_PROC_NFDS`: the number of open descriptors (FreeBSD 13 and later).
pub(crate) const KERN_PROC_NFDS: c_int = 43;

/// `CLOCK_UPTIME` (time.h): time since boot, as `uptime(1)` reads it.
pub(crate) const CLOCK_UPTIME: c_int = 5;

/// `KENV_GET` (kenv.h).
pub(crate) const KENV_GET: c_int = 0;
/// `KENV_MVALLEN`: the longest value, without the NUL.
pub(crate) const KENV_MVALLEN: usize = 128;

// ---------------------------------------------------------------------------
// Records read as bytes
// ---------------------------------------------------------------------------

/// `KINFO_PROC_SIZE` on 64-bit targets (sys/user.h); `ki_structsize` holds it.
pub(crate) const KINFO_PROC_SIZE: usize = 1088;
// Offsets into `struct kinfo_proc`.
pub(crate) const KI_STRUCTSIZE: usize = 0;
pub(crate) const KI_SIZE: usize = 256;
pub(crate) const KI_RSSIZE: usize = 264;
pub(crate) const KI_START: usize = 336;
pub(crate) const KI_NUMTHREADS: usize = 596;

/// `DEVSTAT_VERSION` (sys/devicestat.h): `kern.devstat.version` for the
/// layout below.
pub(crate) const DEVSTAT_VERSION: u64 = 6;
/// `sizeof(struct devstat)` on 64-bit targets.
pub(crate) const DEVSTAT_SIZE: usize = 288;
// Offsets into `struct devstat`.
pub(crate) const DS_DEVICE_NAME: usize = 44;
pub(crate) const DS_UNIT_NUMBER: usize = 60;
pub(crate) const DS_BYTES: usize = 64;
pub(crate) const DS_OPERATIONS: usize = 96;
pub(crate) const DS_DURATION: usize = 128;
pub(crate) const DS_BUSY_TIME: usize = 192;
pub(crate) const DS_DEVICE_TYPE: usize = 260;
/// `DEVSTAT_NAME_LEN`.
pub(crate) const DEVSTAT_NAME_LEN: usize = 16;
/// `sizeof(struct bintime)`: seconds, then a 64-bit binary fraction.
pub(crate) const BINTIME_SIZE: usize = 16;
// `devstat_trans_flags`: the index of each counter.
pub(crate) const DEVSTAT_READ: usize = 1;
pub(crate) const DEVSTAT_WRITE: usize = 2;
// `devstat_type_flags`.
pub(crate) const DEVSTAT_TYPE_DIRECT: i32 = 0x000;
pub(crate) const DEVSTAT_TYPE_MASK: i32 = 0x00f;
pub(crate) const DEVSTAT_TYPE_PASS: i32 = 0x100;

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

unsafe extern "C" {
    // sys/sysctl.h
    pub(crate) fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> c_int;
    pub(crate) fn sysctl(
        name: *const c_int,
        namelen: c_uint,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> c_int;
    pub(crate) fn sysctlnametomib(
        name: *const c_char,
        mibp: *mut c_int,
        sizep: *mut usize,
    ) -> c_int;
    // kenv.h
    pub(crate) fn kenv(action: c_int, name: *const c_char, value: *mut c_char, len: c_int)
    -> c_int;
    // unistd.h
    pub(crate) fn gethostname(name: *mut c_char, namelen: usize) -> c_int;
    pub(crate) safe fn geteuid() -> Uid;
    pub(crate) safe fn getppid() -> Pid;
    // pwd.h
    pub(crate) fn getpwuid_r(
        uid: Uid,
        pwd: *mut Passwd,
        buf: *mut c_char,
        buflen: usize,
        result: *mut *mut Passwd,
    ) -> c_int;
    // sys/stat.h
    pub(crate) safe fn umask(mask: Mode) -> Mode;
    // sys/resource.h
    pub(crate) fn getrlimit(resource: c_int, rlp: *mut Rlimit) -> c_int;
    pub(crate) fn setrlimit(resource: c_int, rlp: *const Rlimit) -> c_int;
    pub(crate) fn getrusage(who: c_int, usage: *mut Rusage) -> c_int;
    // sys/mount.h
    pub(crate) fn statfs(path: *const c_char, buf: *mut Statfs) -> c_int;
    pub(crate) fn getfsstat(buf: *mut Statfs, bufsize: c_long, mode: c_int) -> c_int;
    // ifaddrs.h
    pub(crate) fn getifaddrs(ifap: *mut *mut Ifaddrs) -> c_int;
    pub(crate) fn freeifaddrs(ifp: *mut Ifaddrs);
    // stdlib.h
    pub(crate) fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
    // time.h
    pub(crate) fn clock_gettime(clock_id: c_int, tp: *mut Timespec) -> c_int;
    pub(crate) fn localtime_r(clock: *const Time, result: *mut Tm) -> *mut Tm;
    // utmpx.h
    pub(crate) fn setutxent();
    pub(crate) fn getutxent() -> *mut Utmpx;
    pub(crate) fn endutxent();
    // sys/timex.h
    pub(crate) fn ntp_adjtime(timex: *mut Timex) -> c_int;
}

#[cfg(test)]
mod tests {
    //! Every struct, offset, and constant above against `libc` (a
    //! dev-dependency used as an oracle only): size, alignment, and the offset
    //! of every field it makes public.

    use core::mem::{offset_of, size_of};

    use super::*;

    same_layout!(Statfs, libc::statfs,
        version => f_version, ftype => f_type, flags => f_flags, bsize => f_bsize,
        iosize => f_iosize, blocks => f_blocks, bfree => f_bfree, bavail => f_bavail,
        files => f_files, ffree => f_ffree, syncwrites => f_syncwrites,
        asyncwrites => f_asyncwrites, syncreads => f_syncreads, asyncreads => f_asyncreads,
        namemax => f_namemax, owner => f_owner, fsid => f_fsid,
        fstypename => f_fstypename, mntfromname => f_mntfromname, mntonname => f_mntonname);
    same_layout!(Timeval, libc::timeval, sec => tv_sec, usec => tv_usec);
    same_layout!(Timespec, libc::timespec, sec => tv_sec, nsec => tv_nsec);
    same_layout!(Rusage, libc::rusage,
        utime => ru_utime, stime => ru_stime, maxrss => ru_maxrss, ixrss => ru_ixrss,
        idrss => ru_idrss, isrss => ru_isrss, minflt => ru_minflt, majflt => ru_majflt,
        nswap => ru_nswap, inblock => ru_inblock, oublock => ru_oublock,
        msgsnd => ru_msgsnd, msgrcv => ru_msgrcv, nsignals => ru_nsignals,
        nvcsw => ru_nvcsw, nivcsw => ru_nivcsw);
    same_layout!(Rlimit, libc::rlimit, cur => rlim_cur, max => rlim_max);
    same_layout!(Passwd, libc::passwd,
        name => pw_name, passwd => pw_passwd, uid => pw_uid, gid => pw_gid,
        change => pw_change, class => pw_class, gecos => pw_gecos, dir => pw_dir,
        shell => pw_shell, expire => pw_expire, fields => pw_fields);
    same_layout!(Tm, libc::tm,
        sec => tm_sec, min => tm_min, hour => tm_hour, mday => tm_mday, mon => tm_mon,
        year => tm_year, wday => tm_wday, yday => tm_yday, isdst => tm_isdst,
        gmtoff => tm_gmtoff, zone => tm_zone);
    same_layout!(Clockinfo, libc::clockinfo,
        hz => hz, tick => tick, spare => spare, stathz => stathz, profhz => profhz);
    same_layout!(Timex, libc::timex,
        modes => modes, offset => offset, freq => freq, maxerror => maxerror,
        esterror => esterror, status => status, constant => constant, precision => precision,
        tolerance => tolerance, ppsfreq => ppsfreq, jitter => jitter, shift => shift,
        stabil => stabil, jitcnt => jitcnt, calcnt => calcnt, errcnt => errcnt, stbcnt => stbcnt);
    same_layout!(Utmpx, libc::utmpx,
        utype => ut_type, tv => ut_tv, id => ut_id, pid => ut_pid, user => ut_user,
        line => ut_line, host => ut_host, spare => __ut_spare);
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
    same_layout!(IfData, libc::if_data,
        ftype => ifi_type, physical => ifi_physical, addrlen => ifi_addrlen,
        hdrlen => ifi_hdrlen, link_state => ifi_link_state, vhid => ifi_vhid,
        datalen => ifi_datalen, mtu => ifi_mtu, metric => ifi_metric,
        baudrate => ifi_baudrate, ipackets => ifi_ipackets, ierrors => ifi_ierrors,
        opackets => ifi_opackets, oerrors => ifi_oerrors, collisions => ifi_collisions,
        ibytes => ifi_ibytes, obytes => ifi_obytes, imcasts => ifi_imcasts,
        omcasts => ifi_omcasts, iqdrops => ifi_iqdrops, oqdrops => ifi_oqdrops,
        noproto => ifi_noproto, hwassist => ifi_hwassist, epoch => __ifi_epoch,
        lastchange => __ifi_lastchange);

    /// The `kinfo_proc` and `devstat` offsets against the `libc` fields.
    const _: () = {
        assert!(size_of::<libc::kinfo_proc>() == KINFO_PROC_SIZE);
        assert!(offset_of!(libc::kinfo_proc, ki_structsize) == KI_STRUCTSIZE);
        assert!(offset_of!(libc::kinfo_proc, ki_size) == KI_SIZE);
        assert!(offset_of!(libc::kinfo_proc, ki_rssize) == KI_RSSIZE);
        assert!(offset_of!(libc::kinfo_proc, ki_start) == KI_START);
        assert!(offset_of!(libc::kinfo_proc, ki_numthreads) == KI_NUMTHREADS);
        assert!(size_of::<libc::devstat>() == DEVSTAT_SIZE);
        assert!(offset_of!(libc::devstat, device_name) == DS_DEVICE_NAME);
        assert!(offset_of!(libc::devstat, unit_number) == DS_UNIT_NUMBER);
        assert!(offset_of!(libc::devstat, bytes) == DS_BYTES);
        assert!(offset_of!(libc::devstat, operations) == DS_OPERATIONS);
        assert!(offset_of!(libc::devstat, duration) == DS_DURATION);
        assert!(offset_of!(libc::devstat, busy_time) == DS_BUSY_TIME);
        assert!(offset_of!(libc::devstat, device_type) == DS_DEVICE_TYPE);
        assert!(size_of::<libc::bintime>() == BINTIME_SIZE);
        assert!(libc::DEVSTAT_NAME_LEN as usize == DEVSTAT_NAME_LEN);
    };

    #[test]
    fn test_ffi_freebsd_constants_match_libc() {
        assert_eq!(O_NOFOLLOW, libc::O_NOFOLLOW);
        assert_eq!(
            (EPERM, ENOENT, ENOMEM, EACCES, EINVAL, ERANGE),
            (
                libc::EPERM,
                libc::ENOENT,
                libc::ENOMEM,
                libc::EACCES,
                libc::EINVAL,
                libc::ERANGE
            )
        );
        assert_eq!(RUSAGE_SELF, libc::RUSAGE_SELF);
        assert_eq!(
            (
                RLIMIT_STACK,
                RLIMIT_CORE,
                RLIMIT_MEMLOCK,
                RLIMIT_NPROC,
                RLIMIT_NOFILE,
                RLIMIT_AS
            ),
            (
                libc::RLIMIT_STACK,
                libc::RLIMIT_CORE,
                libc::RLIMIT_MEMLOCK,
                libc::RLIMIT_NPROC,
                libc::RLIMIT_NOFILE,
                libc::RLIMIT_AS
            )
        );
        assert_eq!(RLIM_INFINITY, libc::RLIM_INFINITY);
        assert_eq!(MNT_RDONLY, libc::MNT_RDONLY as u64);
        assert_eq!(MNT_LOCAL, libc::MNT_LOCAL);
        assert_eq!(MNT_IGNORE, libc::MNT_IGNORE);
        assert_eq!(MNT_NOATIME, libc::MNT_NOATIME as u64);
        assert_eq!(MNT_NOWAIT, libc::MNT_NOWAIT);
        assert_eq!(
            (TIME_ERROR, STA_UNSYNC),
            (libc::TIME_ERROR, libc::STA_UNSYNC)
        );
        assert_eq!(USER_PROCESS, libc::USER_PROCESS);
        assert_eq!(IFF_UP, libc::IFF_UP as c_uint);
        assert_eq!(IFF_LOOPBACK, libc::IFF_LOOPBACK as c_uint);
        assert_eq!(IFF_RUNNING, libc::IFF_RUNNING as c_uint);
        assert_eq!(c_int::from(AF_INET), libc::AF_INET);
        assert_eq!(c_int::from(AF_LINK), libc::AF_LINK);
        assert_eq!(c_int::from(AF_INET6), libc::AF_INET6);
        assert_eq!(PF_ROUTE, libc::PF_ROUTE);
        assert_eq!(NET_RT_DUMP, libc::NET_RT_DUMP);
        assert_eq!(c_int::from(RTM_VERSION), libc::RTM_VERSION);
        assert_eq!(c_int::from(RTM_GET), libc::RTM_GET);
        assert_eq!((RTF_UP, RTF_HOST), (libc::RTF_UP, libc::RTF_HOST));
        assert_eq!(
            (RTA_DST, RTA_GATEWAY, RTA_NETMASK),
            (libc::RTA_DST, libc::RTA_GATEWAY, libc::RTA_NETMASK)
        );
        assert_eq!((CTL_KERN, CTL_NET), (libc::CTL_KERN, libc::CTL_NET));
        assert_eq!(
            (KERN_PROC, KERN_PROC_PID, KERN_PROC_PROC, KERN_PROC_NFDS),
            (
                libc::KERN_PROC,
                libc::KERN_PROC_PID,
                libc::KERN_PROC_PROC,
                libc::KERN_PROC_NFDS
            )
        );
        assert_eq!(CLOCK_UPTIME, libc::CLOCK_UPTIME);
        assert_eq!(KENV_GET, libc::KENV_GET);
        assert_eq!(KENV_MVALLEN, libc::KENV_MVALLEN as usize);
    }
}
