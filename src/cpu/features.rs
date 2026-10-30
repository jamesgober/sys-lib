//! Instruction-set features, detected through std (which checks operating
//! system support as well as the CPU, so a feature the OS disabled reads
//! false), and the x86-64 microarchitecture level.

use core::fmt;
use std::sync::OnceLock;

macro_rules! features {
    ($($(#[$doc:meta])* $variant:ident = $name:literal,)*) => {
        /// An instruction-set feature. [`name`](Feature::name) is rustc's
        /// target-feature name.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        #[non_exhaustive]
        pub enum Feature {
            $($(#[$doc])* $variant,)*
        }

        impl Feature {
            const ALL: &[Feature] = &[$(Feature::$variant,)*];

            /// rustc's target-feature name: `"avx2"`, `"sse4.1"`, `"neon"`.
            ///
            /// ```
            /// assert_eq!(sys_lib::cpu::Feature::Sse41.name(), "sse4.1");
            /// ```
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Feature::$variant => $name,)*
                }
            }
        }
    };
}

features! {
    /// x86 SSE2.
    Sse2 = "sse2",
    /// x86 SSE3.
    Sse3 = "sse3",
    /// x86 SSSE3.
    Ssse3 = "ssse3",
    /// x86 SSE4.1.
    Sse41 = "sse4.1",
    /// x86 SSE4.2.
    Sse42 = "sse4.2",
    /// x86 POPCNT.
    Popcnt = "popcnt",
    /// x86 AVX.
    Avx = "avx",
    /// x86 AVX2.
    Avx2 = "avx2",
    /// x86 FMA3.
    Fma = "fma",
    /// x86 F16C.
    F16c = "f16c",
    /// x86 BMI1.
    Bmi1 = "bmi1",
    /// x86 BMI2.
    Bmi2 = "bmi2",
    /// x86 LZCNT.
    Lzcnt = "lzcnt",
    /// x86 MOVBE.
    Movbe = "movbe",
    /// x86 AVX-512 Foundation.
    Avx512f = "avx512f",
    /// x86 AVX-512 byte and word.
    Avx512bw = "avx512bw",
    /// x86 AVX-512 conflict detection.
    Avx512cd = "avx512cd",
    /// x86 AVX-512 doubleword and quadword.
    Avx512dq = "avx512dq",
    /// x86 AVX-512 vector length.
    Avx512vl = "avx512vl",
    /// x86 AVX-512 VNNI.
    Avx512vnni = "avx512vnni",
    /// x86 AVX-512 VBMI.
    Avx512vbmi = "avx512vbmi",
    /// x86 AVX-VNNI.
    AvxVnni = "avxvnni",
    /// x86 AES-NI.
    Aes = "aes",
    /// x86 carry-less multiplication.
    Pclmulqdq = "pclmulqdq",
    /// x86 vector AES.
    Vaes = "vaes",
    /// x86 vector carry-less multiplication.
    Vpclmulqdq = "vpclmulqdq",
    /// x86 SHA extensions.
    Sha = "sha",
    /// x86 RDRAND.
    Rdrand = "rdrand",
    /// x86 RDSEED.
    Rdseed = "rdseed",
    /// x86 ADX.
    Adx = "adx",
    /// x86 GFNI.
    Gfni = "gfni",
    /// AArch64 Advanced SIMD.
    Neon = "neon",
    /// AArch64 CRC32.
    Crc32 = "crc",
    /// AArch64 AES.
    AesArm = "aes",
    /// AArch64 polynomial multiply.
    Pmull = "pmull",
    /// AArch64 SHA-1 and SHA-256.
    Sha2 = "sha2",
    /// AArch64 SHA-3 and SHA-512 (std's `sha3`).
    Sha3 = "sha3",
    /// AArch64 dot product.
    Dotprod = "dotprod",
    /// AArch64 half-precision arithmetic.
    Fp16 = "fp16",
    /// AArch64 large system extensions (atomics).
    Lse = "lse",
    /// AArch64 SVE.
    Sve = "sve",
    /// AArch64 SVE2.
    Sve2 = "sve2",
    /// AArch64 release-consistent loads.
    Rcpc = "rcpc",
    /// AArch64 BFloat16.
    Bf16 = "bf16",
    /// AArch64 int8 matrix multiply.
    I8mm = "i8mm",
}

/// The features detected on this machine, as a set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Features {
    bits: u64,
}

impl Features {
    fn bit(feature: Feature) -> u64 {
        1 << (feature as u32)
    }

    fn insert(&mut self, feature: Feature) {
        self.bits |= Self::bit(feature);
    }

    /// Whether `feature` is in the set.
    #[must_use]
    pub fn contains(&self, feature: Feature) -> bool {
        self.bits & Self::bit(feature) != 0
    }

    /// The features in the set, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = Feature> + '_ {
        Feature::ALL.iter().copied().filter(|f| self.contains(*f))
    }

    /// How many features are in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bits.count_ones() as usize
    }

    /// Whether the set is empty (an architecture std detects nothing on).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }
}

impl fmt::Display for Features {
    /// The names, space-separated, for a log line: `"sse2 sse3 ... avx2 aes"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, feature) in self.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            f.write_str(feature.name())?;
        }
        Ok(())
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn detect() -> Features {
    let mut set = Features::default();
    macro_rules! check {
        ($($variant:ident: $name:tt),* $(,)?) => {
            $(if std::arch::is_x86_feature_detected!($name) { set.insert(Feature::$variant); })*
        };
    }
    check!(
        Sse2: "sse2", Sse3: "sse3", Ssse3: "ssse3", Sse41: "sse4.1", Sse42: "sse4.2",
        Popcnt: "popcnt", Avx: "avx", Avx2: "avx2", Fma: "fma", F16c: "f16c",
        Bmi1: "bmi1", Bmi2: "bmi2", Lzcnt: "lzcnt", Movbe: "movbe",
        Avx512f: "avx512f", Avx512bw: "avx512bw", Avx512cd: "avx512cd",
        Avx512dq: "avx512dq", Avx512vl: "avx512vl", Avx512vnni: "avx512vnni",
        Avx512vbmi: "avx512vbmi", AvxVnni: "avxvnni", Aes: "aes",
        Pclmulqdq: "pclmulqdq", Vaes: "vaes", Vpclmulqdq: "vpclmulqdq", Sha: "sha",
        Rdrand: "rdrand", Rdseed: "rdseed", Adx: "adx", Gfni: "gfni",
    );
    set
}

#[cfg(target_arch = "aarch64")]
fn detect() -> Features {
    let mut set = Features::default();
    macro_rules! check {
        ($($variant:ident: $name:tt),* $(,)?) => {
            $(if std::arch::is_aarch64_feature_detected!($name) { set.insert(Feature::$variant); })*
        };
    }
    check!(
        Neon: "neon", Crc32: "crc", AesArm: "aes", Pmull: "pmull", Sha2: "sha2",
        Sha3: "sha3", Dotprod: "dotprod", Fp16: "fp16", Lse: "lse", Sve: "sve",
        Sve2: "sve2", Rcpc: "rcpc", Bf16: "bf16", I8mm: "i8mm",
    );
    set
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
fn detect() -> Features {
    Features::default()
}

/// The features this machine supports, detected once (about 1 ns afterwards).
///
/// ```
/// println!("features: {}", sys_lib::cpu::features());
/// ```
#[must_use]
pub fn features() -> &'static Features {
    static FEATURES: OnceLock<Features> = OnceLock::new();
    FEATURES.get_or_init(detect)
}

/// Whether this machine supports `feature`: the same answer as std's
/// `is_x86_feature_detected!` and `is_aarch64_feature_detected!`, which the
/// compiler's `#[target_feature]` dispatch trusts.
///
/// ```
/// use sys_lib::cpu::{self, Feature};
///
/// if cpu::has(Feature::Avx2) {
///     // pick the AVX2 kernel
/// }
/// ```
#[must_use]
pub fn has(feature: Feature) -> bool {
    features().contains(feature)
}

/// An x86-64 microarchitecture level (the psABI's `x86-64-v1` to `v4`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum X86Level {
    /// The x86-64 baseline (SSE2).
    V1,
    /// Adds SSE3, SSSE3, SSE4.1, SSE4.2, POPCNT, CMPXCHG16B.
    V2,
    /// Adds AVX, AVX2, BMI1, BMI2, F16C, FMA, LZCNT, MOVBE, XSAVE.
    V3,
    /// Adds AVX-512 F, BW, CD, DQ, VL.
    V4,
}

#[cfg(target_arch = "x86_64")]
fn level() -> Option<X86Level> {
    use std::arch::is_x86_feature_detected as has;
    let v2 = has!("sse3")
        && has!("ssse3")
        && has!("sse4.1")
        && has!("sse4.2")
        && has!("popcnt")
        && has!("cmpxchg16b");
    let v3 = v2
        && has!("avx")
        && has!("avx2")
        && has!("bmi1")
        && has!("bmi2")
        && has!("f16c")
        && has!("fma")
        && has!("lzcnt")
        && has!("movbe")
        && has!("xsave");
    let v4 = v3
        && has!("avx512f")
        && has!("avx512bw")
        && has!("avx512cd")
        && has!("avx512dq")
        && has!("avx512vl");
    Some(match (v2, v3, v4) {
        (_, _, true) => X86Level::V4,
        (_, true, _) => X86Level::V3,
        (true, _, _) => X86Level::V2,
        _ => X86Level::V1,
    })
}

#[cfg(not(target_arch = "x86_64"))]
fn level() -> Option<X86Level> {
    None
}

/// The x86-64 microarchitecture level this machine reaches; `None` off
/// x86-64. Under Rosetta 2 it is what Rosetta emulates (no AVX: `V2`).
///
/// ```
/// if let Some(level) = sys_lib::cpu::x86_level() { println!("x86-64-{level:?}"); }
/// ```
#[must_use]
pub fn x86_level() -> Option<X86Level> {
    static LEVEL: OnceLock<Option<X86Level>> = OnceLock::new();
    *LEVEL.get_or_init(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_features_fit_the_bitset_and_names_are_unique_per_arch() {
        assert!(Feature::ALL.len() <= 64);
        let mut x86: Vec<&str> = Feature::ALL[..Feature::ALL
            .iter()
            .position(|f| *f == Feature::Neon)
            .unwrap()]
            .iter()
            .map(|f| f.name())
            .collect();
        x86.sort_unstable();
        x86.dedup();
        assert_eq!(x86.len(), 31);
    }

    #[test]
    fn test_cpu_features_agree_with_std_and_the_level() {
        let set = features();
        assert_eq!(set.len(), set.iter().count());
        assert_eq!(
            set.to_string().split(' ').filter(|s| !s.is_empty()).count(),
            set.len()
        );
        #[cfg(target_arch = "x86_64")]
        {
            assert!(has(Feature::Sse2), "SSE2 is the x86-64 baseline");
            assert_eq!(
                has(Feature::Avx2),
                std::arch::is_x86_feature_detected!("avx2")
            );
            let level = x86_level().unwrap();
            assert!(level >= X86Level::V1);
            if level >= X86Level::V3 {
                assert!(has(Feature::Avx2) && has(Feature::Fma));
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            assert!(has(Feature::Neon), "NEON is the AArch64 baseline");
            assert_eq!(x86_level(), None);
        }
    }
}
