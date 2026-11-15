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
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
