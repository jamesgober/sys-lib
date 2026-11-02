//! Every static fact plus one memory snapshot and the uptime, for the start-up
//! log line, a health endpoint, or a support bundle.

use core::{fmt, time::Duration};

use crate::{cpu, host, mem, os, virt};

/// See [`summary`]. It deliberately carries no host name, user name, MAC
/// address, or device id, because it is meant for output that leaves the
/// machine (threat T13).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Summary {
    /// [`os::name`].
    pub os: &'static str,
    /// [`os::kernel`].
    pub kernel: &'static str,
    /// [`cpu::name`].
    pub cpu: &'static str,
    /// [`cpu::count`].
    pub cpu_counts: cpu::Counts,
    /// [`mem::snapshot`].
    pub memory: Option<mem::Memory>,
    /// [`cpu::quota`].
    pub cpu_quota: Option<cpu::CpuQuota>,
    /// [`virt::detect`].
    pub virtualization: &'static virt::Virtualization,
    /// [`os::form_factor`].
    pub form_factor: os::FormFactor,
    /// [`host::uptime`].
    pub uptime: Option<Duration>,
    /// [`cpu::load`].
    pub load: Option<cpu::Load>,
}

/// The facts for a log line or a support bundle. Never fails: a reading that
/// fails becomes `None` in its field.
///
/// ```
/// use sys_lib as sys;
///
/// let summary = sys::summary();
/// println!("{summary}");
///
/// let mut line = String::new();
/// summary.write_json(&mut line)?;
/// # Ok::<(), std::fmt::Error>(())
/// ```
#[must_use]
pub fn summary() -> Summary {
    Summary {
        os: os::name(),
        kernel: os::kernel(),
        cpu: cpu::name(),
        cpu_counts: cpu::count(),
        memory: mem::snapshot().ok(),
        cpu_quota: cpu::quota().ok().flatten(),
        virtualization: virt::detect(),
        form_factor: os::form_factor(),
        uptime: host::uptime().ok(),
        load: cpu::load().ok().flatten(),
    }
}

impl Summary {
    fn virt_name(&self) -> String {
        let v = self.virtualization;
        let mut parts = Vec::new();
        if let Some(c) = v.container {
            parts.push(format!("{c:?}").to_lowercase());
        }
        if let Some(h) = v.hypervisor {
            parts.push(format!("{h:?}").to_lowercase());
        }
        if parts.is_empty() {
            "none".to_owned()
        } else {
            parts.join("+")
        }
    }

    fn limit_name(&self) -> Option<&'static str> {
        match self.memory?.source {
            mem::MemorySource::Cgroup1 => Some("cgroup1"),
            mem::MemorySource::Cgroup2 => Some("cgroup2"),
            mem::MemorySource::JobObject => Some("job"),
            mem::MemorySource::Lxcfs => Some("lxcfs"),
            _ => None,
        }
    }

    /// `key=value` pairs, one per fact, separated by `sep`.
    fn pairs(&self, out: &mut dyn fmt::Write, sep: &str) -> fmt::Result {
        let c = &self.cpu_counts;
        write!(out, "os={}{sep}kernel={}", quoted(self.os), quoted(self.kernel))?;
        write!(out, "{sep}cpu={}", quoted(self.cpu))?;
        write!(out, "{sep}cores={}{sep}threads={}", c.physical, c.logical)?;
        write!(out, "{sep}available={}", c.available)?;
        if let Some(m) = &self.memory {
            write!(out, "{sep}memory.total={}{sep}memory.used={}", m.total, m.used)?;
        }
        if let Some(limit) = self.limit_name() {
            write!(out, "{sep}limit={limit}")?;
        }
        write!(out, "{sep}virt={}", self.virt_name())?;
        write!(out, "{sep}form={}", self.form_factor.name())?;
        if let Some(up) = self.uptime {
            write!(out, "{sep}uptime={}s", up.as_secs())?;
        }
        if let Some(l) = self.load {
            write!(out, "{sep}load={:.2}", l.one)?;
        }
        Ok(())
    }

    /// `key=value` pairs on one line.
    ///
    /// # Errors
    ///
    /// Whatever `out` returns.
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result {
        self.pairs(out, " ")
    }

    /// One JSON object.
    ///
    /// # Errors
    ///
    /// Whatever `out` returns.
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result {
        let c = &self.cpu_counts;
        write!(out, "{{\"os\":")?;
        json(out, self.os)?;
        write!(out, ",\"kernel\":")?;
        json(out, self.kernel)?;
        write!(out, ",\"cpu\":")?;
        json(out, self.cpu)?;
        write!(
            out,
            ",\"cores\":{},\"threads\":{},\"packages\":{},\"available\":{},\"online\":{}",
            c.physical, c.logical, c.packages, c.available, c.online
        )?;
        if let Some(m) = &self.memory {
            write!(
                out,
                ",\"memory\":{{\"total\":{},\"used\":{},\"free\":{}}}",
                m.total.raw(),
                m.used.raw(),
                m.free.raw()
            )?;
        }
        if let Some(q) = &self.cpu_quota {
            write!(out, ",\"cpu_quota\":{}", q.cpus)?;
        }
        if let Some(limit) = self.limit_name() {
            write!(out, ",\"limit\":\"{limit}\"")?;
        }
        write!(out, ",\"virt\":")?;
        json(out, &self.virt_name())?;
        write!(out, ",\"form\":\"{}\"", self.form_factor.name())?;
        if let Some(up) = self.uptime {
            write!(out, ",\"uptime_seconds\":{}", up.as_secs())?;
        }
        if let Some(l) = self.load {
            write!(out, ",\"load\":[{},{},{}]", l.one, l.five, l.fifteen)?;
        }
        out.write_str("}")
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.pairs(f, " ")
    }
}

/// `text`, in double quotes when it has a space, quote, or control character.
fn quoted(text: &str) -> String {
    if text.is_empty() || text.chars().any(|c| c == ' ' || c == '"' || c.is_control()) {
        format!("{:?}", text)
    } else {
        text.to_owned()
    }
}

/// `text` as a JSON string.
fn json(out: &mut dyn fmt::Write, text: &str) -> fmt::Result {
    out.write_char('"')?;
    for c in text.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            c if u32::from(c) < 0x20 => write!(out, "\\u{:04x}", u32::from(c))?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary_live_outputs() {
        let s = summary();
        let text = s.to_string();
        assert!(text.starts_with("os=") && text.contains(" cpu=") && text.contains(" virt="));
        let mut kv = String::new();
        s.write_kv(&mut kv).unwrap();
        assert_eq!(kv, text);
        let mut j = String::new();
        s.write_json(&mut j).unwrap();
        assert!(j.starts_with("{\"os\":\"") && j.ends_with('}'));
    }

    #[test]
    fn test_summary_t13_no_identifying_values() {
        // T13: the host name and the user name stay out.
        let s = summary();
        let (mut kv, mut j) = (String::new(), String::new());
        s.write_kv(&mut kv).unwrap();
        s.write_json(&mut j).unwrap();
        let debug = format!("{s:?}");
        let mut secrets = Vec::new();
        if let Ok(h) = host::name() {
            secrets.push(h);
        }
        if let Ok(u) = crate::process::user() {
            secrets.push(u.name);
        }
        for secret in secrets.iter().filter(|s| s.len() >= 4) {
            // A machine named after its OS or CPU would match by accident.
            if s.os.contains(secret.as_str()) || s.kernel.contains(secret.as_str()) {
                continue;
            }
            assert!(!kv.contains(secret.as_str()), "{secret}");
            assert!(!j.contains(secret.as_str()), "{secret}");
            assert!(!debug.contains(secret.as_str()), "{secret}");
        }
    }

    #[test]
    fn test_summary_json_escapes() {
        let mut out = String::new();
        json(&mut out, "a\"b\\c\nd").unwrap();
        assert_eq!(out, r#""a\"b\\c\u000ad""#);
        assert_eq!(quoted("plain"), "plain");
        assert_eq!(quoted("two words"), "\"two words\"");
        assert_eq!(quoted(""), "\"\"");
    }
}
