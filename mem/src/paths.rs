//! Where everything lives (spec §3). XDG variables are honoured so a test — or
//! a second store on the same machine — can redirect the whole layout without
//! any mem-specific configuration.

use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Dirs {
    pub data: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
    pub config: PathBuf,
}

fn env_dir(var: &str, home: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(home).join(fallback),
    }
}

impl Dirs {
    pub fn from_env() -> Result<Dirs> {
        let home = std::env::var("HOME").context("HOME is not set")?;
        Ok(Dirs {
            data: env_dir("XDG_DATA_HOME", &home, ".local/share"),
            cache: env_dir("XDG_CACHE_HOME", &home, ".cache"),
            state: env_dir("XDG_STATE_HOME", &home, ".local/state"),
            config: env_dir("XDG_CONFIG_HOME", &home, ".config"),
        })
    }

    /// The only synced tree.
    pub fn store(&self) -> PathBuf {
        self.data.join("mem/store")
    }

    /// Disposable index; never synced.
    pub fn index_db(&self) -> PathBuf {
        self.cache.join("mem/index.db")
    }

    pub fn mem_state(&self) -> PathBuf {
        self.state.join("mem")
    }

    pub fn paths_toml(&self) -> PathBuf {
        self.mem_state().join("paths.toml")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.mem_state().join("sessions")
    }

    pub fn outbox_dir(&self) -> PathBuf {
        self.mem_state().join("outbox")
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.mem_state().join("snapshots")
    }

    /// Where `workflow run` puts a task's worktree:
    /// `<here>/<project>/<plan>/<task>`. A question asked from under it is a
    /// worker's, and the orchestrator's to answer.
    pub fn workflow_worktrees(&self) -> PathBuf {
        self.state.join("workflow/worktrees")
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("mem/config.toml")
    }

    /// qshell's machine name file, and the sync status the panel and mem share.
    pub fn qshell_machine(&self) -> PathBuf {
        self.config.join("qshell/machine")
    }

    pub fn qshell_status_json(&self) -> PathBuf {
        self.state.join("qshell/sync/status.json")
    }
}

/// `~/.config/qshell/machine` else `uname -n` (spec §7).
pub fn machine_name(dirs: &Dirs) -> String {
    if let Ok(text) = std::fs::read_to_string(dirs.qshell_machine()) {
        let name = text.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Ok(out) = std::process::Command::new("uname").arg("-n").output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    "unknown".to_string()
}
