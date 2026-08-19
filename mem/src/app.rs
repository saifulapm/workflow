//! What every verb needs: where the store is, which project this is, and an
//! index that has already caught up with the files.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cli::{Cli, Scope as CliScope};
use crate::index::{Index, Purpose};
use crate::paths::{Dirs, machine_name};
use crate::project::{Identity, Mode, resolve};
use crate::search::Scope;
use crate::store::Store;

pub struct App {
    pub dirs: Dirs,
    pub store: Store,
    pub cwd: PathBuf,
    pub machine: String,
    pub project: Option<String>,
    pub scope: Option<CliScope>,
    pub include_archived: bool,
    pub json: bool,
    pub quiet: bool,
    /// From `--session-id` or `MEM_SESSION_ID`; machine-local activity tracking.
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
            project: cli.project.clone(),
            scope: cli.scope,
            include_archived: cli.include_archived,
            json: cli.json,
            quiet: cli.quiet,
            session_id: crate::session::id_from(None),
        })
    }

    pub fn identity(&self, mode: Mode) -> Result<Identity> {
        resolve(
            &self.cwd,
            &self.store,
            &self.dirs,
            self.project.as_deref(),
            mode,
        )
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
