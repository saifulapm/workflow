//! The ownership evaluator of spec §8.7.
//!
//! One evaluator on both sides of the subset. `git status` is the only thing
//! that sees untracked files, and `-z` is the only output that does not quote or
//! collapse names -- so both sides use exactly it (review-3 F-7).
//!
//! Records, not fields: a rename is `R <new>\0<old>`, two NUL fields for one
//! record, and comparing fields would let half a rename look owned.
//!
//! Deliberately case-sensitive: a case-mismatched write should fail, not merge
//! (spec §9).

use std::collections::BTreeSet;
use std::path::Path;

use crate::gitcmd::{self, Git};

/// The separator that joins the fields of one record, and the stand-in for a
/// newline inside a path. Both are bytes no path can hold.
pub const SEP: u8 = 1;
const NL: u8 = 2;

fn tidy(field: &[u8]) -> Vec<u8> {
    field
        .iter()
        .map(|b| if *b == b'\n' { NL } else { *b })
        .collect()
}

/// `git status --porcelain -uall -z`, one entry per record.
fn status_records(bytes: &[u8]) -> BTreeSet<Vec<u8>> {
    let mut out = BTreeSet::new();
    let mut pending: Option<Vec<u8>> = None;
    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            continue;
        }
        let s = tidy(field);
        if let Some(mut rec) = pending.take() {
            rec.push(SEP);
            rec.extend_from_slice(&s);
            out.insert(rec);
            continue;
        }
        let x = s.first().copied().unwrap_or(0);
        let y = s.get(1).copied().unwrap_or(0);
        if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
            pending = Some(s);
            continue;
        }
        out.insert(s);
    }
    if let Some(rec) = pending {
        out.insert(rec);
    }
    out
}

/// `git diff --name-status -z -M`, one entry per record: a status field, then
/// one path, or two when it is a rename or a copy.
fn diff_records(bytes: &[u8]) -> BTreeSet<Vec<u8>> {
    let mut out = BTreeSet::new();
    let mut status: Option<Vec<u8>> = None;
    let mut first: Option<Vec<u8>> = None;
    let mut want = 0;

    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            continue;
        }
        let s = tidy(field);
        if want == 0 {
            want = match s.first() {
                Some(b'R') | Some(b'C') => 2,
                _ => 1,
            };
            status = Some(s);
            first = None;
            continue;
        }
        if want == 2 {
            first = Some(s);
            want = 1;
            continue;
        }
        let mut rec = status.take().unwrap_or_default();
        if let Some(a) = first.take() {
            rec.push(SEP);
            rec.extend_from_slice(&a);
        }
        rec.push(SEP);
        rec.extend_from_slice(&s);
        out.insert(rec);
        want = 0;
    }
    out
}

/// Whitespace-separated globs, double quotes around one that contains a space.
pub fn split_patterns(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Every record the task touched that its `Files:` patterns do not claim.
///
/// `anchor` is what the branch is measured against, and the measurement is
/// from where the two last agreed -- `anchor...branch`, not `anchor..branch`.
/// Two-dot compares tips, and both arrangements of a live run get that wrong
/// (friction #A2JXGNB8): a branch that took the integration branch in to reach
/// a dependency is charged with every file its siblings merged, and a branch
/// that never needed to is charged with deleting them. Measured from the merge
/// base, both see only what this task wrote.
pub fn violations(wt: &Path, anchor: &str, branch: &str, patterns: &[String]) -> Vec<Vec<u8>> {
    let git = Git::at(wt);
    let specs: Vec<String> = if patterns.is_empty() {
        vec![gitcmd::glob_top("__nothing_is_owned__")]
    } else {
        patterns.iter().map(|p| gitcmd::glob_top(p)).collect()
    };
    let spec_args: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();

    let mut out: Vec<Vec<u8>> = Vec::new();

    let status = ["status", "--porcelain", "-uall", "-z"];
    let mut owned_args: Vec<&str> = status.to_vec();
    owned_args.push("--");
    owned_args.extend(&spec_args);
    let all = status_records(&git.bytes(&status));
    let owned = status_records(&git.bytes(&owned_args));
    out.extend(all.difference(&owned).cloned());

    let range = format!("{anchor}...{branch}");
    let diff = ["diff", "--name-status", "-z", "-M", range.as_str()];
    let mut owned_args: Vec<&str> = diff.to_vec();
    owned_args.push("--");
    owned_args.extend(&spec_args);
    let all = diff_records(&git.bytes(&diff));
    let owned = diff_records(&git.bytes(&owned_args));
    out.extend(all.difference(&owned).cloned());

    out
}

/// The records as one line each, with the record separator shown.
pub fn show(records: &[Vec<u8>]) -> Vec<String> {
    records
        .iter()
        .map(|r| {
            String::from_utf8_lossy(r)
                .replace(SEP as char, "|")
                .replace(NL as char, "\\n")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(recs: &BTreeSet<Vec<u8>>) -> Vec<String> {
        recs.iter()
            .map(|r| String::from_utf8_lossy(r).replace(SEP as char, "|"))
            .collect()
    }

    #[test]
    fn a_rename_is_one_status_record_of_two_fields() {
        let bytes = b"R  app/New.php\0app/Old.php\0 M app/Other.php\0?? notes/scratch.txt\0";
        let recs = joined(&status_records(bytes));
        assert!(recs.contains(&"R  app/New.php|app/Old.php".to_string()));
        assert!(recs.contains(&" M app/Other.php".to_string()));
        assert!(recs.contains(&"?? notes/scratch.txt".to_string()));
        assert_eq!(recs.len(), 3);
    }

    #[test]
    fn a_diff_rename_is_one_record_of_three_fields() {
        let bytes = b"R100\0app/Old.php\0app/New.php\0M\0app/Kept.php\0";
        let recs = joined(&diff_records(bytes));
        assert!(recs.contains(&"R100|app/Old.php|app/New.php".to_string()));
        assert!(recs.contains(&"M|app/Kept.php".to_string()));
        assert_eq!(recs.len(), 2);
    }

    #[test]
    fn patterns_split_on_whitespace_and_quotes_keep_their_spaces() {
        assert_eq!(
            split_patterns("app/Services/Cart*.php tests/Unit/Cart*"),
            vec!["app/Services/Cart*.php", "tests/Unit/Cart*"]
        );
        assert_eq!(
            split_patterns("a/b \"one path/with space.php\" c"),
            vec!["a/b", "one path/with space.php", "c"]
        );
        assert!(split_patterns("   ").is_empty());
    }
}
