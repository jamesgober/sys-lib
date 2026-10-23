//! Formatting helpers shared by the value types: a stack buffer, padding that
//! honours width, fill, and alignment without truncating, and the escaping used
//! by the key-value and JSON writers. Nothing here allocates.

use core::fmt::{self, Write};

/// A fixed-size, stack-allocated text buffer.
pub(crate) struct StackBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackBuf<N> {
    pub(crate) const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        // Only `write_str` fills the buffer, and it copies whole `&str`s, so the
        // contents are always valid UTF-8; the fallback is unreachable in practice.
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for StackBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        if end > N {
            return Err(fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Writes `s` with the formatter's width, fill, and alignment (left by default),
/// ignoring its precision, which the caller has already applied.
pub(crate) fn pad(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    let len = s.chars().count();
    let Some(width) = f.width().filter(|&w| w > len) else {
        return f.write_str(s);
    };
    let fill = f.fill();
    let gap = width - len;
    let (before, after) = match f.align() {
        Some(fmt::Alignment::Right) => (gap, 0),
        Some(fmt::Alignment::Center) => (gap / 2, gap - gap / 2),
        _ => (0, gap),
    };
    for _ in 0..before {
        f.write_char(fill)?;
    }
    f.write_str(s)?;
    for _ in 0..after {
        f.write_char(fill)?;
    }
    Ok(())
}

/// Writes `s` as a JSON string literal, quotes included.
pub(crate) fn json_str(out: &mut impl Write, s: &str) -> fmt::Result {
    out.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            c if (c as u32) < 0x20 || c == '\u{7f}' => write!(out, "\\u{:04x}", c as u32)?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

/// Writes `key=value` for a key-value log line: the value is written bare when
/// it is a plain token, and quoted with JSON escaping otherwise.
pub(crate) fn kv(out: &mut impl Write, key: &str, value: &str) -> fmt::Result {
    out.write_str(key)?;
    out.write_char('=')?;
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\' && b != b'=');
    if plain {
        out.write_str(value)
    } else {
        json_str(out, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Padded<'a>(&'a str);
    impl fmt::Display for Padded<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            pad(f, self.0)
        }
    }

    #[test]
    fn test_pad_honours_width_fill_and_alignment() {
        assert_eq!(format!("{:6}|", Padded("ab")), "ab    |");
        assert_eq!(format!("{:>6}|", Padded("ab")), "    ab|");
        assert_eq!(format!("{:*^6}|", Padded("ab")), "**ab**|");
        assert_eq!(format!("{:1}|", Padded("abc")), "abc|");
        // Precision is the caller's job, so it never truncates here.
        assert_eq!(format!("{:.1}|", Padded("abc")), "abc|");
    }

    #[test]
    fn test_stack_buf_refuses_overflow() {
        let mut b = StackBuf::<4>::new();
        assert!(b.write_str("abcd").is_ok());
        assert!(b.write_str("e").is_err());
        assert_eq!(b.as_str(), "abcd");
    }

    #[test]
    fn test_json_str_escapes_quotes_controls_and_backslashes() {
        let mut s = String::new();
        json_str(&mut s, "a\"b\\c\nd\u{1}e\u{7f}").unwrap();
        assert_eq!(s, r#""a\"b\\c\nd\u0001e\u007f""#);
    }

    #[test]
    fn test_kv_quotes_only_when_needed() {
        let mut s = String::new();
        kv(&mut s, "a", "plain-1.2").unwrap();
        s.push(' ');
        kv(&mut s, "b", "two words").unwrap();
        s.push(' ');
        kv(&mut s, "c", "").unwrap();
        s.push(' ');
        kv(&mut s, "d", "x=y").unwrap();
        assert_eq!(s, r#"a=plain-1.2 b="two words" c="" d="x=y""#);
    }
}
