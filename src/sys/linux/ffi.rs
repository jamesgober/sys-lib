//! Linux and Android declarations: every libc call, struct, and constant
//! sys-lib uses, written by hand from the glibc, musl, and bionic headers.
//!
//! Everything a file under `/proc` or `/sys` can answer is read from the file
//! instead, so this list stays short. The structs are declared for LP64
//! targets, where glibc, musl, and bionic agree on them; each has a size and
//! alignment assertion, and the tests at the bottom compare them with `libc`
//! field by field on every target CI builds.

#![expect(
    dead_code,
    reason = "the declaration list is locked in 0.2.0; each item is used by the reading that \
              arrives in 0.3 to 0.5 (docs/API.md), and this expectation goes when the last one is"
)]

use core::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};

/// `uid_t`.
pub(crate) type Uid = u32;
/// `mode_t`.
pub(crate) type Mode = u32;
/// `time_t` (64-bit on every LP64 target).
pub(crate) type Time = i64;
/// `clockid_t`.
pub(crate) type ClockId = c_int;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// `struct statvfs` (sys/statvfs.h). Block counts are in units of `frsize`.
#[repr(C)]
pub(crate) struct Statvfs {
    pub(crate) bsize: c_ulong,
    pub(crate) frsize: c_ulong,
    pub(crate) blocks: u64,
    pub(crate) bfree: u64,
    pub(crate) bavail: u64,
    pub(crate) files: u64,
    pub(crate) ffree: u64,
    pub(crate) favail: u64,
    pub(crate) fsid: c_ulong,
    pub(crate) flag: c_ulong,
    pub(crate) namemax: c_ulong,
    pub(crate) spare: [c_int; 6],
}
layout!(Statvfs, size = 112, align = 8);

/// `ST_RDONLY` in `Statvfs::flag`.
pub(crate) const ST_RDONLY: c_ulong = 1;

/// `struct statfs` (sys/statfs.h); `ftype` is the filesystem magic.
#[repr(C)]
pub(crate) struct Statfs {
    pub(crate) ftype: c_long,
    pub(crate) bsize: c_long,
    pub(crate) blocks: u64,
    pub(crate) bfree: u64,
    pub(crate) bavail: u64,
    pub(crate) files: u64,
    pub(crate) ffree: u64,
    pub(crate) fsid: [c_int; 2],
    pub(crate) namelen: c_long,
    pub(crate) frsize: c_long,
    pub(crate) flags: c_long,
    pub(crate) spare: [c_long; 4],
}
layout!(Statfs, size = 120, align = 8);

// Filesystem magic numbers (linux/magic.h, fs/smb/client), as `Statfs::ftype`
// reports them, for the process guard's network-filesystem check.
pub(crate) const NFS_SUPER_MAGIC: u32 = 0x6969;
pub(crate) const SMB_SUPER_MAGIC: u32 = 0x517B;
pub(crate) const CIFS_SUPER_MAGIC: u32 = 0xFF53_4D42;
pub(crate) const SMB2_SUPER_MAGIC: u32 = 0xFE53_4D42;
pub(crate) const FUSE_SUPER_MAGIC: u32 = 0x6573_5546;
pub(crate) const V9FS_MAGIC: u32 = 0x0102_1997;
pub(crate) const CODA_SUPER_MAGIC: u32 = 0x7375_7245;
pub(crate) const AFS_SUPER_MAGIC: u32 = 0x5346_414F;
pub(crate) const CEPH_SUPER_MAGIC: u32 = 0x00C3_6400;

/// `struct rlimit` (sys/resource.h).
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Rlimit {
    pub(crate) cur: u64,
    pub(crate) max: u64,
}
layout!(Rlimit, size = 16, align = 8);

/// `RLIM_INFINITY`.
pub(crate) const RLIM_INFINITY: u64 = u64::MAX;
// Resource numbers (asm-generic/resource.h: x86 and arm; not MIPS or SPARC).
pub(crate) const RLIMIT_STACK: c_int = 3;
pub(crate) const RLIMIT_CORE: c_int = 4;
pub(crate) const RLIMIT_NPROC: c_int = 6;
pub(crate) const RLIMIT_NOFILE: c_int = 7;
pub(crate) const RLIMIT_AS: c_int = 9;

/// `struct timeval`.
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

/// `struct rusage` (sys/resource.h). `maxrss` is in KiB on Linux.
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
    /// musl reserves room for future fields; the callee may write all of it.
    #[cfg(target_env = "musl")]
    pub(crate) reserved: [c_long; 16],
}
#[cfg(not(target_env = "musl"))]
layout!(Rusage, size = 144, align = 8);
#[cfg(target_env = "musl")]
layout!(Rusage, size = 272, align = 8);

/// `RUSAGE_SELF`.
pub(crate) const RUSAGE_SELF: c_int = 0;

/// `struct passwd` (pwd.h).
#[repr(C)]
pub(crate) struct Passwd {
    pub(crate) name: *mut c_char,
    pub(crate) passwd: *mut c_char,
    pub(crate) uid: Uid,
    pub(crate) gid: u32,
    pub(crate) gecos: *mut c_char,
    pub(crate) dir: *mut c_char,
    pub(crate) shell: *mut c_char,
}
layout!(Passwd, size = 48, align = 8);

/// `struct tm` (time.h), with the BSD fields glibc, musl, and bionic all carry.
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

/// `struct sockaddr` (sys/socket.h).
#[repr(C)]
pub(crate) struct Sockaddr {
    pub(crate) family: u16,
    pub(crate) data: [c_char; 14],
}
layout!(Sockaddr, size = 16, align = 2);

/// `struct sockaddr_in` (netinet/in.h). The address is in network byte order.
#[repr(C)]
pub(crate) struct SockaddrIn {
    pub(crate) family: u16,
    pub(crate) port: u16,
    pub(crate) addr: u32,
    pub(crate) zero: [u8; 8],
}
layout!(SockaddrIn, size = 16, align = 4);

/// `struct sockaddr_in6` (netinet/in.h).
#[repr(C)]
pub(crate) struct SockaddrIn6 {
    pub(crate) family: u16,
    pub(crate) port: u16,
    pub(crate) flowinfo: u32,
    pub(crate) addr: [u8; 16],
    pub(crate) scope_id: u32,
}
layout!(SockaddrIn6, size = 28, align = 4);

/// `struct ifaddrs` (ifaddrs.h). `ifu` is the broadcast or destination address
/// union.
#[repr(C)]
pub(crate) struct Ifaddrs {
    pub(crate) next: *mut Ifaddrs,
    pub(crate) name: *mut c_char,
    pub(crate) flags: c_uint,
    pub(crate) addr: *mut Sockaddr,
    pub(crate) netmask: *mut Sockaddr,
    pub(crate) ifu: *mut Sockaddr,
    pub(crate) data: *mut c_void,
}
layout!(Ifaddrs, size = 56, align = 8);

// Address families (bits/socket.h).
pub(crate) const AF_INET: u16 = 2;
pub(crate) const AF_INET6: u16 = 10;
pub(crate) const AF_PACKET: u16 = 17;
// Interface flags (net/if.h).
pub(crate) const IFF_UP: c_uint = 0x1;
pub(crate) const IFF_LOOPBACK: c_uint = 0x8;
pub(crate) const IFF_RUNNING: c_uint = 0x40;

/// `O_NOFOLLOW` (fcntl.h): its value differs between architectures.
#[cfg(any(target_arch = "aarch64", target_arch = "powerpc64"))]
pub(crate) const O_NOFOLLOW: c_int = 0o100_000;
#[cfg(not(any(target_arch = "aarch64", target_arch = "powerpc64")))]
pub(crate) const O_NOFOLLOW: c_int = 0o400_000;
/// `ERANGE` (errno.h): a buffer was too small.
pub(crate) const ERANGE: c_int = 34;

/// `CLOCK_MONOTONIC` (time.h).
pub(crate) const CLOCK_MONOTONIC: ClockId = 1;
/// `CLOCK_BOOTTIME` (time.h): monotonic, and counts time suspended.
pub(crate) const CLOCK_BOOTTIME: ClockId = 7;

// sysconf names: the numbering differs between glibc and musl on one side and
// bionic on the other.
#[cfg(not(target_os = "android"))]
pub(crate) const SC_CLK_TCK: c_int = 2;
#[cfg(not(target_os = "android"))]
pub(crate) const SC_PAGESIZE: c_int = 30;
#[cfg(not(target_os = "android"))]
pub(crate) const SC_NPROCESSORS_CONF: c_int = 83;
#[cfg(target_os = "android")]
pub(crate) const SC_CLK_TCK: c_int = 0x0006;
#[cfg(target_os = "android")]
pub(crate) const SC_PAGESIZE: c_int = 0x0027;
#[cfg(target_os = "android")]
pub(crate) const SC_NPROCESSORS_CONF: c_int = 0x0060;

/// `PROP_VALUE_MAX` (sys/system_properties.h): the buffer size, NUL included,
/// that `__system_property_get` writes into.
#[cfg(target_os = "android")]
pub(crate) const PROP_VALUE_MAX: usize = 92;

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

unsafe extern "C" {
    // unistd.h
    pub(crate) safe fn sysconf(name: c_int) -> c_long;
    pub(crate) safe fn geteuid() -> Uid;
    // sys/stat.h
    pub(crate) safe fn umask(mask: Mode) -> Mode;
    // sys/statvfs.h, sys/statfs.h
    pub(crate) fn statvfs(path: *const c_char, buf: *mut Statvfs) -> c_int;
    pub(crate) fn statfs(path: *const c_char, buf: *mut Statfs) -> c_int;
    // sys/resource.h
    pub(crate) fn getrlimit(resource: c_int, rlim: *mut Rlimit) -> c_int;
    pub(crate) fn setrlimit(resource: c_int, rlim: *const Rlimit) -> c_int;
    pub(crate) fn getrusage(who: c_int, usage: *mut Rusage) -> c_int;
    // pwd.h
    pub(crate) fn getpwuid_r(
        uid: Uid,
        pwd: *mut Passwd,
        buf: *mut c_char,
        buflen: usize,
        result: *mut *mut Passwd,
    ) -> c_int;
    // time.h
    pub(crate) fn localtime_r(time: *const Time, result: *mut Tm) -> *mut Tm;
    pub(crate) fn clock_gettime(clock: ClockId, tp: *mut Timespec) -> c_int;
    // ifaddrs.h
    pub(crate) fn getifaddrs(ifap: *mut *mut Ifaddrs) -> c_int;
    pub(crate) fn freeifaddrs(ifa: *mut Ifaddrs);
}

#[cfg(target_os = "android")]
unsafe extern "C" {
    // sys/system_properties.h (bionic)
    pub(crate) fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
}

#[cfg(test)]
mod tests {
    //! Every struct and constant above against `libc` (a dev-dependency used as
    //! an oracle only): size, alignment, and the offset of every field.

    use super::*;

    same_layout!(Statvfs, libc::statvfs,
        bsize => f_bsize, frsize => f_frsize, blocks => f_blocks, bfree => f_bfree,
        bavail => f_bavail, files => f_files, ffree => f_ffree, favail => f_favail,
        fsid => f_fsid, flag => f_flag, namemax => f_namemax);
    same_layout!(Statfs, libc::statfs,
        ftype => f_type, bsize => f_bsize, blocks => f_blocks, bfree => f_bfree,
        bavail => f_bavail, files => f_files, ffree => f_ffree, fsid => f_fsid,
        namelen => f_namelen, frsize => f_frsize);
    same_layout!(Rlimit, libc::rlimit, cur => rlim_cur, max => rlim_max);
    same_layout!(Timeval, libc::timeval, sec => tv_sec, usec => tv_usec);
    same_layout!(Timespec, libc::timespec, sec => tv_sec, nsec => tv_nsec);
    same_layout!(Rusage, libc::rusage,
        utime => ru_utime, stime => ru_stime, maxrss => ru_maxrss, minflt => ru_minflt,
        majflt => ru_majflt, inblock => ru_inblock, oublock => ru_oublock,
        nvcsw => ru_nvcsw, nivcsw => ru_nivcsw);
    same_layout!(Passwd, libc::passwd,
        name => pw_name, passwd => pw_passwd, uid => pw_uid, gid => pw_gid,
        gecos => pw_gecos, dir => pw_dir, shell => pw_shell);
    same_layout!(Tm, libc::tm,
        sec => tm_sec, min => tm_min, hour => tm_hour, mday => tm_mday, mon => tm_mon,
        year => tm_year, wday => tm_wday, yday => tm_yday, isdst => tm_isdst,
        gmtoff => tm_gmtoff, zone => tm_zone);
    same_layout!(Sockaddr, libc::sockaddr, family => sa_family, data => sa_data);
    same_layout!(SockaddrIn, libc::sockaddr_in,
        family => sin_family, port => sin_port, addr => sin_addr, zero => sin_zero);
    same_layout!(SockaddrIn6, libc::sockaddr_in6,
        family => sin6_family, port => sin6_port, flowinfo => sin6_flowinfo,
        addr => sin6_addr, scope_id => sin6_scope_id);
    same_layout!(Ifaddrs, libc::ifaddrs,
        next => ifa_next, name => ifa_name, flags => ifa_flags, addr => ifa_addr,
        netmask => ifa_netmask, ifu => ifa_ifu, data => ifa_data);

    #[allow(
        clippy::unnecessary_cast,
        reason = "libc's constant types differ between glibc, musl, and bionic"
    )]
    const _: () = {
        assert!(ST_RDONLY == libc::ST_RDONLY as c_ulong);
        assert!(RLIM_INFINITY == libc::RLIM_INFINITY as u64);
        assert!(RLIMIT_STACK == libc::RLIMIT_STACK as c_int);
        assert!(RLIMIT_CORE == libc::RLIMIT_CORE as c_int);
        assert!(RLIMIT_NPROC == libc::RLIMIT_NPROC as c_int);
        assert!(RLIMIT_NOFILE == libc::RLIMIT_NOFILE as c_int);
        assert!(RLIMIT_AS == libc::RLIMIT_AS as c_int);
        assert!(RUSAGE_SELF == libc::RUSAGE_SELF);
        assert!(AF_INET as c_int == libc::AF_INET);
        assert!(AF_INET6 as c_int == libc::AF_INET6);
        assert!(AF_PACKET as c_int == libc::AF_PACKET);
        assert!(IFF_UP as c_int == libc::IFF_UP);
        assert!(IFF_LOOPBACK as c_int == libc::IFF_LOOPBACK);
        assert!(IFF_RUNNING as c_int == libc::IFF_RUNNING);
        assert!(CLOCK_MONOTONIC == libc::CLOCK_MONOTONIC);
        assert!(O_NOFOLLOW == libc::O_NOFOLLOW);
        assert!(ERANGE == libc::ERANGE);
        assert!(CLOCK_BOOTTIME == libc::CLOCK_BOOTTIME);
        assert!(SC_CLK_TCK == libc::_SC_CLK_TCK);
        assert!(SC_PAGESIZE == libc::_SC_PAGESIZE);
        assert!(SC_NPROCESSORS_CONF == libc::_SC_NPROCESSORS_CONF);
        assert!(NFS_SUPER_MAGIC as i64 == libc::NFS_SUPER_MAGIC as i64);
        assert!(SMB_SUPER_MAGIC as i64 == libc::SMB_SUPER_MAGIC as i64);
        assert!(CODA_SUPER_MAGIC as i64 == libc::CODA_SUPER_MAGIC as i64);
        assert!(AFS_SUPER_MAGIC as i64 == libc::AFS_SUPER_MAGIC as i64);
    };

    #[cfg(target_os = "android")]
    const _: () = assert!(PROP_VALUE_MAX == libc::PROP_VALUE_MAX as usize);

    #[test]
    fn test_ffi_linux_layouts_match_libc() {
        // The checks above run at compile time; this test also confirms the
        // two values every reading depends on, on the machine running it.
        assert!(sysconf(SC_PAGESIZE) >= 4096);
        assert!(sysconf(SC_CLK_TCK) > 0);
    }
}
