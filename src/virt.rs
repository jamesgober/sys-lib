//! Virtualization and containers: what this program runs inside.
//!
//! The answer is layered checks (CPUID, firmware strings, hypervisor files,
//! container markers, the Kubernetes service account, WSL's kernel release),
//! read once and cached. Every check that fired is named in
//! [`Virtualization::evidence`], as a constant string, never a raw value.
//!
//! **Detection is advisory.** Do not make security decisions on it: any of
//! these markers can be planted, and a guest can hide from all of them
//! (threat T14).
//!
//! ```
//! use sys_lib as sys;
//!
//! let v = sys::virt::detect();
//! if sys::virt::is_container() {
//!     println!("in {:?}; kubernetes: {}", v.container, v.kubernetes);
//! }
//! for e in &v.evidence {
//!     println!("  {} ({:?})", e.check, e.strength);
//! }
//! ```

use std::sync::OnceLock;

/// What this program runs inside. A container inside a VM sets both
/// `container` and `hypervisor`; inside WSL 2 both `hypervisor` and `wsl`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Virtualization {
    /// The innermost hypervisor visible to this machine.
    pub hypervisor: Option<Hypervisor>,
    /// The container runtime.
    pub container: Option<Container>,
    /// The WSL version, 1 or 2.
    pub wsl: Option<u8>,
    /// Running under Kubernetes. With [`Strength::Weak`] evidence only, this
    /// rests on an environment variable (E-87).
    pub kubernetes: bool,
    /// Every check that fired.
    pub evidence: Vec<Evidence>,
}

impl Virtualization {
    /// Records a check that fired.
    pub(crate) fn fired(&mut self, check: &'static str, strength: Strength) {
        self.evidence.push(Evidence { check, strength });
    }
}

/// A hypervisor.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Hypervisor {
    /// Linux KVM.
    Kvm,
    /// QEMU without KVM (TCG).
    Qemu,
    /// VMware.
    Vmware,
    /// Microsoft Hyper-V.
    HyperV,
    /// Xen.
    Xen,
    /// Oracle VirtualBox.
    VirtualBox,
    /// Parallels.
    Parallels,
    /// FreeBSD bhyve.
    Bhyve,
    /// ACRN.
    Acrn,
    /// QNX Hypervisor.
    Qnx,
    /// Apple's Virtualization framework.
    AppleVirtualization,
    /// AWS Nitro.
    AmazonNitro,
    /// Google Compute Engine.
    GoogleCompute,
    /// Bochs.
    Bochs,
    /// A hypervisor sys-lib cannot name.
    Other,
}

/// A container runtime.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Container {
    /// Docker.
    Docker,
    /// Podman.
    Podman,
    /// containerd or CRI-O, as Kubernetes runs them.
    Containerd,
    /// LXC or LXD.
    Lxc,
    /// `systemd-nspawn`.
    SystemdNspawn,
    /// OpenVZ or Virtuozzo.
    OpenVz,
    /// A container runtime sys-lib cannot name.
    Other,
}

/// One check that fired.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Evidence {
    /// What was checked: a constant, never a value read from the machine.
    pub check: &'static str,
    /// How much it proves.
    pub strength: Strength,
}

/// How much a piece of evidence proves.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Strength {
    /// Set by the kernel, the firmware, or the runtime.
    Strong,
    /// An environment variable or the like: any parent process can set it.
    Weak,
}

/// A cloud vendor, from firmware strings only: sys-lib never contacts a
/// metadata service.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Cloud {
    /// Amazon Web Services.
    Aws,
    /// Google Cloud.
    Gcp,
    /// Microsoft Azure.
    Azure,
    /// Oracle Cloud.
    Oracle,
    /// Alibaba Cloud.
    Alibaba,
    /// Hetzner.
    Hetzner,
    /// DigitalOcean.
    DigitalOcean,
    /// Linode (Akamai).
    Linode,
    /// Vultr.
    Vultr,
    /// Scaleway.
    Scaleway,
    /// OVHcloud.
    Ovh,
    /// A cloud sys-lib cannot name.
    Other,
}

/// What this program runs inside. Cached: about 1 ns after the first call.
#[must_use]
pub fn detect() -> &'static Virtualization {
    static FACTS: OnceLock<Virtualization> = OnceLock::new();
    FACTS.get_or_init(crate::sys::virtualization)
}

/// Running on a hypervisor. Inside WSL 2 this and [`is_wsl`] are both true.
/// A Windows host with Hyper-V, WSL 2, or virtualization-based security
/// enabled is not a VM (E-84).
#[must_use]
pub fn is_vm() -> bool {
    detect().hypervisor.is_some()
}

/// Running in a container.
#[must_use]
pub fn is_container() -> bool {
    detect().container.is_some()
}

/// Running under WSL.
#[must_use]
pub fn is_wsl() -> bool {
    detect().wsl.is_some()
}

/// Running under Kubernetes (see [`Virtualization::kubernetes`]).
#[must_use]
pub fn is_kubernetes() -> bool {
    detect().kubernetes
}

/// The hypervisor, when there is one.
#[must_use]
pub fn hypervisor() -> Option<Hypervisor> {
    detect().hypervisor
}

/// The container runtime, when there is one.
#[must_use]
pub fn container() -> Option<Container> {
    detect().container
}

/// The cloud vendor, from firmware strings and the hypervisor leaves. Instance
/// types, regions, and zones need a network call and belong in a cloud crate.
#[must_use]
pub fn cloud() -> Option<Cloud> {
    static FACTS: OnceLock<Option<Cloud>> = OnceLock::new();
    *FACTS.get_or_init(crate::sys::cloud)
}

/// The firmware strings the detection reads, from sysfs or SMBIOS.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Dmi<'a> {
    pub(crate) sys_vendor: &'a str,
    pub(crate) product: &'a str,
    pub(crate) product_version: &'a str,
    pub(crate) board_vendor: &'a str,
    pub(crate) bios_vendor: &'a str,
    pub(crate) asset_tag: &'a str,
}

impl Dmi<'_> {
    fn any(&self, needle: &str) -> bool {
        [
            self.sys_vendor,
            self.product,
            self.product_version,
            self.board_vendor,
            self.bios_vendor,
        ]
        .iter()
        .any(|s| contains_ignore_case(s, needle))
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// The hypervisor a CPUID vendor signature (leaf 0x40000000) names.
pub(crate) fn hypervisor_from_signature(signature: &[u8; 12]) -> Hypervisor {
    match signature {
        b"KVMKVMKVM\0\0\0" | b"Linux KVM Hv" => Hypervisor::Kvm,
        b"TCGTCGTCGTCG" => Hypervisor::Qemu,
        b"VMwareVMware" => Hypervisor::Vmware,
        b"Microsoft Hv" => Hypervisor::HyperV,
        b"XenVMMXenVMM" => Hypervisor::Xen,
        b"bhyve bhyve " => Hypervisor::Bhyve,
        b"QNXQVMBSQG\0\0" => Hypervisor::Qnx,
        b"ACRNACRNACRN" => Hypervisor::Acrn,
        b"Apple VZ\0\0\0\0" | b"VirtualApple" => Hypervisor::AppleVirtualization,
        b"VBoxVBoxVBox" => Hypervisor::VirtualBox,
        b" prl hyperv " | b" lrpepyh vr " => Hypervisor::Parallels,
        _ => Hypervisor::Other,
    }
}

/// The hypervisor firmware strings name. The clouds' own hypervisors are named
/// first: a Nitro or GCE guest also says KVM.
pub(crate) fn hypervisor_from_dmi(dmi: &Dmi<'_>) -> Option<Hypervisor> {
    let table: &[(&str, Hypervisor)] = &[
        ("Amazon EC2", Hypervisor::AmazonNitro),
        ("Google Compute Engine", Hypervisor::GoogleCompute),
        ("Alibaba Cloud ECS", Hypervisor::Kvm),
        ("KubeVirt", Hypervisor::Kvm),
        ("OpenStack", Hypervisor::Kvm),
        ("KVM", Hypervisor::Kvm),
        ("QEMU", Hypervisor::Qemu),
        ("VMware", Hypervisor::Vmware),
        ("innotek GmbH", Hypervisor::VirtualBox),
        ("VirtualBox", Hypervisor::VirtualBox),
        ("Xen", Hypervisor::Xen),
        ("Bochs", Hypervisor::Bochs),
        ("Parallels", Hypervisor::Parallels),
        ("BHYVE", Hypervisor::Bhyve),
        ("Apple Virtualization", Hypervisor::AppleVirtualization),
    ];
    if let Some((_, h)) = table.iter().find(|(needle, _)| dmi.any(needle)) {
        return Some(*h);
    }
    // Hyper-V says only "Microsoft Corporation" and "Virtual Machine": physical
    // Surface machines say the first too.
    let microsoft = contains_ignore_case(dmi.sys_vendor, "Microsoft Corporation");
    (microsoft && contains_ignore_case(dmi.product, "Virtual Machine"))
        .then_some(Hypervisor::HyperV)
}

/// The cloud firmware strings or the hypervisor name.
pub(crate) fn cloud_from_dmi(dmi: &Dmi<'_>, hypervisor: Option<Hypervisor>) -> Option<Cloud> {
    let table: &[(&str, Cloud)] = &[
        ("Amazon EC2", Cloud::Aws),
        ("Amazon", Cloud::Aws),
        ("Google Compute Engine", Cloud::Gcp),
        ("Google", Cloud::Gcp),
        ("Alibaba Cloud", Cloud::Alibaba),
        ("Hetzner", Cloud::Hetzner),
        ("DigitalOcean", Cloud::DigitalOcean),
        ("Linode", Cloud::Linode),
        ("Akamai", Cloud::Linode),
        ("Vultr", Cloud::Vultr),
        ("Scaleway", Cloud::Scaleway),
        ("OVHcloud", Cloud::Ovh),
    ];
    if let Some((_, c)) = table.iter().find(|(needle, _)| dmi.any(needle)) {
        return Some(*c);
    }
    // Azure's asset tag is a fixed string; Oracle's is too.
    if dmi.asset_tag == "7783-7084-3265-9085-8269-3286-77" {
        return Some(Cloud::Azure);
    }
    if dmi.asset_tag == "OracleCloud.com" {
        return Some(Cloud::Oracle);
    }
    match hypervisor {
        Some(Hypervisor::AmazonNitro) => Some(Cloud::Aws),
        Some(Hypervisor::GoogleCompute) => Some(Cloud::Gcp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virt_signatures() {
        assert_eq!(
            hypervisor_from_signature(b"KVMKVMKVM\0\0\0"),
            Hypervisor::Kvm
        );
        assert_eq!(hypervisor_from_signature(b"Linux KVM Hv"), Hypervisor::Kvm);
        assert_eq!(
            hypervisor_from_signature(b"Microsoft Hv"),
            Hypervisor::HyperV
        );
        assert_eq!(
            hypervisor_from_signature(b"VMwareVMware"),
            Hypervisor::Vmware
        );
        assert_eq!(
            hypervisor_from_signature(b"something new"[..12].try_into().unwrap()),
            Hypervisor::Other
        );
    }

    #[test]
    fn test_virt_dmi_strings() {
        let dmi = |product, sys_vendor| Dmi {
            product,
            sys_vendor,
            ..Dmi::default()
        };
        assert_eq!(
            hypervisor_from_dmi(&dmi("Standard PC (Q35 + ICH9, 2009)", "QEMU")),
            Some(Hypervisor::Qemu)
        );
        assert_eq!(
            hypervisor_from_dmi(&dmi("VirtualBox", "innotek GmbH")),
            Some(Hypervisor::VirtualBox)
        );
        assert_eq!(
            hypervisor_from_dmi(&dmi("m5.large", "Amazon EC2")),
            Some(Hypervisor::AmazonNitro)
        );
        assert_eq!(
            hypervisor_from_dmi(&dmi("Virtual Machine", "Microsoft Corporation")),
            Some(Hypervisor::HyperV)
        );
        // A real Surface is not a VM.
        assert_eq!(
            hypervisor_from_dmi(&dmi("Surface Laptop 5", "Microsoft Corporation")),
            None
        );
        assert_eq!(hypervisor_from_dmi(&dmi("ThinkPad X1", "LENOVO")), None);
    }

    #[test]
    fn test_virt_clouds() {
        let aws = Dmi {
            sys_vendor: "Amazon EC2",
            ..Dmi::default()
        };
        assert_eq!(cloud_from_dmi(&aws, None), Some(Cloud::Aws));
        let azure = Dmi {
            sys_vendor: "Microsoft Corporation",
            product: "Virtual Machine",
            asset_tag: "7783-7084-3265-9085-8269-3286-77",
            ..Dmi::default()
        };
        assert_eq!(
            cloud_from_dmi(&azure, Some(Hypervisor::HyperV)),
            Some(Cloud::Azure)
        );
        let hyperv = Dmi {
            asset_tag: "",
            ..azure
        };
        assert_eq!(cloud_from_dmi(&hyperv, Some(Hypervisor::HyperV)), None);
        assert_eq!(cloud_from_dmi(&Dmi::default(), None), None);
    }

    #[test]
    fn test_virt_live_is_consistent() {
        let v = detect();
        assert!(std::ptr::eq(v, detect()));
        assert_eq!(is_vm(), v.hypervisor.is_some());
        assert_eq!(is_container(), v.container.is_some());
        if let Some(w) = v.wsl {
            assert!(matches!(w, 1 | 2));
        }
        // Evidence names constants, one per check.
        for e in &v.evidence {
            assert!(!e.check.is_empty());
        }
    }
}
