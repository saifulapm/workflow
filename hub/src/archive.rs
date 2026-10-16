//! `hub share archive <tarball>`: evidence and raw files leave the store on
//! Cloudflare 90 days after their upload, once a Dropbox snapshot holds
//! them. A file the snapshot lacks is fetched into this machine's replica
//! instead, so the next week's snapshot holds it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::share::Store;

/// How an archive run went.
#[derive(Debug, Default)]
pub struct Archived {
    /// Files uploaded more than 90 days ago.
    pub old: usize,
    pub deleted: usize,
    pub fetched: usize,
    /// Requests the store did not answer as asked.
    pub failed: usize,
}

impl Archived {
    pub fn summary(&self) -> String {
        format!(
            "archive: {} files older than {} days; {} left the store, \
             {} fetched into the replica, {} failed",
            self.old,
            hubpages::page_evidence::ARCHIVE_DAYS,
            self.deleted,
            self.fetched,
            self.failed,
        )
    }
}

/// What to do with a project's stored files.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Old and in the snapshot: drop from the store.
    pub delete: Vec<String>,
    /// Old, not in the snapshot, and not in the replica: fetch into it.
    pub fetch: Vec<String>,
}

/// One archive run: every project folder of this machine's store, against
/// the snapshot at `tarball`.
pub fn run(store: &Store, tarball: &Path) -> Result<Archived> {
    let listing = std::process::Command::new("tar")
        .arg("-tzf")
        .arg(tarball)
        .output()
        .context("running tar")?;
    if !listing.status.success() {
        bail!("tar could not list {}", tarball.display());
    }
    let snapshot = paths_of(&String::from_utf8_lossy(&listing.stdout));
    let root = store_root()?;
    let days = i64::from(hubpages::page_evidence::ARCHIVE_DAYS);
    let cutoff = crate::share::now().as_millisecond() - days * 86_400_000;
    let mut archived = Archived::default();
    let mut projects: Vec<PathBuf> = std::fs::read_dir(root.join("projects"))
        .map(|dir| dir.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    projects.sort();
    for folder in projects.iter().filter(|p| p.is_dir()) {
        let Some(project) = folder.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let stored = match list(store, project) {
            Ok(stored) => stored,
            Err(e) => {
                archived.failed += 1;
                eprintln!("hub: share archive: listing {project}: {e:#}");
                continue;
            }
        };
        archived.old += stored.iter().filter(|(_, at)| *at < cutoff).count();
        let plan = plan(&stored, &snapshot, cutoff, |path| root.join(path).exists());
        for path in &plan.delete {
            match delete(store, path) {
                Ok(()) => archived.deleted += 1,
                Err(e) => {
                    archived.failed += 1;
                    eprintln!("hub: share archive: deleting {path}: {e:#}");
                }
            }
        }
        for path in &plan.fetch {
            match fetch(store, &root, path) {
                Ok(()) => archived.fetched += 1,
                Err(e) => {
                    archived.failed += 1;
                    eprintln!("hub: share archive: fetching {path}: {e:#}");
                }
            }
        }
    }
    Ok(archived)
}

/// The store paths a `tar -tzf` listing of a `mem snapshot` holds. mem tars
/// the store under its own folder name, so each entry loses its first part.
pub fn paths_of(listing: &str) -> HashSet<String> {
    listing
        .lines()
        .filter(|entry| !entry.ends_with('/'))
        .filter_map(|entry| entry.split_once('/'))
        .map(|(_, path)| path.to_string())
        .collect()
}

/// The plan for `stored`, each path with its upload time in ms, against the
/// snapshot's paths: a file uploaded before `cutoff_ms` is old.
pub fn plan(
    stored: &[(String, i64)],
    snapshot: &HashSet<String>,
    cutoff_ms: i64,
    in_replica: impl Fn(&str) -> bool,
) -> Plan {
    let mut plan = Plan::default();
    for (path, _) in stored.iter().filter(|(_, at)| *at < cutoff_ms) {
        if snapshot.contains(path) {
            plan.delete.push(path.clone());
        } else if !in_replica(path) {
            plan.fetch.push(path.clone());
        }
    }
    plan
}

/// mem's store: `$XDG_DATA_HOME/mem/store`, else `~/.local/share/mem/store`.
fn store_root() -> Result<PathBuf> {
    let data = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(data) => PathBuf::from(data),
        None => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/share")
        }
    };
    Ok(data.join("mem/store"))
}

/// The project's stored files with their upload times.
fn list(store: &Store, project: &str) -> Result<Vec<(String, i64)>> {
    let url = format!("{}/store/files?project={project}", store.base()?);
    let mut request = store.agent.get(url);
    for (name, value) in store.headers() {
        request = request.header(name, value);
    }
    let mut response = request.call().context("reaching the store")?;
    let status = response.status().as_u16();
    if status != 200 {
        bail!("the store answered {status}");
    }
    let rows: Vec<Value> =
        serde_json::from_reader(response.body_mut().as_reader()).context("reading the list")?;
    Ok(rows
        .iter()
        .filter_map(|row| Some((row["path"].as_str()?.to_string(), row["at"].as_i64()?)))
        .collect())
}

fn delete(store: &Store, path: &str) -> Result<()> {
    let mut request = store.agent.delete(store.file_url(path)?);
    for (name, value) in store.headers() {
        request = request.header(name, value);
    }
    let response = request.call().context("reaching the store")?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        status => bail!("the store answered {status}"),
    }
}

/// Writes the store's bytes for `path` into the replica under a dot name
/// first, which mem never shares, so no reader sees half a file.
fn fetch(store: &Store, root: &Path, path: &str) -> Result<()> {
    let bytes = store
        .get_file(path)?
        .context("the store listed the file but holds no bytes")?;
    let target = root.join(path);
    let dir = target.parent().context("a file path has a folder")?;
    std::fs::create_dir_all(dir)?;
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!(".{name}.archive"));
    std::fs::write(&temp, bytes)
        .and_then(|()| std::fs::rename(&temp, &target))
        .with_context(|| format!("writing {}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_listing_gives_store_paths_of_files() {
        let listing = "store/\nstore/projects/\nstore/projects/P1/evidence/t1/a b.png\n\
                       store/projects/P1/raw/n.pdf\nstore/global/items/x.md\n";
        let mut paths: Vec<String> = paths_of(listing).into_iter().collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "global/items/x.md",
                "projects/P1/evidence/t1/a b.png",
                "projects/P1/raw/n.pdf"
            ]
        );
    }

    #[test]
    fn old_files_leave_when_snapshotted_and_are_fetched_when_not() {
        let stored = [
            ("projects/P1/evidence/t1/old.png".to_string(), 1_000),
            ("projects/P1/evidence/t1/lost.png".to_string(), 1_000),
            ("projects/P1/evidence/t1/kept.png".to_string(), 1_000),
            ("projects/P1/evidence/t1/young.png".to_string(), 5_000),
        ];
        let snapshot: HashSet<String> = [
            "projects/P1/evidence/t1/old.png",
            "projects/P1/evidence/t1/young.png",
        ]
        .map(String::from)
        .into();
        let plan = plan(&stored, &snapshot, 2_000, |path| path.ends_with("kept.png"));
        assert_eq!(
            plan,
            Plan {
                delete: vec!["projects/P1/evidence/t1/old.png".to_string()],
                fetch: vec!["projects/P1/evidence/t1/lost.png".to_string()],
            }
        );
    }
}
