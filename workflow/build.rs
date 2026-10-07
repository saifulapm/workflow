//! Embeds this repository's `skills/`, `agents/` and `hooks/` directories as a
//! static table of (path below the repository, bytes), which `workflow
//! install` writes out. A file added to one of the directories is carried
//! without any code change.
//!
//! Hidden files and directories are left out, and `agents/` carries only its
//! `.md` files.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

const DIRS: [&str; 3] = ["skills", "agents", "hooks"];

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let repo = manifest.parent().expect("the crate sits inside the repo");

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for dir in DIRS {
        let top = repo.join(dir);
        println!("cargo::rerun-if-changed={}", top.display());
        collect(&top, dir, dir == "agents", &mut files);
    }
    files.sort();

    let mut table = String::from(
        "/// Every embedded file: its path below the repository, and its bytes.\n\
         pub static FILES: &[(&str, &[u8])] = &[\n",
    );
    for (rel, abs) in &files {
        let abs = abs.to_str().expect("embedded paths are UTF-8");
        writeln!(table, "    ({rel:?}, include_bytes!({abs:?})),").unwrap();
    }
    table.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets this")).join("embedded.rs");
    fs::write(out, table).expect("write the embedded table");
}

/// Every file under `dir`, named by its `/`-joined path from the repository.
fn collect(dir: &Path, rel: &str, only_md: bool, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let rel = format!("{rel}/{name}");
        if path.is_dir() {
            collect(&path, &rel, only_md, files);
        } else if path.is_file() && (!only_md || name.ends_with(".md")) {
            files.push((rel, path));
        }
    }
}
