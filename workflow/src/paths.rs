//! Where everything lives (spec §3), and the two path questions the gate keeps
//! asking: what is this really, and is it under the orchestrator's roots.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

fn env_dir(key: &str) -> Option<PathBuf> {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => None,
    }
}

pub fn home() -> PathBuf {
    env_dir("HOME").unwrap_or_else(|| PathBuf::from("/"))
}

pub fn state_home() -> PathBuf {
    env_dir("XDG_STATE_HOME").unwrap_or_else(|| home().join(".local/state"))
}

pub fn cache_home() -> PathBuf {
    env_dir("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache"))
}

pub fn worktrees_root() -> PathBuf {
    state_home().join("workflow/worktrees")
}

pub fn runs_root() -> PathBuf {
    state_home().join("workflow/runs")
}

pub fn green_root() -> PathBuf {
    state_home().join("workflow/green")
}

pub fn suite_lock_root() -> PathBuf {
    state_home().join("workflow/suite-lock")
}

pub fn briefs_root() -> PathBuf {
    cache_home().join("workflow/briefs")
}

pub fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// `.` and `..` resolved without touching the filesystem.
fn absolutize(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd().join(p)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `realpath`: the real thing, or nothing when part of it is missing.
pub fn realpath(p: impl AsRef<Path>) -> Option<PathBuf> {
    std::fs::canonicalize(p.as_ref()).ok()
}

/// `realpath -m`: as much of it as exists, resolved; the rest appended.
pub fn realpath_m(p: impl AsRef<Path>) -> PathBuf {
    let abs = absolutize(p.as_ref());
    let mut rest: Vec<OsString> = Vec::new();
    let mut cur = abs.clone();
    loop {
        if let Ok(c) = std::fs::canonicalize(&cur) {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match cur.file_name() {
            Some(n) => {
                rest.push(n.to_os_string());
                if !cur.pop() {
                    return abs;
                }
            }
            None => return abs,
        }
    }
}

/// Is this checkout one the orchestrator made? Both sides are resolved, so a
/// symlinked state directory does not change the answer.
pub fn under_worktrees_root(dir: impl AsRef<Path>) -> bool {
    let root = realpath_m(worktrees_root());
    match realpath(dir) {
        Some(d) => d.starts_with(&root),
        None => false,
    }
}

/// Print-mode workers write `~/.claude/projects/<slug>/<session>.jsonl`, the
/// slug being the worker's cwd with every non-alphanumeric turned into a dash.
pub fn path_slug(p: impl AsRef<Path>) -> String {
    p.as_ref()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn transcript_path(worktree: impl AsRef<Path>, session: &str) -> PathBuf {
    home()
        .join(".claude/projects")
        .join(path_slug(worktree))
        .join(format!("{session}.jsonl"))
}

/// The checkout this binary was built in, when there is one: the directory
/// above `hooks/pre-commit`. An install that copies the binary somewhere else
/// says so with `WORKFLOW_HOME`.
pub fn wf_home() -> Option<PathBuf> {
    if let Some(h) = env_dir("WORKFLOW_HOME") {
        return Some(realpath_m(h));
    }
    let exe = std::env::current_exe().ok()?;
    let exe = realpath(&exe).unwrap_or(exe);
    let mut dir = exe.parent()?;
    loop {
        if dir.join("hooks/pre-commit").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_is_the_path_with_every_other_character_dashed() {
        assert_eq!(
            path_slug("/home/me/Sites/my project"),
            "-home-me-Sites-my-project"
        );
    }

    #[test]
    fn dots_resolve_without_the_filesystem() {
        assert_eq!(
            absolutize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }
}
