//! Unified diff output of `git diff` / `git show` with [`DIFF_FORMAT_ARGS`].

use gf_core::model::{DiffLine, FileChange, FileDiff, Hunk, LineKind};
use gf_core::{GfError, Result};

use super::util::{bytes_to_path, unquote_c_path};

/// Flags that pin the diff format regardless of user config
/// (`diff.noprefix`, `diff.relative`, external diff drivers, colors).
pub const DIFF_FORMAT_ARGS: &[&str] = &[
    "--no-color",
    "--no-ext-diff",
    "--no-relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--find-renames",
];

#[derive(Debug, Default)]
struct FileBuilder {
    old_path: Option<Vec<u8>>,
    new_path: Option<Vec<u8>>,
    is_new: bool,
    is_deleted: bool,
    rename: bool,
    copy: bool,
    similarity: u8,
    old_mode: Option<String>,
    new_mode: Option<String>,
    is_binary: bool,
    hunks: Vec<Hunk>,
}

impl FileBuilder {
    fn finish(self) -> FileDiff {
        let change = if self.is_new {
            FileChange::Added
        } else if self.is_deleted {
            FileChange::Deleted
        } else if self.rename {
            FileChange::Renamed {
                similarity: self.similarity,
            }
        } else if self.copy {
            FileChange::Copied {
                similarity: self.similarity,
            }
        } else {
            FileChange::Modified
        };
        FileDiff {
            old_path: (!self.is_new)
                .then_some(self.old_path)
                .flatten()
                .map(|p| bytes_to_path(&p)),
            new_path: (!self.is_deleted)
                .then_some(self.new_path)
                .flatten()
                .map(|p| bytes_to_path(&p)),
            change,
            old_mode: self.old_mode,
            new_mode: self.new_mode,
            is_binary: self.is_binary,
            hunks: self.hunks,
        }
    }
}

/// Lines still expected in the current hunk, and the next line numbers.
#[derive(Debug, Default)]
struct HunkCursor {
    old_left: u32,
    new_left: u32,
    old_no: u32,
    new_no: u32,
}

impl HunkCursor {
    fn active(&self) -> bool {
        self.old_left > 0 || self.new_left > 0
    }
}

pub fn parse_diff(out: &[u8]) -> Result<Vec<FileDiff>> {
    let mut files = Vec::new();
    let mut file: Option<FileBuilder> = None;
    let mut cursor = HunkCursor::default();

    let out = out.strip_suffix(b"\n").unwrap_or(out);
    for line in out.split(|&b| b == b'\n') {
        if let Some(f) = file.as_mut() {
            if cursor.active() {
                hunk_line(f, &mut cursor, line)?;
                continue;
            }
            if line.starts_with(b"\\") {
                mark_no_newline(f);
                continue;
            }
        }

        if let Some(header) = line.strip_prefix(b"diff --git ") {
            files.extend(file.take().map(FileBuilder::finish));
            let (old, new) = split_git_header(header);
            file = Some(FileBuilder {
                old_path: old,
                new_path: new,
                ..FileBuilder::default()
            });
            continue;
        }

        // Anything before the first `diff --git` (e.g. a commit header) is skipped.
        let Some(f) = file.as_mut() else { continue };
        let text = String::from_utf8_lossy(line);

        if line.starts_with(b"@@ ") {
            let hunk = parse_hunk_header(&text)?;
            cursor = HunkCursor {
                old_left: hunk.old_lines,
                new_left: hunk.new_lines,
                old_no: hunk.old_start,
                new_no: hunk.new_start,
            };
            f.hunks.push(hunk);
        } else if let Some(p) = line.strip_prefix(b"--- ") {
            if let Some(p) = side_path(p, b"a/") {
                f.old_path = Some(p);
            }
        } else if let Some(p) = line.strip_prefix(b"+++ ") {
            if let Some(p) = side_path(p, b"b/") {
                f.new_path = Some(p);
            }
        } else if let Some(p) = line.strip_prefix(b"rename from ") {
            f.rename = true;
            f.old_path = Some(unquote_c_path(p));
        } else if let Some(p) = line.strip_prefix(b"rename to ") {
            f.rename = true;
            f.new_path = Some(unquote_c_path(p));
        } else if let Some(p) = line.strip_prefix(b"copy from ") {
            f.copy = true;
            f.old_path = Some(unquote_c_path(p));
        } else if let Some(p) = line.strip_prefix(b"copy to ") {
            f.copy = true;
            f.new_path = Some(unquote_c_path(p));
        } else if let Some(v) = text.strip_prefix("similarity index ") {
            f.similarity = v.trim_end_matches('%').parse().unwrap_or(0);
        } else if let Some(m) = text.strip_prefix("new file mode ") {
            f.is_new = true;
            f.new_mode = Some(m.to_owned());
        } else if let Some(m) = text.strip_prefix("deleted file mode ") {
            f.is_deleted = true;
            f.old_mode = Some(m.to_owned());
        } else if let Some(m) = text.strip_prefix("old mode ") {
            f.old_mode = Some(m.to_owned());
        } else if let Some(m) = text.strip_prefix("new mode ") {
            f.new_mode = Some(m.to_owned());
        } else if let Some(rest) = text.strip_prefix("index ") {
            // `index abc..def 100644` carries the mode when it is unchanged.
            if let Some((_, mode)) = rest.split_once(' ') {
                f.old_mode.get_or_insert_with(|| mode.to_owned());
                f.new_mode.get_or_insert_with(|| mode.to_owned());
            }
        } else if line.starts_with(b"Binary files ") || line == b"GIT binary patch" {
            f.is_binary = true;
        }
    }
    files.extend(file.map(FileBuilder::finish));
    Ok(files)
}

fn hunk_line(f: &mut FileBuilder, c: &mut HunkCursor, line: &[u8]) -> Result<()> {
    let (kind, content) = match line.split_first() {
        Some((b' ', rest)) => (LineKind::Context, rest),
        Some((b'+', rest)) => (LineKind::Added, rest),
        Some((b'-', rest)) => (LineKind::Removed, rest),
        Some((b'\\', _)) => {
            mark_no_newline(f);
            return Ok(());
        }
        // Some tools strip the space from empty context lines.
        None => (LineKind::Context, line),
        Some(_) => {
            return Err(GfError::parse(
                "diff",
                format!("unexpected hunk line: {}", String::from_utf8_lossy(line)),
            ));
        }
    };
    let (old_no, new_no) = match kind {
        LineKind::Context => (Some(c.old_no), Some(c.new_no)),
        LineKind::Removed => (Some(c.old_no), None),
        LineKind::Added => (None, Some(c.new_no)),
    };
    if old_no.is_some() {
        c.old_no += 1;
        c.old_left = c.old_left.saturating_sub(1);
    }
    if new_no.is_some() {
        c.new_no += 1;
        c.new_left = c.new_left.saturating_sub(1);
    }
    if let Some(h) = f.hunks.last_mut() {
        h.lines.push(DiffLine {
            kind,
            content: String::from_utf8_lossy(content).into_owned(),
            old_lineno: old_no,
            new_lineno: new_no,
            no_newline_at_eof: false,
        });
    }
    Ok(())
}

fn mark_no_newline(f: &mut FileBuilder) {
    if let Some(l) = f.hunks.last_mut().and_then(|h| h.lines.last_mut()) {
        l.no_newline_at_eof = true;
    }
}

/// `@@ -1,5 +1,6 @@ fn section` (counts default to 1 when omitted).
fn parse_hunk_header(line: &str) -> Result<Hunk> {
    let bad = || GfError::parse("hunk header", line.to_owned());
    let rest = line.strip_prefix("@@ -").ok_or_else(bad)?;
    let (ranges, section) = rest.split_once(" @@").ok_or_else(bad)?;
    let (old, new) = ranges.split_once(" +").ok_or_else(bad)?;
    let range = |r: &str| -> Result<(u32, u32)> {
        let (start, len) = r.split_once(',').unwrap_or((r, "1"));
        Ok((
            start.parse().map_err(|_| bad())?,
            len.parse().map_err(|_| bad())?,
        ))
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Ok(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        section: section.trim_start().to_owned(),
        lines: Vec::new(),
    })
}

/// Path from a `---`/`+++` line; `None` for `/dev/null`.
fn side_path(raw: &[u8], prefix: &[u8]) -> Option<Vec<u8>> {
    // Paths with spaces may carry a trailing tab in some git versions.
    let raw = raw.strip_suffix(b"\t").unwrap_or(raw);
    if raw == b"/dev/null" {
        return None;
    }
    let path = unquote_c_path(raw);
    Some(
        path.strip_prefix(prefix)
            .map(<[u8]>::to_vec)
            .unwrap_or(path),
    )
}

/// Best-effort split of `a/<old> b/<new>`. Exact paths are filled in later
/// from `---`/`+++`/`rename` lines when present; this matters for entries
/// without them (mode-only changes, binary files, pure renames).
fn split_git_header(h: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let strip = |p: Vec<u8>, prefix: &[u8]| p.strip_prefix(prefix).map(<[u8]>::to_vec).unwrap_or(p);

    if h.first() == Some(&b'"') {
        // Quoted old path: find its closing quote, skipping escapes.
        let mut i = 1;
        while i < h.len() && h[i] != b'"' {
            i += if h[i] == b'\\' { 2 } else { 1 };
        }
        let old = unquote_c_path(&h[..(i + 1).min(h.len())]);
        let new = unquote_c_path(h.get(i + 2..).unwrap_or_default());
        return (Some(strip(old, b"a/")), Some(strip(new, b"b/")));
    }
    if let Some(pos) = h.windows(4).position(|w| w == b" \"b/") {
        return (
            Some(strip(h[..pos].to_vec(), b"a/")),
            Some(strip(unquote_c_path(&h[pos + 1..]), b"b/")),
        );
    }
    // Unquoted and same name on both sides: "a/X b/X".
    if h.len() % 2 == 1 {
        let mid = h.len() / 2;
        let (old, new) = (&h[..mid], &h[mid + 1..]);
        if old.get(2..) == new.get(2..) {
            return (
                Some(strip(old.to_vec(), b"a/")),
                Some(strip(new.to_vec(), b"b/")),
            );
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const MODIFY: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,3 @@ mod a;
 fn a() {}
-fn b() {}
+fn b() { todo() }
-- a/x looks like a header
 fn c() {}
@@ -10 +10,2 @@
 last
+added
\\ No newline at end of file
";

    #[test]
    fn parses_modification_with_line_numbers() {
        let files = parse_diff(MODIFY.as_bytes()).expect("parse");
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.change, FileChange::Modified);
        assert_eq!(f.display_path(), Some(&PathBuf::from("src/lib.rs")));
        assert_eq!(f.old_mode.as_deref(), Some("100644"));
        assert_eq!(f.hunks.len(), 2);

        let h = &f.hunks[0];
        assert_eq!(h.section, "mod a;");
        assert_eq!(h.lines.len(), 5);
        assert_eq!(h.lines[1].kind, LineKind::Removed);
        assert_eq!(h.lines[1].old_lineno, Some(2));
        assert_eq!(h.lines[2].new_lineno, Some(2));
        // A removed line whose text starts with "- " must not be taken as a header.
        assert_eq!(h.lines[3].kind, LineKind::Removed);
        assert_eq!(h.lines[3].content, "- a/x looks like a header");
        assert_eq!(h.lines[4].old_lineno, Some(4));

        let h2 = &f.hunks[1];
        assert_eq!((h2.old_start, h2.old_lines, h2.new_lines), (10, 1, 2));
        assert!(h2.lines[1].no_newline_at_eof);
        assert_eq!(f.line_stats(), (2, 2));
    }

    #[test]
    fn parses_add_delete_rename_mode_and_binary() {
        let out = "\
diff --git a/new file.txt b/new file.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new file.txt
@@ -0,0 +1 @@
+hello
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 3333333..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/old.rs b/new.rs
similarity index 100%
rename from old.rs
rename to new.rs
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
diff --git a/logo.png b/logo.png
index 4444444..5555555 100644
Binary files a/logo.png and b/logo.png differ
";
        let files = parse_diff(out.as_bytes()).expect("parse");
        assert_eq!(files.len(), 5);

        assert_eq!(files[0].change, FileChange::Added);
        assert_eq!(files[0].old_path, None);
        assert_eq!(files[0].new_path, Some(PathBuf::from("new file.txt")));

        assert_eq!(files[1].change, FileChange::Deleted);
        assert_eq!(files[1].new_path, None);

        assert_eq!(files[2].change, FileChange::Renamed { similarity: 100 });
        assert_eq!(files[2].old_path, Some(PathBuf::from("old.rs")));
        assert_eq!(files[2].new_path, Some(PathBuf::from("new.rs")));

        assert!(files[3].mode_changed());
        assert_eq!(files[3].new_path, Some(PathBuf::from("run.sh")));

        assert!(files[4].is_binary);
        assert!(files[4].hunks.is_empty());
    }

    #[test]
    fn handles_quoted_paths() {
        let out = "\
diff --git \"a/tab\\there.txt\" \"b/tab\\there.txt\"
old mode 100644
new mode 100755
";
        let files = parse_diff(out.as_bytes()).expect("parse");
        assert_eq!(files[0].new_path, Some(PathBuf::from("tab\there.txt")));
    }

    #[test]
    fn empty_diff() {
        assert!(parse_diff(b"").expect("parse").is_empty());
    }

    #[test]
    fn rejects_bad_hunk_header() {
        assert!(parse_diff(b"diff --git a/x b/x\n@@ -a +b @@\n").is_err());
    }
}
