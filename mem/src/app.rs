//! What every verb needs: where the store is, which project this is, and an
//! index that has already caught up with the files.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cli::{Cli, Scope as CliScope};
use crate::index::{Index, Purpose};
use crate::paths::{Dirs, machine_name};
use crate::project::{Identity, Mode, Registry, resolve};
use crate::search::Scope;
use crate::store::Store;

pub struct App {
    pub dirs: Dirs,
    pub store: Store,
    pub cwd: PathBuf,
    pub machine: String,
    pub project: Option<String>,
    /// Whether `project` came from `MEM_PROJECT` rather than `--project`.
    pub project_from_env: bool,
    pub scope: Option<CliScope>,
    pub include_archived: bool,
    pub json: bool,
    pub quiet: bool,
    /// From `context --session-id`; machine-local batch tracking.
    pub session_id: Option<String>,
}

impl App {
    pub fn new(cli: &Cli) -> Result<App> {
        let dirs = Dirs::from_env()?;
        let cwd = std::env::current_dir().context("reading the working directory")?;
        Ok(App {
            store: Store::new(dirs.store()),
            machine: machine_name(&dirs),
            dirs,
            cwd,
            project: cli.project.clone().or_else(project_from_env),
            project_from_env: cli.project.is_none() && project_from_env().is_some(),
            scope: cli.scope,
            include_archived: cli.include_archived,
            json: cli.json,
            quiet: cli.quiet,
            session_id: None,
        })
    }

    pub fn identity(&self, mode: Mode) -> Result<Identity> {
        if self.project_from_env
            && let Some(named) = self.project.as_deref()
            && let Some(here) = self.overruled(named)
        {
            return Ok(here);
        }
        resolve(
            &self.cwd,
            &self.store,
            &self.dirs,
            self.project.as_deref(),
            mode,
        )
    }

    /// The working directory's project, when `MEM_PROJECT` names another the
    /// directory has no part in. The variable is for a process that cannot
    /// choose its directory -- a worker at its worktree's root, which is the
    /// named child's parent -- and a session that inherited it from another
    /// wrote into amx's memory from shortcart's checkout, and replaced a live
    /// run's handoff. A directory in no
    /// project, the named one, its parent or its child keeps the variable.
    fn overruled(&self, named: &str) -> Option<Identity> {
        let here = resolve(&self.cwd, &self.store, &self.dirs, None, Mode::Read).ok()?;
        let Identity::Known { id, name } = &here else {
            return None;
        };
        let registry = Registry::load(&self.store);
        let wanted = registry.by_name(named).ok()??;
        let related = wanted.id == *id
            || wanted.parent.as_deref() == Some(id.as_str())
            || registry
                .by_id(id)
                .is_some_and(|p| p.parent.as_deref() == Some(wanted.id.as_str()));
        if related {
            return None;
        }
        if !self.quiet {
            eprintln!(
                "mem: MEM_PROJECT names {named}, but this directory is {name}'s -- acting on {name}; `--project {named}` acts on the other"
            );
        }
        Some(here)
    }

    /// An index brought up to date with the store. A reindex that cannot take
    /// the lock leaves the existing index in place and serves it stale.
    pub fn read_index(&self) -> Result<Index> {
        let index = Index::open(&self.dirs.index_db(), Purpose::Read)?;
        index.reindex(&self.store, false)?;
        Ok(index)
    }

    pub fn search_scope(&self, project_id: Option<String>) -> Scope {
        match self.scope {
            Some(CliScope::Global) => Scope::Global,
            Some(CliScope::All) => Scope::All,
            _ => Scope::Project(project_id),
        }
    }
}

/// `MEM_PROJECT`, for a process that cannot choose its working directory: a
/// workflow worker stands at its worktree's root, which resolves to a
/// monorepo's root project, while its task belongs to the child under
/// `apps/<name>`. The flag still wins, and an empty value means unset.
fn project_from_env() -> Option<String> {
    std::env::var("MEM_PROJECT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
