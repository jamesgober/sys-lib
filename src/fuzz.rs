//! Entry points for the fuzz targets in `fuzz/`: one per parser, each feeding
//! the input through and checking the parser's promises (bounded output, no
//! panic, round trips). Compiled only under `cfg(fuzzing)`, which cargo-fuzz
//! sets; never part of the public API.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use crate::parse::{
    cgroup, cpu_list::CpuSet, cpu_name, cpuinfo, mountinfo, os_release, pid_file, proc_status,
    procfs, release, version,
};

pub fn os_release(data: &[u8]) {
    let r = os_release::parse(data);
    for v in [
        &r.id,
        &r.name,
        &r.pretty_name,
        &r.version,
        &r.version_id,
        &r.build_id,
    ]
    .into_iter()
    .flatten()
    {
        assert!(v.len() <= os_release::MAX_VALUE + 4);
    }
    assert!(r.id_like.len() <= 16);
    if let Some(id) = &r.id {
        assert_eq!(os_release::valid_id(id.clone()).as_ref(), Some(id));
    }
}

pub fn release(data: &[u8]) {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let (_, kind) = release::FILES[usize::from(selector) % release::FILES.len()];
    if let Some(l) = release::parse(kind, rest) {
        assert!(!l.id.is_empty() && !l.name.is_empty());
    }
}

pub fn version(data: &[u8]) {
    if let Ok(text) = core::str::from_utf8(data) {
        let _ = version::parse(text);
    }
}

pub fn cpu_list(data: &[u8]) {
    if let Ok(set) = CpuSet::parse(data) {
        let count = set.count();
        assert_eq!(set.iter().count(), count);
        assert!(set.iter().all(|c| set.contains(c)));
    }
}

pub fn cpuinfo(data: &[u8]) {
    let info = cpuinfo::parse(data);
    if let Some(name) = &info.model_name {
        let _ = cpu_name::clean(name);
    }
    let _ = cpu_name::arm_name(&info.arm_parts);
}

pub fn cpu_name(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let clean = cpu_name::clean(&text);
    assert!(clean.len() <= text.len().max(64));
    // Cleaning is idempotent: a clean name stays as it is.
    assert_eq!(cpu_name::clean(&clean), clean);
}

pub fn proc_status(data: &[u8]) {
    for key in [&b"Cpus_allowed_list"[..], b"Threads", b"Uid", b"VmRSS", b""] {
        let _ = proc_status::field(data, key);
        let _ = proc_status::number(data, key);
    }
}

pub fn mountinfo(data: &[u8]) {
    for m in mountinfo::mounts(data) {
        assert!(m.mount_point.len() <= data.len());
        let _ = mountinfo::has_option(m.super_options, b"ro");
    }
}

pub fn cgroup(data: &[u8]) {
    for m in cgroup::memberships(data) {
        assert!(cgroup::valid_path(m.path));
        assert!(m.path.len() <= cgroup::MAX_PATH);
    }
    if let Some(Some(q)) = cgroup::cpu_max(data) {
        assert!(cgroup::quota_cpus(q) >= 1);
    }
    let (a, b) = data.split_at(data.len() / 2);
    if let Some(Some(q)) = cgroup::cfs(a, b) {
        assert!(cgroup::quota_cpus(q) >= 1);
    }
}

pub fn pid_file(data: &[u8]) {
    if let Some(pid) = pid_file::parse(data) {
        assert!(pid > 0);
    }
}

#[cfg(feature = "instance")]
pub fn instance_id(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    if let Ok(id) = crate::InstanceId::parse(text) {
        assert_eq!(crate::InstanceId::parse(&id.to_string()).unwrap(), id);
    }
}

pub fn bytes_display(data: &[u8]) {
    use core::fmt::Write as _;
    let mut raw = [0u8; 9];
    for (dst, src) in raw.iter_mut().zip(data) {
        *dst = *src;
    }
    let value = u64::from_le_bytes(raw[..8].try_into().unwrap());
    let precision = usize::from(raw[8] % 16);
    let bytes = crate::Bytes::from(value);
    let mut out = String::new();
    write!(
        out,
        "{bytes} {bytes:#} {:.*} {}",
        precision,
        bytes,
        bytes.jedec()
    )
    .unwrap();
    assert!(out.len() < 128, "{out}");
    let p = crate::Percent::from_ratio(value, u64::from_le_bytes(raw[1..9].try_into().unwrap()));
    write!(out, " {p} {p:.9}").unwrap();
}

pub fn procfs(data: &[u8]) {
    let m = procfs::meminfo(data);
    if let (Some(a), Some(t)) = (m.available(), m.total) {
        let _ = a.min(t);
    }
    let _ = m.cache();
    for line in data.split(|&b| b == b'\n').take(64) {
        if let Some((_, ticks)) = procfs::stat_cpu_line(line) {
            assert_eq!(ticks.len(), 8);
        }
        if let Some(l) = procfs::diskstats_line(line) {
            assert!(l.name.len() <= line.len());
        }
        if let Some(l) = procfs::net_dev_line(line) {
            assert!(!l.name.is_empty());
        }
        let _ = mountinfo::parse_line(line);
    }
    if let Some((one, five, fifteen, _, _)) = procfs::loadavg(data) {
        assert!(one >= 0.0 && five >= 0.0 && fifteen >= 0.0);
    }
    let (some, full) = procfs::psi(data);
    for line in [some, full].into_iter().flatten() {
        assert!(line.avg10 >= 0.0 && line.avg60 >= 0.0 && line.avg300 >= 0.0);
    }
    let _ = procfs::space_value(data, b"btime");
    let _ = procfs::sockstat(data, b"TCP", b"inuse");
    let _ = procfs::snmp(data, b"Tcp", b"CurrEstab");
    let _ = procfs::statm(data);
    let _ = procfs::stat_start_ticks(data);
    let _ = procfs::file_nr(data);
    let _ = procfs::uptime(data);
    let _ = procfs::cgroup_bytes(data);
    if let Some(word) = procfs::bracketed(data) {
        assert!(word.len() < data.len());
    }
}
