//! The `/proc` and cgroup files behind the live readings: `/proc/meminfo`,
//! `/proc/stat`, `/proc/loadavg`, the pressure-stall files, `/proc/diskstats`,
//! `/proc/net/dev`, `/proc/net/sockstat`, `/proc/net/snmp`, `/proc/vmstat`,
//! `/proc/self/statm`, `/proc/self/stat`, `/proc/sys/fs/file-nr`, and the
//! cgroup memory files. Each parser is safe Rust over `&[u8]`, bounded,
//! allocation-free, and never panics.

use core::time::Duration;

use super::{lines, parse_u64, trim};

/// At most this many lines are read from a line-based file (T7).
const MAX_LINES: usize = 1 << 16;

/// The fields of `/proc/meminfo` sys-lib uses, in bytes (`kB` there is 1024).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Meminfo {
    pub(crate) total: Option<u64>,
    pub(crate) free: Option<u64>,
    pub(crate) available: Option<u64>,
    pub(crate) buffers: Option<u64>,
    pub(crate) cached: Option<u64>,
    pub(crate) sreclaimable: Option<u64>,
    pub(crate) shmem: Option<u64>,
    pub(crate) swap_total: Option<u64>,
    pub(crate) swap_free: Option<u64>,
    pub(crate) huge_total: Option<u64>,
    pub(crate) huge_free: Option<u64>,
    pub(crate) huge_rsvd: Option<u64>,
    pub(crate) huge_size: Option<u64>,
}

impl Meminfo {
    /// Available memory: `MemAvailable`, or on kernels before 3.14
    /// `MemFree + Buffers + Cached` (E-28).
    pub(crate) fn available(&self) -> Option<u64> {
        self.available.or_else(|| {
            Some(
                self.free?
                    .saturating_add(self.buffers.unwrap_or(0))
                    .saturating_add(self.cached.unwrap_or(0)),
            )
        })
    }

    /// Reclaimable cache: `Cached + SReclaimable - Shmem`.
    pub(crate) fn cache(&self) -> Option<u64> {
        Some(
            self.cached?
                .saturating_add(self.sreclaimable.unwrap_or(0))
                .saturating_sub(self.shmem.unwrap_or(0)),
        )
    }
}

/// `Key:   value [kB]` with the value in bytes when the unit is `kB`.
fn kib_line(line: &[u8]) -> Option<(&[u8], u64)> {
    let colon = line.iter().position(|&b| b == b':')?;
    let (key, rest) = (&line[..colon], trim(&line[colon + 1..]));
    let (number, unit) = match rest.iter().position(|&b| b == b' ') {
        Some(i) => (&rest[..i], trim(&rest[i..])),
        None => (rest, &b""[..]),
    };
    let n = parse_u64(number)?;
    let value = match unit {
        b"kB" => n.checked_mul(1024)?,
        b"" => n,
        _ => return None,
    };
    Some((key, value))
}

/// Parses `/proc/meminfo`.
pub(crate) fn meminfo(data: &[u8]) -> Meminfo {
    let mut m = Meminfo::default();
    for (key, value) in lines(data, 512).filter_map(kib_line) {
        let slot = match key {
            b"MemTotal" => &mut m.total,
            b"MemFree" => &mut m.free,
            b"MemAvailable" => &mut m.available,
            b"Buffers" => &mut m.buffers,
            b"Cached" => &mut m.cached,
            b"SReclaimable" => &mut m.sreclaimable,
            b"Shmem" => &mut m.shmem,
            b"SwapTotal" => &mut m.swap_total,
            b"SwapFree" => &mut m.swap_free,
            b"HugePages_Total" => &mut m.huge_total,
            b"HugePages_Free" => &mut m.huge_free,
            b"HugePages_Rsvd" => &mut m.huge_rsvd,
            b"Hugepagesize" => &mut m.huge_size,
            _ => continue,
        };
        slot.get_or_insert(value);
    }
    m
}

/// One `cpu` line of `/proc/stat`: user, nice, system, idle, iowait, irq,
/// softirq, steal, in ticks (guest time is already in user and nice).
pub(crate) type CpuTicks = [u64; 8];

/// The values of a `cpu` or `cpuN` line, and the CPU number (`None` for the
/// total).
pub(crate) fn stat_cpu_line(line: &[u8]) -> Option<(Option<u32>, CpuTicks)> {
    let rest = line.strip_prefix(b"cpu")?;
    let split = rest.iter().position(|&b| b == b' ')?;
    let id = match &rest[..split] {
        b"" => None,
        digits => Some(u32::try_from(parse_u64(digits)?).ok()?),
    };
    let mut ticks = [0u64; 8];
    let mut fields = rest[split..]
        .split(|&b| b == b' ')
        .filter(|f| !f.is_empty());
    for slot in &mut ticks[..4] {
        *slot = parse_u64(fields.next()?)?;
    }
    // Older kernels stop after idle, iowait, or irq: the rest are zero.
    for slot in &mut ticks[4..] {
        match fields.next() {
            Some(f) => *slot = parse_u64(f)?,
            None => break,
        }
    }
    Some((id, ticks))
}

/// A `key N` line's value in `/proc/stat` (`btime`, `procs_running`).
pub(crate) fn space_value(data: &[u8], key: &[u8]) -> Option<u64> {
    lines(data, MAX_LINES).find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(b" ")?;
        parse_u64(trim(rest))
    })
}

/// `/proc/loadavg`: `0.52 0.58 0.59 3/1234 56789` as the three averages,
/// runnable entities, and total entities.
pub(crate) fn loadavg(data: &[u8]) -> Option<(f32, f32, f32, u64, u64)> {
    let mut f = trim(data).split(|&b| b == b' ').filter(|f| !f.is_empty());
    let one = decimal(f.next()?)?;
    let five = decimal(f.next()?)?;
    let fifteen = decimal(f.next()?)?;
    let (running, total) = f.next().and_then(|rt| {
        let slash = rt.iter().position(|&b| b == b'/')?;
        Some((parse_u64(&rt[..slash])?, parse_u64(&rt[slash + 1..])?))
    })?;
    Some((one, five, fifteen, running, total))
}

/// A non-negative decimal such as `0.52` or `1234.5`, bounded, as `f32`.
fn decimal(data: &[u8]) -> Option<f32> {
    if data.is_empty() || data.len() > 24 {
        return None;
    }
    let (whole, frac) = match data.iter().position(|&b| b == b'.') {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &b""[..]),
    };
    let whole = parse_u64(whole)?;
    let mut value = whole as f64;
    let mut scale = 0.1f64;
    for &b in frac.iter().take(9) {
        let digit = b.checked_sub(b'0').filter(|d| *d <= 9)?;
        value += f64::from(digit) * scale;
        scale /= 10.0;
    }
    if frac.iter().skip(9).any(|b| !b.is_ascii_digit()) {
        return None;
    }
    Some(value as f32)
}

/// One line of a pressure-stall file: `some avg10=1.53 avg60=0.89 avg300=0.40 total=12345`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PsiLine {
    pub(crate) avg10: f32,
    pub(crate) avg60: f32,
    pub(crate) avg300: f32,
    pub(crate) total: Duration,
}

/// The `some` and `full` lines of a pressure-stall file.
pub(crate) fn psi(data: &[u8]) -> (Option<PsiLine>, Option<PsiLine>) {
    let mut some = None;
    let mut full = None;
    for line in lines(data, 8) {
        let (slot, rest) = if let Some(r) = line.strip_prefix(b"some ") {
            (&mut some, r)
        } else if let Some(r) = line.strip_prefix(b"full ") {
            (&mut full, r)
        } else {
            continue;
        };
        *slot = psi_fields(rest);
    }
    (some, full)
}

fn psi_fields(rest: &[u8]) -> Option<PsiLine> {
    let (mut a10, mut a60, mut a300, mut total) = (None, None, None, None);
    for field in rest.split(|&b| b == b' ').filter(|f| !f.is_empty()) {
        let eq = field.iter().position(|&b| b == b'=')?;
        let (key, value) = (&field[..eq], &field[eq + 1..]);
        match key {
            b"avg10" => a10 = decimal(value),
            b"avg60" => a60 = decimal(value),
            b"avg300" => a300 = decimal(value),
            // Microseconds.
            b"total" => total = parse_u64(value).map(Duration::from_micros),
            _ => {}
        }
    }
    Some(PsiLine {
        avg10: a10?,
        avg60: a60?,
        avg300: a300?,
        total: total?,
    })
}

/// One line of `/proc/diskstats`: the device name and the counters sys-lib
/// uses (fields 4 to 14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiskLine<'a> {
    pub(crate) name: &'a [u8],
    pub(crate) reads: u64,
    pub(crate) sectors_read: u64,
    pub(crate) read_ms: u64,
    pub(crate) writes: u64,
    pub(crate) sectors_written: u64,
    pub(crate) write_ms: u64,
    pub(crate) io_ms: u64,
}

/// Parses one `/proc/diskstats` line.
pub(crate) fn diskstats_line(line: &[u8]) -> Option<DiskLine<'_>> {
    let mut f = line.split(|&b| b == b' ').filter(|f| !f.is_empty());
    let _major = f.next()?;
    let _minor = f.next()?;
    let name = f.next()?;
    let mut n = [0u64; 10];
    for slot in &mut n {
        *slot = parse_u64(f.next()?)?;
    }
    // reads, reads merged, sectors read, ms reading, writes, writes merged,
    // sectors written, ms writing, in flight, ms doing I/O.
    Some(DiskLine {
        name,
        reads: n[0],
        sectors_read: n[2],
        read_ms: n[3],
        writes: n[4],
        sectors_written: n[6],
        write_ms: n[7],
        io_ms: n[9],
    })
}

/// One interface line of `/proc/net/dev`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NetDevLine<'a> {
    pub(crate) name: &'a [u8],
    pub(crate) rx_bytes: u64,
    pub(crate) rx_packets: u64,
    pub(crate) rx_errors: u64,
    pub(crate) rx_dropped: u64,
    pub(crate) tx_bytes: u64,
    pub(crate) tx_packets: u64,
    pub(crate) tx_errors: u64,
    pub(crate) tx_dropped: u64,
}

/// Parses one `/proc/net/dev` line (`  eth0: 1234 5 0 0 0 0 0 0 6789 ...`);
/// the two header lines give `None`.
pub(crate) fn net_dev_line(line: &[u8]) -> Option<NetDevLine<'_>> {
    let colon = line.iter().position(|&b| b == b':')?;
    let name = trim(&line[..colon]);
    if name.is_empty() || name.contains(&b'|') {
        return None;
    }
    let mut f = line[colon + 1..]
        .split(|&b| b == b' ')
        .filter(|f| !f.is_empty());
    let mut n = [0u64; 16];
    for slot in &mut n {
        *slot = parse_u64(f.next()?)?;
    }
    Some(NetDevLine {
        name,
        rx_bytes: n[0],
        rx_packets: n[1],
        rx_errors: n[2],
        rx_dropped: n[3],
        tx_bytes: n[8],
        tx_packets: n[9],
        tx_errors: n[10],
        tx_dropped: n[11],
    })
}

/// A value in `/proc/net/sockstat`: `TCP: inuse 5 orphan 0 tw 2 ...` gives
/// `sockstat(data, b"TCP", b"tw") == Some(2)`.
pub(crate) fn sockstat(data: &[u8], protocol: &[u8], key: &[u8]) -> Option<u64> {
    lines(data, 32).find_map(|line| {
        let rest = line.strip_prefix(protocol)?.strip_prefix(b":")?;
        let mut f = rest.split(|&b| b == b' ').filter(|f| !f.is_empty());
        while let Some(k) = f.next() {
            let v = f.next()?;
            if k == key {
                return parse_u64(v);
            }
        }
        None
    })
}

/// A value in `/proc/net/snmp`, whose protocols have a header line of names
/// and a line of values: `snmp(data, b"Tcp", b"CurrEstab")`.
pub(crate) fn snmp(data: &[u8], protocol: &[u8], key: &[u8]) -> Option<u64> {
    let mut rows = lines(data, 64).filter_map(|l| {
        let rest = l.strip_prefix(protocol)?.strip_prefix(b":")?;
        Some(rest)
    });
    let names = rows.next()?;
    let values = rows.next()?;
    let index = names
        .split(|&b| b == b' ')
        .filter(|f| !f.is_empty())
        .position(|n| n == key)?;
    let value = values
        .split(|&b| b == b' ')
        .filter(|f| !f.is_empty())
        .nth(index)?;
    // CurrEstab is a gauge; some counters are signed (`MaxConn -1`).
    parse_u64(value)
}

/// `/proc/self/statm`: the size and resident fields, in pages.
pub(crate) fn statm(data: &[u8]) -> Option<(u64, u64)> {
    let mut f = trim(data).split(|&b| b == b' ').filter(|f| !f.is_empty());
    Some((parse_u64(f.next()?)?, parse_u64(f.next()?)?))
}

/// Field 22 of `/proc/self/stat` (start time in ticks since boot). The command
/// name in field 2 is in parentheses and may contain spaces and parentheses,
/// so the fields are counted from the last `)`.
pub(crate) fn stat_start_ticks(data: &[u8]) -> Option<u64> {
    let close = data.iter().rposition(|&b| b == b')')?;
    // After ")": field 3 (state) is the first.
    let field = trim(&data[close + 1..])
        .split(|&b| b == b' ')
        .filter(|f| !f.is_empty())
        .nth(22 - 3)?;
    parse_u64(field)
}

/// `/proc/sys/fs/file-nr`: open, unused, and maximum file handles.
pub(crate) fn file_nr(data: &[u8]) -> Option<(u64, u64)> {
    let mut f = trim(data)
        .split(|&b| b == b'\t' || b == b' ')
        .filter(|f| !f.is_empty());
    let open = parse_u64(f.next()?)?;
    let _unused = f.next()?;
    let max = parse_u64(f.next()?)?;
    Some((open, max))
}

/// `/proc/uptime`'s first field, seconds since boot.
pub(crate) fn uptime(data: &[u8]) -> Option<Duration> {
    let first = trim(data).split(|&b| b == b' ').next()?;
    let secs = decimal(first)?;
    Some(Duration::from_secs_f64(f64::from(secs)))
}

/// A cgroup memory value: a number of bytes, or `max` (no limit: `None`).
pub(crate) fn cgroup_bytes(data: &[u8]) -> Option<Option<u64>> {
    match trim(data) {
        b"max" => Some(None),
        v => parse_u64(v).map(Some),
    }
}

/// A transparent-huge-page mode file: `always [madvise] never`, the bracketed
/// word.
pub(crate) fn bracketed(data: &[u8]) -> Option<&[u8]> {
    let open = data.iter().position(|&b| b == b'[')?;
    let close = data[open..].iter().position(|&b| b == b']')? + open;
    Some(&data[open + 1..close])
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &[u8] = b"MemTotal:       32737560 kB\nMemFree:         1234560 kB\n\
        MemAvailable:   20000000 kB\nBuffers:          100000 kB\nCached:          9000000 kB\n\
        SwapCached:            0 kB\nShmem:            500000 kB\nSReclaimable:     400000 kB\n\
        SwapTotal:       8388604 kB\nSwapFree:        8000000 kB\nHugePages_Total:       4\n\
        HugePages_Free:        3\nHugePages_Rsvd:        1\nHugepagesize:       2048 kB\n";

    #[test]
    fn test_procfs_meminfo_fields_in_bytes() {
        let m = meminfo(MEMINFO);
        assert_eq!(m.total, Some(32_737_560 * 1024));
        assert_eq!(m.available(), Some(20_000_000 * 1024));
        assert_eq!(m.cache(), Some((9_000_000 + 400_000 - 500_000) * 1024));
        assert_eq!(
            (m.huge_total, m.huge_free, m.huge_rsvd),
            (Some(4), Some(3), Some(1))
        );
        assert_eq!(m.huge_size, Some(2 << 20));
        assert_eq!(m.swap_total, Some(8_388_604 * 1024));
    }

    #[test]
    fn test_procfs_meminfo_old_kernel_and_junk() {
        // E-28: no MemAvailable.
        let m = meminfo(b"MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 10 kB\nCached: 50 kB\n");
        assert_eq!(m.available(), Some(160 * 1024));
        // Overflowing, negative, and unit-less junk is ignored, never a panic.
        let m = meminfo(
            b"MemTotal: 99999999999999999999 kB\nMemFree: -1 kB\nCached: 5 MB\nMemTotal: 7 kB\n",
        );
        assert_eq!((m.total, m.free, m.cached), (Some(7 * 1024), None, None));
    }

    #[test]
    fn test_procfs_stat_cpu_lines() {
        let (id, t) = stat_cpu_line(b"cpu  10 2 30 400 5 6 7 8 9 10").unwrap();
        assert_eq!((id, t), (None, [10, 2, 30, 400, 5, 6, 7, 8]));
        let (id, t) = stat_cpu_line(b"cpu17 1 2 3 4").unwrap();
        assert_eq!((id, t), (Some(17), [1, 2, 3, 4, 0, 0, 0, 0]));
        assert_eq!(stat_cpu_line(b"cpu 1 2 3"), None);
        assert_eq!(stat_cpu_line(b"cpux 1 2 3 4"), None);
        assert_eq!(stat_cpu_line(b"intr 1 2 3 4"), None);
        let stat = b"cpu  1 2 3 4\nbtime 1700000000\nprocs_running 3\n";
        assert_eq!(space_value(stat, b"btime"), Some(1_700_000_000));
        assert_eq!(space_value(stat, b"procs_running"), Some(3));
        assert_eq!(space_value(stat, b"procs_blocked"), None);
    }

    #[test]
    fn test_procfs_loadavg_and_decimal() {
        let (a, b, c, r, t) = loadavg(b"0.52 1.58 12.59 3/1234 56789\n").unwrap();
        assert!((a - 0.52).abs() < 1e-6 && (b - 1.58).abs() < 1e-6 && (c - 12.59).abs() < 1e-5);
        assert_eq!((r, t), (3, 1234));
        assert_eq!(loadavg(b"0.52 0.58"), None);
        assert_eq!(decimal(b"-1.0"), None);
        assert_eq!(decimal(b"1.2.3"), None);
        assert_eq!(decimal(b"7"), Some(7.0));
    }

    #[test]
    fn test_procfs_psi_some_and_full() {
        let (some, full) = psi(b"some avg10=1.53 avg60=0.89 avg300=0.40 total=12345\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n");
        let some = some.unwrap();
        assert!((some.avg10 - 1.53).abs() < 1e-6);
        assert_eq!(some.total, Duration::from_micros(12_345));
        assert_eq!(full.unwrap().avg60, 0.0);
        assert_eq!(psi(b"some avg10=1.0 avg60=2.0\n"), (None, None));
        assert_eq!(psi(b""), (None, None));
    }

    #[test]
    fn test_procfs_diskstats_line() {
        let l = diskstats_line(
            b" 259       0 nvme0n1 1000 5 20000 300 2000 10 40000 600 0 700 900 0 0 0 0",
        )
        .unwrap();
        assert_eq!(l.name, b"nvme0n1");
        assert_eq!((l.reads, l.sectors_read, l.read_ms), (1000, 20000, 300));
        assert_eq!(
            (l.writes, l.sectors_written, l.write_ms, l.io_ms),
            (2000, 40000, 600, 700)
        );
        assert_eq!(diskstats_line(b"8 0 sda 1 2 3"), None);
    }

    #[test]
    fn test_procfs_net_dev_line() {
        let l = net_dev_line(b"  eth0: 1234 5 1 2 0 0 0 0 6789 10 3 4 0 0 0 0").unwrap();
        assert_eq!(l.name, b"eth0");
        assert_eq!(
            (l.rx_bytes, l.rx_packets, l.rx_errors, l.rx_dropped),
            (1234, 5, 1, 2)
        );
        assert_eq!(
            (l.tx_bytes, l.tx_packets, l.tx_errors, l.tx_dropped),
            (6789, 10, 3, 4)
        );
        assert_eq!(
            net_dev_line(
                b"Inter-|   Receive                                                |  Transmit"
            ),
            None
        );
        assert_eq!(net_dev_line(b" face |bytes    packets errs drop"), None);
    }

    #[test]
    fn test_procfs_sockstat_and_snmp() {
        let s =
            b"sockets: used 300\nTCP: inuse 5 orphan 0 tw 2 alloc 7 mem 1\nUDP: inuse 3 mem 2\n";
        assert_eq!(sockstat(s, b"TCP", b"inuse"), Some(5));
        assert_eq!(sockstat(s, b"TCP", b"tw"), Some(2));
        assert_eq!(sockstat(s, b"UDP", b"inuse"), Some(3));
        assert_eq!(sockstat(s, b"TCP6", b"inuse"), None);
        let snmp_data = b"Ip: Forwarding DefaultTTL\nIp: 1 64\nTcp: RtoAlgorithm RtoMin MaxConn CurrEstab\nTcp: 1 200 -1 42\n";
        assert_eq!(snmp(snmp_data, b"Tcp", b"CurrEstab"), Some(42));
        assert_eq!(snmp(snmp_data, b"Tcp", b"MaxConn"), None);
        assert_eq!(snmp(snmp_data, b"Udp", b"InDatagrams"), None);
    }

    #[test]
    fn test_procfs_small_files() {
        assert_eq!(statm(b"5000 1200 300 10 0 900 0\n"), Some((5000, 1200)));
        let stat = b"4821 (my (odd) prog) S 1 4821 4821 0 -1 4194560 300 0 0 0 5 3 0 0 20 0 1 0 98765 1000000 250";
        assert_eq!(stat_start_ticks(stat), Some(98_765));
        assert_eq!(stat_start_ticks(b"1 (x) S 0"), None);
        assert_eq!(
            file_nr(b"7424\t0\t9223372036854775807\n"),
            Some((7424, 9_223_372_036_854_775_807))
        );
        assert_eq!(
            uptime(b"12345.67 400000.00\n"),
            Some(Duration::from_secs_f64(f64::from(12345.67f32)))
        );
        assert_eq!(cgroup_bytes(b"max\n"), Some(None));
        assert_eq!(cgroup_bytes(b"268435456\n"), Some(Some(268_435_456)));
        assert_eq!(cgroup_bytes(b"junk"), None);
        assert_eq!(
            bracketed(b"always [madvise] never\n"),
            Some(&b"madvise"[..])
        );
        assert_eq!(bracketed(b"always madvise never"), None);
    }

    #[test]
    fn test_procfs_parsers_never_panic_on_odd_input() {
        for input in [
            &b""[..],
            b"\xff\xfe",
            b":",
            b"cpu ",
            b"(",
            b")",
            b"[",
            b"]",
            b"=",
            b"some =",
            b"a: b: c",
            b"\n\n\n",
        ] {
            let _ = meminfo(input);
            let _ = stat_cpu_line(input);
            let _ = space_value(input, b"btime");
            let _ = loadavg(input);
            let _ = psi(input);
            let _ = diskstats_line(input);
            let _ = net_dev_line(input);
            let _ = sockstat(input, b"TCP", b"inuse");
            let _ = snmp(input, b"Tcp", b"CurrEstab");
            let _ = statm(input);
            let _ = stat_start_ticks(input);
            let _ = file_nr(input);
            let _ = uptime(input);
            let _ = cgroup_bytes(input);
            let _ = bracketed(input);
        }
    }
}
