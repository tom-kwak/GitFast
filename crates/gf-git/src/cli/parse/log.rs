//! `git log -z --date=raw --format=<LOG_FORMAT>`.
//!
//! Commits are NUL-separated; fields within a commit are separated by the
//! ASCII unit separator (0x1f), which does not occur in names or emails.
//! The message is last so that any byte it contains is preserved.

use gf_core::model::{Commit, Oid, Signature, Timestamp};
use gf_core::{GfError, Result};

use super::util::split_records;

/// hash, parents, author name/email/date, committer name/email/date, message.
pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%ad%x1f%cn%x1f%ce%x1f%cd%x1f%B";

pub const LOG_ARGS: &[&str] = &[
    "-c",
    "log.showSignature=false",
    "log",
    "-z",
    "--date=raw",
    "--no-decorate",
    "--no-abbrev-commit",
    LOG_FORMAT,
];

const FIELDS: usize = 9;

pub fn parse_log(out: &[u8]) -> Result<Vec<Commit>> {
    split_records(out, 0).map(parse_commit).collect()
}

fn parse_commit(rec: &[u8]) -> Result<Commit> {
    let text = String::from_utf8_lossy(rec);
    // `git log -z` separates commits with NUL but may leave the newline that
    // `%B` ends with; tolerate a leading newline from the previous record.
    let text = text.trim_start_matches('\n');
    let f: Vec<&str> = text.splitn(FIELDS, '\x1f').collect();
    let [id, parents, an, ae, ad, cn, ce, cd, message] = f.as_slice() else {
        return Err(GfError::parse(
            "log",
            format!("expected {FIELDS} fields, got {}", f.len()),
        ));
    };

    let (summary, body) = split_message(message);
    Ok(Commit {
        id: id.parse()?,
        parents: parents
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<Vec<Oid>>>()?,
        author: signature(an, ae, ad)?,
        committer: signature(cn, ce, cd)?,
        summary,
        body,
    })
}

fn signature(name: &str, email: &str, raw_date: &str) -> Result<Signature> {
    Ok(Signature {
        name: name.to_owned(),
        email: email.to_owned(),
        time: parse_raw_date(raw_date)?,
    })
}

/// Parses `--date=raw` output: `1704067200 +0900`.
pub fn parse_raw_date(raw: &str) -> Result<Timestamp> {
    let bad = || GfError::parse("date", raw.to_owned());
    let (secs, tz) = raw.trim().split_once(' ').ok_or_else(bad)?;
    let seconds = secs.parse().map_err(|_| bad())?;
    let (sign, digits) = match tz.as_bytes().first() {
        Some(b'-') => (-1, &tz[1..]),
        Some(b'+') => (1, &tz[1..]),
        _ => return Err(bad()),
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let hours: i32 = digits[..2].parse().map_err(|_| bad())?;
    let minutes: i32 = digits[2..].parse().map_err(|_| bad())?;
    Ok(Timestamp {
        seconds,
        offset_minutes: sign * (hours * 60 + minutes),
    })
}

/// Splits a raw message into summary (first paragraph joined into one line,
/// as git does for `%s`) and body.
pub fn split_message(message: &str) -> (String, String) {
    let message = message.trim_end();
    let (subject, body) = match message.split_once("\n\n") {
        Some((s, b)) => (s, b.trim_start_matches('\n')),
        None => (message, ""),
    };
    let summary = subject.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    (summary, body.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
    const B: &str = "b94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
    const C: &str = "c94a8fe5ccb19ba61c4c0873d391e987982fbbd3";

    fn record(id: &str, parents: &str, message: &str) -> String {
        format!(
            "{id}\x1f{parents}\x1fKim\x1fkim@example.com\x1f1704067200 +0900\x1f\
             Lee\x1flee@example.com\x1f1704070800 -0130\x1f{message}"
        )
    }

    #[test]
    fn parses_commits() {
        let out = format!(
            "{}\0{}\n\0{}\n",
            record(A, &format!("{B} {C}"), "Merge branch 'x'\n"),
            record(B, C, "Fix bug\n\nLonger explanation\nwith lines\n"),
            record(C, "", "root\n"),
        );
        let commits = parse_log(out.as_bytes()).expect("parse");
        assert_eq!(commits.len(), 3);

        let merge = &commits[0];
        assert_eq!(merge.id.as_str(), A);
        assert!(merge.is_merge());
        assert_eq!(merge.summary, "Merge branch 'x'");
        assert_eq!(merge.author.name, "Kim");
        assert_eq!(merge.author.time.offset_minutes, 9 * 60);
        assert_eq!(merge.committer.email, "lee@example.com");
        assert_eq!(merge.committer.time.offset_minutes, -90);

        assert_eq!(commits[1].summary, "Fix bug");
        assert_eq!(commits[1].body, "Longer explanation\nwith lines");
        assert!(commits[2].is_root());
    }

    #[test]
    fn message_with_separator_chars_survives() {
        let out = record(A, "", "weird \x1f subject\n");
        let commits = parse_log(out.as_bytes()).expect("parse");
        assert_eq!(commits[0].summary, "weird \x1f subject");
    }

    #[test]
    fn multi_line_subject_is_joined() {
        assert_eq!(
            split_message("line one\nline two\n\nbody"),
            ("line one line two".into(), "body".into())
        );
    }

    #[test]
    fn rejects_bad_dates_and_records() {
        assert!(parse_raw_date("123").is_err());
        assert!(parse_raw_date("123 0900").is_err());
        assert!(parse_log(b"not a commit").is_err());
    }
}
