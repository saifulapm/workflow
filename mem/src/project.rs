//! Project identity (spec §5): which project is this working directory, and
//! what happens when the answer is "none yet".
//!
//! The ladder is: git common dir → machine-local `paths.toml` → normalized
//! origin remote matched against the store → unknown. Read verbs stop at
//! "unknown" and serve global scope; only write verbs register.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::atomic::{read_mtime, write_atomic, write_atomic_cas};
use crate::exit;
use crate::git::Checkout;
use crate::ids::ShortIds;
use crate::paths::Dirs;
use crate::store::{Store, read_dir_sorted};

/// One `projects/<id>/project.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub created: Timestamp,
    /// The command that verifies this project, set by `mem project set verify`.
    /// It lives here rather than in the checkout so the project owns its checks
    /// without the repository carrying a file about them (spec §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
}

impl Project {
    pub fn matches_name(&self, name: &str) -> bool {
        self.name == name
    }

    pub fn matches_alias(&self, name: &str) -> bool {
        self.aliases.iter().any(|a| a == name)
    }
}

/// Every project the store knows about. The name→id map is a derived view:
/// items carry the project NAME, so this is reconstructible from items alone.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub projects: Vec<Project>,
}

impl Registry {
    pub fn load(store: &Store) -> Registry {
        let mut projects = Vec::new();
        for dir in read_dir_sorted(&store.projects_dir()) {
            let toml_path = dir.join("project.toml");
            let Ok(text) = std::fs::read_to_string(&toml_path) else {
                continue;
            };
            if let Ok(p) = toml::from_str::<Project>(&text) {
                projects.push(p);
            }
        }
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        Registry { projects }
    }

    pub fn by_id(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn by_remote(&self, remote: &str) -> Option<&Project> {
        self.projects
            .iter()
            .find(|p| p.remote.as_deref() == Some(remote))
    }

    /// A name matches a project's name first; aliases only get a say when no
    /// project owns the name outright. An alias shared by two projects is
    /// ambiguous rather than a coin flip.
    pub fn by_name(&self, name: &str) -> Result<Option<&Project>> {
        if let Some(p) = self.projects.iter().find(|p| p.matches_name(name)) {
            return Ok(Some(p));
        }
        let aliased: Vec<&Project> = self
            .projects
            .iter()
            .filter(|p| p.matches_alias(name))
            .collect();
        match aliased.len() {
            0 => Ok(None),
            1 => Ok(Some(aliased[0])),
            _ => {
                let names: Vec<&str> = aliased.iter().map(|p| p.name.as_str()).collect();
                Err(exit::coded(
                    exit::AMBIGUOUS,
                    format!(
                        "'{name}' is an alias of several projects: {}",
                        names.join(", ")
                    ),
                ))
            }
        }
    }

    pub fn name_taken(&self, name: &str) -> bool {
        self.projects.iter().any(|p| p.matches_name(name))
    }

    /// `workflow`, then `workflow-2`, `workflow-3`… A suffixed project keeps
    /// the name it wanted as an alias, which is what makes the collision
    /// visible to doctor instead of silent.
    pub fn free_name(&self, wanted: &str) -> String {
        if !self.name_taken(wanted) {
            return wanted.to_string();
        }
        for n in 2..1000 {
            let candidate = format!("{wanted}-{n}");
            if !self.name_taken(&candidate) {
                return candidate;
            }
        }
        format!("{wanted}-{}", ulid::Ulid::generate())
    }
}

/// Machine-local `paths.toml`: project id → the checkouts of it on this
/// machine. Never synced — a path has no meaning on another machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PathMap {
    #[serde(default)]
    pub projects: BTreeMap<String, Vec<String>>,
}

impl PathMap {
    pub fn load(path: &Path) -> PathMap {
        // A corrupt path map is a cache miss, not an error: everything in it is
        // rediscoverable from the remote or by re-registering.
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn lookup(&self, common_dir: &Path) -> Option<&str> {
        let want = common_dir.to_string_lossy();
        self.projects
            .iter()
            .find(|(_, paths)| paths.iter().any(|p| *p == want))
            .map(|(id, _)| id.as_str())
    }

    pub fn record(&mut self, id: &str, common_dir: &Path) -> bool {
        let want = common_dir.to_string_lossy().to_string();
        let entry = self.projects.entry(id.to_string()).or_default();
        if entry.contains(&want) {
            return false;
        }
        entry.push(want);
        entry.sort();
        true
    }
}

/// Applies an edit to `paths.toml` and writes it back atomically. If someone
/// else rewrote the file in between, re-read — which merges their entries in,
/// since the edit is applied to their version — and try once more. If that also
/// races, leave their file alone: the entry we wanted to add is rediscoverable
/// on the next invocation.
pub fn update_path_map(path: &Path, edit: impl Fn(&mut PathMap)) -> Result<bool> {
    for _ in 0..2 {
        let seen = read_mtime(path);
        let mut map = PathMap::load(path);
        edit(&mut map);
        let text = toml::to_string(&map).context("serializing paths.toml")?;
        if write_atomic_cas(path, text.as_bytes(), seen)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The answer to "which project is this?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// A registered project.
    Known { id: String, name: String },
    /// A git checkout the store has never seen. Reads serve global scope and
    /// say so; only a write registers it.
    UnknownRepo { name_hint: String },
    /// Not a git checkout at all: global scope, and writes need `--project`.
    NonGit,
}

impl Identity {
    pub fn id(&self) -> Option<&str> {
        match self {
            Identity::Known { id, .. } => Some(id),
            _ => None,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Identity::Known { name, .. } => Some(name),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Read,
    Write,
}

/// Resolves the project for `cwd`. In `Mode::Write` an unregistered git
/// checkout is registered; in `Mode::Read` nothing is ever created.
pub fn resolve(
    cwd: &Path,
    store: &Store,
    dirs: &Dirs,
    explicit: Option<&str>,
    mode: Mode,
) -> Result<Identity> {
    let registry = Registry::load(store);

    if let Some(name) = explicit {
        return match registry.by_name(name)? {
            Some(p) => Ok(Identity::Known {
                id: p.id.clone(),
                name: p.name.clone(),
            }),
            None => Err(exit::not_found(format!(
                "no project named '{name}' — `mem projects` lists them"
            ))),
        };
    }

    // A non-git directory is global scope in both modes; a write there needs
    // `--project`, which the caller enforces where it knows what it is writing.
    let Some(checkout) = Checkout::detect(cwd) else {
        return Ok(Identity::NonGit);
    };

    let map_path = dirs.paths_toml();
    let map = PathMap::load(&map_path);
    if let Some(id) = map.lookup(&checkout.common_dir)
        && let Some(p) = registry.by_id(id)
    {
        return Ok(Identity::Known {
            id: p.id.clone(),
            name: p.name.clone(),
        });
    }

    if let Some(remote) = &checkout.remote
        && let Some(p) = registry.by_remote(remote)
    {
        let (id, name) = (p.id.clone(), p.name.clone());
        let _ = update_path_map(&map_path, |m| {
            m.record(&id, &checkout.common_dir);
        });
        return Ok(Identity::Known { id, name });
    }

    match mode {
        Mode::Read => Ok(Identity::UnknownRepo {
            name_hint: checkout.default_name(),
        }),
        Mode::Write => {
            let project = register(store, dirs, &registry, &checkout)?;
            Ok(Identity::Known {
                id: project.id,
                name: project.name,
            })
        }
    }
}

/// Creates `projects/<id>/project.toml` and records the checkout locally.
pub fn register(
    store: &Store,
    dirs: &Dirs,
    registry: &Registry,
    checkout: &Checkout,
) -> Result<Project> {
    let wanted = checkout.default_name();
    let name = registry.free_name(&wanted);
    let id = ShortIds::scan(&store.root).mint().to_string();
    let project = Project {
        id: id.clone(),
        name: name.clone(),
        remote: checkout.remote.clone(),
        // The name it wanted, kept so a collision is visible rather than silent.
        aliases: if name == wanted {
            Vec::new()
        } else {
            vec![wanted]
        },
        created: Timestamp::now(),
        verify: None,
    };
    write_project(store, &project)?;
    std::fs::create_dir_all(store.project_items(&id))
        .with_context(|| format!("creating items dir for {name}"))?;
    let _ = update_path_map(&dirs.paths_toml(), |m| {
        m.record(&id, &checkout.common_dir);
    });
    Ok(project)
}

pub fn write_project(store: &Store, project: &Project) -> Result<PathBuf> {
    let path = store.project_toml(&project.id);
    let text = toml::to_string(project).context("serializing project.toml")?;
    write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// `mem project set verify "<cmd>"` — records the project's own verification
/// command.
///
/// The edit goes through the parsed document rather than through `Project`, so
/// a key a later version of mem writes and this one has never heard of survives
/// the write instead of being silently dropped.
pub fn set_verify(store: &Store, project_id: &str, cmd: &str) -> Result<PathBuf> {
    let path = store.project_toml(project_id);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))
        .map_err(|e| exit::store_error(format!("{e:#}")))?;
    let mut doc: toml::Table = toml::from_str(&text)
        .map_err(|e| exit::store_error(format!("{} is not valid toml: {e}", path.display())))?;
    doc.insert("verify".to_string(), toml::Value::String(cmd.to_string()));
    let out = toml::to_string(&doc).context("serializing project.toml")?;
    write_atomic(&path, out.as_bytes())?;
    Ok(path)
}
