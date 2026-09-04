//! The checkout half of `workflow plan-check` (frictions #DBHZBFY1 and
//! #6485CNC0): a plan is judged against the tree it will run in, not only
//! against its own grammar. Everything here is knowable before dispatch, and
//! each finding used to cost a worker a whole attempt to rediscover.
//!
//! Refusals are Verify lines that cannot pass in this checkout. Warnings are
//! Files lines that do not look like they can hold their task -- warnings,
//! because a plan can mean to create what is not there yet.

use std::path::Path;

use crate::gitcmd::{self, Git};
use crate::ownership;
use crate::plan::{Plan, Task};

pub struct Findings {
    pub refusals: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn findings(plan: &Plan, root: &Path, plan_file: Option<&Path>) -> Findings {
    let git = Git::at(root);
    let mut f = Findings {
        refusals: Vec::new(),
        warnings: Vec::new(),
    };
    // A plan names every interface it plans, so it answers `git grep` for all
    // of them. Counting itself made every new symbol look like a change the
    // plan forgot to own (friction #33WY4FAR).
    let itself = repo_relative(plan_file, root);
    // Every path some task's Files could carry. A Gives identifier living in
    // a file outside this union is a change the plan forgot to own: the value
    // moves in the files one worker holds while the file asserting it belongs
    // to nobody (friction #8M2YDDXH).
    let mut claimed = std::collections::HashSet::new();
    for t in &plan.tasks {
        for p in ownership::split_patterns(t.files.as_deref().unwrap_or("")) {
            let spec = gitcmd::glob_top(&p);
            claimed.extend(zlines(&git.bytes(&["ls-files", "-z", "--", &spec])));
        }
    }
    for t in &plan.tasks {
        if t.checked {
            continue; // never dispatched, so its lines are history, not risk
        }
        let verify = t.verify.as_deref().unwrap_or("");
        if let Some(msg) = lib_test_without_lib(&t.id, verify, root) {
            f.refusals.push(msg);
        }
        // Done: states what the task delivers, so deferral there refuses. A
        // title only warns: "Sweep every TBD out of the docs" names the
        // marker it removes, and blocking that plan would be the check
        // defeating its own point.
        match deferral(t.done.as_deref().unwrap_or("")) {
            Some(Deferral::Refuse(p)) => f.refusals.push(format!(
                "plan: task {}: Done says '{p}' -- that defers work this plan should finish; cut the scope honestly or plan the work",
                t.id
            )),
            Some(Deferral::Warn(p)) => f.warnings.push(format!(
                "plan: task {}: Done says '{p}' -- fine if the task removes one, a deferral if it ships one",
                t.id
            )),
            None => {}
        }
        if let Some(Deferral::Refuse(p) | Deferral::Warn(p)) = deferral(&t.title) {
            f.warnings.push(format!(
                "plan: task {}: '{p}' in the title -- fine if the task removes it, a deferral if it ships it",
                t.id
            ));
        }
        let patterns = ownership::split_patterns(t.files.as_deref().unwrap_or(""));
        for p in &patterns {
            if matches_nothing(&git, root, p) {
                f.warnings.push(format!(
                    "plan: task {}: '{p}' matches nothing here and its directory does not exist -- a task creating it, or a typo",
                    t.id
                ));
            } else if only_ignored(&git, p) {
                f.warnings.push(format!(
                    "plan: task {}: '{p}' matches only gitignored paths -- no worktree can carry them, so the merge gate will never see this work",
                    t.id
                ));
            }
        }
        // A Done sentence that names a file is a claim about what the task's
        // commit holds, and the gate refuses everything outside Files: -- so
        // the two disagreeing is knowable here rather than after a worker has
        // spent a whole context on the task (friction #RT818QJG).
        let owned: std::collections::HashSet<String> = patterns
            .iter()
            .flat_map(|p| zlines(&git.bytes(&["ls-files", "-z", "--", &gitcmd::glob_top(p)])))
            .collect();
        for path in done_paths(&git, t.done.as_deref().unwrap_or(""), &owned) {
            f.warnings.push(format!(
                "plan: task {}: Done names '{path}' and no Files: pattern claims it -- the gate refuses whatever a task writes outside them",
                t.id
            ));
        }
        // Read and Pattern point at what the worker opens before editing. By
        // the time it runs its dependencies have landed, so a file one of them
        // writes is there to be read even though this checkout has no such
        // path yet (friction #33WY4FAR).
        let waited_for = ancestors(plan, t);
        let missing = |p: &str| {
            !root.join(p).exists()
                && git.bytes(&["ls-files", "-z", "--", p]).is_empty()
                && !written_by(plan, &waited_for, p)
        };
        let read = t.read.as_deref().unwrap_or("");
        for p in ownership::split_patterns(read) {
            if missing(&p) {
                f.warnings.push(format!(
                    "plan: task {}: Read names '{p}' and it is not here to be read, nor does a task it waits for write it",
                    t.id
                ));
            }
        }
        if let Some(pat) = t.pattern.as_deref() {
            let path = pattern_path(pat);
            if missing(path) {
                f.warnings.push(format!(
                    "plan: task {}: Pattern points at '{path}' and it is not here to copy from",
                    t.id
                ));
            }
        }
        // A consumed interface comes from a dependency's Gives or the tree;
        // one that comes from neither is a name the worker will hunt for. The
        // dependency need not be a direct one: a plan chains `[after:]`, and
        // what t1 gives reaches t3 through t2 (friction #EYC8DHKV).
        for ident in uses_idents(t.uses.as_deref().unwrap_or("")) {
            if !greppable(&ident) {
                continue;
            }
            let given = waited_for.iter().any(|d| {
                plan.get(d)
                    .and_then(|dep| dep.gives.as_deref())
                    .is_some_and(|g| g.contains(&ident))
            });
            if !given && named_by(&git, &ident, itself.as_deref()).is_empty() {
                f.warnings.push(format!(
                    "plan: task {}: Uses names '{ident}' and no task it waits for Gives it, nor does the tree",
                    t.id
                ));
            }
        }
        for ident in uses_idents(t.gives.as_deref().unwrap_or("")) {
            if !greppable(&ident) {
                continue;
            }
            let named: Vec<String> = named_by(&git, &ident, itself.as_deref())
                .into_iter()
                .filter(|file| !claimed.contains(file))
                .collect();
            if named.is_empty() {
                continue;
            }
            let shown = named.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
            let more = match named.len().saturating_sub(5) {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            f.warnings.push(format!(
                "plan: task {}: Gives '{ident}' -- {shown}{more} also name(s) it and no task's Files claims them, so that side of the change is nobody's to make",
                t.id
            ));
        }
        if runs_tests(verify) && !patterns.iter().any(|p| p.to_lowercase().contains("test")) {
            f.warnings.push(format!(
                "plan: task {}: its Verify runs tests and its Files list no test file -- the worker cannot add the test that proves it",
                t.id
            ));
        }
    }
    f
}

/// `cargo test --lib` in a crate with no library target runs nothing and can
/// never go green -- the trap four tasks each paid an attempt to find
/// (friction #DBHZBFY1). A workspace manifest is left alone: the member that
/// Verify would run in is not knowable from here.
fn lib_test_without_lib(task: &str, verify: &str, root: &Path) -> Option<String> {
    if !verify.contains("cargo test") || !verify.split_whitespace().any(|w| w == "--lib") {
        return None;
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    if manifest.contains("[workspace]") || manifest.contains("[lib]") {
        return None;
    }
    if root.join("src/lib.rs").is_file() {
        return None;
    }
    Some(format!(
        "plan: task {task}: Verify runs 'cargo test --lib' and this crate has no library target -- it can never pass here"
    ))
}

/// Nothing tracked matches the pattern, the literal path is not there, and
/// neither is the directory it points into. One of those three usually holds
/// even for a file the task will create; when none does, the pattern is
/// probably not naming what the author thought.
fn matches_nothing(git: &Git, root: &Path, pattern: &str) -> bool {
    if root.join(pattern).exists() {
        return false;
    }
    let spec = gitcmd::glob_top(pattern);
    if !git.bytes(&["ls-files", "-z", "--", &spec]).is_empty() {
        return false;
    }
    let fixed: String = pattern
        .chars()
        .take_while(|c| !matches!(c, '*' | '?' | '['))
        .collect();
    let dir = match fixed.rsplit_once('/') {
        Some((d, _)) => root.join(d),
        None => root.to_path_buf(),
    };
    !dir.is_dir()
}

/// Nothing tracked answers the pattern and git sees only ignored matches --
/// files that exist on disk, which is why `matches_nothing` waves them
/// through, but that no commit can carry (friction #A3WHPGE3). The
/// `check-ignore` probe catches the literal path a task would create straight
/// into an ignored directory; on a glob it never matches and decides nothing.
fn only_ignored(git: &Git, pattern: &str) -> bool {
    let spec = gitcmd::glob_top(pattern);
    if !git.bytes(&["ls-files", "-z", "--", &spec]).is_empty() {
        return false;
    }
    let ignored = ["ls-files", "-z", "-o", "-i", "--exclude-standard", "--", &spec];
    !git.bytes(&ignored).is_empty() || git.quiet(&["check-ignore", "-q", "--", pattern])
}

fn runs_tests(verify: &str) -> bool {
    verify.contains("test")
}

/// The paths a Done sentence names that `owned` does not hold, in the order
/// the sentence names them and once each.
///
/// A token is a candidate when it carries a dot or a slash; the tree settles
/// the rest, so prose that merely looks path-shaped -- "e.g.", "identical." --
/// names nothing here and is dropped. A token naming a directory counts as
/// claimed the moment any file under it is.
fn done_paths(git: &Git, done: &str, owned: &std::collections::HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in done.split_whitespace() {
        let token = token
            .trim_matches(|c| "`'\"()[]{},;:!?".contains(c))
            .trim_end_matches('.');
        if !token.contains('.') && !token.contains('/') {
            continue;
        }
        if out.iter().any(|seen| seen == token) {
            continue;
        }
        let hits = zlines(&git.bytes(&["ls-files", "-z", "--", token]));
        if hits.is_empty() || hits.iter().any(|h| owned.contains(h)) {
            continue;
        }
        out.push(token.to_string());
    }
    out
}

/// The plan file as git names it, when it is inside this checkout.
fn repo_relative(file: Option<&Path>, root: &Path) -> Option<String> {
    let file = file?.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    Some(file.strip_prefix(root).ok()?.to_string_lossy().to_string())
}

/// The tracked files naming the identifier, the plan's own file aside.
fn named_by(git: &Git, ident: &str, itself: Option<&str>) -> Vec<String> {
    zlines(&git.bytes(&["grep", "-l", "-z", "-F", ident]))
        .into_iter()
        .filter(|file| Some(file.as_str()) != itself)
        .collect()
}

/// Every task this one waits for, directly or through another. A cycle would
/// already have been refused by the wave sort; `seen` guards anyway.
fn ancestors(plan: &Plan, task: &Task) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut queue: Vec<String> = task.deps.clone();
    while let Some(id) = queue.pop() {
        if seen.contains(&id) {
            continue;
        }
        if let Some(t) = plan.get(&id) {
            queue.extend(t.deps.iter().cloned());
        }
        seen.push(id);
    }
    seen
}

/// Some task in `deps` claims the path in its Files, so it will exist by the
/// time this task runs.
fn written_by(plan: &Plan, deps: &[String], path: &str) -> bool {
    deps.iter().filter_map(|d| plan.get(d)).any(|dep| {
        ownership::split_patterns(dep.files.as_deref().unwrap_or(""))
            .iter()
            .any(|p| covers(p, path))
    })
}

/// Does the pattern claim the path? A pattern with no glob in it is the path
/// itself or a directory holding it; otherwise `*` stops at a slash and `**`
/// crosses one, as in the pathspec the pattern becomes.
fn covers(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    if pattern == path {
        return true;
    }
    if !pattern.contains(['*', '?']) {
        return path.starts_with(&format!("{pattern}/"));
    }
    glob(pattern.as_bytes(), path.as_bytes())
}

fn glob(pat: &[u8], s: &[u8]) -> bool {
    match pat.first() {
        None => s.is_empty(),
        Some(b'*') if pat.get(1) == Some(&b'*') => {
            // `a/**/b` covers `a/b` too, so the crossing wildcard may eat the
            // separator that follows it or nothing at all.
            let rest = &pat[2..];
            let rest = if rest.first() == Some(&b'/') {
                &rest[1..]
            } else {
                rest
            };
            (0..=s.len()).any(|i| glob(rest, &s[i..]))
        }
        Some(b'*') => {
            let rest = &pat[1..];
            let stop = s.iter().position(|c| *c == b'/').unwrap_or(s.len());
            (0..=stop).any(|i| glob(rest, &s[i..]))
        }
        Some(b'?') => !s.is_empty() && s[0] != b'/' && glob(&pat[1..], &s[1..]),
        Some(c) => s.first() == Some(c) && glob(&pat[1..], &s[1..]),
    }
}

/// An identifier worth asking the tree about. Uses and Gives name exact
/// signatures, so a real symbol carries an underscore or a capital --
/// `set_data`, `StackEntry`, `setData`. A bare lowercase word is usually what
/// this reader made of prose it could not parse: 'untouched' out of
/// "engine.rs untouched", 'rs' out of "pub mod data in lib.rs". Grepping one
/// names half the repo and says nothing (friction #33WY4FAR).
fn greppable(ident: &str) -> bool {
    ident.contains('_') || ident.chars().any(|c| c.is_ascii_uppercase())
}

/// NUL-separated git output, one path per entry.
fn zlines(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect()
}

/// `path:12-25` and `path:40` point into a file; anything else is the path
/// itself, colons and all.
fn pattern_path(p: &str) -> &str {
    match p.rsplit_once(':') {
        Some((path, lines))
            if !lines.is_empty() && lines.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            path
        }
        _ => p,
    }
}

/// One identifier per ` · `-separated item: the token nearest the call site
/// (`CartPricing::price(...)` names `price`), or the item's only token when
/// nothing is called. Declaration keywords never count as the name. Two items
/// reducing to one identifier are reported once, not twice.
fn uses_idents(uses: &str) -> Vec<String> {
    const KEYWORDS: [&str; 12] = [
        "fn", "pub", "struct", "enum", "class", "function", "def", "let", "const", "type",
        "impl", "trait",
    ];
    uses.split(" · ")
        .filter_map(|item| {
            let head = item.split('(').next().unwrap_or(item);
            // `CartPricing::price: Cents` names price, not its return type.
            let head = match head.rsplit_once(": ") {
                Some((h, _)) if !h.is_empty() => h,
                _ => head,
            };
            head.split(|c: char| !c.is_alphanumeric() && c != '_')
                .rfind(|t| !t.is_empty() && !KEYWORDS.contains(t))
                .map(str::to_string)
        })
        .fold(Vec::new(), |mut out, ident| {
            if !out.contains(&ident) {
                out.push(ident);
            }
            out
        })
}

/// What a deferral idiom in a task block means for the plan.
#[derive(Debug)]
pub enum Deferral {
    /// The plan defers work it should finish; not ready to dispatch.
    Refuse(&'static str),
    /// The word can name deferred work or work that removes it; a human reads.
    Warn(&'static str),
}

/// Scope-reduction language is how a plan quietly ships less than was asked
/// (research/reports/11): the idioms below are refused outright, while a bare
/// "placeholder" or "stub" only warns because a removal task legitimately
/// names the thing it deletes. Matches are case-insensitive and stop at word
/// boundaries, so "for nowhere" and "stubborn" pass. Only the title and the
/// Done: line are read -- a Verify that greps for TBD or a Files path named
/// tbd/ is a task removing deferrals, not making one.
pub fn deferral(intent: &str) -> Option<Deferral> {
    const REFUSE: [&str; 5] = [
        "tbd",
        "for now",
        "wired later",
        "implement later",
        "simplified version",
    ];
    const WARN: [&str; 2] = ["placeholder", "stub"];
    let lower = intent.to_lowercase();
    for p in REFUSE {
        if found_whole(&lower, p) {
            return Some(Deferral::Refuse(p));
        }
    }
    for p in WARN {
        if found_whole(&lower, p) {
            return Some(Deferral::Warn(p));
        }
    }
    None
}

/// The phrase appears with nothing word-like touching either end.
fn found_whole(haystack: &str, phrase: &str) -> bool {
    let mut from = 0;
    while let Some(at) = haystack[from..].find(phrase) {
        let start = from + at;
        let end = start + phrase.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if boundary(before) && boundary(after) {
            return true;
        }
        from = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_pointer_gives_up_its_line_suffix_and_nothing_else() {
        assert_eq!(pattern_path("src/old.rs:12-25"), "src/old.rs");
        assert_eq!(pattern_path("src/old.rs:40"), "src/old.rs");
        assert_eq!(pattern_path("src/old.rs"), "src/old.rs");
        assert_eq!(pattern_path("scripts/build:release"), "scripts/build:release");
    }

    #[test]
    fn a_uses_item_yields_the_identifier_nearest_its_call_site() {
        assert_eq!(
            uses_idents("fn price(basket: &Basket) -> Cents · Basket::fixture(): Basket"),
            vec!["price", "fixture"]
        );
        assert_eq!(uses_idents("CartPricing::price(Basket $b): Cents"), vec!["price"]);
        // A colon-typed item without parens names the symbol, not its type.
        assert_eq!(uses_idents("CartPricing::price: Cents"), vec!["price"]);
        assert_eq!(uses_idents("DEFAULT_MODEL"), vec!["DEFAULT_MODEL"]);
        assert!(uses_idents("").is_empty());
    }

    /// deferral reads only what findings() hands it -- the Done: line
    /// (refusals) and the title (downgraded to warnings there), never Verify
    /// commands or Files/Read paths -- so a task that greps TBD out of the
    /// docs is removing deferrals, not making one.
    #[test]
    fn deferral_idioms_are_refused_and_a_bare_placeholder_only_warns() {
        for text in [
            "the simplified version of pricing",
            "static for now",
            "TBD",
            "the button gets wired later by t2",
            "render it, implement later the rest",
        ] {
            assert!(
                matches!(deferral(text), Some(Deferral::Refuse(_))),
                "{text:?} should be refused"
            );
        }
        for text in ["Remove the placeholder hub page", "Drop the stub route"] {
            assert!(
                matches!(deferral(text), Some(Deferral::Warn(_))),
                "{text:?} should only warn"
            );
        }
        for text in [
            "totals identical for the fixture basket",
            "A stubborn cache is invalidated", // 'stub' inside a word is not a stub
            "Reach for nowhere-else state",    // nor is 'for now' inside one
        ] {
            assert!(deferral(text).is_none(), "{text:?} is honest work");
        }
    }

    /// The identifiers that made the dactyl m8 plan print 45 warnings for two
    /// real ones: every junk token this reader pulled out of a prose Gives
    /// item is a bare lowercase word (friction #33WY4FAR).
    #[test]
    fn only_a_symbol_shaped_identifier_is_worth_grepping_for() {
        for real in [
            "set_data", "setData", "StackEntry", "text_of", "screen_tree_bound", "DEFAULT_MODEL",
            "Value",
        ] {
            assert!(greppable(real), "{real:?} is a symbol");
        }
        for junk in [
            "rs",
            "a",
            "l",
            "10",
            "item",
            "untouched",
            "update",
            "once",
            "after",
        ] {
            assert!(!greppable(junk), "{junk:?} is prose, not a symbol");
        }
    }

    #[test]
    fn a_files_pattern_covers_the_paths_its_pathspec_would() {
        assert!(covers("engine/src/data.rs", "engine/src/data.rs"));
        assert!(covers("engine", "engine/src/data.rs"));
        assert!(covers("engine/", "engine/src/data.rs"));
        assert!(covers("engine/tests/*.rs", "engine/tests/repeat.rs"));
        assert!(covers("engine/**/*.rs", "engine/src/a/b.rs"));
        assert!(covers("docs/**", "docs/plan/11.md"));
        // `*` stops at a separator; `**` is how a pattern crosses one.
        assert!(!covers("engine/*.rs", "engine/src/data.rs"));
        assert!(!covers("engine/src/data.rs", "engine/src/data.rs.bak"));
        assert!(!covers("engine", "engineer/x.rs"));
        assert!(!covers("host-web/src/*.ts", "engine/src/data.rs"));
    }

    /// A plan chains `[after:]`, so what t1 gives reaches t3 through t2 --
    /// which is why the Uses check reads the whole chain (friction #EYC8DHKV).
    #[test]
    fn a_task_waits_for_its_dependencies_dependencies_too() {
        let text = "\
# plan: chain

- [ ] t1 first
      Files: a.rs
      Verify: true
- [ ] t2 second  [after: t1]
      Files: b.rs
      Verify: true
- [ ] t3 third  [after: t2]
      Files: c.rs
      Verify: true
- [ ] t4 apart
      Files: d.rs
      Verify: true
";
        let plan = crate::plan::parse(text, true).expect("the plan parses");
        let mut chain = ancestors(&plan, plan.get("t3").unwrap());
        chain.sort();
        assert_eq!(chain, vec!["t1", "t2"]);
        assert!(ancestors(&plan, plan.get("t1").unwrap()).is_empty());
        assert!(ancestors(&plan, plan.get("t4").unwrap()).is_empty());
        // Read: is answered by any of them, not only the direct one.
        assert!(written_by(&plan, &chain, "a.rs"));
        assert!(written_by(&plan, &chain, "b.rs"));
        assert!(!written_by(&plan, &chain, "d.rs"));
    }

    #[test]
    fn a_test_run_is_recognised_by_the_word_and_true_is_not() {
        assert!(runs_tests("cargo test --test e2e"));
        assert!(runs_tests("pnpm run test:unit"));
        assert!(runs_tests("bin/php artisan test --filter=Cart"));
        assert!(!runs_tests("true"));
        assert!(!runs_tests("cargo build"));
    }
}
