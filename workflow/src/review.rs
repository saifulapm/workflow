//! `workflow review-needed` -- the table of spec §9 and the change set that
//! feeds it.

use std::collections::BTreeSet;

use crate::gitcmd::{self, Git};
use crate::{exit, repo, warn};

/// Every row is matched case-insensitively: on the primary stack the
/// interesting files are StudlyCase, and a case-sensitive table was blind to
/// most of them (review-4 B-1).
pub const ROWS: &[&str] = &[
    "**/auth/**",
    "**/middleware/*auth*",
    "**/policies/**",
    "**/permission*",
    "**/payment*",
    "**/billing/**",
    "**/stripe*",
    "**/checkout/**",
    "**/checkout*",
    "**/.env*",
    "**/secrets*",
    "**/credential*",
    "**/*key*.pem",
    "**/migrations/**",
    "**/jobs/**",
    "**/queue*/**",
    "**/cron*",
    "package.json",
    "pnpm-lock.yaml",
    "composer.*",
    "Cargo.*",
    "Gemfile*",
    "Dockerfile*",
    "docker-compose*",
    ".github/workflows/**",
    "**/deploy*/**",
    "Caddyfile*",
    "**/routes/api*",
    "**/openapi*",
    "**/*.graphql",
];

/// `XY <path>` from porcelain output, with the two status letters removed.
fn strip_status(field: &str) -> &str {
    let b = field.as_bytes();
    let is_code = |c: u8| b" MADRCU?!".contains(&c);
    if b.len() >= 3 && is_code(b[0]) && is_code(b[1]) && b[2] == b' ' {
        &field[3..]
    } else {
        field
    }
}

/// The change set is what is in the tree *and* what the range holds: a brand-new
/// auth file or an untracked .env is invisible to diff alone (review-3 F-12).
fn matches(git: &Git, range: &str, specs: &[String]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();

    let mut args: Vec<&str> = vec!["status", "--porcelain", "-uall", "-z", "--"];
    args.extend(specs.iter().map(|s| s.as_str()));
    for field in gitcmd::nul_fields(&git.bytes(&args)) {
        let text = gitcmd::lossy(&field);
        let path = strip_status(&text).trim();
        if !path.is_empty() {
            found.insert(path.to_string());
        }
    }

    if !range.is_empty() {
        let mut args: Vec<&str> = vec!["diff", "--name-only", "-z", range, "--"];
        args.extend(specs.iter().map(|s| s.as_str()));
        for field in gitcmd::nul_fields(&git.bytes(&args)) {
            let path = gitcmd::lossy(&field).trim().to_string();
            if !path.is_empty() {
                found.insert(path);
            }
        }
    }

    found
}

pub fn cmd_review_needed(range: Option<&str>) -> i32 {
    let range = range.unwrap_or("").to_string();

    if !Git::here().inside_worktree() {
        warn("not inside a git work tree");
        return exit::FAILED;
    }
    let Some((git, _top)) = repo::goto_toplevel() else {
        warn("cannot resolve the repository toplevel");
        return exit::FAILED;
    };

    // An unreadable range is not an answer of "no": say so and ask for the review.
    if !range.is_empty() && !git.quiet(&["rev-list", "--max-count=1", &range]) {
        warn(format!("review-needed: cannot read the range '{range}'"));
        println!("review-needed: yes");
        return exit::OK;
    }

    let specs: Vec<String> = ROWS.iter().map(|r| gitcmd::glob_icase_top(r)).collect();
    let all = matches(&git, &range, &specs);
    if all.is_empty() {
        println!("review-needed: no");
        return exit::FAILED;
    }

    println!("review-needed: yes");
    for row in ROWS {
        let hits = matches(&git, &range, &[gitcmd::glob_icase_top(row)]);
        if hits.is_empty() {
            continue;
        }
        let joined: Vec<String> = hits.into_iter().collect();
        println!("  {:<24} {}", row, joined.join(" "));
    }
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_status_letters_come_off_and_nothing_else_does() {
        assert_eq!(strip_status("?? .env"), ".env");
        assert_eq!(
            strip_status("M  app/Services/Cart.php"),
            "app/Services/Cart.php"
        );
        assert_eq!(strip_status("R  app/New.php"), "app/New.php");
        // The second field of a rename record is a bare path.
        assert_eq!(strip_status("app/Old.php"), "app/Old.php");
    }
}
