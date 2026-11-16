//! Builds patches for partial staging: a whole hunk or selected lines of it.
//!
//! The patch is applied with `git apply --recount` (see
//! [`Repository::apply_patch`](crate::Repository::apply_patch)):
//!
//! | action                  | diff it comes from | apply with           | `reverse` |
//! |-------------------------|--------------------|----------------------|-----------|
//! | stage lines             | work tree          | `--cached`           | false     |
//! | unstage lines           | staged             | `--cached --reverse` | true      |
//! | discard lines           | work tree          | `--reverse`          | true      |
//!
//! Unselected changes must still match the side the patch is applied to:
//! forward patches turn unselected `-` lines into context and drop
//! unselected `+` lines; reverse patches do the opposite.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use gf_core::model::{FileChange, FileDiff, LineKind};

/// Whether partial (hunk/line) operations are possible for this file.
/// New, deleted, renamed and binary files, and submodules (whose
/// `Subproject commit` patch `git apply` accepts but ignores), are handled
/// as whole entries.
pub fn supports_partial(file: &FileDiff) -> bool {
    file.change == FileChange::Modified
        && !file.is_binary
        && !file.hunks.is_empty()
        && !file.is_submodule()
}

/// Patch for hunk `hunk` of `file`, limited to `lines` (indices into the
/// hunk's lines) when given. `None` when the selection contains no change
/// or the file does not support partial operations.
pub fn hunk_patch(
    file: &FileDiff,
    hunk: usize,
    lines: Option<&BTreeSet<usize>>,
    reverse: bool,
) -> Option<String> {
    if !supports_partial(file) {
        return None;
    }
    let h = file.hunks.get(hunk)?;
    let path = file.display_path()?;

    let mut body = String::new();
    let (mut old_count, mut new_count, mut changes) = (0u32, 0u32, 0usize);
    for (i, line) in h.lines.iter().enumerate() {
        let selected = lines.is_none_or(|s| s.contains(&i));
        let marker = match (line.kind, selected, reverse) {
            (LineKind::Context, _, _) => ' ',
            (LineKind::Added, true, _) => '+',
            (LineKind::Removed, true, _) => '-',
            (LineKind::Added, false, false) | (LineKind::Removed, false, true) => continue,
            (LineKind::Removed, false, false) | (LineKind::Added, false, true) => ' ',
        };
        match marker {
            ' ' => {
                old_count += 1;
                new_count += 1;
            }
            '-' => {
                old_count += 1;
                changes += 1;
            }
            _ => {
                new_count += 1;
                changes += 1;
            }
        }
        body.push(marker);
        body.push_str(&line.content);
        body.push('\n');
        if line.no_newline_at_eof {
            body.push_str("\\ No newline at end of file\n");
        }
    }
    if changes == 0 {
        return None;
    }

    let name = quote_path(path);
    let mut patch = String::new();
    let _ = writeln!(patch, "diff --git a/{name} b/{name}");
    let _ = writeln!(patch, "--- a/{name}");
    let _ = writeln!(patch, "+++ b/{name}");
    let _ = writeln!(
        patch,
        "@@ -{},{old_count} +{},{new_count} @@",
        h.old_start, h.new_start
    );
    patch.push_str(&body);
    Some(patch)
}

/// Paths are written unquoted unless they contain characters git quotes.
fn quote_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if !s.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return s.into_owned();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\{:03o}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::parse::diff::parse_diff;

    const DIFF: &str = "\
diff --git a/f.txt b/f.txt
index 1111111..2222222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,4 @@
 a
-b
-c
+B
+C
 d
";

    fn file() -> FileDiff {
        parse_diff(DIFF.as_bytes()).expect("parse").remove(0)
    }

    #[test]
    fn whole_hunk() {
        let p = hunk_patch(&file(), 0, None, false).expect("patch");
        assert!(
            p.contains("@@ -1,4 +1,4 @@\n a\n-b\n-c\n+B\n+C\n d\n"),
            "{p}"
        );
    }

    #[test]
    fn selected_lines_forward() {
        // Select "-b" (1) and "+B" (3): "-c" becomes context, "+C" is dropped.
        let sel = BTreeSet::from([1, 3]);
        let p = hunk_patch(&file(), 0, Some(&sel), false).expect("patch");
        assert!(p.contains("@@ -1,4 +1,4 @@\n a\n-b\n c\n+B\n d\n"), "{p}");
    }

    #[test]
    fn selected_lines_reverse() {
        // Reverse: unselected "+C" becomes context, unselected "-c" is dropped.
        let sel = BTreeSet::from([1, 3]);
        let p = hunk_patch(&file(), 0, Some(&sel), true).expect("patch");
        assert!(p.contains("@@ -1,4 +1,4 @@\n a\n-b\n+B\n C\n d\n"), "{p}");
    }

    #[test]
    fn no_change_selected_gives_none() {
        let sel = BTreeSet::from([0, 5]);
        assert!(hunk_patch(&file(), 0, Some(&sel), false).is_none());
    }

    #[test]
    fn submodules_have_no_hunk_patches() {
        let diff = "\
diff --git a/lib b/lib
--- a/lib
+++ b/lib
@@ -1 +1 @@
-Subproject commit aed28f9933c9e8435c0f1d07aa563c0a830859bc
+Subproject commit aed28f9933c9e8435c0f1d07aa563c0a830859bc-dirty
";
        let f = parse_diff(diff.as_bytes()).expect("parse").remove(0);
        assert!(f.is_submodule());
        assert!(!file().is_submodule());
        assert!(hunk_patch(&f, 0, None, true).is_none());
    }

    #[test]
    fn quotes_special_paths() {
        assert_eq!(quote_path(Path::new("plain dir/a.txt")), "plain dir/a.txt");
        assert_eq!(quote_path(Path::new("tab\there")), "\"tab\\there\"");
    }
}
