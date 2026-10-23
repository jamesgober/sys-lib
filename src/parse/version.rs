//! Dotted version numbers: "24.04", "15.1", "10.0.26200.6899", "3.20.3".

/// The numeric parts of a version, or `None` when it does not start with a number.
/// Each part is the leading digits of a dot-separated component ("8.10-beta" is
/// 8.10); missing parts are 0; a fourth part is the build.
pub(crate) fn parse(text: &str) -> Option<(u32, u32, u32, Option<u32>)> {
    let text = text.trim();
    if !text.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let mut parts = [None::<u32>; 4];
    for (slot, component) in parts.iter_mut().zip(text.split('.')) {
        let digits: &str = &component[..component.bytes().take_while(u8::is_ascii_digit).count()];
        if digits.is_empty() {
            break;
        }
        *slot = Some(digits.parse().ok()?);
        if digits.len() != component.len() {
            break;
        }
    }
    Some((
        parts[0]?,
        parts[1].unwrap_or(0),
        parts[2].unwrap_or(0),
        parts[3],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parse_shapes() {
        assert_eq!(parse("24.04"), Some((24, 4, 0, None)));
        assert_eq!(parse("15.1"), Some((15, 1, 0, None)));
        assert_eq!(parse("10.0.26200.6899"), Some((10, 0, 26200, Some(6899))));
        assert_eq!(parse("3.20.3"), Some((3, 20, 3, None)));
        assert_eq!(parse("12"), Some((12, 0, 0, None)));
        assert_eq!(parse("8.10-beta.3"), Some((8, 10, 0, None)));
        assert_eq!(parse(" 7.9.2009 "), Some((7, 9, 2009, None)));
        assert_eq!(parse("rolling"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("99999999999"), None);
        assert_eq!(parse("1.2.3.4.5"), Some((1, 2, 3, Some(4))));
    }
}
