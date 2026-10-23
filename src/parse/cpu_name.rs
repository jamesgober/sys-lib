//! CPU names: cleaning the brand strings x86 processors report (FR-11), and
//! naming ARM cores from their implementer and part numbers.

/// Cleans a brand string: trademark marks, the trailing frequency, the word
/// "CPU", core-count suffixes, integrated-graphics suffixes, and repeated
/// spaces go. The vendor-specific rules apply only to Intel, AMD, Hygon, and
/// Zhaoxin names, so names like "QEMU Virtual CPU version 2.5+" stay intact.
pub(crate) fn clean(raw: &str) -> String {
    // Marks vanish rather than becoming spaces: "FX(tm)-8350" is "FX-8350".
    // Repeated until none is left, since removing one can join another
    // ("(T(TM)M)"), so a cleaned name never cleans further (fuzzing).
    let mut s = raw.replace(['®', '™'], "");
    loop {
        let before = s.len();
        for mark in ["(R)", "(r)", "(TM)", "(tm)", "(Tm)"] {
            s = s.replace(mark, "");
        }
        if s.len() == before {
            break;
        }
    }
    // Trailing frequencies: " CPU @ 3.00GHz", " @ 2.40GHz", "@2.7GHz"; a
    // number must come before the unit.
    while let Some(at) = s.rfind('@') {
        let tail = s[at + 1..].trim().to_ascii_lowercase();
        let number = tail
            .strip_suffix("ghz")
            .or_else(|| tail.strip_suffix("mhz"));
        let is_frequency = number.is_some_and(|n| {
            let n = n.trim_end();
            n.starts_with(|c: char| c.is_ascii_digit())
                && n.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        });
        if !is_frequency {
            break;
        }
        s.truncate(at);
    }
    let mut words: Vec<&str> = s.split_whitespace().collect();
    let vendor_rules = words.first().is_some_and(|w| {
        let w = w.to_ascii_lowercase();
        matches!(
            w.as_str(),
            "intel" | "amd" | "hygon" | "zhaoxin" | "genuine"
        ) || w.ends_with("th")
            && words
                .get(2)
                .is_some_and(|i| i.eq_ignore_ascii_case("intel"))
    });
    if vendor_rules {
        // " with Radeon Graphics", " w/ Radeon 780M Graphics".
        if let Some(pos) = words
            .iter()
            .position(|w| w.eq_ignore_ascii_case("with") || *w == "w/")
            && words
                .last()
                .is_some_and(|w| w.eq_ignore_ascii_case("graphics"))
        {
            words.truncate(pos);
        }
        words.retain(|w| !w.eq_ignore_ascii_case("cpu"));
        if words
            .last()
            .is_some_and(|w| w.eq_ignore_ascii_case("processor"))
        {
            words.pop();
        }
        // "16-Core", "Eight-Core", "64-Cores", "32-core".
        if words.last().is_some_and(|w| {
            let l = w.to_ascii_lowercase();
            l.ends_with("-core") || l.ends_with("-cores")
        }) {
            words.pop();
        }
    }
    words.join(" ")
}

/// The name of an ARM core, and its rank for listing big cores first.
pub(crate) fn arm_part(implementer: u32, part: u32) -> Option<(&'static str, u8)> {
    let name = match (implementer, part) {
        (0x41, 0xc07) => ("Cortex-A7", 1),
        (0x41, 0xc09) => ("Cortex-A9", 1),
        (0x41, 0xc0f) => ("Cortex-A15", 2),
        (0x41, 0xd01) => ("Cortex-A32", 1),
        (0x41, 0xd02) => ("Cortex-A34", 1),
        (0x41, 0xd03) => ("Cortex-A53", 1),
        (0x41, 0xd04) => ("Cortex-A35", 1),
        (0x41, 0xd05) => ("Cortex-A55", 1),
        (0x41, 0xd06) => ("Cortex-A65", 1),
        (0x41, 0xd07) => ("Cortex-A57", 2),
        (0x41, 0xd08) => ("Cortex-A72", 2),
        (0x41, 0xd09) => ("Cortex-A73", 2),
        (0x41, 0xd0a) => ("Cortex-A75", 2),
        (0x41, 0xd0b) => ("Cortex-A76", 2),
        (0x41, 0xd0c) => ("Neoverse-N1", 2),
        (0x41, 0xd0d) => ("Cortex-A77", 2),
        (0x41, 0xd0e) => ("Cortex-A76AE", 2),
        (0x41, 0xd40) => ("Neoverse-V1", 3),
        (0x41, 0xd41) => ("Cortex-A78", 2),
        (0x41, 0xd42) => ("Cortex-A78AE", 2),
        (0x41, 0xd43) => ("Cortex-A65AE", 1),
        (0x41, 0xd44) => ("Cortex-X1", 3),
        (0x41, 0xd46) => ("Cortex-A510", 1),
        (0x41, 0xd47) => ("Cortex-A710", 2),
        (0x41, 0xd48) => ("Cortex-X2", 3),
        (0x41, 0xd49) => ("Neoverse-N2", 2),
        (0x41, 0xd4a) => ("Neoverse-E1", 1),
        (0x41, 0xd4b) => ("Cortex-A78C", 2),
        (0x41, 0xd4c) => ("Cortex-X1C", 3),
        (0x41, 0xd4d) => ("Cortex-A715", 2),
        (0x41, 0xd4e) => ("Cortex-X3", 3),
        (0x41, 0xd4f) => ("Neoverse-V2", 3),
        (0x41, 0xd80) => ("Cortex-A520", 1),
        (0x41, 0xd81) => ("Cortex-A720", 2),
        (0x41, 0xd82) => ("Cortex-X4", 3),
        (0x41, 0xd84) => ("Neoverse-V3", 3),
        (0x41, 0xd85) => ("Cortex-X925", 3),
        (0x41, 0xd87) => ("Cortex-A725", 2),
        (0x41, 0xd8e) => ("Neoverse-N3", 2),
        (0x43, 0x0af) => ("ThunderX2", 2),
        (0x46, 0x001) => ("A64FX", 2),
        (0x48, 0xd01) => ("TaiShan v110", 2),
        (0x4e, 0x003) => ("Denver 2", 2),
        (0x4e, 0x004) => ("Carmel", 2),
        (0x51, 0x001) => ("Oryon", 3),
        (0x61, 0x022) => ("Apple Icestorm", 1),
        (0x61, 0x023) => ("Apple Firestorm", 3),
        (0x6d, 0xd49) => ("Azure Cobalt 100", 2),
        (0xc0, 0xac3) => ("Ampere-1", 2),
        (0xc0, 0xac4) => ("Ampere-1a", 2),
        _ => return None,
    };
    Some(name)
}

/// The name for a set of ARM cores: big cores first, `" + "` between kinds.
pub(crate) fn arm_name(parts: &[(u32, u32)]) -> Option<String> {
    let mut named: Vec<(&str, u8)> = parts.iter().filter_map(|&(i, p)| arm_part(i, p)).collect();
    if named.is_empty() {
        return None;
    }
    named.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    named.dedup_by(|a, b| a.0 == b.0);
    Some(named.iter().map(|n| n.0).collect::<Vec<_>>().join(" + "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real brand strings, as `/proc/cpuinfo`, `machdep.cpu.brand_string`, and the
    /// Windows registry report them, with the expected clean name.
    const CORPUS: &[(&str, &str)] = &[
        ("Intel(R) Core(TM) i9-13900K", "Intel Core i9-13900K"),
        (
            "13th Gen Intel(R) Core(TM) i7-13700H",
            "13th Gen Intel Core i7-13700H",
        ),
        (
            "12th Gen Intel(R) Core(TM) i5-12400",
            "12th Gen Intel Core i5-12400",
        ),
        (
            "Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz",
            "Intel Core i7-8700",
        ),
        (
            "Intel(R) Core(TM) i5-10210U CPU @ 1.60GHz",
            "Intel Core i5-10210U",
        ),
        (
            "Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz",
            "Intel Core i7-4790K",
        ),
        (
            "Intel(R) Core(TM)2 Duo CPU     E8400  @ 3.00GHz",
            "Intel Core2 Duo E8400",
        ),
        ("Intel(R) Core(TM) Ultra 7 155H", "Intel Core Ultra 7 155H"),
        ("Intel(R) Core(TM) Ultra 9 285K", "Intel Core Ultra 9 285K"),
        ("Intel(R) Core(TM) i9-14900KF", "Intel Core i9-14900KF"),
        (
            "Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz",
            "Intel Xeon E5-2680 v4",
        ),
        (
            "Intel(R) Xeon(R) CPU E5-2686 v4 @ 2.30GHz",
            "Intel Xeon E5-2686 v4",
        ),
        (
            "Intel(R) Xeon(R) Platinum 8375C CPU @ 2.90GHz",
            "Intel Xeon Platinum 8375C",
        ),
        (
            "Intel(R) Xeon(R) Platinum 8488C",
            "Intel Xeon Platinum 8488C",
        ),
        (
            "Intel(R) Xeon(R) Gold 6338 CPU @ 2.00GHz",
            "Intel Xeon Gold 6338",
        ),
        ("Intel(R) Xeon(R) CPU @ 2.20GHz", "Intel Xeon"),
        ("Intel(R) Xeon(R) W-2295 CPU @ 3.00GHz", "Intel Xeon W-2295"),
        (
            "Intel(R) Pentium(R) CPU G4560 @ 3.50GHz",
            "Intel Pentium G4560",
        ),
        (
            "Intel(R) Celeron(R) N4020 CPU @ 1.10GHz",
            "Intel Celeron N4020",
        ),
        ("Intel(R) Atom(TM) CPU N270   @ 1.60GHz", "Intel Atom N270"),
        ("Intel(R) N100", "Intel N100"),
        ("Genuine Intel(R) CPU 0000 @ 2.20GHz", "Genuine Intel 0000"),
        ("AMD Ryzen 9 7950X 16-Core Processor", "AMD Ryzen 9 7950X"),
        (
            "AMD Ryzen 9 5950X 16-Core Processor            ",
            "AMD Ryzen 9 5950X",
        ),
        ("AMD Ryzen 7 5800X 8-Core Processor", "AMD Ryzen 7 5800X"),
        ("AMD Ryzen 5 3600 6-Core Processor", "AMD Ryzen 5 3600"),
        (
            "AMD Ryzen 7 5800H with Radeon Graphics",
            "AMD Ryzen 7 5800H",
        ),
        (
            "AMD Ryzen 5 PRO 4650G with Radeon Graphics",
            "AMD Ryzen 5 PRO 4650G",
        ),
        (
            "AMD Ryzen 7 7840U w/ Radeon  780M Graphics",
            "AMD Ryzen 7 7840U",
        ),
        (
            "AMD Ryzen 7 PRO 7840U w/ Radeon 780M Graphics",
            "AMD Ryzen 7 PRO 7840U",
        ),
        (
            "AMD Ryzen AI 9 HX 370 w/ Radeon 890M",
            "AMD Ryzen AI 9 HX 370 w/ Radeon 890M",
        ),
        (
            "AMD Ryzen Threadripper PRO 5995WX 64-Cores",
            "AMD Ryzen Threadripper PRO 5995WX",
        ),
        (
            "AMD Ryzen Threadripper 3990X 64-Core Processor",
            "AMD Ryzen Threadripper 3990X",
        ),
        ("AMD EPYC 7763 64-Core Processor", "AMD EPYC 7763"),
        ("AMD EPYC 9654 96-Core Processor", "AMD EPYC 9654"),
        ("AMD EPYC 7R13 Processor", "AMD EPYC 7R13"),
        (
            "AMD EPYC Processor (with IBPB)",
            "AMD EPYC Processor (with IBPB)",
        ),
        ("AMD Athlon(tm) II X2 250 Processor", "AMD Athlon II X2 250"),
        (
            "AMD Phenom(tm) II X6 1090T Processor",
            "AMD Phenom II X6 1090T",
        ),
        ("AMD FX(tm)-8350 Eight-Core Processor", "AMD FX-8350"),
        (
            "AMD A10-7850K Radeon R7, 12 Compute Cores 4C+8G",
            "AMD A10-7850K Radeon R7, 12 Compute Cores 4C+8G",
        ),
        ("AMD Custom APU 0405", "AMD Custom APU 0405"),
        ("Hygon C86 7285 32-core Processor", "Hygon C86 7285"),
        (
            "ZHAOXIN KaiXian KX-U6780A@2.7GHz",
            "ZHAOXIN KaiXian KX-U6780A",
        ),
        ("Apple M1", "Apple M1"),
        ("Apple M2 Pro", "Apple M2 Pro"),
        ("Apple M3 Max", "Apple M3 Max"),
        ("Apple M4", "Apple M4"),
        (
            "QEMU Virtual CPU version 2.5+",
            "QEMU Virtual CPU version 2.5+",
        ),
        ("Common KVM processor", "Common KVM processor"),
        (
            "Snapdragon(R) X Elite - X1E80100 - Qualcomm(R) Oryon(TM) CPU",
            "Snapdragon X Elite - X1E80100 - Qualcomm Oryon CPU",
        ),
        ("Virtual CPU 8ef5c3d4b1a2", "Virtual CPU 8ef5c3d4b1a2"),
        ("ARMv8 Processor rev 1 (v8l)", "ARMv8 Processor rev 1 (v8l)"),
        ("Intel(R) Core(TM) i3-N305", "Intel Core i3-N305"),
        (
            "Intel(R) Xeon(R) Silver 4314 CPU @ 2.40GHz",
            "Intel Xeon Silver 4314",
        ),
        (
            "Intel(R) Core(TM) i7-1185G7 @ 3.00GHz",
            "Intel Core i7-1185G7",
        ),
        (
            "Intel(R) Core(TM) m3-7Y30 CPU @ 1.00GHz",
            "Intel Core m3-7Y30",
        ),
        (
            "AMD Opteron(tm) Processor 6276",
            "AMD Opteron Processor 6276",
        ),
        ("AMD Ryzen 9 9950X 16-Core Processor", "AMD Ryzen 9 9950X"),
        (
            "AMD Eng Sample: 100-000000051-40_Y",
            "AMD Eng Sample: 100-000000051-40_Y",
        ),
    ];

    #[test]
    fn test_cpu_name_clean_corpus() {
        assert!(CORPUS.len() >= 60);
        for (raw, want) in CORPUS {
            assert_eq!(clean(raw), *want, "cleaning {raw:?}");
        }
    }

    #[test]
    fn test_cpu_name_clean_is_idempotent() {
        for (raw, _) in CORPUS {
            let once = clean(raw);
            assert_eq!(clean(&once), once, "{raw:?}");
        }
    }

    #[test]
    fn test_cpu_name_clean_edge_cases() {
        assert_eq!(clean(""), "");
        assert_eq!(clean("   "), "");
        assert_eq!(clean("@"), "@");
        assert_eq!(clean("Intel(R)"), "Intel");
        assert_eq!(clean("x @ fast"), "x @ fast");
        assert_eq!(clean("x @ mhz"), "x @ mhz");
        // Found by fuzzing: two frequencies, and marks that join into another.
        assert_eq!(clean("Old CPU @ 1.0GHz @ 2.0GHz"), "Old CPU");
        assert_eq!(clean("AMD(T(TM)M) X"), "AMD X");
        let fuzzed =
            String::from_utf8_lossy(b"09<\x15.0\xe0\xff\xff\xff@@\xc780ghz@mhz").into_owned();
        let once = clean(&fuzzed);
        assert_eq!(clean(&once), once);
    }

    #[test]
    fn test_cpu_name_arm_big_cores_first() {
        // E-16: three-tier ARM.
        let parts = [(0x41, 0xd05), (0x41, 0xd41), (0x41, 0xd44)];
        assert_eq!(
            arm_name(&parts).as_deref(),
            Some("Cortex-X1 + Cortex-A78 + Cortex-A55")
        );
        assert_eq!(arm_name(&[(0x41, 0xd0b)]).as_deref(), Some("Cortex-A76"));
        assert_eq!(
            arm_name(&[(0x41, 0xd0b), (0x41, 0xd05)]).as_deref(),
            Some("Cortex-A76 + Cortex-A55")
        );
        assert_eq!(arm_name(&[(0x99, 0x1)]), None);
        assert_eq!(arm_name(&[]), None);
    }
}
