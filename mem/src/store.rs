//! The store tree: layout, scanning, and item read/write (spec §3).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::atomic::write_atomic;
use crate::ids::is_item_filename;
use crate::item::Item;

/// Store format version. A binary older than the store refuses writes and
/// degrades reads (spec §3).
pub const STORE_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store { root }
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    pub fn version_path(&self) -> PathBuf {
        self.root.join("VERSION")
    }

    pub fn global_items(&self) -> PathBuf {
        self.root.join("global/items")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }

    pub fn project_dir(&self, project_id: &str) -> PathBuf {
        self.projects_dir().join(project_id)
    }

    pub fn project_items(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("items")
    }

    pub fn project_toml(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("project.toml")
    }

    pub fn plan_path(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("plan.md")
    }

    pub fn status_path(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("status.md")
    }

    /// Every well-formed item file in the store.
    pub fn item_paths(&self) -> Vec<PathBuf> {
        item_files(&self.root)
    }

    /// Files that sit where items live but are not items: dot-temps, bisync
    /// conflict losers (`*.path1`/`*.path2`), anything hand-dropped. Never
    /// indexed; reported by doctor.
    pub fn stray_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for dir in items_dirs(&self.root) {
            for entry in read_dir_sorted(&dir) {
                let name = match entry.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if !is_item_filename(&name) && entry.is_file() {
                    out.push(entry);
                }
            }
        }
        out
    }

    pub fn read_item(&self, path: &Path) -> Result<Item> {
        read_item(path)
    }

    /// Writes an item to its canonical path: `<items dir>/<ulid>.md`.
    pub fn write_item(&self, items_dir: &Path, item: &Item) -> Result<PathBuf> {
        let path = items_dir.join(format!("{}.md", item.meta.id));
        write_atomic(&path, &item.to_bytes()?)?;
        Ok(path)
    }
}

pub fn read_item(path: &Path) -> Result<Item> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Item::parse(&bytes).with_context(|| format!("parsing {}", path.display()))
}

/// Every directory that may hold items.
pub fn items_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("global/items")];
    for project in read_dir_sorted(&root.join("projects")) {
        if project.is_dir() {
            dirs.push(project.join("items"));
        }
    }
    dirs
}

/// `<root>/**/items/<ulid>.md`. A missing root yields nothing — a fresh
/// machine whose hub has not materialised yet is not an error.
pub fn item_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in items_dirs(root) {
        for entry in read_dir_sorted(&dir) {
            if entry
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_item_filename)
            {
                out.push(entry);
            }
        }
    }
    out
}

/// Directory listing sorted by name, with unreadable directories treated as
/// empty: every read path in mem degrades rather than failing.
pub fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => return Vec::new(),
    };
    out.sort();
    out
}
