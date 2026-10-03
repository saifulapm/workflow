//! `workflow doctor` -- what this machine's wiring actually says (spec §7, §11).
//! Plain `doctor` only reports; `--fix` writes the embedded hook stubs, amx
//! roles, skills and the `workflow.service` unit, the one edit this command
//! makes. The ignore list and the
//! settings belong to the dotfiles, so for those `--fix` prints the edit.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::gitcmd::{self, Git};
use crate::{exit, have, paths};

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

/// The lines the global ignore list must hold so no repo on this machine
/// tracks the files a coding harness leaves behind.
const IGNORED: [&str; 5] = [".claude/", "CLAUDE.md", "AGENTS.md", ".scratch/", ".amx/"];

/// The env keys that switch off the traffic the advisor tool depends on.
const TRAFFIC_OFF: [&str; 3] = [
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "DISABLE_TELEMETRY",
    "DO_NOT_TRACK",
];

/// Under `--fix`, the change to make by hand. Both files belong to the
/// dotfiles, which would undo any edit made here on their next apply.
fn edit(r: &Report, fix: bool, msg: impl AsRef<str>) {
    if fix {
        r.note("dotfiles edit", msg);
    }
}

fn ignore_list(r: &mut Report, fix: bool) {
    let Some(file) = Git::here().out(&[
        "config",
        "--global",
        "--type=path",
        "--get",
        "core.excludesFile",
    ]) else {
        r.finding(
            "ignore list",
            "no global core.excludesFile: nothing keeps harness files out of every repo",
        );
        edit(
            r,
            fix,
            format!(
                "set core.excludesFile to a file listing {}",
                IGNORED.join(" ")
            ),
        );
        return;
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        r.finding("ignore list", format!("{file} is not there to read"));
        edit(
            r,
            fix,
            format!("create {file} listing {}", IGNORED.join(" ")),
        );
        return;
    };
    for want in IGNORED {
        if !text.lines().any(|l| l.trim() == want) {
            r.finding("ignore list", format!("{file} lacks {want}"));
            edit(r, fix, format!("add {want} to {file}"));
        }
    }
}

/// Claude Code's user settings file, where the attribution and advisor keys
/// live.
fn default_file() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => paths::home().join(".claude"),
    }
    .join("settings.json")
}

fn settings_keys(r: &mut Report, fix: bool) {
    let f = default_file();
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

    // The edit goes where the content lives, which for a linked file is the
    // link's target in the dotfiles.
    let target = if f.is_symlink() {
        std::fs::canonicalize(&f).unwrap_or(f)
    } else {
        f
    };
    let target = target.display().to_string();
    if v.get("advisorModel")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        r.finding(
            "settings advisor",
            "advisorModel is not set; the advisor is off",
        );
        edit(r, fix, format!("set advisorModel in {target}"));
    }
    let env = v.get("env");
    if env
        .and_then(|e| e.get("CLAUDE_CODE_SUBAGENT_MODEL"))
        .and_then(Value::as_str)
        != Some("sonnet")
    {
        r.finding(
            "settings env",
            "env.CLAUDE_CODE_SUBAGENT_MODEL is not \"sonnet\"",
        );
        edit(
            r,
            fix,
            format!("set env.CLAUDE_CODE_SUBAGENT_MODEL to \"sonnet\" in {target}"),
        );
    }
    for key in TRAFFIC_OFF {
        if env.and_then(|e| e.get(key)).is_some() {
            r.finding(
                "settings env",
                format!("env.{key} is set; the advisor needs it unset"),
            );
            edit(r, fix, format!("remove env.{key} from {target}"));
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
    // A run dispatches through whichever `amx` PATH answers first. Two on PATH
    // means the flags the run uses may belong to the other one, and a refusal
    // that reads like a workflow bug.
    let amx = every_on_path("amx");
    if amx.len() > 1 {
        let rest: Vec<String> = amx[1..].iter().map(|p| p.display().to_string()).collect();
        r.finding(
            "PATH",
            format!(
                "{} amx on PATH: {} answers, {} shadowed -- keep one",
                amx.len(),
                amx[0].display(),
                rest.join(", ")
            ),
        );
    }
}

/// Every executable of that name on PATH, first match first.
fn every_on_path(program: &str) -> Vec<PathBuf> {
    let Ok(path) = std::env::var("PATH") else {
        return Vec::new();
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .filter(|p| gitcmd::exists_x(p))
        .collect()
}

/// mem's own skill, installed beside the ones [`crate::skill::SKILLS`] holds
/// so a harness finds all of them in one place. mem serves the same text as
/// `mem skill mem`.
const MEM_SKILL: (&str, &str) = ("mem", include_str!("../../skills/mem/SKILL.md"));

/// The three git hook stubs, embedded the same way.
const STUBS: [(&str, &str); 3] = [
    ("pre-commit", include_str!("../../hooks/pre-commit")),
    ("commit-msg", include_str!("../../hooks/commit-msg")),
    ("pre-push", include_str!("../../hooks/pre-push")),
];

/// The amx roles a session starts on (`amx new --role <name>`), one per kind
/// of agent. A role's brief goes in front of the task amx
/// hands the agent, so the rules for how to work -- narrow reads, files
/// written in steps, an output cap -- live here and not in every brief. A
/// project overrides one whole with `.amx/agents/<name>.md`.
pub const ROLES: [(&str, &str); 7] = [
    ("worker", include_str!("../../roles/worker.md")),
    ("lead", include_str!("../../roles/lead.md")),
    ("dogfood", include_str!("../../roles/dogfood.md")),
    ("research", include_str!("../../roles/research.md")),
    ("plan", include_str!("../../roles/plan.md")),
    ("plan-refresh", include_str!("../../roles/plan-refresh.md")),
    ("grill", include_str!("../../roles/grill.md")),
];

/// The user unit that keeps `workflow serve` running. `ConditionPathExists`
/// keeps a machine that never installed the binary from a restart loop, and
/// PATH is spelled out because a user unit does not inherit the shell's.
const WORKFLOW_UNIT: &str = "\
[Unit]
Description=workflow serve
ConditionPathExists=%h/.local/bin/workflow

[Service]
ExecStart=%h/.local/bin/workflow serve
Environment=PATH=%h/.local/bin:%h/.cargo/bin:/usr/local/bin:/usr/bin
Restart=always
RestartSec=5
TimeoutStopSec=60

[Install]
WantedBy=default.target
";

/// The two directories `--fix` installs the skills into: Claude Code's own,
/// and the one pi, codex and opencode all read.
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

/// The hook stubs, roles and skills, against the copy this binary carries.
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
    // A harness lists the skills it finds on disk, so each is a file in both
    // directories, verbatim: the frontmatter is where the listing reads the
    // name and description from.
    for (name, text) in crate::skill::SKILLS.iter().chain([&MEM_SKILL]) {
        for (dest, dir) in skill_dirs() {
            let path = dir.join(name).join("SKILL.md");
            check_or_write(r, &format!("skill {name} ({dest})"), &path, text, fix, None);
        }
    }
}

/// The `workflow.service` unit. With the dotfiles' unit directory present the
/// text lives there and the config home holds a link to it, as chezmoi lays
/// out every other unit; without it the config home holds the file itself.
fn unit(r: &mut Report, fix: bool) {
    const LABEL: &str = "unit workflow.service";
    let path = paths::config_home().join("systemd/user/workflow.service");
    let units = paths::dotfiles_units();
    if units.is_dir() {
        let source = units.join("workflow.service");
        let wrote = fix && compare(&source, WORKFLOW_UNIT, None).is_some();
        check_or_write(r, LABEL, &source, WORKFLOW_UNIT, fix, None);
        let linked = path.is_symlink() && paths::realpath(&path) == paths::realpath(&source);
        if !linked && !fix {
            r.finding(
                LABEL,
                format!("{} is not a link to {}", path.display(), source.display()),
            );
        } else if !linked {
            match link(&path, &source) {
                Ok(()) => r.note(
                    LABEL,
                    format!("linked {} to {}", path.display(), source.display()),
                ),
                Err(_) => r.finding(LABEL, format!("could not link {}", path.display())),
            }
        }
        if wrote {
            r.note(LABEL, "commit the dotfiles");
        }
    } else {
        check_or_write(r, LABEL, &path, WORKFLOW_UNIT, fix, None);
    }

    // Enabling starts a service on this machine, which is a person's call:
    // doctor names the command and never runs it.
    if !have("systemctl") {
        return;
    }
    let state = std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "workflow.service"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if state != "enabled" {
        r.finding(
            LABEL,
            format!("systemctl --user is-enabled says \"{state}\": run systemctl --user enable --now workflow.service"),
        );
    }
}

/// Point `path` at `target`, replacing whatever stood at `path`.
fn link(path: &Path, target: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.is_symlink() || path.exists() {
        std::fs::remove_file(path)?;
    }
    std::os::unix::fs::symlink(target, path)
}

pub fn cmd_doctor(fix: bool) -> i32 {
    println!("workflow doctor");
    let mut r = Report::default();
    tools(&mut r);
    hooks(&mut r);
    ignore_list(&mut r, fix);
    settings_keys(&mut r, fix);
    install(&mut r, fix);
    unit(&mut r, fix);

    if r.findings == 0 {
        println!("healthy: nothing to fix");
        return exit::OK;
    }
    println!("{} finding(s)", r.findings);
    exit::FAILED
}
