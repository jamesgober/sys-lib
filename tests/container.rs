//! The container CI job: this test runs inside `docker run --memory=256m
//! --cpus=1.5 --cpuset-cpus=0-1` on a cgroup v2 runner and asserts that
//! sys-lib sees the container's real limits, not the host's (PLAN.md 19.1,
//! the container layer). Outside that job it does nothing: the expected
//! values come from the job's environment.

fn expected(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

#[test]
fn test_container_limits_are_the_containers() {
    let (Some(memory), Some(cpus)) = (
        expected("SYS_LIB_EXPECT_MEMORY"),
        expected("SYS_LIB_EXPECT_CPUS"),
    ) else {
        return;
    };
    use sys_lib::{cpu, mem};
    let m = mem::snapshot().unwrap();
    assert_eq!(m.total.raw(), memory, "{m:?}");
    assert!(
        m.host_total > m.total,
        "the host has more than the container"
    );
    let limit = m.limit.expect("a memory limit");
    assert_eq!(limit.source, mem::LimitSource::Cgroup2);
    assert_eq!(m.source, mem::MemorySource::Cgroup2);
    assert!(m.used <= m.total);
    assert_eq!(cpu::available() as u64, cpus, "cpuset and quota");
    let q = cpu::quota().unwrap().expect("a CPU quota");
    assert!((q.cpus - 1.5).abs() < 1e-6, "{q:?}");
    assert!(cpu::logical() >= 2);
    if let Some(p) = mem::pressure().unwrap() {
        assert!(p.some.avg10 <= sys_lib::Percent::FULL);
    }
    let e = mem::events().unwrap().expect("cgroup memory events");
    assert_eq!(e.scope, mem::EventScope::Cgroup);
    println!(
        "container: {} of {} memory, {} CPUs, quota {:.1}",
        m.used,
        m.total,
        cpu::available(),
        q.cpus
    );
}
