//! Text formatting for display (dates, paths, durations). Dependency-free.

use std::path::Path;
use std::time::Duration;

use gf_core::model::Timestamp;

/// `2024-01-31 09:05` in the timestamp's own UTC offset (as `git log` shows).
pub fn datetime(ts: Timestamp) -> String {
    let local = ts.seconds + i64::from(ts.offset_minutes) * 60;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // Both values are small and non-negative by construction.
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// Compact age for labels: `now`, `45s`, `5m`, `3h`, `2d`, `6w`, `3mo`, `2y`.
pub fn age(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..10 => "now".into(),
        10..60 => format!("{s}s"),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 => format!("{}h", s / 3_600),
        86_400..1_209_600 => format!("{}d", s / 86_400),
        1_209_600..5_184_000 => format!("{}w", s / 604_800),
        5_184_000..31_536_000 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// `12ms`, `1.4s`, `2m03s`.
pub fn duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        format!("{}m{:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// Replaces the home directory prefix with `~`.
pub fn tilde(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// Expands a leading `~` in user-typed paths.
pub fn expand_tilde(input: &str) -> std::path::PathBuf {
    match (input.strip_prefix('~'), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            Path::new(&home).join(rest.trim_start_matches('/'))
        }
        _ => input.into(),
    }
}

/// Truncates to `max` characters with a trailing `…`.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Truncates to `max` display columns (wide characters count two) with a
/// trailing `…`, so monospace columns line up.
pub fn truncate_columns(s: &str, max: usize) -> String {
    use crate::widgets::virtual_list::{char_columns, columns};
    if columns(s) <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_columns(c);
        if used + w > max.saturating_sub(1) {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// A line of file content ready to draw: tabs expanded, the line's own
/// trailing `\r` (CRLF files) dropped, other control characters shown as
/// their Unicode pictures (`␜`, `␍` …).
///
/// Drawn raw, line and paragraph separators (`\r`, `\x1c`–`\x1e`, U+0085,
/// U+2028/9) split one line into several bidi paragraphs, and cosmic-text
/// panics when they differ in direction — a blamed binary file or a
/// right-to-left text file with CRLF endings crashed the whole app.
pub fn code_line(s: &str) -> String {
    code_text(s.strip_suffix('\r').unwrap_or(s))
}

/// [`code_line`] for a piece of a line (a highlighted span): keeps `\r`
/// visible wherever it is.
pub fn code_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => out.push_str("    "),
            '\0'..='\x1f' => {
                out.push(char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'));
            }
            '\x7f' => out.push('␡'),
            '\u{85}' | '\u{2028}' | '\u{2029}' => out.push('␤'),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_columns_counts_wide_characters_twice() {
        assert_eq!(truncate_columns("abcdef", 6), "abcdef");
        assert_eq!(truncate_columns("abcdefg", 6), "abcde…");
        // 한글 is two columns a character: 2 + 2 fit in 5 with the `…`.
        assert_eq!(truncate_columns("한글커밋", 5), "한글…");
        assert_eq!(truncate_columns("한글커밋", 8), "한글커밋");
    }

    #[test]
    fn code_lines_have_no_control_characters() {
        assert_eq!(code_line("a\tb\r"), "a    b");
        assert_eq!(code_line("a\rb\x1c\u{85}\u{2029}\x7f\0"), "a␍b␜␤␤␡␀");
        assert_eq!(code_text("x\r"), "x␍");
        assert_eq!(code_line("한글 שלום"), "한글 שלום");
    }

    /// The text renderer must accept whatever a binary file's bytes decode
    /// to (it asserted on mixed-direction paragraphs before).
    #[test]
    fn binary_looking_lines_render_without_panicking() {
        use iced::advanced::graphics::text::cosmic_text::{
            Attrs, Buffer, FontSystem, Metrics, Shaping,
        };
        let raw = "abc\x1cשלום\x1dxyz\rמה";
        let mut fonts = FontSystem::new();
        let mut buf = Buffer::new(&mut fonts, Metrics::new(14.0, 20.0));
        buf.set_text(
            &mut fonts,
            &code_line(raw),
            &Attrs::new(),
            Shaping::Advanced,
            None,
        );
        buf.shape_until_scroll(&mut fonts, false);
    }

    #[test]
    fn formats_dates_in_their_offset() {
        // 2024-01-01T00:00:00Z
        let utc = Timestamp {
            seconds: 1_704_067_200,
            offset_minutes: 0,
        };
        assert_eq!(datetime(utc), "2024-01-01 00:00");
        let kst = Timestamp {
            seconds: 1_704_067_200,
            offset_minutes: 540,
        };
        assert_eq!(datetime(kst), "2024-01-01 09:00");
        let west = Timestamp {
            seconds: 1_704_067_200,
            offset_minutes: -90,
        };
        assert_eq!(datetime(west), "2023-12-31 22:30");
        // Leap day.
        let leap = Timestamp {
            seconds: 1_709_164_800,
            offset_minutes: 0,
        };
        assert_eq!(datetime(leap), "2024-02-29 00:00");
    }

    #[test]
    fn formats_ages() {
        assert_eq!(age(Duration::from_secs(3)), "now");
        assert_eq!(age(Duration::from_secs(42)), "42s");
        assert_eq!(age(Duration::from_secs(300)), "5m");
        assert_eq!(age(Duration::from_secs(7_200)), "2h");
        assert_eq!(age(Duration::from_secs(3 * 86_400)), "3d");
        assert_eq!(age(Duration::from_secs(30 * 86_400)), "4w");
        assert_eq!(age(Duration::from_secs(59 * 86_400)), "8w");
        assert_eq!(age(Duration::from_secs(90 * 86_400)), "3mo");
        assert_eq!(age(Duration::from_secs(364 * 86_400)), "12mo");
        assert_eq!(age(Duration::from_secs(800 * 86_400)), "2y");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(Duration::from_millis(12)), "12ms");
        assert_eq!(duration(Duration::from_millis(1400)), "1.4s");
        assert_eq!(duration(Duration::from_secs(123)), "2m03s");
    }

    #[test]
    fn truncates_and_expands() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(expand_tilde("~/x"), Path::new(&home).join("x"));
            assert_eq!(expand_tilde("~"), Path::new(&home));
        }
        assert_eq!(expand_tilde("/abs"), Path::new("/abs"));
        assert_eq!(expand_tilde("~user/x"), Path::new("~user/x"));
    }
}
