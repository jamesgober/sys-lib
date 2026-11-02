//! Virtualization on Linux: the hypervisor from CPUID, the DMI strings, and the
//! hypervisor files; the container from its markers; WSL from the kernel
//! release; Kubernetes from the service account. Each check that fires is
//! recorded by name (T14).

use super::{Root, hardware};
use crate::{
    parse::mountinfo,
    virt::{
        self, Cloud, Container, Hypervisor,
        Strength::{Strong, Weak},
        Virtualization,
    },
};

/// The environment variables the detection looks at: set by the caller, so a
/// fixture can say what it likes. Weak evidence.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Env {
    pub(crate) kubernetes_host: bool,
    pub(crate) wsl_distro: bool,
}

/// Runs every check. `signature` is the CPUID hypervisor vendor, on x86.
pub(crate) fn detect(root: &Root, signature: Option<[u8; 12]>, env: Env) -> Virtualization {
    let mut v = Virtualization::default();
    wsl(root, env, &mut v);
    hypervisor(root, signature, &mut v);
    container(root, &mut v);
    kubernetes(root, env, &mut v);
    v
}

fn wsl(root: &Root, env: Env, v: &mut Virtualization) {
    let release = root.text("/proc/sys/kernel/osrelease").unwrap_or_default();
    if release.contains("WSL2") || release.contains("microsoft-standard") {
        v.wsl = Some(2);
        v.fired("kernel release names WSL 2", Strong);
    } else if release.contains("Microsoft") {
        v.wsl = Some(1);
        v.fired("kernel release names WSL 1", Strong);
    } else if env.wsl_distro {
        // Weak alone, and the version is not known: WSL 2 is the default.
        v.fired("WSL_DISTRO_NAME", Weak);
    }
    if root.exists("/proc/sys/fs/binfmt_misc/WSLInterop") {
        v.fired("WSLInterop", Strong);
    }
}

fn hypervisor(root: &Root, signature: Option<[u8; 12]>, v: &mut Virtualization) {
    if let Some(s) = signature {
        v.hypervisor = Some(virt::hypervisor_from_signature(&s));
        v.fired("CPUID hypervisor leaf", Strong);
    }
    // The clouds' firmware names their hypervisor more exactly than CPUID does.
    let strings = hardware::dmi(root);
    if let Some(h) = virt::hypervisor_from_dmi(&strings.view()) {
        v.fired("DMI strings", Strong);
        if v.hypervisor.is_none_or(|c| {
            c == Hypervisor::Other
                || matches!(h, Hypervisor::AmazonNitro | Hypervisor::GoogleCompute)
        }) {
            v.hypervisor = Some(h);
        }
    }
    if let Some(t) = root.text("/sys/hypervisor/type") {
        // Dom0 runs on Xen but is not its guest.
        let dom0 = root
            .text("/proc/xen/capabilities")
            .is_some_and(|c| c.contains("control_d"));
        if t.trim() == "xen" && !dom0 {
            v.fired("/sys/hypervisor/type", Strong);
            v.hypervisor.get_or_insert(Hypervisor::Xen);
        }
    }
    if v.hypervisor.is_none() && root.exists("/proc/device-tree/hypervisor/compatible") {
        v.fired("device tree hypervisor node", Strong);
        let compatible = root
            .text("/proc/device-tree/hypervisor/compatible")
            .unwrap_or_default();
        v.hypervisor = Some(match compatible.as_str() {
            c if c.contains("xen") => Hypervisor::Xen,
            c if c.contains("kvm") => Hypervisor::Kvm,
            _ => Hypervisor::Other,
        });
    }
}

/// The runtime `/run/systemd/container` names.
fn runtime(name: &str) -> Container {
    match name.trim() {
        "docker" => Container::Docker,
        "podman" => Container::Podman,
        "lxc" | "lxc-libvirt" => Container::Lxc,
        "systemd-nspawn" => Container::SystemdNspawn,
        "oci" | "crio" | "containerd" => Container::Containerd,
        _ => Container::Other,
    }
}

/// The runtime a line of `/proc/1/cgroup` (v1) names.
fn runtime_in_cgroup(data: &str) -> Option<Container> {
    if data.contains("/kubepods") {
        Some(Container::Containerd)
    } else if data.contains("/docker") {
        Some(Container::Docker)
    } else if data.contains("/lxc") {
        Some(Container::Lxc)
    } else {
        None
    }
}

fn container(root: &Root, v: &mut Virtualization) {
    // systemd under WSL says `wsl` here: a VM, not a container.
    if let Some(name) = root
        .text("/run/systemd/container")
        .filter(|n| n.trim() != "wsl")
    {
        v.fired("/run/systemd/container", Strong);
        v.container = Some(runtime(&name));
    }
    if root.exists("/run/.containerenv") {
        v.fired("/run/.containerenv", Strong);
        v.container.get_or_insert(Container::Podman);
    }
    if root.exists("/.dockerenv") {
        v.fired("/.dockerenv", Strong);
        v.container.get_or_insert(Container::Docker);
    }
    // hidepid hides this file; the other checks stand (E-96).
    if let Some(c) = root
        .text("/proc/1/cgroup")
        .and_then(|d| runtime_in_cgroup(&d))
    {
        v.fired("/proc/1/cgroup", Strong);
        v.container.get_or_insert(c);
    }
    if root.exists("/proc/vz") && !root.exists("/proc/bc") {
        v.fired("/proc/vz", Strong);
        v.container.get_or_insert(Container::OpenVz);
    }
    // WSL and live images boot from an overlay too.
    if overlay_root(root) {
        v.fired("overlay root", Weak);
        if v.wsl.is_none() {
            v.container.get_or_insert(Container::Other);
        }
    }
}

/// An overlay filesystem at `/`: containers, and also some live images.
fn overlay_root(root: &Root) -> bool {
    root.read(
        "/proc/self/mountinfo",
        super::FILE_LIMIT,
        "mounts",
        "/proc/self/mountinfo",
    )
    .is_ok_and(|data| {
        data.split(|&b| b == b'\n').any(|line| {
            mountinfo::parse_line(line)
                .is_some_and(|m| &*m.mount_point == b"/" && m.fstype == b"overlay")
        })
    })
}

fn kubernetes(root: &Root, env: Env, v: &mut Virtualization) {
    if root.exists("/var/run/secrets/kubernetes.io/serviceaccount") {
        v.kubernetes = true;
        v.fired("Kubernetes service account", Strong);
    }
    if env.kubernetes_host {
        v.kubernetes = true;
        v.fired("KUBERNETES_SERVICE_HOST", Weak);
    }
}

/// The cloud firmware names.
pub(crate) fn cloud(root: &Root, hypervisor: Option<Hypervisor>) -> Option<Cloud> {
    virt::cloud_from_dmi(&hardware::dmi(root).view(), hypervisor)
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};
    use crate::virt::Strength;

    fn run(name: &str, signature: Option<[u8; 12]>) -> Virtualization {
        detect(&fixtures::root(name), signature, Env::default())
    }

    #[test]
    fn test_virt_wsl2_fixture() {
        // E-40, E-46: WSL 2 and a VM at once.
        let v = run("wsl2", Some(*b"Microsoft Hv"));
        assert_eq!(v.wsl, Some(2));
        assert_eq!(v.hypervisor, Some(Hypervisor::HyperV));
        assert_eq!(v.container, None);
        assert!(!v.kubernetes);
        assert!(v.evidence.iter().any(|e| e.check == "kernel release names WSL 2"));
    }

    #[test]
    fn test_virt_docker_fixtures() {
        for name in ["docker-cgroup2-limited", "minimal-scratch", "docker-cgroup1-limited"] {
            let v = run(name, None);
            assert!(v.container.is_some(), "{name}: {v:?}");
            assert_eq!(v.wsl, None, "{name}");
        }
        // E-86: a container in a VM sets both.
        let v = run("docker-cgroup2-limited", Some(*b"Microsoft Hv"));
        assert!(v.container.is_some() && v.hypervisor.is_some());
    }

    #[test]
    fn test_virt_t14_environment_is_weak_evidence() {
        // E-87: the variable alone says Kubernetes, weakly.
        let env = Env {
            kubernetes_host: true,
            wsl_distro: false,
        };
        let v = detect(&fixtures::root("wsl2"), None, env);
        assert!(v.kubernetes);
        let e: Vec<_> = v
            .evidence
            .iter()
            .filter(|e| e.check.contains("KUBERNETES"))
            .collect();
        assert_eq!(e[0].strength, Strength::Weak);
    }

    #[test]
    fn test_virt_runtime_names() {
        assert_eq!(runtime("docker\n"), Container::Docker);
        assert_eq!(runtime("lxc-libvirt"), Container::Lxc);
        assert_eq!(runtime("something"), Container::Other);
        assert_eq!(runtime_in_cgroup("1:name=systemd:/kubepods/x"), Some(Container::Containerd));
        assert_eq!(runtime_in_cgroup("0::/init.scope"), None);
    }
}
