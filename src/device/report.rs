//! How far to trust the id on this machine, without revealing any value.

use core::fmt;

use super::{Anchor, Basis, Raw, Reading, read_anchor};
use crate::{os::Confidence, virt};

/// The state of every source, with no values in it.
///
/// ```
/// use sys_lib::device;
///
/// let report = device::report();
/// println!("{report}");
/// if !report.clone_risk.is_empty() {
///     println!("this id may be shared with other machines: {:?}", report.clone_risk);
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StabilityReport {
    /// One per [`Anchor`], in `Anchor` order.
    pub sources: Vec<SourceStatus>,
    /// Whether [`Basis::Install`] has a usable source.
    pub default_basis_available: bool,
    /// What suggests this id may be shared; empty when nothing does.
    pub clone_risk: Vec<CloneRisk>,
    /// How far to trust the [`Basis::Install`] id.
    pub confidence: Confidence,
    /// What would change the [`Basis::Install`] id on this OS.
    pub changes_on: Vec<ChangeEvent>,
}

/// One source and its state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct SourceStatus {
    /// The source.
    pub anchor: Anchor,
    /// Its state.
    pub state: SourceState,
}

/// What became of reading a source.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum SourceState {
    /// A usable value exists.
    Present,
    /// No value exists.
    Absent,
    /// The value is one manufacturers and hypervisors ship, treated as absent.
    Placeholder,
    /// The OS refused; on Linux the DMI identifiers need root.
    PermissionDenied,
    /// This OS has no such source.
    Unsupported,
}

/// What suggests an id may be shared with other machines.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CloneRisk {
    /// Running in a container: every container of one image may share an id.
    Container,
    /// Running in a VM: clones of one template share an id until regenerated.
    VirtualMachine,
    /// `/etc/machine-id` is empty or `uninitialized`.
    MachineIdUninitialized,
    /// A container with a machine id, which came from its image.
    SharedImageLikely,
}

/// What would change an id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ChangeEvent {
    /// The OS is reinstalled.
    OsReinstall,
    /// The main board is replaced.
    BoardReplacement,
    /// The network card is replaced.
    NicReplacement,
    /// The CPU is upgraded.
    CpuUpgrade,
    /// The VM is created again.
    VmRecreated,
    /// The container is created again.
    ContainerRecreated,
    /// An administrator sets a new value.
    AdminReset,
}

/// Probes every source once. Sources only: no application id is needed, and no
/// value appears in the result.
#[must_use]
pub fn report() -> StabilityReport {
    let sources: Vec<SourceStatus> = Anchor::ALL
        .into_iter()
        .map(|anchor| {
            let mut raw = Raw::new();
            let state = match read_anchor(anchor, &mut raw) {
                Reading::Value(_) => SourceState::Present,
                Reading::Absent => SourceState::Absent,
                Reading::Placeholder => SourceState::Placeholder,
                Reading::PermissionDenied => SourceState::PermissionDenied,
                Reading::Unsupported => SourceState::Unsupported,
            };
            SourceStatus { anchor, state }
        })
        .collect();
    assess(sources, virt::is_container(), virt::is_vm())
}

/// The anchors behind [`Basis::Install`] on this OS.
fn install_state(sources: &[SourceStatus]) -> SourceState {
    let anchor = Basis::Install.anchors()[0];
    sources
        .iter()
        .find(|s| s.anchor == anchor)
        .map_or(SourceState::Absent, |s| s.state)
}

fn assess(sources: Vec<SourceStatus>, container: bool, vm: bool) -> StabilityReport {
    let state = install_state(&sources);
    let available = state == SourceState::Present;
    let mut clone_risk = Vec::new();
    if container {
        clone_risk.push(CloneRisk::Container);
    }
    if vm {
        clone_risk.push(CloneRisk::VirtualMachine);
    }
    if state == SourceState::Placeholder {
        clone_risk.push(CloneRisk::MachineIdUninitialized);
    }
    if container && available {
        clone_risk.push(CloneRisk::SharedImageLikely);
    }
    let confidence = if !available {
        Confidence::None
    } else if container {
        Confidence::Low
    } else if vm {
        Confidence::Medium
    } else {
        Confidence::High
    };
    let mut changes_on = if cfg!(any(target_os = "macos", target_os = "ios")) {
        vec![ChangeEvent::BoardReplacement]
    } else {
        vec![ChangeEvent::OsReinstall, ChangeEvent::AdminReset]
    };
    if vm {
        changes_on.push(ChangeEvent::VmRecreated);
    }
    if container {
        changes_on.push(ChangeEvent::ContainerRecreated);
    }
    StabilityReport {
        sources,
        default_basis_available: available,
        clone_risk,
        confidence,
        changes_on,
    }
}

impl fmt::Display for StabilityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for s in &self.sources {
            let state = match s.state {
                SourceState::Present => "present",
                SourceState::Absent => "absent",
                SourceState::Placeholder => "placeholder",
                SourceState::PermissionDenied => "permission denied",
                SourceState::Unsupported => "unsupported",
            };
            writeln!(f, "{:<16}{state}", s.anchor.name())?;
        }
        writeln!(f, "{:<16}{:?}", "confidence", self.confidence)?;
        write!(f, "{:<16}{:?}", "clone risk", self.clone_risk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(state: SourceState) -> Vec<SourceStatus> {
        Anchor::ALL
            .into_iter()
            .map(|anchor| SourceStatus { anchor, state })
            .collect()
    }

    #[test]
    fn test_report_bare_metal() {
        let r = assess(sources(SourceState::Present), false, false);
        assert!(r.default_basis_available);
        assert!(r.clone_risk.is_empty());
        assert_eq!(r.confidence, Confidence::High);
    }

    #[test]
    fn test_report_clone_risks() {
        // E-71, E-72.
        let r = assess(sources(SourceState::Present), true, true);
        assert_eq!(
            r.clone_risk,
            vec![
                CloneRisk::Container,
                CloneRisk::VirtualMachine,
                CloneRisk::SharedImageLikely
            ]
        );
        assert_eq!(r.confidence, Confidence::Low);
        assert!(r.changes_on.contains(&ChangeEvent::ContainerRecreated));
        assert_eq!(
            assess(sources(SourceState::Present), false, true).confidence,
            Confidence::Medium
        );
        // E-69.
        let r = assess(sources(SourceState::Placeholder), false, false);
        assert_eq!(r.clone_risk, vec![CloneRisk::MachineIdUninitialized]);
        assert!(!r.default_basis_available);
        assert_eq!(r.confidence, Confidence::None);
    }

    #[test]
    fn test_report_live_has_every_source_and_no_values() {
        let r = report();
        assert_eq!(r.sources.len(), Anchor::ALL.len());
        let text = r.to_string();
        assert!(text.contains("install id") && text.contains("confidence"));
    }
}
