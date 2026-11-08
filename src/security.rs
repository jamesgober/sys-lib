//! The machine's security settings, read-only.
//!
//! For compliance reports, security tooling, and a service's start-up log. Like
//! [`virt`](crate::virt), it is a report, not a control: each value is what the
//! OS says, and an administrator controls the OS. Each field is `None` where
//! the OS has no unprivileged answer. Raw identifiers (TPM endorsement keys,
//! volume ids) are never read.
//!
//! ```
//! use sys_lib::security;
//!
//! let p = security::posture();
//! println!("{p}");
//! for v in p.cpu_vulnerabilities.iter().filter(|v| v.status == security::VulnStatus::Vulnerable) {
//!     println!("vulnerable: {} ({})", v.name, v.detail);
//! }
//! ```

use core::fmt;

/// The machine's security settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Posture {
    /// UEFI Secure Boot is on.
    pub secure_boot: Option<bool>,
    /// The TPM, when the OS reports one. A machine without a TPM and one that
    /// cannot tell are both `None`.
    pub tpm: Option<Tpm>,
    /// The encryption of the volume holding the OS.
    pub disk_encryption: Option<Encryption>,
    /// SELinux or AppArmor, and whether it enforces.
    pub mandatory_access_control: Option<Mac>,
    /// Address-space layout randomisation is on (for this process on Windows,
    /// system-wide elsewhere).
    pub aslr: Option<bool>,
    /// The Linux kernel lockdown mode; on Windows, virtualization-based
    /// security (`Integrity` when it runs).
    pub kernel_lockdown: Option<Lockdown>,
    /// macOS System Integrity Protection; Windows hypervisor-protected code
    /// integrity (HVCI).
    pub code_integrity: Option<bool>,
    /// The host firewall is on (Windows: the active profile's).
    pub firewall: Option<bool>,
    /// What the kernel says about each known CPU vulnerability (Linux).
    pub cpu_vulnerabilities: Vec<Vulnerability>,
    /// This process runs as root or elevated Administrator
    /// ([`process::is_elevated`](crate::process::is_elevated)).
    pub elevated: Option<bool>,
}

/// A trusted platform module.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Tpm {
    /// Its specification version.
    pub version: TpmVersion,
}

/// A TPM specification version.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum TpmVersion {
    /// TPM 1.2.
    V1_2,
    /// TPM 2.0.
    V2_0,
}

/// The encryption of a volume.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Encryption {
    /// Encrypted, by `kind`: `"LUKS"`, `"BitLocker"`, `"FileVault"`.
    Enabled {
        /// The scheme.
        kind: &'static str,
    },
    /// Not encrypted.
    Disabled,
}

/// A mandatory access control system.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Mac {
    /// `"SELinux"` or `"AppArmor"`.
    pub kind: &'static str,
    /// It enforces its policy, rather than only logging.
    pub enforcing: bool,
}

/// How far the kernel restricts even root.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Lockdown {
    /// No restriction.
    Off,
    /// Root cannot modify the running kernel.
    Integrity,
    /// Root can neither modify nor read kernel memory.
    Confidentiality,
}

/// One CPU vulnerability and what the kernel did about it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Vulnerability {
    /// The kernel's name: `"spectre_v2"`, `"meltdown"`.
    pub name: String,
    /// Its state.
    pub status: VulnStatus,
    /// The kernel's text: `"Mitigation: Enhanced / Automatic IBRS"`.
    pub detail: String,
}

/// The state of a CPU vulnerability.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum VulnStatus {
    /// This CPU is not affected.
    NotAffected,
    /// Affected, with a mitigation in place.
    Mitigated,
    /// Affected, without a full mitigation.
    Vulnerable,
    /// The kernel's text says neither.
    Unknown,
}

impl VulnStatus {
    /// The status of a Linux `/sys/devices/system/cpu/vulnerabilities/*` text.
    #[allow(dead_code)] // INTERIM until the Linux backend reads it
    pub(crate) fn of(detail: &str) -> VulnStatus {
        if detail.starts_with("Not affected") {
            Self::NotAffected
        } else if detail.starts_with("Mitigation") {
            Self::Mitigated
        } else if detail.starts_with("Vulnerable") {
            Self::Vulnerable
        } else {
            Self::Unknown
        }
    }
}

/// The machine's security settings, read once now. Never fails: each field the
/// OS does not answer is `None`. Costs 0.1 to 2 ms [est.].
#[must_use]
pub fn posture() -> Posture {
    let mut p = crate::sys::posture();
    p.elevated = crate::process::is_elevated().ok();
    p
}

impl fmt::Display for Posture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn flag(v: Option<bool>) -> &'static str {
            match v {
                Some(true) => "on",
                Some(false) => "off",
                None => "unknown",
            }
        }
        writeln!(f, "{:<18}{}", "secure boot", flag(self.secure_boot))?;
        let tpm = match self.tpm.map(|t| t.version) {
            Some(TpmVersion::V2_0) => "2.0",
            Some(TpmVersion::V1_2) => "1.2",
            None => "unknown",
        };
        writeln!(f, "{:<18}{tpm}", "tpm")?;
        let enc = match self.disk_encryption {
            Some(Encryption::Enabled { kind }) => kind,
            Some(Encryption::Disabled) => "off",
            None => "unknown",
        };
        writeln!(f, "{:<18}{enc}", "disk encryption")?;
        match self.mandatory_access_control {
            Some(m) => writeln!(
                f,
                "{:<18}{} {}",
                "access control",
                m.kind,
                if m.enforcing {
                    "enforcing"
                } else {
                    "permissive"
                }
            )?,
            None => writeln!(f, "{:<18}unknown", "access control")?,
        }
        writeln!(f, "{:<18}{}", "aslr", flag(self.aslr))?;
        let lockdown = match self.kernel_lockdown {
            Some(Lockdown::Off) => "off",
            Some(Lockdown::Integrity) => "integrity",
            Some(Lockdown::Confidentiality) => "confidentiality",
            None => "unknown",
        };
        writeln!(f, "{:<18}{lockdown}", "kernel lockdown")?;
        writeln!(f, "{:<18}{}", "code integrity", flag(self.code_integrity))?;
        writeln!(f, "{:<18}{}", "firewall", flag(self.firewall))?;
        let vulnerable = self
            .cpu_vulnerabilities
            .iter()
            .filter(|v| v.status == VulnStatus::Vulnerable)
            .count();
        writeln!(
            f,
            "{:<18}{} known, {vulnerable} vulnerable",
            "cpu issues",
            self.cpu_vulnerabilities.len()
        )?;
        write!(f, "{:<18}{}", "elevated", flag(self.elevated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_vuln_status_of() {
        assert_eq!(VulnStatus::of("Not affected"), VulnStatus::NotAffected);
        assert_eq!(
            VulnStatus::of("Mitigation: Enhanced / Automatic IBRS"),
            VulnStatus::Mitigated
        );
        assert_eq!(
            VulnStatus::of("Vulnerable: no microcode"),
            VulnStatus::Vulnerable
        );
        assert_eq!(
            VulnStatus::of("Unknown: no mitigations"),
            VulnStatus::Unknown
        );
        assert_eq!(VulnStatus::of(""), VulnStatus::Unknown);
    }

    #[test]
    fn test_security_live_posture_displays() {
        let p = posture();
        let text = p.to_string();
        assert!(text.contains("secure boot") && text.contains("elevated"));
        assert_eq!(p.elevated, crate::process::is_elevated().ok());
    }
}
