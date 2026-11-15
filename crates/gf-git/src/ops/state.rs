//! Which multi-step operation (merge, rebase, cherry-pick, revert, bisect)
//! the repository is in the middle of, read from the marker files git keeps
//! in the git directory.

use std::path::Path;

use gf_core::Result;

use crate::Repository;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RepoState {
    #[default]
    Clean,
    Merging {
        /// First line of the prepared merge message (`Merge branch 'x'`).
        message: Option<String>,
    },
    Rebasing {
        /// Branch being rebased.
        branch: Option<String>,
        /// `(current step, total steps)` when git reports it.
        progress: Option<(u32, u32)>,
        interactive: bool,
    },
    CherryPicking,
    Reverting,
    Bisecting,
}

impl RepoState {
    pub fn is_clean(&self) -> bool {
        *self == Self::Clean
    }

    /// Short name for banners: `MERGING`, `REBASING 2/5`, ...
    pub fn label(&self) -> String {
        match self {
            Self::Clean => "CLEAN".into(),
            Self::Merging { .. } => "MERGING".into(),
            Self::Rebasing {
                progress: Some((n, total)),
                ..
            } => format!("REBASING {n}/{total}"),
            Self::Rebasing { .. } => "REBASING".into(),
            Self::CherryPicking => "CHERRY-PICKING".into(),
            Self::Reverting => "REVERTING".into(),
            Self::Bisecting => "BISECTING".into(),
        }
    }

    /// Whether `--skip` applies (rebase, cherry-pick, revert).
    pub fn can_skip(&self) -> bool {
        matches!(
            self,
            Self::Rebasing { .. } | Self::CherryPicking | Self::Reverting
        )
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
}

/// Reads the state from a git directory (pure filesystem check).
pub fn detect(git_dir: &Path) -> RepoState {
    // Rebase takes precedence: a rebase can stop on a cherry-pick-like step.
    for (dir, interactive) in [("rebase-merge", true), ("rebase-apply", false)] {
        let d = git_dir.join(dir);
        if d.is_dir() {
            let num = |f: &str| read_trimmed(&d.join(f)).and_then(|s| s.parse::<u32>().ok());
            let progress = match dir {
                "rebase-merge" => num("msgnum").zip(num("end")),
                _ => num("next").zip(num("last")),
            };
            let branch = read_trimmed(&d.join("head-name"))
                .map(|h| h.trim_start_matches("refs/heads/").to_owned())
                .filter(|h| h != "detached HEAD");
            let interactive = interactive && d.join("interactive").exists();
            return RepoState::Rebasing {
                branch,
                progress,
                interactive,
            };
        }
    }
    if git_dir.join("MERGE_HEAD").exists() {
        let message = read_trimmed(&git_dir.join("MERGE_MSG"))
            .and_then(|m| m.lines().next().map(str::to_owned));
        return RepoState::Merging { message };
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return RepoState::CherryPicking;
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return RepoState::Reverting;
    }
    if git_dir.join("BISECT_LOG").exists() {
        return RepoState::Bisecting;
    }
    RepoState::Clean
}

impl Repository {
    pub async fn state(&self) -> Result<RepoState> {
        Ok(detect(&self.handle().git_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_marker_files() {
        let dir = tempfile_dir();
        let g = dir.as_path();
        assert_eq!(detect(g), RepoState::Clean);

        std::fs::write(g.join("MERGE_HEAD"), "x").expect("write");
        std::fs::write(g.join("MERGE_MSG"), "Merge branch 'feature'\n\n# comment").expect("write");
        assert_eq!(
            detect(g),
            RepoState::Merging {
                message: Some("Merge branch 'feature'".into())
            }
        );
        std::fs::remove_file(g.join("MERGE_HEAD")).expect("rm");

        let rm = g.join("rebase-merge");
        std::fs::create_dir(&rm).expect("mkdir");
        std::fs::write(rm.join("msgnum"), "2\n").expect("write");
        std::fs::write(rm.join("end"), "5\n").expect("write");
        std::fs::write(rm.join("head-name"), "refs/heads/topic\n").expect("write");
        std::fs::write(rm.join("interactive"), "").expect("write");
        let s = detect(g);
        assert_eq!(s.label(), "REBASING 2/5");
        assert!(
            matches!(s, RepoState::Rebasing { branch: Some(ref b), interactive: true, .. } if b == "topic")
        );
        assert!(s.can_skip());
        std::fs::remove_dir_all(&rm).expect("rm");

        std::fs::write(g.join("BISECT_LOG"), "").expect("write");
        assert_eq!(detect(g), RepoState::Bisecting);
        let _ = std::fs::remove_dir_all(g);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("gf-state-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }
}
