//! Value types: [`Bytes`], [`Percent`], [`Rate`], and [`Mhz`].
//!
//! Each prints itself ("125.6 GiB", "32.8%", "12.5 MiB/s", "5.7 GHz") and
//! exposes its exact value (`raw()`, `value()`, `bytes_per_sec()`). Formatting
//! is deterministic, allocation-free, and for `Bytes`, `Rate`, and `Mhz` done
//! entirely in integer arithmetic, so the output never depends on
//! floating-point printing.

use core::{
    cmp::Ordering,
    fmt::{self, Write as _},
    hash::{Hash, Hasher},
    time::Duration,
};

use crate::fmt::{StackBuf, pad};

/// A byte count: an exact `u64` inside, with a `Display` that picks the unit.
///
/// `{}` prints binary values with IEC labels (`125.6 GiB`), `{:#}` decimal
/// values with SI labels (`134.9 GB`), and [`jedec`](Bytes::jedec) binary
/// values with JEDEC labels (`125.6 GB`, what Windows Explorer shows). One
/// decimal place by default with a trailing `.0` dropped; `{:.0}` to `{:.9}`
/// are honoured, as are width, fill, and alignment. Values under 1 KiB print
/// as integers. Rounding is half away from zero, and a value that rounds up to
/// 1024 of a unit prints as 1 of the next.
///
/// # Examples
///
/// ```
/// use sys_lib::Bytes;
///
/// let ram = Bytes::new(134_876_545_024);
/// assert_eq!(ram.to_string(), "125.6 GiB");
/// assert_eq!(format!("{ram:#}"), "134.9 GB");
/// assert_eq!(ram.jedec().to_string(), "125.6 GB");
/// assert_eq!(format!("{ram:.2}"), "125.61 GiB");
/// assert_eq!(ram.raw(), 134_876_545_024);
///
/// assert_eq!(Bytes::new(512).to_string(), "512 B");
/// assert_eq!(Bytes::from_gib(128).to_string(), "128 GiB");
/// assert_eq!(Bytes::new(1_048_575).to_string(), "1 MiB");
/// assert_eq!(Bytes::MAX.to_string(), "16 EiB");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bytes(u64);

/// Which unit system [`Bytes::display`] uses.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Units {
    /// 1024-based, IEC labels: `KiB`, `MiB`, `GiB`.
    Binary,
    /// 1000-based, SI labels: `kB`, `MB`, `GB`.
    Decimal,
    /// 1024-based, JEDEC labels: `KB`, `MB`, `GB`.
    Jedec,
}

const BINARY: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
const DECIMAL: [&str; 7] = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];
const JEDEC: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
const BITS: [&str; 7] = ["bit", "kbit", "Mbit", "Gbit", "Tbit", "Pbit", "Ebit"];
const MAX_PRECISION: usize = 9;

impl Units {
    const fn base(self) -> u128 {
        match self {
            Self::Decimal => 1000,
            Self::Binary | Self::Jedec => 1024,
        }
    }

    const fn labels(self) -> &'static [&'static str; 7] {
        match self {
            Self::Binary => &BINARY,
            Self::Decimal => &DECIMAL,
            Self::Jedec => &JEDEC,
        }
    }
}

impl Bytes {
    /// Zero bytes.
    pub const ZERO: Bytes = Bytes(0);
    /// The largest count, `u64::MAX` bytes.
    pub const MAX: Bytes = Bytes(u64::MAX);

    /// A count of `bytes`.
    #[must_use]
    pub const fn new(bytes: u64) -> Bytes {
        Bytes(bytes)
    }

    /// The exact number of bytes.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// `n` KiB (1024 bytes each), saturating at [`Bytes::MAX`].
    #[must_use]
    pub const fn from_kib(n: u64) -> Bytes {
        Bytes(n.saturating_mul(1 << 10))
    }

    /// `n` MiB, saturating at [`Bytes::MAX`].
    #[must_use]
    pub const fn from_mib(n: u64) -> Bytes {
        Bytes(n.saturating_mul(1 << 20))
    }

    /// `n` GiB, saturating at [`Bytes::MAX`].
    ///
    /// ```
    /// use sys_lib::Bytes;
    /// assert_eq!(Bytes::from_gib(2).raw(), 2 * 1024 * 1024 * 1024);
    /// assert_eq!(Bytes::from_gib(u64::MAX), Bytes::MAX);
    /// ```
    #[must_use]
    pub const fn from_gib(n: u64) -> Bytes {
        Bytes(n.saturating_mul(1 << 30))
    }

    /// `self + other`, saturating at [`Bytes::MAX`].
    #[must_use]
    pub const fn saturating_add(self, other: Bytes) -> Bytes {
        Bytes(self.0.saturating_add(other.0))
    }

    /// `self - other`, saturating at zero.
    #[must_use]
    pub const fn saturating_sub(self, other: Bytes) -> Bytes {
        Bytes(self.0.saturating_sub(other.0))
    }

    /// `self + other`, or `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, other: Bytes) -> Option<Bytes> {
        match self.0.checked_add(other.0) {
            Some(v) => Some(Bytes(v)),
            None => None,
        }
    }

    /// `self - other`, or `None` when `other` is larger.
    ///
    /// ```
    /// use sys_lib::Bytes;
    /// assert_eq!(Bytes::new(5).checked_sub(Bytes::new(3)), Some(Bytes::new(2)));
    /// assert_eq!(Bytes::new(3).checked_sub(Bytes::new(5)), None);
    /// ```
    #[must_use]
    pub const fn checked_sub(self, other: Bytes) -> Option<Bytes> {
        match self.0.checked_sub(other.0) {
            Some(v) => Some(Bytes(v)),
            None => None,
        }
    }

    /// The share of `total` this is: 0% when `total` is zero, at most 100%.
    ///
    /// ```
    /// use sys_lib::Bytes;
    /// assert_eq!(Bytes::new(41).percent_of(Bytes::new(125)).to_string(), "32.8%");
    /// ```
    #[must_use]
    pub fn percent_of(self, total: Bytes) -> Percent {
        Percent::from_ratio(self.0, total.0)
    }

    /// Formats in the given unit system.
    ///
    /// ```
    /// use sys_lib::{Bytes, Units};
    /// assert_eq!(Bytes::new(1_500_000).display(Units::Decimal).to_string(), "1.5 MB");
    /// ```
    pub fn display(self, units: Units) -> impl fmt::Display {
        Formatted { bytes: self, units }
    }

    /// Formats with binary values and IEC labels: `125.6 GiB`.
    pub fn binary(self) -> impl fmt::Display {
        self.display(Units::Binary)
    }

    /// Formats with decimal values and SI labels: `134.9 GB`.
    pub fn decimal(self) -> impl fmt::Display {
        self.display(Units::Decimal)
    }

    /// Formats with binary values and JEDEC labels: `125.6 GB`.
    pub fn jedec(self) -> impl fmt::Display {
        self.display(Units::Jedec)
    }
}

impl From<u64> for Bytes {
    fn from(bytes: u64) -> Self {
        Bytes(bytes)
    }
}

impl From<Bytes> for u64 {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let units = if f.alternate() {
            Units::Decimal
        } else {
            Units::Binary
        };
        write_bytes(f, self.0, units)
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({})", self.0)
    }
}

struct Formatted {
    bytes: Bytes,
    units: Units,
}

impl fmt::Display for Formatted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_bytes(f, self.bytes.0, self.units)
    }
}

/// The byte-count formatter, in integer arithmetic.
fn write_bytes(f: &mut fmt::Formatter<'_>, value: u64, units: Units) -> fmt::Result {
    let mut buf = StackBuf::<48>::new();
    render_bytes(&mut buf, value, units, f.precision())?;
    pad(f, buf.as_str())
}

pub(crate) fn render_bytes(
    out: &mut impl fmt::Write,
    value: u64,
    units: Units,
    precision: Option<usize>,
) -> fmt::Result {
    render_scaled(out, value, units.base(), units.labels(), precision, "")
}

/// `value` in the largest of `labels` (each `base` times the last) for which
/// it is at least 1, with `suffix` after the label.
fn render_scaled(
    out: &mut impl fmt::Write,
    value: u64,
    base: u128,
    labels: &[&str; 7],
    precision: Option<usize>,
    suffix: &str,
) -> fmt::Result {
    let v = u128::from(value);
    if v < base {
        return write!(out, "{value} {}{suffix}", labels[0]);
    }
    let digits = precision.unwrap_or(1).min(MAX_PRECISION);
    let scale = 10u128.pow(digits as u32);

    // The largest unit for which the value is at least 1.
    let mut index = 0;
    let mut divisor = 1u128;
    while index < labels.len() - 1 && v >= divisor * base {
        divisor *= base;
        index += 1;
    }
    let mut scaled = round_div(v * scale, divisor);
    // A value that rounds up to `base` of a unit carries into the next unit.
    if scaled >= base * scale && index < labels.len() - 1 {
        divisor *= base;
        index += 1;
        scaled = round_div(v * scale, divisor);
    }
    let whole = scaled / scale;
    let fraction = scaled % scale;
    write!(out, "{whole}")?;
    let keep_fraction = digits > 0 && (precision.is_some() || fraction != 0);
    if keep_fraction {
        write!(out, ".{fraction:0width$}", width = digits)?;
    }
    write!(out, " {}{suffix}", labels[index])
}

/// A transfer rate in bytes per second, for network download and upload and
/// disk read and write.
///
/// `{}` prints like [`Bytes`] with `/s` (`12.5 MiB/s`), `{:#}` in decimal
/// units (`13.1 MB/s`), and [`bits`](Rate::bits) the bit rate network links
/// are sold in (`104.9 Mbit/s`). The same precision, width, and rounding rules
/// as `Bytes` apply.
///
/// ```
/// use sys_lib::Rate;
///
/// let r = Rate::from_bytes_per_sec(13_107_200);
/// assert_eq!(r.to_string(), "12.5 MiB/s");
/// assert_eq!(format!("{r:#}"), "13.1 MB/s");
/// assert_eq!(r.bits().to_string(), "104.9 Mbit/s");
/// assert_eq!(Rate::ZERO.to_string(), "0 B/s");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Rate(u64);

impl Rate {
    /// Nothing moving.
    pub const ZERO: Rate = Rate(0);

    /// `n` bytes per second.
    #[must_use]
    pub const fn from_bytes_per_sec(n: u64) -> Rate {
        Rate(n)
    }

    /// The rate in bytes per second.
    #[must_use]
    pub const fn bytes_per_sec(self) -> u64 {
        self.0
    }

    /// The rate in bits per second, saturating at `u64::MAX`.
    #[must_use]
    pub const fn bits_per_sec(self) -> u64 {
        self.0.saturating_mul(8)
    }

    /// `bytes` moved over `interval`, rounded to the nearest byte per second;
    /// zero for an empty interval, saturating at `u64::MAX`.
    ///
    /// ```
    /// use std::time::Duration;
    /// use sys_lib::{Bytes, Rate};
    ///
    /// let r = Rate::per(Bytes::from_mib(25), Duration::from_secs(2));
    /// assert_eq!(r.to_string(), "12.5 MiB/s");
    /// assert_eq!(Rate::per(Bytes::new(5), Duration::ZERO), Rate::ZERO);
    /// ```
    #[must_use]
    pub fn per(bytes: Bytes, interval: Duration) -> Rate {
        Rate::from_delta(bytes.raw(), interval)
    }

    /// `delta` bytes over `interval`, in integer arithmetic.
    pub(crate) fn from_delta(delta: u64, interval: Duration) -> Rate {
        let nanos = interval.as_nanos();
        if nanos == 0 {
            return Rate::ZERO;
        }
        let per_sec = round_div(u128::from(delta) * 1_000_000_000, nanos);
        Rate(u64::try_from(per_sec).unwrap_or(u64::MAX))
    }

    /// Formats the bit rate with SI labels: `104.9 Mbit/s`.
    pub fn bits(self) -> impl fmt::Display {
        RateBits(self)
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let units = if f.alternate() {
            Units::Decimal
        } else {
            Units::Binary
        };
        let mut buf = StackBuf::<48>::new();
        render_scaled(
            &mut buf,
            self.0,
            units.base(),
            units.labels(),
            f.precision(),
            "/s",
        )?;
        pad(f, buf.as_str())
    }
}

impl fmt::Debug for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rate({}/s)", self.0)
    }
}

struct RateBits(Rate);

impl fmt::Display for RateBits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = StackBuf::<48>::new();
        render_scaled(
            &mut buf,
            self.0.bits_per_sec(),
            1000,
            &BITS,
            f.precision(),
            "/s",
        )?;
        pad(f, buf.as_str())
    }
}

/// A clock frequency in megahertz.
///
/// Prints `GHz` with one decimal place at or above 1000 MHz (`5.7 GHz`,
/// `3.0 GHz`) and whole `MHz` below (`800 MHz`).
///
/// ```
/// use sys_lib::Mhz;
///
/// assert_eq!(Mhz(5_700).to_string(), "5.7 GHz");
/// assert_eq!(Mhz(3_000).to_string(), "3.0 GHz");
/// assert_eq!(Mhz(800).to_string(), "800 MHz");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Mhz(pub u32);

impl fmt::Display for Mhz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = StackBuf::<24>::new();
        if self.0 >= 1000 {
            let tenths = (u64::from(self.0) + 50) / 100;
            write!(buf, "{}.{} GHz", tenths / 10, tenths % 10)?;
        } else {
            write!(buf, "{} MHz", self.0)?;
        }
        pad(f, buf.as_str())
    }
}

/// `numerator / divisor`, rounded half away from zero (both are non-negative).
const fn round_div(numerator: u128, divisor: u128) -> u128 {
    (numerator + divisor / 2) / divisor
}

/// A share between 0 and 100 percent, never NaN.
///
/// Because the value is never NaN, `Percent` is `Eq`, `Ord`, and `Hash`, so it
/// sorts and compares without surprises. `Display` prints one decimal place
/// (`32.8%`) and honours a precision (`{:.0}` gives `33%`).
///
/// # Examples
///
/// ```
/// use sys_lib::Percent;
///
/// let p = Percent::from_ratio(41, 125);
/// assert_eq!(p.to_string(), "32.8%");
/// assert_eq!(format!("{p:.0}"), "33%");
/// assert_eq!(p.complement().to_string(), "67.2%");
/// assert_eq!(Percent::from_ratio(5, 0), Percent::ZERO);
/// assert_eq!(Percent::from_ratio(9, 4), Percent::FULL);
/// assert_eq!(Percent::from_value(f32::NAN), Percent::ZERO);
/// assert!(Percent::FULL > p);
/// ```
#[derive(Clone, Copy, Default)]
pub struct Percent(f32);

impl Percent {
    /// 0%.
    pub const ZERO: Percent = Percent(0.0);
    /// 100%.
    pub const FULL: Percent = Percent(100.0);

    /// `part` as a share of `whole`, exact to 0.1%: clamped to 100%, and 0% when
    /// `whole` is zero.
    #[must_use]
    pub fn from_ratio(part: u64, whole: u64) -> Percent {
        if whole == 0 {
            return Percent::ZERO;
        }
        let tenths = round_div(u128::from(part) * 1000, u128::from(whole)).min(1000);
        // At most 1000, so the conversion is exact.
        Percent(tenths as f32 / 10.0)
    }

    /// `part` of `whole` for wide counters (nanoseconds of CPU time).
    pub(crate) fn from_ratio_u128(part: u128, whole: u128) -> Percent {
        if whole == 0 {
            return Percent::ZERO;
        }
        let tenths = round_div(part.saturating_mul(1000), whole).min(1000);
        Percent(tenths as f32 / 10.0)
    }

    /// `value` percent, clamped to 0..=100; NaN gives 0%.
    #[must_use]
    pub fn from_value(value: f32) -> Percent {
        if value.is_nan() {
            Percent::ZERO
        } else {
            // `+ 0.0` turns -0.0 into 0.0, so equal values have equal bits.
            Percent(value.clamp(0.0, 100.0) + 0.0)
        }
    }

    /// The value, 0.0 to 100.0.
    #[must_use]
    pub fn value(self) -> f32 {
        self.0
    }

    /// The value as a fraction, 0.0 to 1.0.
    #[must_use]
    pub fn fraction(self) -> f32 {
        self.0 / 100.0
    }

    /// 100% minus this.
    #[must_use]
    pub fn complement(self) -> Percent {
        Percent::from_value(100.0 - self.0)
    }
}

impl PartialEq for Percent {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Percent {}

impl PartialOrd for Percent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Percent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Hash for Percent {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = f.precision().unwrap_or(1).min(MAX_PRECISION);
        let mut buf = StackBuf::<24>::new();
        write!(buf, "{:.*}%", digits, self.0)?;
        pad(f, buf.as_str())
    }
}

impl fmt::Debug for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Percent({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn test_rate_display_and_bits() {
        let r = Rate::from_bytes_per_sec(13_107_200);
        assert_eq!(r.to_string(), "12.5 MiB/s");
        assert_eq!(format!("{r:#}"), "13.1 MB/s");
        assert_eq!(r.bits().to_string(), "104.9 Mbit/s");
        assert_eq!(format!("{:.2}", r.bits()), "104.86 Mbit/s");
        assert_eq!(format!("{r:?}"), "Rate(13107200/s)");
        assert_eq!(
            Rate::from_bytes_per_sec(100).bits().to_string(),
            "800 bit/s"
        );
        assert_eq!(Rate::from_bytes_per_sec(u64::MAX).bits_per_sec(), u64::MAX);
        assert_eq!(format!("{:>12}", Rate::ZERO), "       0 B/s");
    }

    #[test]
    fn test_rate_from_delta_rounds_and_saturates() {
        assert_eq!(
            Rate::from_delta(1000, Duration::from_millis(500)).bytes_per_sec(),
            2000
        );
        assert_eq!(
            Rate::from_delta(1, Duration::from_secs(3)).bytes_per_sec(),
            0
        );
        assert_eq!(
            Rate::from_delta(2, Duration::from_secs(3)).bytes_per_sec(),
            1
        );
        assert_eq!(
            Rate::from_delta(u64::MAX, Duration::from_nanos(1)),
            Rate(u64::MAX)
        );
        assert_eq!(Rate::from_delta(7, Duration::ZERO), Rate::ZERO);
    }

    #[test]
    fn test_mhz_display() {
        assert_eq!(Mhz(5_700).to_string(), "5.7 GHz");
        assert_eq!(Mhz(999).to_string(), "999 MHz");
        assert_eq!(Mhz(1_000).to_string(), "1.0 GHz");
        assert_eq!(Mhz(2_449).to_string(), "2.4 GHz");
        assert_eq!(Mhz(2_450).to_string(), "2.5 GHz");
        assert_eq!(Mhz(u32::MAX).to_string(), "4294967.3 GHz");
        assert_eq!(Mhz(0).to_string(), "0 MHz");
    }

    #[test]
    fn test_bytes_display_examples_from_the_api() {
        let ram = Bytes::new(134_876_545_024);
        assert_eq!(ram.to_string(), "125.6 GiB");
        assert_eq!(format!("{ram:#}"), "134.9 GB");
        assert_eq!(ram.jedec().to_string(), "125.6 GB");
        assert_eq!(format!("{ram:.2}"), "125.61 GiB");
        assert_eq!(format!("{ram:.0}"), "126 GiB");
    }

    #[test]
    fn test_bytes_display_under_one_kib_is_an_integer() {
        assert_eq!(Bytes::new(0).to_string(), "0 B");
        assert_eq!(Bytes::new(1).to_string(), "1 B");
        assert_eq!(Bytes::new(1023).to_string(), "1023 B");
        assert_eq!(format!("{:.3}", Bytes::new(512)), "512 B");
        assert_eq!(format!("{:#}", Bytes::new(999)), "999 B");
    }

    #[test]
    fn test_bytes_display_unit_boundaries() {
        assert_eq!(Bytes::new(1024).to_string(), "1 KiB");
        assert_eq!(Bytes::new(1536).to_string(), "1.5 KiB");
        assert_eq!(format!("{:#}", Bytes::new(1000)), "1 kB");
        assert_eq!(Bytes::from_mib(1).to_string(), "1 MiB");
        assert_eq!(Bytes::from_gib(128).to_string(), "128 GiB");
        assert_eq!(Bytes::new(137_331_519_488).to_string(), "127.9 GiB");
    }

    #[test]
    fn test_bytes_display_1023_99_kib_carries_to_1_mib() {
        assert_eq!(Bytes::new(1_048_575).to_string(), "1 MiB");
        assert_eq!(Bytes::new(1_048_524).to_string(), "1023.9 KiB");
        assert_eq!(format!("{:.2}", Bytes::new(1_048_575)), "1.00 MiB");
    }

    #[test]
    fn test_bytes_display_maximum() {
        assert_eq!(Bytes::MAX.to_string(), "16 EiB");
        assert_eq!(format!("{:#}", Bytes::MAX), "18.4 EB");
        assert_eq!(format!("{:.9}", Bytes::MAX), "16.000000000 EiB");
    }

    #[test]
    fn test_bytes_display_precision_capped_at_nine() {
        assert_eq!(format!("{:.30}", Bytes::from_kib(1)), "1.000000000 KiB");
    }

    #[test]
    fn test_bytes_display_width_and_alignment() {
        assert_eq!(format!("{:>10}|", Bytes::from_kib(2)), "     2 KiB|");
        assert_eq!(format!("{:<10}|", Bytes::from_kib(2)), "2 KiB     |");
        assert_eq!(format!("{:^9}|", Bytes::new(5)), "   5 B   |");
    }

    #[test]
    fn test_bytes_debug_is_exact() {
        assert_eq!(
            format!("{:?}", Bytes::new(134_876_545_024)),
            "Bytes(134876545024)"
        );
    }

    #[test]
    fn test_bytes_arithmetic_saturates_and_checks() {
        assert_eq!(Bytes::MAX.saturating_add(Bytes::new(1)), Bytes::MAX);
        assert_eq!(Bytes::ZERO.saturating_sub(Bytes::new(1)), Bytes::ZERO);
        assert_eq!(Bytes::MAX.checked_add(Bytes::new(1)), None);
        assert_eq!(Bytes::from_kib(u64::MAX), Bytes::MAX);
        assert_eq!(Bytes::from_mib(u64::MAX), Bytes::MAX);
        assert_eq!(u64::from(Bytes::from(7_u64)), 7);
    }

    #[test]
    fn test_percent_from_ratio_examples() {
        assert_eq!(Percent::from_ratio(41, 125).to_string(), "32.8%");
        assert_eq!(Percent::from_ratio(0, 7).to_string(), "0.0%");
        assert_eq!(Percent::from_ratio(7, 7), Percent::FULL);
        assert_eq!(Percent::from_ratio(1, 3).to_string(), "33.3%");
        assert_eq!(Percent::from_ratio(2, 3).to_string(), "66.7%");
        assert_eq!(Percent::from_ratio(u64::MAX, u64::MAX), Percent::FULL);
        assert_eq!(Percent::from_ratio(u64::MAX, 1), Percent::FULL);
        assert_eq!(Percent::from_ratio(1, 0), Percent::ZERO);
    }

    #[test]
    fn test_percent_from_value_clamps_and_rejects_nan() {
        assert_eq!(Percent::from_value(-5.0), Percent::ZERO);
        assert_eq!(Percent::from_value(250.0), Percent::FULL);
        assert_eq!(Percent::from_value(f32::NAN), Percent::ZERO);
        assert_eq!(Percent::from_value(f32::INFINITY), Percent::FULL);
        assert_eq!(Percent::from_value(-0.0), Percent::ZERO);
    }

    #[test]
    fn test_percent_display_precision_and_padding() {
        let p = Percent::from_ratio(41, 125);
        assert_eq!(format!("{p:.0}"), "33%");
        assert_eq!(format!("{p:.2}"), "32.80%");
        assert_eq!(format!("{p:>7}"), "  32.8%");
        assert_eq!(format!("{p:?}"), "Percent(32.8)");
        assert!((p.fraction() - 0.328).abs() < 1e-6);
        assert_eq!(p.complement().to_string(), "67.2%");
    }

    /// Parses the number in front of the unit of a formatted `Bytes`.
    fn parse_display(s: &str) -> (f64, &str) {
        let (num, unit) = s.split_once(' ').unwrap();
        (num.parse().unwrap(), unit)
    }

    proptest! {
        #[test]
        fn prop_bytes_display_round_trips_within_rounding(v in any::<u64>()) {
            for (units, base) in [(Units::Binary, 1024f64), (Units::Decimal, 1000f64)] {
                let s = Bytes::new(v).display(units).to_string();
                let (num, unit) = parse_display(&s);
                let index = units.labels().iter().position(|&l| l == unit).unwrap();
                prop_assert!(num < base || index == 6, "{s} should have carried");
                let back = num * base.powi(index as i32);
                // One decimal place: within half a tenth of the unit.
                let tolerance = 0.05 * base.powi(index as i32) + 0.5;
                prop_assert!((back - v as f64).abs() <= tolerance, "{v} -> {s}");
            }
        }

        #[test]
        fn prop_bytes_order_matches_raw(a in any::<u64>(), b in any::<u64>()) {
            prop_assert_eq!(Bytes::new(a).cmp(&Bytes::new(b)), a.cmp(&b));
        }

        #[test]
        fn prop_percent_always_in_range(part in any::<u64>(), whole in any::<u64>(), v in any::<f32>()) {
            for p in [Percent::from_ratio(part, whole), Percent::from_value(v)] {
                prop_assert!(!p.value().is_nan());
                prop_assert!((0.0..=100.0).contains(&p.value()));
                prop_assert!((0.0..=100.0).contains(&p.complement().value()));
            }
        }

        #[test]
        fn prop_percent_eq_matches_hash(a in any::<f32>(), b in any::<f32>()) {
            use std::collections::hash_map::DefaultHasher;
            let (pa, pb) = (Percent::from_value(a), Percent::from_value(b));
            let hash = |p: Percent| { let mut h = DefaultHasher::new(); p.hash(&mut h); h.finish() };
            if pa == pb { prop_assert_eq!(hash(pa), hash(pb)); }
            prop_assert_eq!(pa == pb, pa.cmp(&pb) == Ordering::Equal);
        }
    }
}
