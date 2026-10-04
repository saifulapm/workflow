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
    // usage-billing.server.ts, billing.ts. Both rows stand, the way checkout
    // has both of its.
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
/// `packages/shopify-core/**` is one repo's blast radius and nobody else's.
/// Same grammar as a task's `Files:` line, so a glob with a space in it goes in
/// double quotes.
fn rows() -> Vec<String> {
    let declared = memcli::project_current()
        .and_then(|p| p.review_paths)
        .unwrap_or_default();
    merged(&declared)
}

/// The shipped table with the project's `review-paths` value added.
fn merged(review_paths: &str) -> Vec<String> {
    let mut rows: Vec<String> = ROWS.iter().map(|r| r.to_string()).collect();
    for pattern in ownership::split_patterns(review_paths) {
        if !rows.contains(&pattern) {
            rows.push(pattern);
        }
    }
    rows
}

/// The first of `paths` that the table or the project's `review-paths` globs
/// match, case aside as `review-needed` matches. A fix plan is judged by this
/// before any diff exists, so it takes paths rather than a change set.
pub fn risky(paths: &[String], review_paths: &str) -> Option<String> {
    let rows: Vec<String> = merged(review_paths)
        .iter()
        .map(|r| r.to_lowercase())
        .collect();
    paths
        .iter()
        .find(|p| {
            let p = p.to_lowercase();
            rows.iter().any(|r| glob_match(r, &p))
        })
        .cloned()
}

/// Whether `path` matches `pattern` as a `:(glob,top)` pathspec does: `*` and
/// `?` stay inside one segment, a `**` segment spans any number of them, none
/// included, and the pattern is anchored at the root. Case counts.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let segs: Vec<&str> = path.split('/').collect();
    match_segments(&pat, &segs)
}

fn match_segments(pat: &[&str], segs: &[&str]) -> bool {
    match pat.split_first() {
        None => segs.is_empty(),
        Some((&"**", rest)) => (0..=segs.len()).any(|i| match_segments(rest, &segs[i..])),
        Some((p, rest)) => match segs.split_first() {
            Some((s, more)) => {
                match_segment(p.as_bytes(), s.as_bytes()) && match_segments(rest, more)
            }
            None => false,
        },
    }
}

fn match_segment(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| match_segment(rest, &s[i..])),
        Some((&c, rest)) => match s.split_first() {
            Some((&d, more)) => (c == b'?' || c == d) && match_segment(rest, more),
            None => false,
        },
    }
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
    fn risky_names_the_first_path_a_row_matches_whatever_its_case() {
        let paths = |ps: &[&str]| ps.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(
            risky(
                &paths(&["app/Services/Cart.php", "app/billing/tax.php"]),
                ""
            ),
            Some("app/billing/tax.php".into())
        );
        // `**/` matches no directory at all, and a row matches StudlyCase.
        assert_eq!(
            risky(&paths(&["billing.server.ts"]), ""),
            Some("billing.server.ts".into())
        );
        assert_eq!(
            risky(&paths(&["Auth/Guard.php"]), ""),
            Some("Auth/Guard.php".into())
        );
        assert_eq!(
            risky(&paths(&["app/Http/Middleware/Authenticate.php"]), ""),
            Some("app/Http/Middleware/Authenticate.php".into())
        );
        // A root row stays at the root, and `*` stops at a slash.
        assert_eq!(risky(&paths(&["apps/x/package.json"]), ""), None);
        assert_eq!(
            risky(&paths(&["package.json"]), ""),
            Some("package.json".into())
        );
        assert_eq!(risky(&paths(&["app/Services/Cart.php"]), ""), None);
    }

    #[test]
    fn risky_reads_the_projects_review_paths_too() {
        let paths = vec!["packages/shopify-core/src/Client.ts".to_string()];
        assert_eq!(risky(&paths, ""), None);
        assert_eq!(
            risky(&paths, "docs/** \"packages/shopify-core/**\""),
            Some("packages/shopify-core/src/Client.ts".into())
        );
    }

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
