//! CPU lists: `0-3,8-11,64`, as in `Cpus_allowed_list`, `cpuset.cpus`, and
//! `/sys/devices/system/cpu/present`. Held in a fixed bitset, so parsing never
//! allocates and works for hosts with more than 1 024 CPUs (E-14).

/// The largest CPU index plus one sys-lib accepts (threat T7's CPU budget).
pub(crate) const MAX_CPUS: usize = 16_384;
const WORDS: usize = MAX_CPUS / 64;

/// A set of CPU indexes below [`MAX_CPUS`].
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CpuSet {
    bits: [u64; WORDS],
}

impl core::fmt::Debug for CpuSet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CpuSet({} cpus)", self.count())
    }
}

/// Why a list did not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListError {
    /// Not the `a-b,c` syntax.
    Malformed,
    /// An index at or above [`MAX_CPUS`].
    TooLarge,
}

impl CpuSet {
    pub(crate) const fn empty() -> Self {
        Self { bits: [0; WORDS] }
    }

    /// Every CPU below `n` (`n` capped at [`MAX_CPUS`]).
    pub(crate) fn first(n: usize) -> Self {
        let mut s = Self::empty();
        for cpu in 0..n.min(MAX_CPUS) {
            s.insert(cpu);
        }
        s
    }

    /// Adds `cpu`; an index at or above [`MAX_CPUS`] is ignored.
    pub(crate) fn insert(&mut self, cpu: usize) {
        if cpu < MAX_CPUS {
            self.bits[cpu / 64] |= 1 << (cpu % 64);
        }
    }

    pub(crate) fn union(&mut self, other: &CpuSet) {
        for (a, b) in self.bits.iter_mut().zip(other.bits.iter()) {
            *a |= *b;
        }
    }

    pub(crate) fn contains(&self, cpu: usize) -> bool {
        cpu < MAX_CPUS && self.bits[cpu / 64] & (1 << (cpu % 64)) != 0
    }

    pub(crate) fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub(crate) fn intersect(&mut self, other: &CpuSet) {
        for (a, b) in self.bits.iter_mut().zip(other.bits.iter()) {
            *a &= *b;
        }
    }

    /// The CPUs in the set, ascending.
    pub(crate) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..MAX_CPUS).filter(move |&c| self.contains(c))
    }

    /// Parses a list. An empty list (only whitespace) is an empty set.
    pub(crate) fn parse(data: &[u8]) -> Result<Self, ListError> {
        let mut set = Self::empty();
        let data = super::trim(data);
        if data.is_empty() {
            return Ok(set);
        }
        for item in data.split(|&b| b == b',') {
            let item = super::trim(item);
            let (lo, hi) = match item.iter().position(|&b| b == b'-') {
                Some(dash) => (number(&item[..dash])?, number(&item[dash + 1..])?),
                None => {
                    let n = number(item)?;
                    (n, n)
                }
            };
            if lo > hi {
                return Err(ListError::Malformed);
            }
            if hi >= MAX_CPUS {
                return Err(ListError::TooLarge);
            }
            for cpu in lo..=hi {
                set.insert(cpu);
            }
        }
        Ok(set)
    }
}

fn number(data: &[u8]) -> Result<usize, ListError> {
    let n = super::parse_u64(data).ok_or(ListError::Malformed)?;
    usize::try_from(n).map_err(|_| ListError::TooLarge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_list_ranges_gaps_and_singles() {
        let s = CpuSet::parse(b"0-3,8-11,64\n").unwrap();
        assert_eq!(s.count(), 9);
        assert!(s.contains(64) && s.contains(8) && !s.contains(4));
        assert_eq!(
            s.iter().collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 8, 9, 10, 11, 64]
        );
    }

    #[test]
    fn test_cpu_list_more_than_1024_cpus() {
        // E-14: no fixed 1 024-bit mask.
        let s = CpuSet::parse(b"0-4095").unwrap();
        assert_eq!(s.count(), 4096);
        assert_eq!(CpuSet::parse(b"16383").unwrap().count(), 1);
        assert_eq!(CpuSet::parse(b"16384"), Err(ListError::TooLarge));
        assert_eq!(
            CpuSet::parse(b"0-99999999999999999999"),
            Err(ListError::Malformed)
        );
    }

    #[test]
    fn test_cpu_list_empty_and_malformed() {
        assert_eq!(CpuSet::parse(b"").unwrap().count(), 0);
        assert_eq!(CpuSet::parse(b" \n").unwrap().count(), 0);
        for bad in [&b"a"[..], b"1-", b"-1", b"3-1", b"1,,2", b"1--2", b"0x1"] {
            assert_eq!(
                CpuSet::parse(bad),
                Err(ListError::Malformed),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn test_cpu_list_intersect() {
        let mut a = CpuSet::parse(b"0-7").unwrap();
        a.intersect(&CpuSet::parse(b"6-9").unwrap());
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![6, 7]);
        assert_eq!(CpuSet::first(3).count(), 3);
        assert_eq!(CpuSet::first(usize::MAX).count(), MAX_CPUS);
    }
}
