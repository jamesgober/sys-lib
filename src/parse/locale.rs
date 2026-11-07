//! POSIX locale names: `en_US.UTF-8`, `pt_BR`, `sr_RS.UTF-8@latin`. The values
//! come from the environment, so they are checked and never used as a path
//! (threat T4).

/// The longest locale value kept.
pub(crate) const MAX_LOCALE: usize = 64;

/// A locale value as a BCP 47 tag (`en_US.UTF-8` is `en-US`). `None` for `C`,
/// `POSIX`, and `C.UTF-8`, which express no preference (E-94), for anything
/// over [`MAX_LOCALE`] bytes, and for anything that is not a locale name.
pub(crate) fn tag(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > MAX_LOCALE {
        return None;
    }
    let base = raw.split(['.', '@']).next()?;
    if base == "C" || base == "POSIX" {
        return None;
    }
    let (language, region) = match base.split_once('_') {
        Some((l, r)) => (l, Some(r)),
        None => (base, None),
    };
    let language_ok =
        (2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_lowercase());
    let region_ok = region
        .is_none_or(|r| (2..=3).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric()));
    if !language_ok || !region_ok {
        return None;
    }
    Some(match region {
        Some(r) => format!("{language}-{r}"),
        None => language.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locale_tags() {
        assert_eq!(tag("en_US.UTF-8").as_deref(), Some("en-US"));
        assert_eq!(tag("pt_BR").as_deref(), Some("pt-BR"));
        assert_eq!(tag("sr_RS.UTF-8@latin").as_deref(), Some("sr-RS"));
        assert_eq!(tag("de").as_deref(), Some("de"));
        assert_eq!(tag("es_419.UTF-8").as_deref(), Some("es-419"));
    }

    #[test]
    fn test_locale_no_preference_and_hostile() {
        for raw in [
            "",
            "C",
            "POSIX",
            "C.UTF-8",
            "C.utf8",
            "../../etc/passwd",
            "en_US/../x",
            "EN_us",
            "e",
            "english",
            "en_",
        ] {
            assert_eq!(tag(raw), None, "{raw:?}");
        }
        assert_eq!(tag(&"a".repeat(MAX_LOCALE + 1)), None);
    }
}
