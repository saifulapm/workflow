//! `workflow doctor` -- what this machine's wiring actually says (spec §7, §11).
//! It reports and never edits.

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

/// "<name> <frontmatter bytes> <body bytes>" per skill.
fn skill_sizes() -> Vec<(String, usize, usize)> {
    let dir = match std::env::var("WORKFLOW_SKILLS_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => match paths::wf_home() {
            Some(h) => h.join("skills"),
            None => return Vec::new(),
        },
    };
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        let f = d.join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let name = d
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
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
        out.push((name, fm, body));
    }
    out
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
    } else {
        r.note("hooks path", &installed);
    }

    let home = paths::wf_home();
    for name in ["pre-commit", "commit-msg", "pre-push"] {
        if installed.is_empty() {
            continue;
        }
        let h = PathBuf::from(&installed).join(name);
        if !gitcmd::exists_x(&h) {
            r.finding(
                &format!("hook {name}"),
                format!("missing or not executable at {}", h.display()),
            );
            continue;
        }
        let target = paths::realpath(&h).unwrap_or_else(|| h.clone());
        match &home {
            Some(home) => {
                if target != home.join("hooks").join(name) {
                    r.finding(
                        &format!("hook {name}"),
                        format!(
                            "does not resolve to this checkout's hooks/{name} (it is {})",
                            target.display()
                        ),
                    );
                }
            }
            // No checkout to compare paths against, so judge the slot by what
            // the file does: the stub's whole job is to exec `workflow hook`,
            // and a hook that never does was written by something else --
            // git-lfs installs its own pre-push here (friction #13D9MGCP).
            None => {
                let text = std::fs::read_to_string(&h).unwrap_or_default();
                if !text.contains("workflow hook") {
                    r.finding(
                        &format!("hook {name}"),
                        format!(
                            "{} never invokes `workflow hook`, so the gate is not on this slot -- move its owner into the repos' own hooks (git-lfs: `git lfs update` per repo; the stub chains to a repo's own hook with stdin intact) and symlink the checkout's hooks/{name} here",
                            h.display()
                        ),
                    );
                }
            }
        }
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
        // implement and plan carry recorded exceptions to the 3,200 byte
        // budget: implement holds the whole loop, plan the middle-tier keys.
        let body_max = match name.as_str() {
            "implement" => 4800,
            "plan" => 4000,
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

pub fn cmd_doctor() -> i32 {
    println!("workflow doctor");
    let mut r = Report::default();
    tools(&mut r);
    // Without a checkout the hook-identity and skill-budget checks have
    // nothing to read; a silent pass here reported an ungated machine as
    // healthy (friction #13D9MGCP).
    if paths::wf_home().is_none() {
        r.finding(
            "checkout",
            "no workflow checkout found above this binary -- set WORKFLOW_HOME so hook identity and skill budgets can be checked",
        );
    }
    hooks(&mut r);
    settings_keys(&mut r);
    budgets(&mut r);

    if r.findings == 0 {
        println!("healthy: nothing to fix");
        return exit::OK;
    }
    println!("{} finding(s)", r.findings);
    exit::FAILED
}
