//! Talking to git. Every question the gate asks goes through here so the
//! environment rule of spec §7 is kept in one place: git is queried with the
//! environment we inherited (a partial commit's index lives in it), and only
//! the child test suite is scrubbed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Default)]
pub struct Git {
    cwd: Option<PathBuf>,
    /// Overrides applied to every invocation: `GIT_INDEX_FILE` and `GIT_DIR`
    /// made absolute before a chdir, and nothing else.
    env: Vec<(String, String)>,
}

pub struct Out {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Out {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout)
            .trim_end_matches('\n')
            .to_string()
    }
}

impl Git {
    /// git in the working directory, with the environment we were handed.
    pub fn here() -> Git {
        Git::default()
    }

    pub fn at(dir: impl Into<PathBuf>) -> Git {
        Git {
            cwd: Some(dir.into()),
            env: Vec::new(),
        }
    }

    pub fn with_env(mut self, key: &str, value: impl Into<String>) -> Git {
        self.env.push((key.to_string(), value.into()));
        self
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new("git");
        if let Some(dir) = &self.cwd {
            c.current_dir(dir);
        }
        for (k, v) in &self.env {
            c.env(k, v);
        }
        c.args(args);
        c
    }

    /// stdout and stderr captured, exit status reported.
    pub fn capture(&self, args: &[&str]) -> Out {
        match self.command(args).output() {
            Ok(o) => Out {
                ok: o.status.success(),
                stdout: o.stdout,
                stderr: o.stderr,
            },
            Err(_) => Out {
                ok: false,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        }
    }

    /// The trimmed stdout of a command that succeeded, or nothing.
    pub fn out(&self, args: &[&str]) -> Option<String> {
        let o = self.capture(args);
        if !o.ok {
            return None;
        }
        let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    }

    /// Raw stdout of a command that succeeded: `-z` output is not text.
    pub fn bytes(&self, args: &[&str]) -> Vec<u8> {
        let o = self.capture(args);
        if o.ok { o.stdout } else { Vec::new() }
    }

    /// Did it work? Output goes nowhere.
    pub fn quiet(&self, args: &[&str]) -> bool {
        self.command(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Did it work? Output goes where ours goes.
    pub fn loud(&self, args: &[&str]) -> bool {
        self.command(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn inside_worktree(&self) -> bool {
        self.out(&["rev-parse", "--is-inside-work-tree"]).as_deref() == Some("true")
    }

    pub fn toplevel(&self) -> Option<PathBuf> {
        self.out(&["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
    }

    /// The repository's real hook directory. Unlike `rev-parse --git-path
    /// hooks/x` it does not follow `core.hooksPath` back to the stub itself.
    pub fn common_dir(&self) -> Option<PathBuf> {
        self.out(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .map(PathBuf::from)
    }

    pub fn head(&self) -> Option<String> {
        self.out(&["rev-parse", "HEAD"])
    }

    pub fn rev_parse_commit(&self, rev: &str) -> Option<String> {
        self.out(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.quiet(&["merge-base", "--is-ancestor", a, b])
    }

    pub fn count(&self, range: &str) -> u64 {
        self.out(&["rev-list", "--count", range])
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

/// Split `-z` output into its NUL-separated fields, empties dropped.
pub fn nul_fields(bytes: &[u8]) -> Vec<Vec<u8>> {
    bytes
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| f.to_vec())
        .collect()
}

/// A pathspec that means what the author wrote: `*` stops at `/`, `**` crosses,
/// and the pattern is anchored at the repo root wherever git is standing
/// (spec §5.3).
pub fn glob_top(pattern: &str) -> String {
    format!(":(glob,top){pattern}")
}

/// The same, case-insensitively: the review table's rows (spec §9, review-4
/// B-1). Ownership deliberately does not use this.
pub fn glob_icase_top(pattern: &str) -> String {
    format!(":(glob,icase,top){pattern}")
}

/// A path git printed, as a string we can compare and show.
pub fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

pub fn exists_x(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
