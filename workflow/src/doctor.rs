//! `workflow doctor` -- what this machine's wiring actually says (spec §7, §11).
//! Plain `doctor` only reports; `--fix` writes the embedded hook stubs and
//! amx roles, the one edit this command makes.

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

/// The eight skills `doctor --fix` used to write into every harness's skills
/// directory, kept so it can recognise its own handiwork and take it back out.
///
/// This is a retirement manifest, not ownership: the seven workflow serves live
/// in [`crate::skill::SKILLS`] and mem serves its own. Text is what tells a copy
/// this binary wrote from one somebody edited by hand, and only the first is
/// safe to delete. Once the machines are clean this can go.
const RETIRED: [(&str, &str); 8] = [
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

/// The four amx roles a run dispatches on (`amx new --role <name>`), one per
/// kind of agent it starts. A role's brief goes in front of the task amx
/// hands the agent, so the rules for how to work -- narrow reads, files
/// written in steps, an output cap -- live here and not in every brief. A
/// project overrides one whole with `.amx/agents/<name>.md`.
pub const ROLES: [(&str, &str); 4] = [
    ("worker", include_str!("../../roles/worker.md")),
    ("reader", include_str!("../../roles/reader.md")),
    ("fixer", include_str!("../../roles/fixer.md")),
    ("advisor", include_str!("../../roles/advisor.md")),
];

/// The two directories `--fix` used to install into, and [`retire`] now empties:
/// Claude Code's own, and the one pi, codex and opencode all read.
fn skill_dirs() -> [(&'static str, PathBuf); 2] {
    [
        ("claude", paths::home().join(".claude/skills")),
        ("agents", paths::agents_skills()),
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

/// The three hook stubs, against the copy this binary carries.
///
/// The skills used to be installed here too, into both harness directories. A
/// skill is served now -- `workflow skill <name>`, `mem skill mem` -- so there
/// is no copy on disk to keep in step, and nothing a harness auto-discovers to
/// gate. [`retire`] takes the old copies back out.
fn install(r: &mut Report, fix: bool) {
    let hooks = hooks_dir();
    for (name, text) in STUBS {
        let path = hooks.join(name);
        check_or_write(r, &format!("hook {name}"), &path, text, fix, Some(0o755));
    }
    // The roles go where amx reads a person's: a dispatch names one on every
    // `amx new`, and a name amx does not know is a launch refused.
    let roles = paths::amx_roles();
    for (name, text) in ROLES {
        let path = roles.join(format!("{name}.md"));
        check_or_write(r, &format!("role {name}"), &path, text, fix, None);
    }
}

/// Take the installed skills back off disk, one name directory at a time.
///
/// Only what this binary put there: a leaf holding exactly the text in
/// [`RETIRED`] is ours to remove, and so is a symlink at our own path, which
/// costs the link and never its target. Anything else is a hand edit, and
/// deleting one silently is data loss -- it is reported and left, which is also
/// what `doctor` without `--fix` does with every copy it finds.
///
/// A machine with nothing left here says nothing at all.
fn retire(r: &mut Report, fix: bool) {
    for (name, text) in RETIRED {
        for (dest, dir) in skill_dirs() {
            let leaf = dir.join(name).join("SKILL.md");
            let linked = leaf.is_symlink() || dir.join(name).is_symlink();
            if !linked && !leaf.exists() {
                continue;
            }
            let label = format!("skill {name} ({dest})");
            let ours = linked || std::fs::read_to_string(&leaf).is_ok_and(|t| t == text);
            if !ours {
                r.finding(
                    &label,
                    format!(
                        "{} is not the copy this binary wrote -- move it aside, or keep it",
                        leaf.display()
                    ),
                );
                continue;
            }
            if !fix {
                r.finding(
                    &label,
                    format!("{} is installed; skills are served now", leaf.display()),
                );
                continue;
            }
            let target = dir.join(name);
            let removed = if target.is_symlink() {
                std::fs::remove_file(&target)
            } else {
                std::fs::remove_dir_all(&target)
            };
            match removed {
                Ok(()) => r.note(&label, format!("retired {}", target.display())),
                Err(_) => r.finding(&label, format!("could not remove {}", target.display())),
            }
        }
    }
}

pub fn cmd_doctor(fix: bool) -> i32 {
    println!("workflow doctor");
    let mut r = Report::default();
    tools(&mut r);
    hooks(&mut r);
    settings_keys(&mut r);
    install(&mut r, fix);
    retire(&mut r, fix);

    if r.findings == 0 {
        println!("healthy: nothing to fix");
        return exit::OK;
    }
    println!("{} finding(s)", r.findings);
    exit::FAILED
}
