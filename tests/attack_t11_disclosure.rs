//! Threat T11: disclosure through build and instance info served on public
//! endpoints. `public()` shows the name and version only; nothing records the
//! build host, the user, or a path; and the writers keep even hostile values
//! to one escaped line. (The build helper's own validation of git output and
//! `HEAD` is tested beside it in `src/build.rs`.)

use sys_lib::BuildInfo;

const SECRETS: [&str; 6] = [
    "4f2c9a1e0b7d3c5a8e6f1b2d4c7a9e0f3b5d8c1a",
    "4f2c9a1",
    "2026-10-22T09:30:00Z",
    "x86_64-unknown-linux-gnu",
    "1.89.0",
    "device_id,guard,instance",
];

fn full() -> BuildInfo {
    BuildInfo::__from_env(
        "site",
        "2.4.1",
        false,
        Some(SECRETS[0]),
        Some(SECRETS[1]),
        Some("true"),
        Some(SECRETS[2]),
        Some("release"),
        Some(SECRETS[3]),
        Some(SECRETS[4]),
        Some(SECRETS[5]),
    )
}

#[test]
fn test_attack_t11_public_shows_name_and_version_only() {
    let b = full();
    assert_eq!(b.public().to_string(), "site 2.4.1");
    for secret in SECRETS {
        assert!(!b.public().to_string().contains(secret));
    }
    // The full forms do carry them: they are for logs, not endpoints.
    assert!(b.to_string().contains(SECRETS[1]));
}

#[test]
fn test_attack_t11_nothing_records_host_user_or_path() {
    let b = sys_lib::build_info!();
    let mut all = b.to_string();
    b.write_kv(&mut all).unwrap();
    b.write_json(&mut all).unwrap();
    let dir = env!("CARGO_MANIFEST_DIR");
    assert!(!all.contains(dir), "{all}");
    if let Ok(host) = sys_lib::host::name()
        && host.len() >= 4
    {
        assert!(!all.contains(&host), "{all}");
    }
    if let Ok(user) = sys_lib::process::user()
        && user.name.len() >= 4
    {
        assert!(!all.contains(&user.name), "{all}");
    }
}

#[test]
fn test_attack_t11_writers_escape_hostile_values() {
    // The build helper never lets these through; the writers hold even so.
    let hostile = "evil\"\n\r\t\\\u{1b}[2J\u{7f}";
    let b = BuildInfo::__from_env(
        "site",
        "1.0.0",
        true,
        Some(hostile),
        Some(hostile),
        None,
        Some(hostile),
        Some(hostile),
        Some(hostile),
        Some(hostile),
        Some(hostile),
    );
    let mut json = String::new();
    b.write_json(&mut json).unwrap();
    assert!(json.starts_with('{') && json.ends_with('}'), "{json}");
    assert!(
        !json.chars().any(char::is_control),
        "raw control character in {json:?}"
    );
    assert!(json.contains(r#"evil\"\n\r\t\\\u001b[2J\u007f"#), "{json}");

    let mut kv = String::new();
    b.write_kv(&mut kv).unwrap();
    assert!(
        !kv.contains(['\n', '\r', '\u{1b}']),
        "raw control character in {kv:?}"
    );
}
