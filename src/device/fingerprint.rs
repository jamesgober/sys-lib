//! A fingerprint hashes every available anchor separately, so a stored one can
//! tell "this machine, one part changed" from "another machine". Concatenating
//! anchors into one id would do the opposite: any one change would change
//! everything.

use core::fmt;

use crypt_io::hash::blake3;

use super::{Anchor, ct_eq};
use crate::{Error, Result};

/// The length of a stored fingerprint.
pub const FINGERPRINT_LEN: usize = 112;

const MAGIC: &[u8; 4] = b"SLFP";
const VERSION: u8 = 1;
const ANCHORS: usize = Anchor::ALL.len();
const HASHES_AT: usize = 8;
const CHECKSUM_AT: usize = HASHES_AT + ANCHORS * 16;

/// How much an anchor counts in a comparison.
const fn weight(anchor: Anchor) -> u32 {
    match anchor {
        Anchor::InstallId | Anchor::PlatformUuid => 3,
        Anchor::BoardSerial | Anchor::ProductSerial => 2,
        Anchor::CpuSignature | Anchor::PermanentMac => 1,
    }
}

/// An app-keyed 16-byte hash per anchor, or absent.
///
/// ```no_run
/// use sys_lib::device::{self, Fingerprint, Verdict};
///
/// let now = device::fingerprint("com.example.app")?;
/// let stored = Fingerprint::from_bytes(&now.to_bytes())?;
/// match now.compare(&stored).verdict {
///     Verdict::Same => {}
///     Verdict::LikelySame => { /* one part changed: update the record */ }
///     Verdict::Different => { /* treat as a new device */ }
///     _ => {}
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Fingerprint {
    present: u8,
    hashes: [[u8; 16]; ANCHORS],
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.anchors()).finish()
    }
}

impl Fingerprint {
    pub(super) const fn empty() -> Self {
        Self {
            present: 0,
            hashes: [[0; 16]; ANCHORS],
        }
    }

    pub(super) fn set(&mut self, slot: usize, tag: &[u8; 32]) {
        self.hashes[slot].copy_from_slice(&tag[..16]);
        self.present |= 1 << slot;
    }

    fn has(&self, slot: usize) -> bool {
        self.present & (1 << slot) != 0
    }

    /// Which anchors are present.
    pub fn anchors(&self) -> impl Iterator<Item = Anchor> + '_ {
        Anchor::ALL
            .into_iter()
            .enumerate()
            .filter(|&(slot, _)| self.has(slot))
            .map(|(_, a)| a)
    }

    /// Compares with a stored fingerprint, each hash in constant time.
    #[must_use]
    pub fn compare(&self, stored: &Fingerprint) -> Comparison {
        let (mut matched, mut changed, mut missing) = (Vec::new(), Vec::new(), Vec::new());
        for (slot, anchor) in Anchor::ALL.into_iter().enumerate() {
            match (self.has(slot), stored.has(slot)) {
                (true, true) if ct_eq(&self.hashes[slot], &stored.hashes[slot]) => {
                    matched.push(anchor);
                }
                (true, true) => changed.push(anchor),
                (false, false) => {}
                _ => missing.push(anchor),
            }
        }
        let weight_of = |list: &[Anchor]| list.iter().map(|&a| weight(a)).sum::<u32>();
        let strong_changed = changed.iter().filter(|&&a| weight(a) >= 2).count();
        // Identical CPUs share a signature, so it alone proves nothing.
        let identifies = matched.iter().any(|&a| a != Anchor::CpuSignature);
        let verdict = if identifies && changed.is_empty() {
            Verdict::Same
        } else if identifies && weight_of(&matched) >= 5 && strong_changed <= 1 {
            Verdict::LikelySame
        } else {
            Verdict::Different
        };
        Comparison {
            verdict,
            matched,
            changed,
            missing,
        }
    }

    /// The stored form: magic, version, anchor bitmap, two reserved bytes, six
    /// 16-byte hashes, and an 8-byte checksum.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; FINGERPRINT_LEN] {
        let mut out = [0u8; FINGERPRINT_LEN];
        out[..4].copy_from_slice(MAGIC);
        out[4] = VERSION;
        out[5] = self.present;
        for (slot, hash) in self.hashes.iter().enumerate() {
            out[HASHES_AT + slot * 16..HASHES_AT + (slot + 1) * 16].copy_from_slice(hash);
        }
        let sum = blake3(&out[..CHECKSUM_AT]);
        out[CHECKSUM_AT..].copy_from_slice(&sum[..8]);
        out
    }

    /// Reads the stored form. The checksum catches storage corruption, not
    /// tampering.
    ///
    /// # Errors
    ///
    /// `Malformed` for a wrong length, magic, or version, non-zero reserved
    /// bytes, a bitmap that disagrees with the hashes, or a bad checksum,
    /// checked in that order (E-78).
    pub fn from_bytes(bytes: &[u8]) -> Result<Fingerprint> {
        const BAD: Error = Error::Malformed {
            what: "device fingerprint",
            source: "stored fingerprint",
            line: None,
        };
        if bytes.len() != FINGERPRINT_LEN
            || bytes[..4] != *MAGIC
            || bytes[4] != VERSION
            || bytes[6..8] != [0, 0]
            || bytes[5] >> ANCHORS != 0
        {
            return Err(BAD);
        }
        let mut out = Self {
            present: bytes[5],
            ..Self::empty()
        };
        for slot in 0..ANCHORS {
            let at = HASHES_AT + slot * 16;
            out.hashes[slot].copy_from_slice(&bytes[at..at + 16]);
            if !out.has(slot) && out.hashes[slot] != [0; 16] {
                return Err(BAD);
            }
        }
        if blake3(&bytes[..CHECKSUM_AT])[..8] != bytes[CHECKSUM_AT..] {
            return Err(BAD);
        }
        Ok(out)
    }
}

/// What [`Fingerprint::compare`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Comparison {
    /// The answer.
    pub verdict: Verdict,
    /// Anchors present in both and equal.
    pub matched: Vec<Anchor>,
    /// Anchors present in both and different.
    pub changed: Vec<Anchor>,
    /// Anchors present in only one.
    pub missing: Vec<Anchor>,
}

/// Whether two fingerprints are of one machine.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Verdict {
    /// Every anchor present in both matches, and more than the CPU signature
    /// (which identical CPUs share) was compared.
    Same,
    /// Matched weight of at least 5, with at most one strong anchor changed.
    LikelySame,
    /// Neither.
    Different,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(entries: &[(Anchor, u8)]) -> Fingerprint {
        let mut f = Fingerprint::empty();
        for &(anchor, byte) in entries {
            let slot = Anchor::ALL.iter().position(|&a| a == anchor).unwrap();
            f.set(slot, &[byte; 32]);
        }
        f
    }

    const FULL: &[(Anchor, u8)] = &[
        (Anchor::InstallId, 1),
        (Anchor::PlatformUuid, 2),
        (Anchor::BoardSerial, 3),
        (Anchor::ProductSerial, 4),
        (Anchor::CpuSignature, 5),
        (Anchor::PermanentMac, 6),
    ];

    #[test]
    fn test_fingerprint_verdicts() {
        let a = with(FULL);
        assert_eq!(a.compare(&a).verdict, Verdict::Same);
        // A NIC swap: one weak anchor changed.
        let mut parts = FULL.to_vec();
        parts[5].1 = 66;
        let c = a.compare(&with(&parts));
        assert_eq!(c.verdict, Verdict::LikelySame);
        assert_eq!(c.changed, vec![Anchor::PermanentMac]);
        // A reinstall: one strong anchor changed.
        let mut parts = FULL.to_vec();
        parts[0].1 = 11;
        assert_eq!(a.compare(&with(&parts)).verdict, Verdict::LikelySame);
        // Two strong anchors changed.
        parts[1].1 = 22;
        assert_eq!(a.compare(&with(&parts)).verdict, Verdict::Different);
        // Nothing in common.
        let other: Vec<_> = FULL.iter().map(|&(a, b)| (a, b + 100)).collect();
        assert_eq!(a.compare(&with(&other)).verdict, Verdict::Different);
    }

    #[test]
    fn test_fingerprint_cpu_alone_proves_nothing() {
        let a = with(&[(Anchor::CpuSignature, 5)]);
        assert_eq!(a.compare(&a).verdict, Verdict::Different);
        assert_eq!(
            Fingerprint::empty().compare(&Fingerprint::empty()).verdict,
            Verdict::Different
        );
    }

    #[test]
    fn test_fingerprint_missing_anchors_do_not_count_against() {
        let a = with(&FULL[..2]);
        let b = with(FULL);
        let c = a.compare(&b);
        assert_eq!(c.verdict, Verdict::Same);
        assert_eq!(c.missing.len(), 4);
    }

    #[test]
    fn test_fingerprint_round_trip() {
        let f = with(&FULL[1..4]);
        let bytes = f.to_bytes();
        assert_eq!(&bytes[..5], b"SLFP\x01");
        assert_eq!(bytes[5], 0b1110);
        let back = Fingerprint::from_bytes(&bytes).unwrap();
        assert_eq!(back, f);
        assert_eq!(
            back.anchors().collect::<Vec<_>>(),
            vec![Anchor::PlatformUuid, Anchor::BoardSerial, Anchor::ProductSerial]
        );
        assert_eq!(format!("{f:?}"), "[PlatformUuid, BoardSerial, ProductSerial]");
    }

    #[test]
    fn test_fingerprint_e78_every_bad_form_is_malformed() {
        let good = with(FULL).to_bytes();
        let reject = |bytes: &[u8], why: &str| {
            assert!(
                matches!(Fingerprint::from_bytes(bytes), Err(Error::Malformed { .. })),
                "{why}"
            );
        };
        reject(&good[..FINGERPRINT_LEN - 1], "truncated");
        reject(&[good.as_slice(), &[0]].concat(), "extended");
        reject(&[], "empty");
        for (at, why) in [
            (0, "magic"),
            (4, "version"),
            (6, "reserved"),
            (7, "reserved"),
            (20, "hash without a checksum"),
            (FINGERPRINT_LEN - 1, "checksum"),
        ] {
            let mut bad = good;
            bad[at] ^= 0xFF;
            reject(&bad, why);
        }
        // A bitmap bit past the anchors.
        let mut bad = good;
        bad[5] |= 0x80;
        reject(&bad, "bitmap");
        // A hash where the bitmap says absent, even with a fresh checksum.
        let mut f = with(&FULL[..1]).to_bytes();
        f[HASHES_AT + 16] = 1;
        let sum = blake3(&f[..CHECKSUM_AT]);
        f[CHECKSUM_AT..].copy_from_slice(&sum[..8]);
        reject(&f, "inconsistent bitmap");
    }
}
