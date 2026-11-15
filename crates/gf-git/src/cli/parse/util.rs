use std::path::PathBuf;

/// Converts raw path bytes from git into a `PathBuf` without losing
/// non-UTF-8 names on Unix.
pub fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Undoes git's C-style path quoting (`"a\tb\303\251"`), used in diff headers
/// for names with special characters. Unquoted input is returned unchanged.
pub fn unquote_c_path(s: &[u8]) -> Vec<u8> {
    let Some(inner) = s
        .strip_prefix(b"\"")
        .and_then(|rest| rest.strip_suffix(b"\""))
    else {
        return s.to_vec();
    };

    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let b = inner[i];
        if b != b'\\' || i + 1 >= inner.len() {
            out.push(b);
            i += 1;
            continue;
        }
        let esc = inner[i + 1];
        i += 2;
        match esc {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'v' => out.push(0x0b),
            b'0'..=b'7' => {
                // Up to three octal digits; the first was `esc`.
                let mut value = u32::from(esc - b'0');
                let mut digits = 1;
                while digits < 3 && i < inner.len() && (b'0'..=b'7').contains(&inner[i]) {
                    value = value * 8 + u32::from(inner[i] - b'0');
                    i += 1;
                    digits += 1;
                }
                out.push(u8::try_from(value).unwrap_or(b'?'));
            }
            other => out.push(other),
        }
    }
    out
}

/// Splits `s` on `sep`, dropping one trailing empty piece (git terminates
/// the last record as well as separating them).
pub(crate) fn split_records(s: &[u8], sep: u8) -> impl Iterator<Item = &[u8]> {
    let s = s.strip_suffix(&[sep]).unwrap_or(s);
    s.split(move |&b| b == sep).filter(|r| !r.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquotes_c_style_paths() {
        assert_eq!(unquote_c_path(b"plain.txt"), b"plain.txt");
        assert_eq!(unquote_c_path(b"\"a\\tb\""), b"a\tb");
        assert_eq!(unquote_c_path(b"\"q\\\"uote\""), b"q\"uote");
        // "é" is \303\251 in UTF-8.
        assert_eq!(
            String::from_utf8(unquote_c_path(b"\"caf\\303\\251.md\"")).ok(),
            Some("café.md".to_owned())
        );
    }

    #[test]
    fn splits_terminated_records() {
        let parts: Vec<_> = split_records(b"a\0b\0", 0).collect();
        assert_eq!(parts, [b"a".as_slice(), b"b"]);
        assert_eq!(split_records(b"", 0).count(), 0);
    }
}
