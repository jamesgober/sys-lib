//! How the program was started: so a service can choose its behaviour without
//! configuration. Each answer carries its evidence, and environment variables
//! are weak evidence: any parent can set them.

use std::{
    io::{IsTerminal, stdin, stdout},
    sync::OnceLock,
};

use crate::virt::{Evidence, Strength};

/// What started this program.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Launch {
    /// The launcher.
    pub by: Launcher,
    /// "site.service", the launchd label, the Windows service name.
    pub unit: Option<&'static str>,
    /// systemd's `NOTIFY_SOCKET` is set: readiness can be reported. Sending the
    /// message is the service kernel's job.
    pub notify_socket: bool,
    /// What the answer rests on.
    pub evidence: Vec<Evidence>,
}

/// What started this program.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Launcher {
    /// systemd.
    Systemd,
    /// launchd.
    Launchd,
    /// The Windows service control manager.
    WindowsService,
    /// This program is the init process of a container.
    ContainerInit,
    /// This program is the init process of the machine.
    Init,
    /// A terminal.
    Terminal,
    /// An SSH session.
    Ssh,
    /// cron.
    Cron,
    /// None of these.
    Unknown,
}

/// What a platform found, for [`decide`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LaunchFacts {
    /// A service manager: the launcher, its unit, and how sure.
    pub(crate) service: Option<(Launcher, Option<String>, Strength)>,
    /// This process is process 1.
    pub(crate) is_pid1: bool,
    /// `NOTIFY_SOCKET` is set.
    pub(crate) notify_socket: bool,
    /// `SSH_CONNECTION` is set.
    pub(crate) ssh: bool,
    /// Strong evidence of cron (the `cron.service` cgroup).
    pub(crate) cron: bool,
}

/// How this program was started. Decided at the first call.
///
/// ```
/// use sys_lib as sys;
///
/// match sys::process::launch().by {
///     sys::process::Launcher::Systemd => { /* JSON logs to the journal; report readiness */ }
///     sys::process::Launcher::Terminal => { /* human-readable logs */ }
///     _ => {}
/// }
/// ```
#[must_use]
pub fn launch() -> &'static Launch {
    static FACTS: OnceLock<Launch> = OnceLock::new();
    FACTS.get_or_init(|| {
        decide(
            crate::sys::launch(),
            crate::virt::is_container(),
            stdin().is_terminal() || stdout().is_terminal(),
        )
    })
}

/// Started by a service manager or an orchestrator.
#[must_use]
pub fn is_service() -> bool {
    matches!(
        launch().by,
        Launcher::Systemd | Launcher::Launchd | Launcher::WindowsService | Launcher::ContainerInit
    )
}

/// Standard input and output are both a terminal. A live reading: a program can
/// have its streams redirected.
#[must_use]
pub fn is_interactive() -> bool {
    stdin().is_terminal() && stdout().is_terminal()
}

/// The first answer wins: a service manager and an init role are facts about
/// this process; a terminal, SSH, and the like describe how it is attached.
pub(crate) fn decide(facts: LaunchFacts, in_container: bool, terminal: bool) -> Launch {
    let mut evidence = Vec::new();
    let mut note = |check, strength| evidence.push(Evidence { check, strength });
    let mut unit = None;
    let by = if let Some((by, name, strength)) = facts.service {
        unit = name.map(crate::os::keep);
        note("service manager", strength);
        by
    } else if facts.is_pid1 && in_container {
        note("process id 1 in a container", Strength::Strong);
        Launcher::ContainerInit
    } else if facts.is_pid1 {
        note("process id 1", Strength::Strong);
        Launcher::Init
    } else if facts.cron {
        note("cron.service cgroup", Strength::Strong);
        Launcher::Cron
    } else if terminal && facts.ssh {
        note("SSH_CONNECTION", Strength::Weak);
        Launcher::Ssh
    } else if terminal {
        note("terminal", Strength::Strong);
        Launcher::Terminal
    } else if facts.ssh {
        note("SSH_CONNECTION", Strength::Weak);
        Launcher::Ssh
    } else {
        Launcher::Unknown
    };
    Launch {
        by,
        unit,
        notify_socket: facts.notify_socket,
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_launch_order() {
        let service = LaunchFacts {
            service: Some((
                Launcher::Systemd,
                Some("site.service".into()),
                Strength::Strong,
            )),
            notify_socket: true,
            ..LaunchFacts::default()
        };
        let l = decide(service, true, true);
        assert_eq!(l.by, Launcher::Systemd);
        assert_eq!(l.unit, Some("site.service"));
        assert!(l.notify_socket);

        let pid1 = LaunchFacts {
            is_pid1: true,
            ..LaunchFacts::default()
        };
        assert_eq!(
            decide(pid1.clone(), true, false).by,
            Launcher::ContainerInit
        );
        assert_eq!(decide(pid1, false, false).by, Launcher::Init);

        let cron = LaunchFacts {
            cron: true,
            ..LaunchFacts::default()
        };
        assert_eq!(decide(cron, false, false).by, Launcher::Cron);

        let ssh = LaunchFacts {
            ssh: true,
            ..LaunchFacts::default()
        };
        let l = decide(ssh, false, true);
        assert_eq!(l.by, Launcher::Ssh);
        assert_eq!(l.evidence[0].strength, Strength::Weak);

        assert_eq!(
            decide(LaunchFacts::default(), false, true).by,
            Launcher::Terminal
        );
        let l = decide(LaunchFacts::default(), false, false);
        assert_eq!(l.by, Launcher::Unknown);
        assert!(l.evidence.is_empty());
    }

    #[test]
    fn test_launch_live_is_cached() {
        assert!(std::ptr::eq(launch(), launch()));
        let _ = (is_service(), is_interactive());
    }
}
