//! Talking to git. Every question the hooks ask goes through here so the
//! environment rule is kept in one place: git is queried with the environment
//! we inherited, because a partial commit's index lives in it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Default)]
pub struct Git {
    cwd: Option<PathBuf>,
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
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new("git");
        if let Some(dir) = &self.cwd {
            c.current_dir(dir);
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
}

pub fn exists_x(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
