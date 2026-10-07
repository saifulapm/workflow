//! `workflow hygiene` -- what a product repository must never carry: files an
//! agent harness writes, references a reader of the repository cannot follow,
//! and the tells of generated text in commit messages.
//!
//! Two tiers. A hard finding exits 1 and nothing clears it. Soft findings are
//! read in messages only, lint-msg's vocabulary and style tables plus a few
//! process words, and a lint-exception ruling naming the term clears it.
//! Source is never read for the soft words: this repository and its siblings
//! use "agent" and "session" as product vocabulary.

use std::path::Path;

use serde::Serialize;

use crate::gitcmd::Git;
use crate::{exit, lint, memcli, warn};

/// Directories an agent harness or its tools write. Anywhere in the tree,
/// tracked, they are a tell.
const IGNORED_DIRS: &[&str] = &[
    ".claude",
    ".agents",
    ".cursor",
    ".scratch",
    ".e2e",
    ".amx",
    ".playwright-cli",
    "agent-memory",
];

/// Files the same tools write, matched by name at any depth.
const IGNORED_FILES: &[&str] = &[
    "CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    ".mcp.json",
    "opencode.json",
    "opencode.jsonc",
    "skills-lock.json",
];

/// Files an agent harness reads as its instructions, at any depth.
const INSTRUCTION_FILES: &[&str] = &["CLAUDE.md", "CLAUDE.local.md", "AGENTS.md", ".mcp.json"];

/// Directories whose every file an agent harness reads as instructions.
const INSTRUCTION_DIRS: &[&str] = &[".agents", ".cursor", ".scratch"];

/// Directories tools write their own output to; a file named like an
/// instruction file in there is the tool's, not the repository's.
const TOOL_DIRS: &[&str] = &[
    ".amx",
    ".playwright-cli",
    "agent-memory",
    "agent-memory-local",
];

/// What the no-flag sweep reads of history.
const SWEEP_COMMITS: usize = 200;

/// Past this a subject wraps in `git log --oneline` and most review tools.
const SUBJECT_MAX: usize = 72;

/// Excerpts are cut here so a minified line does not flood the report.
const EXCERPT_MAX: usize = 120;

/// The hard tier on a line of source or a message. The phrases are built
/// with `concat!` so this file passes its own check.
const HARD: &[Rule] = &[
    Rule::Numbered("ruling"),
    Rule::Numbered("milestone"),
    Rule::Issue,
    Rule::Numbered("ticket"),
    Rule::Adr,
    Rule::Phrase(concat!("owner ", "decision")),
    Rule::Phrase(concat!("per the ", "plan")),
    Rule::Phrase(concat!("plan of ", "record")),
    Rule::MemId,
    Rule::CoAuthor,
    Rule::GeneratedWith,
    Rule::ClaudeCode,
    Rule::Robot,
];

/// Process words warned about in a message, beside lint-msg's own table,
/// which already carries agent, subagent and orchestrator.
const SOFT: &[&str] = &["session", "advisor", "dogfood", "handoff"];

#[derive(Clone, Copy)]
enum Rule {
    /// The word, then one space and a digit: a numbered reference.
    Numbered(&'static str),
    /// `(issue <digits>)`
    Issue,
    /// `ADR` and three or four digits.
    Adr,
    /// A fixed phrase, case-insensitively.
    Phrase(&'static str),
    /// `#` and eight of A-Z 0-9, at least one a digit and not all of them
    /// hexadecimal, so an eight-digit CSS colour never matches.
    MemId,
    CoAuthor,
    GeneratedWith,
    ClaudeCode,
    Robot,
}

impl Rule {
    fn label(&self) -> &'static str {
        match self {
            Rule::Numbered("ruling") => "numbered ruling",
            Rule::Numbered("milestone") => "numbered milestone",
            Rule::Numbered(_) => "numbered ticket",
            Rule::Issue => "issue number",
            Rule::Adr => "numbered ADR",
            Rule::Phrase(p) => p,
            Rule::MemId => "memory id",
            Rule::CoAuthor => "co-author trailer",
            Rule::GeneratedWith => "generated-with line",
            Rule::ClaudeCode => "Claude Code link",
            Rule::Robot => "robot emoji",
        }
    }

    fn hits(&self, line: &str) -> bool {
        let lower = line.to_lowercase();
        match self {
            Rule::Numbered(word) => lower.match_indices(word).any(|(i, w)| {
                let rest = &lower.as_bytes()[i + w.len()..];
                starts_word(&lower, i)
                    && rest.first() == Some(&b' ')
                    && rest.get(1).is_some_and(u8::is_ascii_digit)
            }),
            Rule::Issue => lower.match_indices("(issue ").any(|(i, w)| {
                let rest = &lower.as_bytes()[i + w.len()..];
                let digits = leading_digits(rest);
                digits > 0 && rest.get(digits) == Some(&b')')
            }),
            Rule::Adr => lower.match_indices("adr ").any(|(i, w)| {
                let rest = &lower.as_bytes()[i + w.len()..];
                starts_word(&lower, i) && (3..=4).contains(&leading_digits(rest))
            }),
            Rule::Phrase(p) => lower.contains(p),
            Rule::MemId => mem_id(line),
            Rule::CoAuthor => lower.contains(concat!("co-authored", "-by")),
            Rule::GeneratedWith => lower.contains(concat!("generated", " with")),
            Rule::ClaudeCode => lower.contains(concat!("claude.ai", "/code")),
            Rule::Robot => line.contains('\u{1f916}'),
        }
    }
}

fn leading_digits(s: &[u8]) -> usize {
    s.iter().take_while(|c| c.is_ascii_digit()).count()
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Nothing word-like stands right before byte `i`.
fn starts_word(s: &str, i: usize) -> bool {
    s[..i].chars().next_back().is_none_or(|c| !word_char(c))
}

fn mem_id(line: &str) -> bool {
    let b = line.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        let Some(id) = b.get(i + 1..i + 9) else {
            return false;
        };
        c == b'#'
            && id
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && id.iter().any(u8::is_ascii_digit)
            && !id.iter().all(u8::is_ascii_hexdigit)
            && b.get(i + 9).is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

/// `t3` standing alone: a task id.
fn task_id(line: &str) -> bool {
    line.match_indices('t').any(|(i, _)| {
        let rest = &line[i + 1..];
        let digits = leading_digits(rest.as_bytes());
        starts_word(line, i)
            && digits > 0
            && rest[digits..].chars().next().is_none_or(|c| !word_char(c))
    })
}

/// `m4-` standing alone: the head of a milestone slug.
fn milestone_id(line: &str) -> bool {
    line.match_indices('m').any(|(i, _)| {
        let rest = &line[i + 1..];
        let digits = leading_digits(rest.as_bytes());
        starts_word(line, i) && digits > 0 && rest[digits..].starts_with('-')
    })
}

/// The word, or its plural, with no letter on either side.
fn word_hit(lower: &str, word: &str) -> bool {
    lower.match_indices(word).any(|(i, w)| {
        let rest = &lower[i + w.len()..];
        let rest = rest.strip_prefix('s').unwrap_or(rest);
        lower[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphabetic())
            && rest.chars().next().is_none_or(|c| !c.is_alphabetic())
    })
}

fn ignored_path(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    let file = parts.pop().unwrap_or_default();
    IGNORED_FILES.contains(&file) || parts.iter().any(|d| IGNORED_DIRS.contains(d))
}

/// A path an agent harness reads as instructions, which belong in mem. Tool
/// output directories and Claude Code's own worktrees are left alone whatever
/// they hold: a worktree is judged from its own top.
fn instruction_path(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let under = |dir: &str, subs: &[&str]| {
        parts[..parts.len() - 1]
            .windows(2)
            .any(|w| w[0] == dir && subs.contains(&w[1]))
    };
    if parts.iter().any(|p| TOOL_DIRS.contains(p)) || under(".claude", &["worktrees"]) {
        return false;
    }
    let file = parts[parts.len() - 1];
    INSTRUCTION_FILES.contains(&file)
        || parts.iter().any(|p| INSTRUCTION_DIRS.contains(p))
        || under(".claude", &["skills", "agents", "commands", "rules"])
}

/// A `.gitignore` line naming something on the ignore list. The list is
/// global through `core.excludesFile`, so a repository's own line for it adds
/// nothing but the tell.
fn agent_ignore_line(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return false;
    }
    line.trim_start_matches('!')
        .split('/')
        .map(|part| part.trim_end_matches('*'))
        .any(|part| IGNORED_DIRS.contains(&part) || IGNORED_FILES.contains(&part))
}

fn is_gitignore(path: &str) -> bool {
    path.rsplit('/').next() == Some(".gitignore")
}

fn is_markdown(path: &str) -> bool {
    path.ends_with(".md") || path.ends_with(".markdown")
}

#[derive(Debug, Serialize)]
struct Finding {
    path: String,
    /// 0 for a finding about the path itself.
    line: usize,
    tier: &'static str,
    label: &'static str,
    excerpt: String,
}

#[derive(Default)]
struct Report {
    findings: Vec<Finding>,
    /// The lint-exception rulings, read once and only when a soft word hits.
    exceptions: Option<String>,
}

impl Report {
    fn add(
        &mut self,
        path: &str,
        line: usize,
        tier: &'static str,
        label: &'static str,
        text: &str,
    ) {
        let text = text.trim();
        let excerpt = match text.char_indices().nth(EXCERPT_MAX) {
            Some((cut, _)) => format!("{}...", &text[..cut]),
            None => text.to_string(),
        };
        self.findings.push(Finding {
            path: path.to_string(),
            line,
            tier,
            label,
            excerpt,
        });
    }

    fn hard(&self) -> bool {
        self.findings.iter().any(|f| f.tier == "hard")
    }

    fn path(&mut self, path: &str) {
        if ignored_path(path) {
            self.add(path, 0, "hard", "ignored path", path);
        }
    }

    /// One line of a file: the hard tier, and the ignore list in a .gitignore.
    fn source_line(&mut self, path: &str, n: usize, line: &str) {
        for rule in HARD {
            if rule.hits(line) {
                self.add(path, n, "hard", rule.label(), line);
            }
        }
        if is_gitignore(path) && agent_ignore_line(line) {
            self.add(path, n, "hard", "agent ignore line", line);
        }
    }

    /// A commit message, named by `path`: the hard tier, the subject, task
    /// and milestone ids, the soft words, then lint-msg's own tiers, which
    /// print themselves.
    fn message(&mut self, path: &str, text: &str) {
        let before = self.findings.len();
        for (i, line) in text.lines().enumerate() {
            let n = i + 1;
            for rule in HARD {
                if rule.hits(line) {
                    self.add(path, n, "hard", rule.label(), line);
                }
            }
            if n == 1 && line.chars().count() > SUBJECT_MAX {
                self.add(path, n, "hard", "long subject", line);
            }
            if task_id(line) {
                self.add(path, n, "hard", "task id", line);
            }
            if milestone_id(line) {
                self.add(path, n, "hard", "milestone id", line);
            }
            let lower = line.to_lowercase();
            for word in SOFT {
                if word_hit(&lower, word) && !self.cleared(word) {
                    self.add(path, n, "soft", word, line);
                }
            }
        }
        let found_hard = self.findings[before..].iter().any(|f| f.tier == "hard");
        if !lint::lint_text(text) && !found_hard {
            let subject = text.lines().next().unwrap_or_default();
            self.add(path, 1, "hard", "lint-msg provenance", subject);
        }
    }

    fn cleared(&mut self, word: &str) -> bool {
        self.exceptions
            .get_or_insert_with(|| memcli::ruling_bodies("lint-exception").to_lowercase())
            .contains(word)
    }

    fn print(&self, json: bool) {
        if json {
            println!(
                "{}",
                serde_json::to_string(&self.findings).unwrap_or_else(|_| "[]".into())
            );
            return;
        }
        for f in &self.findings {
            println!(
                "{}:{}: {} {}: {}",
                f.path, f.line, f.tier, f.label, f.excerpt
            );
        }
    }
}

/// Every added line of a `git diff -U0`, as (path, line number, text).
/// Header lines are told apart from added lines that happen to start with
/// `++` by where they sit: between `diff --git` and the first hunk.
fn added_lines(diff: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut path = String::new();
    let mut in_header = false;
    let mut next = 0;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            in_header = true;
            path.clear();
        } else if in_header {
            if let Some(p) = line.strip_prefix("+++ ") {
                let p = p.trim_matches('"');
                path = p.strip_prefix("b/").unwrap_or(p).to_string();
            } else if line.starts_with("@@") {
                in_header = false;
                next = hunk_start(line);
            }
        } else if line.starts_with("@@") {
            next = hunk_start(line);
        } else if let Some(text) = line.strip_prefix('+') {
            out.push((path.clone(), next, text.to_string()));
            next += 1;
        }
    }
    out
}

/// The new side's first line number in `@@ -a,b +c,d @@`.
fn hunk_start(header: &str) -> usize {
    header
        .split_whitespace()
        .find_map(|w| w.strip_prefix('+'))
        .and_then(|w| w.split(',').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn exempt(globs: &[String], path: &str) -> bool {
    globs.iter().any(|g| covers(g, path))
}

/// Does the pattern claim the path? A pattern with no glob in it is the path
/// itself or a directory holding it; otherwise `*` stops at a slash and `**`
/// crosses one, as in a pathspec.
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

fn with_spec<'a>(args: &[&'a str], spec: &'a [String]) -> Vec<&'a str> {
    let mut v = args.to_vec();
    v.push("--");
    v.extend(spec.iter().map(String::as_str));
    v
}

fn split_z(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).to_string())
        .collect()
}

fn scan_staged(git: &Git, spec: &[String], globs: &[String], report: &mut Report) {
    let names = git.bytes(&with_spec(
        &[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=ACMR",
        ],
        spec,
    ));
    for path in split_z(&names) {
        report.path(&path);
    }
    let diff = git.capture(&with_spec(
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--cached",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            "--diff-filter=ACMR",
        ],
        spec,
    ));
    for (path, n, text) in added_lines(&String::from_utf8_lossy(&diff.stdout)) {
        if !is_markdown(&path) && !exempt(globs, &path) {
            report.source_line(&path, n, &text);
        }
    }
}

/// Paths are the index's, read off the working tree; a file that will not
/// read as text is binary or gone and has no lines to check.
fn scan_tree(git: &Git, top: &Path, spec: &[String], globs: &[String], report: &mut Report) {
    let names = git.bytes(&with_spec(&["ls-files", "-z", "--full-name"], spec));
    for path in split_z(&names) {
        report.path(&path);
        if is_markdown(&path) || exempt(globs, &path) {
            continue;
        }
        let file = top.join(&path);
        if !std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let Ok(text) = std::fs::read(&file).map(String::from_utf8) else {
            continue;
        };
        let Ok(text) = text else { continue };
        if text.contains('\0') {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            report.source_line(&path, i + 1, line);
        }
    }
}

fn scan_history(git: &Git, count: usize, spec: &[String], report: &mut Report) {
    let n = count.to_string();
    let log = git.bytes(&with_spec(
        &["log", "-z", "-n", &n, "--format=%h%n%B"],
        spec,
    ));
    for commit in split_z(&log) {
        let (sha, body) = commit.split_once('\n').unwrap_or((&commit, ""));
        report.message(sha, body);
    }
}

/// Untrack every file on the ignore list and take the ignore-list lines out
/// of every tracked .gitignore, staging both. Source is never edited.
fn fix(git: &Git, top: &Path, spec: &[String]) {
    let top_git = Git::at(top);
    let names = split_z(&git.bytes(&with_spec(&["ls-files", "-z", "--full-name"], spec)));
    let ignored: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|p| ignored_path(p))
        .collect();
    if !ignored.is_empty() {
        let mut args = vec!["rm", "--cached", "-q", "--"];
        args.extend(&ignored);
        if top_git.quiet(&args) {
            for p in &ignored {
                warn(format!("hygiene: untracked {p}"));
            }
        } else {
            warn("hygiene: git rm --cached failed; nothing was untracked");
        }
    }
    for path in names.iter().filter(|p| is_gitignore(p) && !ignored_path(p)) {
        let file = top.join(path);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let kept: Vec<&str> = text.lines().filter(|l| !agent_ignore_line(l)).collect();
        if kept.len() == text.lines().count() {
            continue;
        }
        let mut out = kept.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        if std::fs::write(&file, out).is_ok() && top_git.quiet(&["add", "--", path]) {
            warn(format!("hygiene: took the agent lines out of {path}"));
        }
    }
}

/// What to read. No mode at all is the sweep: the tree and recent history.
pub struct Scope<'a> {
    pub staged: bool,
    pub tree: bool,
    pub history: Option<usize>,
    pub message: Option<&'a Path>,
    pub string: Option<&'a str>,
}

/// A path about to be written, judged in the work tree of its nearest
/// existing directory. Only a new file is refused: a repository not yet
/// cleaned still has tracked instruction files its agents must edit.
pub fn cmd_would_create(path: &Path, json: bool) -> i32 {
    let mut report = Report::default();
    if let Some(rel) = new_instruction_file(path) {
        report.add(&rel, 0, "hard", "agent file", "this belongs in mem");
    }
    report.print(json);
    if report.hard() {
        exit::FAILED
    } else {
        exit::OK
    }
}

/// The path relative to its work tree's top, when it is a new instruction
/// file in a checkout mem knows.
fn new_instruction_file(path: &Path) -> Option<String> {
    if std::fs::symlink_metadata(path).is_ok() {
        return None;
    }
    let path = std::path::absolute(path).ok()?;
    let dir = path.ancestors().skip(1).find(|a| a.is_dir())?;
    let git = Git::at(dir);
    if !git.inside_worktree() {
        return None;
    }
    let prefix = git.capture(&["rev-parse", "--show-prefix"]).text();
    let rest = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy();
    let rel = format!("{prefix}{rest}");
    // mem resolves a project from its working directory, so it is asked from
    // the tree the path lands in; it goes last, being the slowest question.
    let refused = instruction_path(&rel)
        && std::env::set_current_dir(dir).is_ok()
        && memcli::knows_this_checkout();
    refused.then_some(rel)
}

pub fn cmd_hygiene(scope: Scope, path: Option<&Path>, json: bool, fix_it: bool) -> i32 {
    let mut report = Report::default();
    if let Some(file) = scope.message {
        let Ok(raw) = std::fs::read_to_string(file) else {
            warn(format!("hygiene: cannot read {}", file.display()));
            return exit::USAGE;
        };
        report.message(
            &file.display().to_string(),
            &lint::strip_git_commentary(&raw),
        );
    } else if let Some(text) = scope.string {
        report.message("string", text);
    } else {
        let git = Git::here();
        let Some(top) = git.toplevel() else {
            warn("hygiene: not inside a git repository");
            return exit::USAGE;
        };
        let spec: Vec<String> = path.map(|p| p.display().to_string()).into_iter().collect();
        let globs = memcli::project_hygiene_exempt();
        if fix_it {
            fix(&git, &top, &spec);
        }
        let sweep = !scope.staged && !scope.tree && scope.history.is_none();
        if scope.staged {
            scan_staged(&git, &spec, &globs, &mut report);
        }
        if scope.tree || sweep {
            scan_tree(&git, &top, &spec, &globs, &mut report);
        }
        if let Some(n) = scope.history.or(sweep.then_some(SWEEP_COMMITS)) {
            scan_history(&git, n, &spec, &mut report);
        }
    }
    report.print(json);
    if report.hard() {
        exit::FAILED
    } else {
        exit::OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard_labels(line: &str) -> Vec<&'static str> {
        HARD.iter()
            .filter(|r| r.hits(line))
            .map(Rule::label)
            .collect()
    }

    // The numbered shapes are built at run time so this file stays clean
    // under its own check.
    #[test]
    fn each_hard_rule_names_its_shape() {
        for (line, label) in [
            (
                format!("cached because Ruling {} says so", 4),
                "numbered ruling",
            ),
            (format!("shipped in milestone {}", 2), "numbered milestone"),
            (format!("see (issue {})", 381), "issue number"),
            (format!("ticket {} asked for it", 7), "numbered ticket"),
            (format!("ADR {} covers this", "0012"), "numbered ADR"),
            (format!("see #{}", "QZ7RKWVM"), "memory id"),
            (format!("{}-By: x", "Co-Authored"), "co-author trailer"),
            (
                format!("{} with a tool", "Generated"),
                "generated-with line",
            ),
            (
                format!("https://{}/code/x", "claude.ai"),
                "Claude Code link",
            ),
            ("done \u{1f916}".to_string(), "robot emoji"),
        ] {
            assert_eq!(hard_labels(&line), vec![label], "for {line:?}");
        }
    }

    #[test]
    fn ordinary_lines_pass_the_hard_tier() {
        for line in [
            "the overruling of the default",
            "milestones are listed in order",
            "an ADR template",
            "issue the refund",
            "color: #AABBCCDD;",
            "color: #ABCDEF12;",
            "a fixture id #ABCDEFGH",
            "a longer run #ABCDEFGH1",
        ] {
            assert!(hard_labels(line).is_empty(), "should pass: {line:?}");
        }
    }

    #[test]
    fn task_and_milestone_ids_only_standing_alone() {
        assert!(task_id("Finish t3 first"));
        assert!(task_id("t12."));
        assert!(!task_id("Rename the t3a column"));
        assert!(!task_id("at3 is a word"));
        assert!(milestone_id("Close m4-ui"));
        assert!(!milestone_id("the m4 board"));
        assert!(!milestone_id("rm4-x"));
    }

    #[test]
    fn the_ignore_list_matches_at_any_depth() {
        for p in [
            "CLAUDE.md",
            "app/AGENTS.md",
            ".claude/settings.json",
            "a/b/agent-memory/x.md",
            "opencode.jsonc",
        ] {
            assert!(ignored_path(p), "{p}");
        }
        for p in ["README.md", "src/claude.rs", "docs/CLAUDE.md.txt"] {
            assert!(!ignored_path(p), "{p}");
        }
        for l in [
            ".claude",
            "/.claude/",
            "**/agent-memory/",
            "opencode.json*",
            ".claude/worktrees/",
        ] {
            assert!(agent_ignore_line(l), "{l}");
        }
        for l in ["node_modules", "# .claude", "", "target/"] {
            assert!(!agent_ignore_line(l), "{l}");
        }
    }

    #[test]
    fn instruction_paths_are_the_files_an_agent_reads() {
        for p in [
            "CLAUDE.md",
            "app/CLAUDE.local.md",
            "a/b/AGENTS.md",
            ".mcp.json",
            ".agents/x.md",
            "app/.cursor/rules/x.mdc",
            ".scratch/notes.txt",
            ".claude/skills/x/SKILL.md",
            ".claude/agents/x.md",
            "app/.claude/commands/x.md",
            ".claude/rules/x.md",
        ] {
            assert!(instruction_path(p), "{p}");
        }
        for p in [
            "src/main.rs",
            "README.md",
            "docs/CLAUDE.md.txt",
            ".claude/settings.json",
            ".amx/x/CLAUDE.md",
            ".playwright-cli/AGENTS.md",
            "a/agent-memory/x/CLAUDE.md",
            "agent-memory-local/.agents/x.md",
            ".claude/worktrees/w/CLAUDE.md",
            ".claude/worktrees/w/.claude/skills/x/SKILL.md",
        ] {
            assert!(!instruction_path(p), "{p}");
        }
    }

    #[test]
    fn added_lines_carry_their_new_line_numbers() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -2,0 +3,2 @@ fn x()
+one
+++two
@@ -9 +10 @@
-old
+three
diff --git a/b.txt b/b.txt
new file mode 100644
--- /dev/null
+++ b/b.txt
@@ -0,0 +1 @@
+four
";
        let got: Vec<(String, usize, String)> = added_lines(diff);
        let want = [
            ("src/a.rs", 3, "one"),
            ("src/a.rs", 4, "++two"),
            ("src/a.rs", 10, "three"),
            ("b.txt", 1, "four"),
        ];
        assert_eq!(got.len(), want.len());
        for ((p, n, t), (wp, wn, wt)) in got.iter().zip(want) {
            assert_eq!((p.as_str(), *n, t.as_str()), (wp, wn, wt));
        }
    }

    #[test]
    fn a_pattern_covers_the_paths_its_pathspec_would() {
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
}
