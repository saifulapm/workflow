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
        crate::share::install(
            dirs.store(),
            dirs.share_db(),
            crate::localhub::socket_path(&dirs.state),
        );
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
        let mut named = self.project.as_deref();
        let mut set_aside = None;
        if self.project_from_env
            && let Some(env) = named
            && let Some(here) = self.overruled(env)
        {
            named = None;
            if mode == Mode::Read {
                self.say_set_aside(env, &here);
                return Ok(here);
            }
            set_aside = Some(env);
        }
        let identity = resolve(&self.cwd, &self.store, &self.dirs, named, mode)?;
        if mode == Mode::Write && named.is_none() {
            self.refuse_a_root_with_children(&identity, set_aside)?;
        }
        if let Some(env) = set_aside {
            self.say_set_aside(env, &identity);
        }
        Ok(identity)
    }

    /// The note that `MEM_PROJECT` yielded to the directory. A write the
    /// root check refuses prints only the refusal, which names the variable.
    fn say_set_aside(&self, named: &str, here: &Identity) {
        if let Identity::Known { name, .. } = here
            && !self.quiet
        {
            eprintln!(
                "mem: MEM_PROJECT names {named}, but this directory is {name}'s -- acting on {name}; `--project {named}` acts on the other"
            );
        }
    }

    /// A write the directory alone files under a monorepo's root, when the
    /// root has child projects. `cd <worktree root> && mem log ...` from an
    /// app's session put 76 writes in 14 days into shopify_apps, which no
    /// app reads; the root's own work names it with `--project`, and a cloud
    /// run's `cloud/<root>-<milestone>` branch names it too.
    fn refuse_a_root_with_children(
        &self,
        identity: &Identity,
        set_aside: Option<&str>,
    ) -> Result<()> {
        let Identity::Known { id, name } = identity else {
            return Ok(());
        };
        let registry = Registry::load(&self.store);
        if let Some(checkout) = crate::git::Checkout::detect(&self.cwd)
            && let (Some(remote), Some(branch)) = (&checkout.remote, &checkout.branch)
            && registry
                .by_cloud_branch(remote, branch)
                .is_some_and(|p| p.id == *id)
        {
            return Ok(());
        }
        let children: Vec<&str> = registry
            .children_of(id)
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        if children.is_empty() {
            return Ok(());
        }
        let children = children.join(", ");
        Err(crate::exit::usage(match set_aside {
            Some(env) => format!(
                "MEM_PROJECT={env} was set aside: this folder is {name}'s root, \
                 and {name} has the child projects {children}: run this in an app's \
                 folder or pass --project <name> (`--project {env}` writes to {env}, \
                 `--project {name}` to the root)"
            ),
            None => format!(
                "this folder is {name}'s root, and {name} has the child projects {children}: \
                 run this in an app's folder or pass --project <name> \
                 (`--project {name}` writes to the root)"
            ),
        }))
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
        let Identity::Known { id, .. } = &here else {
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
