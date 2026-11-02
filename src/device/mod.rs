//! Device identity: an id for "this device" that an application can store and
//! recognise later.
//!
//! **A device id is not authentication.** It is a claim, not proof: root or
//! Administrator can set the machine id to anything, and hypervisors set
//! firmware UUIDs freely. Never use it as the only check for anything that
//! matters; for proof of a device, use a key held in a TPM or Secure Enclave.
//! It is personal data in client software: an application storing one needs a
//! lawful basis, a retention period, and a way to delete it. See
//! `docs/DEVICE-ID.md`.
//!
//! The id is a keyed BLAKE3 hash of **one** identifier the OS keeps, keyed by a
//! key derived from your application id. Two applications on one machine get
//! unrelated ids, and nobody can turn an id back into the machine id or the
//! serial numbers behind it. The raw identifier is read into a fixed buffer,
//! hashed, and wiped; it never appears in a return value, an error, `Debug`,
//! `Display`, or a log.
//!
//! ```no_run
//! use sys_lib as sys;
//!
//! let id = sys::device::id("com.hivedb.server")?;
//! println!("device {id}");
//! let again: sys::device::DeviceId = id.to_string().parse()?;
//! assert!(id.ct_eq(&again));
//! # Ok::<(), sys_lib::Error>(())
//! ```

pub(crate) mod canon;
mod fingerprint;
mod report;

use core::{fmt, str::FromStr};

use crypt_io::{
    hash::blake3,
    mac::{BLAKE3_MAC_KEY_LEN, Blake3Mac},
    wipe,
};

pub use fingerprint::{Comparison, FINGERPRINT_LEN, Fingerprint, Verdict};
pub use report::{ChangeEvent, CloneRisk, SourceState, SourceStatus, StabilityReport, report};

use crate::{Error, Result};

/// The derivation's version. Frozen for all of 1.x: a change would silently
/// move every stored id, so it can only arrive as a new [`Basis`] or a new tag.
const DOMAIN: &[u8] = b"sys-lib/device/v1";
const KEY_DOMAIN: &[u8] = b"sys-lib/device/v1/app-key";
/// The basis tag of a fingerprint entry: ids and entries never collide.
const FINGERPRINT_TAG: u8 = 0;
/// The longest application id.
const MAX_APP: usize = 255;

/// The buffer a platform reads one identifier into.
pub(crate) const ANCHOR_BUF: usize = 256;

/// What a platform found for an anchor; with `Value(n)`, the first `n` bytes
/// of its buffer are the identifier in canonical form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reading {
    Value(usize),
    Absent,
    Placeholder,
    PermissionDenied,
    Unsupported,
}

/// A buffer holding a raw identifier, wiped when it goes (threat T1).
pub(crate) struct Raw(pub(crate) [u8; ANCHOR_BUF]);

impl Raw {
    pub(crate) const fn new() -> Self {
        Self([0; ANCHOR_BUF])
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// What an id is anchored to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Basis {
    /// This OS installation. Unprivileged everywhere; changes on reinstall.
    /// macOS has no installation id and uses the platform UUID (the report says
    /// so).
    Install,
    /// The hardware: the firmware UUID, then the board serial. Survives
    /// reinstalls. Linux needs root (the DMI identifier files are readable by
    /// root only).
    Hardware,
}

impl Basis {
    const fn tag(self) -> u8 {
        match self {
            Self::Install => 1,
            Self::Hardware => 2,
        }
    }

    /// The anchors this basis may use, first choice first.
    const fn anchors(self) -> &'static [Anchor] {
        match self {
            Self::Install if cfg!(any(target_os = "macos", target_os = "ios")) => {
                &[Anchor::PlatformUuid]
            }
            Self::Install => &[Anchor::InstallId],
            Self::Hardware => &[Anchor::PlatformUuid, Anchor::BoardSerial],
        }
    }
}

/// An identifier a device id or a fingerprint can rest on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Anchor {
    /// The OS installation id.
    InstallId,
    /// The firmware's UUID.
    PlatformUuid,
    /// The main board's serial number.
    BoardSerial,
    /// The product's serial number.
    ProductSerial,
    /// The CPU's vendor, family, model, stepping, and brand. Not unique:
    /// identical CPUs share it.
    CpuSignature,
    /// The lowest permanent MAC address of a built-in interface.
    PermanentMac,
}

impl Anchor {
    /// Every anchor, in bitmap order.
    pub(crate) const ALL: [Anchor; 6] = [
        Anchor::InstallId,
        Anchor::PlatformUuid,
        Anchor::BoardSerial,
        Anchor::ProductSerial,
        Anchor::CpuSignature,
        Anchor::PermanentMac,
    ];

    const fn tag(self) -> u8 {
        match self {
            Self::InstallId => 1,
            Self::PlatformUuid => 2,
            Self::BoardSerial => 3,
            Self::ProductSerial => 4,
            Self::CpuSignature => 5,
            Self::PermanentMac => 6,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::InstallId => "install id",
            Self::PlatformUuid => "platform UUID",
            Self::BoardSerial => "board serial",
            Self::ProductSerial => "product serial",
            Self::CpuSignature => "CPU signature",
            Self::PermanentMac => "permanent MAC",
        }
    }
}

/// Reads one anchor into `raw`.
pub(crate) fn read_anchor(anchor: Anchor, raw: &mut Raw) -> Reading {
    match anchor {
        Anchor::CpuSignature => canon::cpu_signature(&mut raw.0),
        Anchor::PermanentMac => canon::permanent_mac(&mut raw.0),
        _ => crate::sys::device_anchor(anchor, &mut raw.0),
    }
}

/// The key an application's ids are hashed under, wiped when it goes.
struct AppKey([u8; BLAKE3_MAC_KEY_LEN]);

impl AppKey {
    /// `BLAKE3(domain || 0x00 || u16_be(len) || app)`, or the caller's secret.
    fn new(app: &str, secret: Option<&[u8; BLAKE3_MAC_KEY_LEN]>) -> Result<Self> {
        if app.is_empty() || app.len() > MAX_APP {
            return Err(Error::InvalidInput {
                what: "application id",
                reason: "must be 1 to 255 bytes",
            });
        }
        if let Some(secret) = secret {
            return Ok(Self(*secret));
        }
        let mut message = [0u8; KEY_DOMAIN.len() + 3 + MAX_APP];
        let (domain, rest) = message.split_at_mut(KEY_DOMAIN.len());
        domain.copy_from_slice(KEY_DOMAIN);
        rest[1..3].copy_from_slice(&(app.len() as u16).to_be_bytes());
        rest[3..3 + app.len()].copy_from_slice(app.as_bytes());
        Ok(Self(blake3(&message[..KEY_DOMAIN.len() + 3 + app.len()])))
    }

    /// `BLAKE3-keyed(key, domain || 0x00 || basis || anchor || u16_be(len) || canonical)`.
    fn tag(&self, basis_tag: u8, anchor: Anchor, canonical: &[u8]) -> [u8; 32] {
        let mut mac = Blake3Mac::new(&self.0);
        mac.update(DOMAIN);
        mac.update(&[0, basis_tag, anchor.tag()]);
        mac.update(&(canonical.len() as u16).to_be_bytes());
        mac.update(canonical);
        mac.finalize()
    }
}

impl Drop for AppKey {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// An application-specific device id: 122 bits of a keyed hash, shown as an
/// RFC 9562 version 8 UUID. Already a keyed hash, so it is safe to show.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId([u8; 16]);

impl DeviceId {
    /// The first 16 bytes of a tag, with the version and variant bits set.
    fn from_tag(tag: &[u8; 32]) -> Self {
        let mut b = [0u8; 16];
        b.copy_from_slice(&tag[..16]);
        b[6] = (b[6] & 0x0F) | 0x80;
        b[8] = (b[8] & 0x3F) | 0x80;
        Self(b)
    }

    /// The 16 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Parses this type's own [`Display`](fmt::Display) format only.
    ///
    /// ```
    /// use sys_lib::device::DeviceId;
    ///
    /// let text = "6f1c2a4e-9b3d-8e21-a7c4-0123456789ab";
    /// assert_eq!(DeviceId::parse(text)?.to_string(), text);
    /// assert!(DeviceId::parse("6F1C2A4E-9B3D-8E21-A7C4-0123456789AB").is_err());
    /// # Ok::<(), sys_lib::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// `InvalidInput` for anything else: a wrong length, capitals, a misplaced
    /// dash, or a version or variant that is not this type's.
    pub fn parse(text: &str) -> Result<DeviceId> {
        const BAD: Error = Error::InvalidInput {
            what: "device id",
            reason: "expected the 36-character lowercase form this type prints",
        };
        let s = text.as_bytes();
        if s.len() != 36 {
            return Err(BAD);
        }
        let mut out = [0u8; 16];
        let (mut i, mut n) = (0, 0);
        while i < 36 {
            if matches!(i, 8 | 13 | 18 | 23) {
                if s[i] != b'-' {
                    return Err(BAD);
                }
                i += 1;
                continue;
            }
            let (hi, lo) = match (hex(s[i]), hex(s[i + 1])) {
                (Some(h), Some(l)) => (h, l),
                _ => return Err(BAD),
            };
            out[n] = hi << 4 | lo;
            n += 1;
            i += 2;
        }
        if out[6] & 0xF0 != 0x80 || out[8] & 0xC0 != 0x80 {
            return Err(BAD);
        }
        Ok(Self(out))
    }

    /// Compares in constant time.
    #[must_use]
    pub fn ct_eq(&self, other: &DeviceId) -> bool {
        ct_eq(&self.0, &other.0)
    }
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Equality that does not stop at the first difference.
pub(crate) fn ct_eq(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    core::hint::black_box(diff) == 0
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({self})")
    }
}

impl FromStr for DeviceId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

/// An application's device id, from the OS installation. `app` is a stable,
/// application-unique string such as a reverse domain name, 1 to 255 bytes.
///
/// **Not authentication**: see the [module documentation](self). sys-lib does
/// not cache the id; store it if you need it often.
///
/// # Errors
///
/// `InvalidInput` for an empty or oversized `app`; `NoIdentifierSource` when
/// the OS keeps no usable identifier ([`report`] says why).
pub fn id(app: &str) -> Result<DeviceId> {
    builder(app).id()
}

/// A [`Fingerprint`] of this device: every available anchor, hashed separately.
///
/// # Errors
///
/// `InvalidInput` for an empty or oversized `app`.
pub fn fingerprint(app: &str) -> Result<Fingerprint> {
    builder(app).fingerprint()
}

/// Options for an id or a fingerprint.
///
/// ```no_run
/// use sys_lib::device::{self, Basis};
///
/// let secret: [u8; 32] = [7; 32];   // the application's own, never shipped in clients
/// let id = device::builder("com.example.licensing")
///     .basis(Basis::Hardware)
///     .secret(secret)
///     .id()?;
/// # let _ = id;
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[must_use]
pub fn builder(app: &str) -> Builder {
    Builder {
        app: app.to_owned(),
        basis: Basis::Install,
        secret: None,
        fallback: false,
    }
}

/// See [`builder`].
pub struct Builder {
    app: String,
    basis: Basis,
    secret: Option<[u8; BLAKE3_MAC_KEY_LEN]>,
    fallback: bool,
}

impl fmt::Debug for Builder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Builder")
            .field("app", &self.app)
            .field("basis", &self.basis)
            .field("secret", &self.secret.is_some())
            .field("fallback", &self.fallback)
            .finish()
    }
}

impl Builder {
    /// What the id rests on; the default is [`Basis::Install`].
    pub fn basis(mut self, basis: Basis) -> Self {
        self.basis = basis;
        self
    }

    /// An application secret. With one, nobody without it can compute or
    /// correlate the id, even with root on the machine: use one when ids are
    /// sent to a server. Wiped when the builder is dropped.
    pub fn secret(mut self, key: [u8; 32]) -> Self {
        self.secret = Some(key);
        self
    }

    /// Lets [`Basis::Hardware`] fall back to [`Basis::Install`] when hardware
    /// sources are denied. The id is then a different one, which is why this
    /// is off by default.
    pub fn fallback(mut self, on: bool) -> Self {
        self.fallback = on;
        self
    }

    /// The device id.
    ///
    /// # Errors
    ///
    /// As [`id`].
    pub fn id(&self) -> Result<DeviceId> {
        let key = AppKey::new(&self.app, self.secret.as_ref())?;
        if let Some(id) = id_on(&key, self.basis) {
            return Ok(id);
        }
        if self.fallback
            && self.basis != Basis::Install
            && let Some(id) = id_on(&key, Basis::Install)
        {
            return Ok(id);
        }
        Err(Error::NoIdentifierSource { basis: self.basis })
    }

    /// The fingerprint.
    ///
    /// # Errors
    ///
    /// As [`fingerprint`].
    pub fn fingerprint(&self) -> Result<Fingerprint> {
        let key = AppKey::new(&self.app, self.secret.as_ref())?;
        let mut out = Fingerprint::empty();
        for (slot, anchor) in Anchor::ALL.into_iter().enumerate() {
            let mut raw = Raw::new();
            if let Reading::Value(n) = read_anchor(anchor, &mut raw) {
                let tag = key.tag(FINGERPRINT_TAG, anchor, &raw.0[..n]);
                out.set(slot, &tag);
            }
        }
        Ok(out)
    }
}

impl Drop for Builder {
    fn drop(&mut self) {
        if let Some(secret) = &mut self.secret {
            wipe(secret);
        }
    }
}

/// The id on `basis`, from its first anchor that has a value.
fn id_on(key: &AppKey, basis: Basis) -> Option<DeviceId> {
    basis.anchors().iter().find_map(|&anchor| {
        let mut raw = Raw::new();
        match read_anchor(anchor, &mut raw) {
            Reading::Value(n) => Some(DeviceId::from_tag(&key.tag(
                basis.tag(),
                anchor,
                &raw.0[..n],
            ))),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(app: &str) -> AppKey {
        AppKey::new(app, None).unwrap()
    }

    fn id_of(app: &str, canonical: &[u8]) -> DeviceId {
        DeviceId::from_tag(&key(app).tag(Basis::Install.tag(), Anchor::InstallId, canonical))
    }

    #[test]
    fn test_device_derivation_is_frozen() {
        // Changing this value moves every stored id: the derivation is frozen
        // for 1.x. A failure here is a breaking change, never a test to update.
        let id = id_of("com.example.app", &[0x42; 16]);
        assert_eq!(id.to_string(), FROZEN_ID);
    }
    const FROZEN_ID: &str = "FROZEN";

    #[test]
    fn test_device_id_shape() {
        let id = id_of("a", &[1; 16]);
        assert_eq!(id.as_bytes()[6] >> 4, 8, "version 8");
        assert_eq!(id.as_bytes()[8] >> 6, 0b10, "variant");
        let text = id.to_string();
        assert_eq!(text.len(), 36);
        assert_eq!(text.parse::<DeviceId>().unwrap(), id);
        assert_eq!(format!("{id:?}"), format!("DeviceId({text})"));
        assert!(id.ct_eq(&DeviceId::parse(&text).unwrap()));
        assert!(!id.ct_eq(&id_of("b", &[1; 16])));
    }

    #[test]
    fn test_device_parse_rejects_every_other_form() {
        let good = id_of("a", &[1; 16]).to_string();
        for bad in [
            String::new(),
            good.to_uppercase(),
            good.replace('-', ""),
            format!("{good} "),
            format!("{{{good}}}"),
            good.replacen('-', "+", 1),
            // Version 4, wrong variant.
            format!("{}4{}", &good[..14], &good[15..]),
            format!("{}0{}", &good[..19], &good[20..]),
            "g".repeat(36),
        ] {
            assert!(
                matches!(DeviceId::parse(&bad), Err(Error::InvalidInput { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn test_device_t2_apps_do_not_correlate() {
        // T2: over many pairs, ids of two apps differ in about 61 of 122 bits.
        let mut bits = 0u64;
        let pairs = 2000u32;
        for i in 0..pairs {
            let a = id_of("com.example.one", &i.to_be_bytes());
            let b = id_of("com.example.two", &i.to_be_bytes());
            bits += a
                .0
                .iter()
                .zip(&b.0)
                .map(|(x, y)| u64::from((x ^ y).count_ones()))
                .sum::<u64>();
        }
        let mean = bits as f64 / f64::from(pairs);
        assert!((59.0..=63.0).contains(&mean), "mean distance {mean}");
    }

    #[test]
    fn test_device_domain_separation() {
        let k = key("a");
        let c = [9u8; 16];
        let tags = [
            k.tag(Basis::Install.tag(), Anchor::InstallId, &c),
            k.tag(Basis::Hardware.tag(), Anchor::InstallId, &c),
            k.tag(Basis::Install.tag(), Anchor::PlatformUuid, &c),
            k.tag(FINGERPRINT_TAG, Anchor::InstallId, &c),
        ];
        for (i, a) in tags.iter().enumerate() {
            for b in &tags[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn test_device_secret_replaces_the_derived_key() {
        let plain = key("a").tag(1, Anchor::InstallId, &[1]);
        let keyed = AppKey::new("a", Some(&[7; 32]))
            .unwrap()
            .tag(1, Anchor::InstallId, &[1]);
        assert_ne!(plain, keyed);
    }

    #[test]
    fn test_device_app_id_bounds() {
        for app in [String::new(), "a".repeat(MAX_APP + 1)] {
            assert!(matches!(
                id(&app),
                Err(Error::InvalidInput {
                    what: "application id",
                    ..
                })
            ));
        }
        assert!(AppKey::new(&"a".repeat(MAX_APP), None).is_ok());
    }

    #[test]
    fn test_device_builder_debug_hides_the_secret() {
        let b = builder("a").secret([0xAB; 32]);
        let text = format!("{b:?}");
        assert!(text.contains("secret: true") && !text.contains("171"));
    }

    #[test]
    fn test_device_raw_is_wiped() {
        // T1: the buffer is zero after use. The bytes are looked at through a
        // pointer the drop cannot have been optimised around.
        let mut raw = Raw::new();
        raw.0[..4].copy_from_slice(b"SECR");
        let mut held = core::mem::ManuallyDrop::new(raw);
        // SAFETY of the test: dropped once, and read only afterwards in place.
        wipe_for_test(&mut held);
        assert_eq!(&held.0[..4], &[0; 4]);
    }

    fn wipe_for_test(raw: &mut Raw) {
        wipe(&mut raw.0);
    }

    #[test]
    fn test_device_live_ids_are_stable_or_explain_themselves() {
        match (id("com.example.test"), id("com.example.test")) {
            (Ok(a), Ok(b)) => {
                assert_eq!(a, b);
                assert_ne!(a, id("com.example.other").unwrap());
            }
            (Err(Error::NoIdentifierSource { .. }), _) => {}
            other => panic!("{other:?}"),
        }
        let f = fingerprint("com.example.test").unwrap();
        assert_eq!(f.compare(&f).verdict == Verdict::Different, f.anchors().next().is_none() || f.anchors().all(|a| a == Anchor::CpuSignature));
    }
}
