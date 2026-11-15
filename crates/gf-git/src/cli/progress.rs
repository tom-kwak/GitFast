/// One progress update parsed from git's stderr, e.g.
/// `Receiving objects:  45% (450/1000), 1.20 MiB | 1.00 MiB/s`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// Phase label such as `Receiving objects` or `Resolving deltas`.
    pub phase: String,
    /// Reported by the remote side (`remote: ` prefix).
    pub remote: bool,
    pub percent: u8,
    pub current: u64,
    pub total: u64,
}

impl Progress {
    /// Parses a single `\r`/`\n`-delimited stderr segment. Returns `None` for
    /// lines that are not percentage progress.
    pub fn parse(segment: &str) -> Option<Self> {
        let segment = segment.trim();
        let (remote, rest) = match segment.strip_prefix("remote:") {
            Some(r) => (true, r.trim_start()),
            None => (false, segment),
        };
        let (phase, rest) = rest.split_once(':')?;
        let rest = rest.trim_start();

        let (percent, rest) = rest.split_once('%')?;
        let percent: u8 = percent.trim().parse().ok()?;

        let counts = rest.trim_start().strip_prefix('(')?;
        let (counts, _) = counts.split_once(')')?;
        let (current, total) = counts.split_once('/')?;

        Some(Self {
            phase: phase.trim().to_owned(),
            remote,
            percent: percent.min(100),
            current: current.trim().parse().ok()?,
            total: total.trim().parse().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote_progress() {
        let p = Progress::parse("Receiving objects:  45% (450/1000), 1.20 MiB | 1.00 MiB/s")
            .expect("progress");
        assert_eq!(
            p,
            Progress {
                phase: "Receiving objects".into(),
                remote: false,
                percent: 45,
                current: 450,
                total: 1000,
            }
        );

        let p = Progress::parse("remote: Counting objects: 100% (5/5), done.").expect("progress");
        assert!(p.remote);
        assert_eq!(p.phase, "Counting objects");
        assert_eq!((p.percent, p.current, p.total), (100, 5, 5));
    }

    #[test]
    fn ignores_other_lines() {
        assert_eq!(Progress::parse("Cloning into 'repo'..."), None);
        assert_eq!(
            Progress::parse("remote: Enumerating objects: 5, done."),
            None
        );
        assert_eq!(Progress::parse("error: something: 12% broken"), None);
        assert_eq!(Progress::parse(""), None);
    }
}
