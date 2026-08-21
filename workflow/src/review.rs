//! `workflow review-needed` -- the table of spec §9 and the change set that
//! feeds it.

use std::collections::BTreeSet;

use crate::gitcmd::{self, Git};
use crate::{exit, memcli, ownership, repo, warn};

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
    // Billing is usually files, not a directory: billing.server.ts,
    // usage-billing.server.ts, billing.ts (friction #HK2PNTR4). Both rows
    // stand, the way checkout has both of its.
    "**/*billing*",
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
    // The Shopify app surface. The config carries scopes, webhook
    // subscriptions and the app's urls, and `shopify app deploy` puts it
    // live; a webhook handler is the HMAC boundary and the GDPR topics; a
    // session file holds offline admin tokens. `**/` on the config because
    // the apps sit under `apps/<name>/` in a monorepo, and the directory row
    // beside the name row because a glob star does not cross a slash.
    "**/shopify.app*.toml",
    "**/webhooks*/**",
    "**/*webhook*",
    "**/*session*",
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

/// The rows this checkout is judged by: the shipped table, plus whatever the
/// project declared with `mem project set review-paths`.
///
/// Merged, never replaced. The shipped rows are what is sensitive in every
/// repository; the project's rows are what is load-bearing in this one --
/// `packages/shopify-core/**` is one repo's blast radius and nobody else's
/// (friction #HK2PNTR4). Same grammar as a task's `Files:` line, so a glob
/// with a space in it goes in double quotes.
fn rows() -> Vec<String> {
    let mut rows: Vec<String> = ROWS.iter().map(|r| r.to_string()).collect();
    let declared = memcli::project_current()
        .and_then(|p| p.review_paths)
        .unwrap_or_default();
    for pattern in ownership::split_patterns(&declared) {
        if !rows.contains(&pattern) {
            rows.push(pattern);
        }
    }
    rows
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

    let rows = rows();
    let specs: Vec<String> = rows.iter().map(|r| gitcmd::glob_icase_top(r)).collect();
    let all = matches(&git, &range, &specs);
    if all.is_empty() {
        println!("review-needed: no");
        return exit::FAILED;
    }

    println!("review-needed: yes");
    for row in &rows {
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
