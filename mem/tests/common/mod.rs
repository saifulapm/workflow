//! Shared helpers for the integration tests.
//!
//! Each test binary compiles the whole module, so items only some of them use
//! are not dead code.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A directory under the system temp dir that removes itself on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mem-test-{tag}-{}-{}",
            std::process::id(),
            ulid::Ulid::generate()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A whole mem installation — store, cache, state and config — under one temp
/// directory, plus room to make git checkouts next to it.
pub struct World {
    pub dir: TempDir,
}

impl World {
    pub fn new(tag: &str) -> World {
        World {
            dir: TempDir::new(tag),
        }
    }

    pub fn dirs(&self) -> mem::paths::Dirs {
        mem::paths::Dirs {
            data: self.dir.join("data"),
            cache: self.dir.join("cache"),
            state: self.dir.join("state"),
            config: self.dir.join("config"),
        }
    }

    pub fn store(&self) -> mem::store::Store {
        mem::store::Store::new(self.dirs().store())
    }

    pub fn index_path(&self) -> PathBuf {
        self.dirs().index_db()
    }

    /// A registered project with a fixed id, without going through git.
    pub fn project(&self, id: &str, name: &str) -> String {
        let store = self.store();
        std::fs::create_dir_all(store.project_items(id)).unwrap();
        mem::project::write_project(
            &store,
            &mem::project::Project {
                id: id.to_string(),
                name: name.to_string(),
                remote: None,
                aliases: Vec::new(),
                created: "2026-08-01T00:00:00Z".parse().unwrap(),
                verify: None,
            },
        )
        .unwrap();
        id.to_string()
    }

    pub fn repo(&self, name: &str, remote: Option<&str>) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        run_git(&path, &["init", "-q"]);
        if let Some(remote) = remote {
            run_git(&path, &["remote", "add", "origin", remote]);
        }
        path
    }

    pub fn plain_dir(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

/// An item with a fresh id, ready to be tweaked before it is written.
pub fn item(kind: mem::item::Kind, title: &str, body: &str) -> mem::item::Item {
    let meta = mem::item::Meta::new(
        ulid::Ulid::generate().to_string(),
        kind,
        title.to_string(),
        "test-machine".to_string(),
    );
    mem::item::Item::new(meta, body.as_bytes().to_vec())
}

/// Writes an item into a project (or the global tree when `project_id` is None).
pub fn put(
    store: &mem::store::Store,
    project_id: Option<&str>,
    item: &mem::item::Item,
) -> std::path::PathBuf {
    let dir = match project_id {
        Some(id) => store.project_items(id),
        None => store.global_items(),
    };
    std::fs::create_dir_all(&dir).unwrap();
    store.write_item(&dir, item).unwrap()
}

/// Runs the built binary against a World's directories.
pub fn mem(w: &World, cwd: &Path, args: &[&str]) -> std::process::Output {
    mem_env(w, cwd, args, &[])
}

/// The same, with extra environment — the seams (`MEM_SYNC_CMD`,
/// `MEM_NOTIFY_CMD`) that keep a test from shelling out to the real thing.
pub fn mem_env(w: &World, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let dirs = w.dirs();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_mem"));
    cmd.current_dir(cwd)
        .args(args)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env_remove("MEM_SESSION_ID");
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().expect("run mem")
}

pub fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

pub fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

pub fn code(out: &std::process::Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

pub fn run_git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Deterministic PRNG (xorshift64*) so property tests are reproducible.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}
