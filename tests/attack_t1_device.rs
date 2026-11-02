//! Threats T1 and T3: raw identifiers never leave `device`, and the documentation
//! says a device id is not authentication.

#![cfg(feature = "device-id")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sys_lib::device::{self, Fingerprint};

/// Every public output for one application id.
fn outputs(app: &str) -> Vec<String> {
    let mut all = vec![
        device::report().to_string(),
        format!("{:?}", device::report()),
        format!("{:?}", device::builder(app).secret([3; 32])),
    ];
    match device::id(app) {
        Ok(id) => {
            all.push(id.to_string());
            all.push(format!("{id:?}"));
        }
        Err(e) => {
            all.push(e.to_string());
            all.push(format!("{e:?}"));
        }
    }
    let f = device::fingerprint(app).unwrap();
    all.push(format!("{f:?}"));
    all.push(hex(&f.to_bytes()));
    all.push(format!("{:?}", f.compare(&f)));
    all
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The raw identifiers this machine exposes to an unprivileged read.
fn raw_identifiers() -> Vec<String> {
    let mut raw = Vec::new();
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(text) = std::fs::read_to_string(path) {
            let t = text.trim().to_lowercase();
            if t.len() == 32 {
                raw.push(t);
            }
        }
    }
    raw
}

#[test]
fn test_attack_t1_no_output_contains_a_raw_identifier() {
    let everything = outputs("com.example.t1").join("\n").to_lowercase();
    for raw in raw_identifiers() {
        assert!(!everything.contains(&raw), "a raw identifier leaked");
        // Nor its first half, in case of a truncated copy.
        assert!(!everything.contains(&raw[..16]), "part of an identifier leaked");
    }
}

#[test]
fn test_attack_t1_fingerprint_debug_names_anchors_only() {
    let f = device::fingerprint("com.example.t1").unwrap();
    let text = format!("{f:?}");
    assert!(!text.chars().any(|c| c.is_ascii_digit()), "{text}");
    let back = Fingerprint::from_bytes(&f.to_bytes()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn test_attack_t2_applications_see_unrelated_fingerprints() {
    let (a, b) = (
        device::fingerprint("com.example.one").unwrap(),
        device::fingerprint("com.example.two").unwrap(),
    );
    if a.anchors().next().is_some() {
        assert_ne!(a.to_bytes(), b.to_bytes());
        assert_eq!(a.compare(&b).matched, vec![]);
    }
}

#[test]
fn test_attack_t3_the_warning_is_in_the_documentation() {
    let module = include_str!("../src/device/mod.rs");
    let first = module.lines().take(12).collect::<Vec<_>>().join(" ");
    assert!(first.contains("not authentication"), "{first}");
    let guide = include_str!("../docs/DEVICE-ID.md");
    assert!(guide.contains("Not authentication"));
    // The API offers no way to verify: there is no `verify` or `authenticate`.
    assert!(!module.contains("pub fn verify") && !module.contains("pub fn authenticate"));
}
