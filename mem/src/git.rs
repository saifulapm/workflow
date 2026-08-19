//! The only thing mem asks git: where am I, and what is this checkout called
//! (spec §5). Nothing is ever written to the repository.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Worktree-invariant identity of a repository: every linked worktree of the
/// same repo reports the same common dir, so they are one project.
pub fn common_dir(cwd: &Path) -> Option<PathBuf> {
    let raw = git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Some(canonical(Path::new(&raw)))
}

pub fn toplevel(cwd: &Path) -> Option<PathBuf> {
    let raw = git(cwd, &["rev-parse", "--show-toplevel"])?;
    Some(canonical(Path::new(&raw)))
}

pub fn origin_url(cwd: &Path) -> Option<String> {
    git(cwd, &["config", "--get", "remote.origin.url"])
}

/// Symlinks resolved where possible so two spellings of one path compare equal.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Strip scheme, user and `.git`, lowercase the host: `git@github.com:Me/Repo.git`
/// and `https://github.com/me/repo` are the same remote.
pub fn normalize_remote(url: &str) -> String {
    let mut s = url.trim();
    for scheme in [
        "ssh://",
        "git+ssh://",
        "https://",
        "http://",
        "git://",
        "ftp://",
    ] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
            break;
        }
    }
    // user@host:path or user@host/path
    let s = match s.split_once('@') {
        Some((user, rest)) if !user.contains('/') => rest,
        _ => s,
    };
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.trim_end_matches('/');
    // scp-style `host:path` and url-style `host/path` both become `host/path`.
    let Some(cut) = s.find([':', '/']) else {
        return s.to_ascii_lowercase();
    };
    let (host, rest) = (&s[..cut], &s[cut + 1..]);
    // `host:22/me/repo` — a port is transport, not identity. `host:me/repo` is
    // scp syntax and `me` is part of the path, so only digits count as a port.
    let rest = match (s.as_bytes()[cut], rest.split_once('/')) {
        (b':', Some((port, tail)))
            if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            tail
        }
        _ => rest,
    };
    format!(
        "{}/{}",
        host.to_ascii_lowercase(),
        rest.trim_start_matches('/')
    )
}

/// What the working directory says about the project, before the store or the
/// machine-local path map get a say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub common_dir: PathBuf,
    pub toplevel: PathBuf,
    pub remote: Option<String>,
}

impl Checkout {
    pub fn detect(cwd: &Path) -> Option<Checkout> {
        let common_dir = common_dir(cwd)?;
        let toplevel = toplevel(cwd).unwrap_or_else(|| common_dir.clone());
        Some(Checkout {
            remote: origin_url(cwd).map(|u| normalize_remote(&u)),
            common_dir,
            toplevel,
        })
    }

    /// The project name a fresh registration would take: the basename of the
    /// git toplevel directory.
    pub fn default_name(&self) -> String {
        self.toplevel
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_normalize_to_one_spelling() {
        let expected = "github.com/me/repo";
        for url in [
            "git@github.com:me/repo.git",
            "git@github.com:me/repo",
            "https://github.com/me/repo.git",
            "https://user@github.com/me/repo",
            "ssh://git@github.com/me/repo.git",
            "git://github.com/me/repo.git",
            "https://GitHub.com/me/repo/",
            "ssh://git@github.com:22/me/repo.git",
        ] {
            assert_eq!(normalize_remote(url), expected, "{url}");
        }
    }

    #[test]
    fn paths_and_odd_remotes_survive_normalization() {
        assert_eq!(normalize_remote("/srv/git/repo.git"), "/srv/git/repo");
        assert_eq!(normalize_remote("repo"), "repo");
        assert_eq!(normalize_remote(""), "");
    }
}
