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

    pub fn wiki_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("wiki")
    }

    /// The file a slug names. The slug is joined verbatim, so every caller
    /// checks it with `is_valid_slug` first.
    pub fn wiki_page(&self, project_id: &str, slug: &str) -> PathBuf {
        self.wiki_dir(project_id).join(format!("{slug}.md"))
    }

    /// One project's pages, by slug. Anything in the directory that is not a
    /// `<slug>.md` file is skipped, the way strays beside items are.
    pub fn wiki_pages(&self, project_id: &str) -> Vec<Page> {
        read_dir_sorted(&self.wiki_dir(project_id))
            .iter()
            .filter_map(|path| read_page(path))
            .collect()
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

/// The longest a page slug may be.
pub const SLUG_MAX: usize = 64;

/// `[a-z0-9][a-z0-9-]{0,63}` — the plan's task-id rule grown up. A slug is also
/// a file name, so this is what keeps `..`, dot-temps and bisync conflict
/// losers out of the wiki.
pub fn is_valid_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    slug.len() <= SLUG_MAX
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A wiki page as the read verbs report it. The text is not carried: a listing
/// of twenty pages has no business holding twenty documents.
#[derive(Debug, Clone)]
pub struct Page {
    pub slug: String,
    /// The first heading, for the listing. Empty when the page has none.
    pub title: String,
    pub bytes: u64,
    pub modified_epoch: i64,
    pub path: PathBuf,
}

/// Reads one page's listing line, or None when the file is not a page.
pub fn read_page(path: &Path) -> Option<Page> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".md")?;
    if !is_valid_slug(stem) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(Page {
        slug: stem.to_string(),
        title: page_title(&std::fs::read_to_string(path).unwrap_or_default()),
        bytes: meta.len(),
        modified_epoch: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64),
        path: path.to_path_buf(),
    })
}

/// What a listing calls a page: its first heading, or its first non-empty line
/// when it has none.
pub fn page_title(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let stripped = line.trim_start_matches('#');
        let title = if stripped.len() < line.len() {
            stripped.trim_start()
        } else {
            line
        };
        return crate::search::truncate_bytes(title, 100);
    }
    String::new()
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
