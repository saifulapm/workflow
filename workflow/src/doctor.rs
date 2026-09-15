//! `workflow doctor` -- what this machine's wiring actually says (spec §7, §11).
//! Plain `doctor` only reports; `--fix` writes the embedded skills and hook
//! stubs, the one edit this command makes.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::gitcmd::{self, Git};
use crate::{exit, have, paths, settings};

#[derive(Default)]
struct Report {
    findings: usize,
}

impl Report {
    fn finding(&mut self, label: &str, msg: impl AsRef<str>) {
        self.findings += 1;
        println!("  {:<22} {}", label, msg.as_ref());
    }

    fn note(&self, label: &str, msg: impl AsRef<str>) {
        println!("  {:<22} {}", label, msg.as_ref());
    }
}

/// The checkouts on this machine a global hooksPath is supposed to cover, so
/// their local settings can be inspected. Injectable for tests.
fn sites_checkouts() -> Vec<PathBuf> {
    let root = match std::env::var("WORKFLOW_SITES") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => paths::home().join("Sites"),
    };
    let mut found = Vec::new();
    fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        if depth > 4 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.file_name().map(|n| n == ".git").unwrap_or(false) {
                if let Some(parent) = path.parent() {
                    found.push(parent.to_path_buf());
                }
                continue; // pruned: nothing inside a .git is a checkout
            }
            walk(&path, depth + 1, found);
        }
    }
    if root.is_dir() {
        walk(&root, 1, &mut found);
    }
    found.sort();
    found
}

/// "<name> <frontmatter bytes> <body bytes>" per skill: the skills this
/// binary carries, or the directory `WORKFLOW_SKILLS_DIR` names, which is
/// how the suite hands doctor a skill to measure.
fn skill_sizes() -> Vec<(String, usize, usize)> {
    match std::env::var("WORKFLOW_SKILLS_DIR") {
        Ok(v) if !v.is_empty() => {
            let mut out = Vec::new();
            let Ok(entries) = std::fs::read_dir(PathBuf::from(v)) else {
                return out;
            };
            let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            dirs.sort();
            for d in dirs {
                let Ok(text) = std::fs::read_to_string(d.join("SKILL.md")) else {
                    continue;
                };
                let name = d
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                out.push(sizes_of(name, &text));
            }
            out
        }
        _ => SKILLS
            .iter()
            .map(|(name, text)| sizes_of(name.to_string(), text))
            .collect(),
    }
}

/// The frontmatter and body byte counts of one SKILL.md.
fn sizes_of(name: String, text: &str) -> (String, usize, usize) {
    let (mut fm, mut body, mut in_fm) = (0usize, 0usize, false);
    for (i, line) in text.lines().enumerate() {
        let n = line.len() + 1;
        if i == 0 && line == "---" {
            in_fm = true;
            fm += n;
        } else if in_fm && line == "---" {
            in_fm = false;
            fm += n;
        } else if in_fm {
            fm += n;
        } else {
            body += n;
        }
    }
    (name, fm, body)
}

fn hooks(r: &mut Report) {
    let installed = Git::here()
        .out(&["config", "--global", "--get", "core.hooksPath"])
        .unwrap_or_default();
    if installed.is_empty() {
        r.finding(
            "hooks path",
            "no global core.hooksPath: the gate is not installed on this machine",
        );
    } else if paths::realpath_m(&installed) != paths::realpath_m(hooks_dir()) {
        r.finding(
            "hooks path",
            format!(
                "core.hooksPath={installed} does not resolve to {}: git never reads the stubs doctor writes there",
                hooks_dir().display()
            ),
        );
    } else {
        r.note("hooks path", &installed);
    }

    for repo in sites_checkouts() {
        let local = Git::at(&repo)
            .out(&["config", "--local", "--get", "core.hooksPath"])
            .unwrap_or_default();
        if !local.is_empty() {
            r.finding(
                "shadowed by hooksPath",
                format!(
                    "{} sets core.hooksPath={local} -- a repo-local path beats the global one and the gate is simply not there",
                    repo.display()
                ),
            );
        }
        for name in ["pre-commit", "commit-msg", "pre-push"] {
            let h = repo.join(".git/hooks").join(name);
            if !gitcmd::exists_x(&h) {
                continue;
            }
            let stub = PathBuf::from(&installed).join(name);
            if !installed.is_empty() && paths::realpath(&h) == paths::realpath(&stub) {
                r.finding(
                    "self-chain",
                    format!(
                        "{} is the stub itself: step 5 would exec in a circle",
                        h.display()
                    ),
                );
            } else {
                r.note(
                    "own hook",
                    format!(
                        "{} has its own {name}; the stub chains into it",
                        repo.display()
                    ),
                );
            }
        }
    }
}

/// `true` only for the value the install writes; a number 1 counts as well as
/// the string, because a hand-edited file might hold either.
fn is_one(v: Option<&Value>) -> bool {
    match v {
        Some(Value::String(s)) => s == "1",
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        _ => false,
    }
}

fn settings_keys(r: &mut Report) {
    let f = settings::default_file();
    let Ok(text) = std::fs::read_to_string(&f) else {
        r.finding("settings", format!("{} is not there to read", f.display()));
        return;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        r.finding("settings", format!("{} is not valid json", f.display()));
        return;
    };

    if !is_one(v.get("env").and_then(|e| e.get("WORKFLOW_AGENT"))) {
        r.finding(
            "settings env",
            "env.WORKFLOW_AGENT is not \"1\"; commits from this runtime are ungated outside worktrees",
        );
    }
    let attribution = v.get("attribution");
    for (key, why) in [
        (
            "commitTrailers",
            "attribution.commitTrailers is not false (it defaults to on)",
        ),
        (
            "sessionUrl",
            "attribution.sessionUrl is not false (it defaults to on)",
        ),
    ] {
        if attribution.and_then(|a| a.get(key)) != Some(&Value::Bool(false)) {
            r.finding("settings attribution", why);
        }
    }
    for key in ["commit", "pr"] {
        if let Some(value) = attribution.and_then(|a| a.get(key)) {
            let shown = value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| value.to_string());
            r.note(
                "settings attribution",
                format!("attribution.{key} is set to \"{shown}\" and was kept"),
            );
        }
    }
}

fn budgets(r: &mut Report) {
    const FM_MAX: usize = 240;
    for (name, fm, body) in skill_sizes() {
        // implement, plan, roadmap and orchestrate carry recorded exceptions
        // to the 3,200 byte budget: implement and orchestrate each hold a
        // whole loop, plan the middle-tier keys, roadmap both halves of a
        // project planned whole -- cutting it in one session and running a
        // milestone of it in another.
        let body_max = match name.as_str() {
            "implement" | "plan" | "roadmap" | "orchestrate" => 4800,
            _ => 3200,
        };
        if fm > FM_MAX {
            r.finding(
                &format!("skill {name}"),
                format!("frontmatter is {fm} bytes, over the {FM_MAX} byte budget"),
            );
        }
        if body > body_max {
            r.finding(
                &format!("skill {name}"),
                format!("body is {body} bytes, over the {body_max} byte budget"),
            );
        }
        if fm <= FM_MAX && body <= body_max {
            r.note(
                &format!("skill {name}"),
                format!("frontmatter {fm} B, body {body} B -- within budget"),
            );
        }
    }
}

fn tools(r: &mut Report) {
    // The stubs exec `workflow hook`; without it on PATH they fail open, which
    // is deliberate and means no gate at all (spec §12b).
    if !have("workflow") {
        r.finding(
            "PATH",
            "workflow is not on PATH: the hook stubs skip their checks and only chain",
        );
    }
}

/// The skills this repo ships, embedded so a copied binary carries them
/// wherever it runs (spec ruling 2): `doctor --fix` writes each verbatim to
/// every harness's skills directory rather than relying on a symlink into a
/// dev checkout.
const SKILLS: [(&str, &str); 8] = [
    ("implement", include_str!("../../skills/implement/SKILL.md")),
    ("mem", include_str!("../../skills/mem/SKILL.md")),
    (
        "orchestrate",
        include_str!("../../skills/orchestrate/SKILL.md"),
    ),
    ("plan", include_str!("../../skills/plan/SKILL.md")),
    ("review", include_str!("../../skills/review/SKILL.md")),
    ("roadmap", include_str!("../../skills/roadmap/SKILL.md")),
    ("route", include_str!("../../skills/route/SKILL.md")),
    ("unslop", include_str!("../../skills/unslop/SKILL.md")),
];

/// The three git hook stubs, embedded the same way.
const STUBS: [(&str, &str); 3] = [
    ("pre-commit", include_str!("../../hooks/pre-commit")),
    ("commit-msg", include_str!("../../hooks/commit-msg")),
    ("pre-push", include_str!("../../hooks/pre-push")),
];

/// The directory pi, codex and opencode all read, beside Claude Code's own.
fn skill_dirs() -> [(&'static str, PathBuf); 2] {
    [
        ("claude", paths::home().join(".claude/skills")),
        ("agents", paths::home().join(".agents/skills")),
    ]
}

fn hooks_dir() -> PathBuf {
    paths::home().join(".config/git/hooks")
}

enum Copy {
    Missing,
    Differs,
    Symlink,
}

/// How `path` compares to the embedded `expected` text: `None` for a plain
/// file that already holds exactly that text (and, when `mode` calls for an
/// executable, already has the x bit). Dotfiles link the *name directory*
/// (`~/.claude/skills/<name>`), not the leaf file, so the leaf itself is
/// never the symlink to catch -- `path.parent()` is.
fn compare(path: &Path, expected: &str, mode: Option<u32>) -> Option<Copy> {
    if path.is_symlink() || path.parent().is_some_and(|p| p.is_symlink()) {
        return Some(Copy::Symlink);
    }
    match std::fs::read_to_string(path) {
        Ok(text) if text == expected => {
            let needs_x = mode.is_some_and(|m| m & 0o111 != 0);
            if needs_x && !gitcmd::exists_x(path) {
                Some(Copy::Differs)
            } else {
                None
            }
        }
        Ok(_) => Some(Copy::Differs),
        Err(_) => Some(Copy::Missing),
    }
}

/// Write `expected` to `path` as a plain file: a symlinked name directory or
/// leaf is removed rather than followed, and a differing file is
/// overwritten. `mode` is applied after the write; the hook stubs need 755,
/// a skill needs nothing beyond what the write gave it.
fn write_copy(path: &Path, expected: &str, mode: Option<u32>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if dir.is_symlink() {
            std::fs::remove_file(dir)?;
        }
        std::fs::create_dir_all(dir)?;
    }
    if path.is_symlink() {
        std::fs::remove_file(path)?;
    }
    std::fs::write(path, expected)?;
    if let Some(mode) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Report `path` as missing, differing or a symlink, or -- in `--fix` --
/// write it and note the write. A healthy copy is silent either way.
fn check_or_write(
    r: &mut Report,
    label: &str,
    path: &Path,
    expected: &str,
    fix: bool,
    mode: Option<u32>,
) {
    let Some(status) = compare(path, expected, mode) else {
        return;
    };
    if !fix {
        let word = match status {
            Copy::Missing => "missing",
            Copy::Differs => "differs",
            Copy::Symlink => "symlink",
        };
        r.finding(label, format!("{word} at {}", path.display()));
        return;
    }
    match write_copy(path, expected, mode) {
        Ok(()) => r.note(label, format!("wrote {}", path.display())),
        Err(_) => r.finding(label, format!("could not write {}", path.display())),
    }
}

/// The eight skills into both harness directories, and the three hook stubs,
/// against the copy this binary carries (spec ruling 2, wiki seam 2).
fn install(r: &mut Report, fix: bool) {
    for (name, text) in SKILLS {
        for (dest, dir) in skill_dirs() {
            let path = dir.join(name).join("SKILL.md");
            check_or_write(r, &format!("skill {name} ({dest})"), &path, text, fix, None);
        }
    }
    let hooks = hooks_dir();
    for (name, text) in STUBS {
        let path = hooks.join(name);
        check_or_write(r, &format!("hook {name}"), &path, text, fix, Some(0o755));
    }
}

pub fn cmd_doctor(fix: bool) -> i32 {
    println!("workflow doctor");
    let mut r = Report::default();
    tools(&mut r);
    hooks(&mut r);
    settings_keys(&mut r);
    budgets(&mut r);
    install(&mut r, fix);

    if r.findings == 0 {
        println!("healthy: nothing to fix");
        return exit::OK;
    }
    println!("{} finding(s)", r.findings);
    exit::FAILED
}
