//! Project identity, registration and the machine-local path map (spec §5).

mod common;

use std::path::{Path, PathBuf};

use common::TempDir;
use mem::git::Checkout;
use mem::paths::Dirs;
use mem::project::{Identity, Mode, PathMap, Registry, resolve, update_path_map};
use mem::store::Store;

/// A store, a state dir and a place to make repositories, all under one temp dir.
struct World {
    dir: TempDir,
}

impl World {
    fn new(tag: &str) -> World {
        World {
            dir: TempDir::new(tag),
        }
    }

    fn dirs(&self) -> Dirs {
        Dirs {
            data: self.dir.join("data"),
            cache: self.dir.join("cache"),
            state: self.dir.join("state"),
            config: self.dir.join("config"),
        }
    }

    fn store(&self) -> Store {
        Store::new(self.dirs().store())
    }

    fn repo(&self, name: &str, remote: Option<&str>) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        run_git(&path, &["init", "-q"]);
        if let Some(remote) = remote {
            run_git(&path, &["remote", "add", "origin", remote]);
        }
        path
    }

    fn plain_dir(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

fn run_git(cwd: &Path, args: &[&str]) {
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

#[test]
fn a_read_in_an_unknown_repo_registers_nothing() {
    let w = World::new("ident-read");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));
    let id = resolve(&repo, &w.store(), &w.dirs(), None, Mode::Read).unwrap();
    assert_eq!(
        id,
        Identity::UnknownRepo {
            name_hint: "thing".to_string()
        }
    );
    assert!(
        !w.store().projects_dir().exists(),
        "read verbs never register"
    );
    assert!(
        !w.dirs().paths_toml().exists(),
        "read verbs never touch state"
    );
}

#[test]
fn a_write_registers_the_checkout_and_reads_then_resolve_it() {
    let w = World::new("ident-write");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));
    let store = w.store();
    let dirs = w.dirs();

    let id = resolve(&repo, &store, &dirs, None, Mode::Write).unwrap();
    let Identity::Known { id, name } = id else {
        panic!("write mode must register: {id:?}");
    };
    assert_eq!(name, "thing", "name is the git toplevel basename");
    assert!(store.project_toml(&id).exists());
    assert!(store.project_items(&id).is_dir());

    let project = &Registry::load(&store).projects[0];
    assert_eq!(project.remote.as_deref(), Some("github.com/me/thing"));
    assert!(project.aliases.is_empty());

    // A read from the same checkout now resolves through paths.toml.
    let again = resolve(&repo, &store, &dirs, None, Mode::Read).unwrap();
    assert_eq!(
        again,
        Identity::Known {
            id: id.clone(),
            name
        }
    );

    // And from a subdirectory of it.
    let sub = repo.join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(
        resolve(&sub, &store, &dirs, None, Mode::Read).unwrap().id(),
        Some(id.as_str())
    );
}

#[test]
fn a_second_checkout_of_the_same_remote_resolves_by_remote() {
    let w = World::new("ident-remote");
    let store = w.store();
    let dirs = w.dirs();
    let first = w.repo("thing", Some("git@github.com:me/thing.git"));
    let id = resolve(&first, &store, &dirs, None, Mode::Write)
        .unwrap()
        .id()
        .unwrap()
        .to_string();

    // A different directory, same origin, spelled differently.
    let second = w.repo("thing-clone", Some("https://GitHub.com/me/thing"));
    let resolved = resolve(&second, &store, &dirs, None, Mode::Read).unwrap();
    assert_eq!(resolved.id(), Some(id.as_str()));

    // Resolving by remote records the path so the next lookup is local.
    let map = PathMap::load(&dirs.paths_toml());
    assert_eq!(map.projects[&id].len(), 2, "{map:?}");
}

#[test]
fn a_name_collision_takes_a_numeric_suffix_and_keeps_the_wanted_name_as_an_alias() {
    let w = World::new("ident-collide");
    let store = w.store();
    let dirs = w.dirs();
    let a = w.repo("thing", Some("git@github.com:me/thing.git"));
    let nested = w.plain_dir("other");
    let b = {
        let path = nested.join("thing");
        std::fs::create_dir_all(&path).unwrap();
        run_git(&path, &["init", "-q"]);
        run_git(
            &path,
            &["remote", "add", "origin", "git@github.com:you/thing.git"],
        );
        path
    };

    let first = resolve(&a, &store, &dirs, None, Mode::Write).unwrap();
    let second = resolve(&b, &store, &dirs, None, Mode::Write).unwrap();
    assert_eq!(first.name(), Some("thing"));
    assert_eq!(second.name(), Some("thing-2"));
    assert_ne!(first.id(), second.id());

    let registry = Registry::load(&store);
    let suffixed = registry.by_id(second.id().unwrap()).unwrap();
    assert_eq!(suffixed.aliases, vec!["thing".to_string()]);
    // The outright name still wins over the alias.
    assert_eq!(
        registry.by_name("thing").unwrap().map(|p| p.id.as_str()),
        first.id()
    );
}

#[test]
fn a_non_git_directory_is_global_scope() {
    let w = World::new("ident-nongit");
    let plain = w.plain_dir("loose");
    assert_eq!(
        resolve(&plain, &w.store(), &w.dirs(), None, Mode::Read).unwrap(),
        Identity::NonGit
    );
    assert_eq!(
        resolve(&plain, &w.store(), &w.dirs(), None, Mode::Write).unwrap(),
        Identity::NonGit,
        "a non-git directory is never auto-registered"
    );
    assert!(!w.store().projects_dir().exists());
}

#[test]
fn an_explicit_project_name_resolves_without_git_and_is_not_invented() {
    let w = World::new("ident-explicit");
    let store = w.store();
    let dirs = w.dirs();
    let repo = w.repo("thing", None);
    let id = resolve(&repo, &store, &dirs, None, Mode::Write)
        .unwrap()
        .id()
        .unwrap()
        .to_string();

    let plain = w.plain_dir("elsewhere");
    let resolved = resolve(&plain, &store, &dirs, Some("thing"), Mode::Write).unwrap();
    assert_eq!(resolved.id(), Some(id.as_str()));

    let err = resolve(&plain, &store, &dirs, Some("nope"), Mode::Write).unwrap_err();
    assert_eq!(mem::exit::code_of(&err), mem::exit::NOT_FOUND);
    assert_eq!(
        Registry::load(&store).projects.len(),
        1,
        "an unknown --project name must not create anything"
    );
}

#[test]
fn a_linked_worktree_is_the_same_project() {
    let w = World::new("ident-worktree");
    let store = w.store();
    let dirs = w.dirs();
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("f.txt"), b"x").unwrap();
    run_git(&repo, &["add", "f.txt"]);
    run_git(&repo, &["commit", "-qm", "first"]);

    let id = resolve(&repo, &store, &dirs, None, Mode::Write)
        .unwrap()
        .id()
        .unwrap()
        .to_string();

    let wt = w.dir.join("wt");
    run_git(&repo, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    let resolved = resolve(&wt, &store, &dirs, None, Mode::Read).unwrap();
    assert_eq!(
        resolved.id(),
        Some(id.as_str()),
        "a linked worktree shares the git common dir, so it is the same project"
    );
}

#[test]
fn the_path_map_merges_a_concurrent_rewrite() {
    let w = World::new("ident-pathmap");
    let path = w.dir.join("paths.toml");

    update_path_map(&path, |m| {
        m.record("01K2AAAAAAAAAAAAAAAAAAAAAA", Path::new("/a/.git"));
    })
    .unwrap();

    // Someone else rewrites the file between our read and our write: the CAS
    // fails, we re-read (picking their entry up) and retry.
    let wrote = update_path_map(&path, |m| {
        if !m.projects.contains_key("01K2CCCCCCCCCCCCCCCCCCCCCC") {
            let other = PathMap::load(&path);
            if !other.projects.contains_key("01K2BBBBBBBBBBBBBBBBBBBBBB") {
                let mut theirs = other.clone();
                theirs.record("01K2BBBBBBBBBBBBBBBBBBBBBB", Path::new("/b/.git"));
                std::thread::sleep(std::time::Duration::from_millis(5));
                std::fs::write(&path, toml::to_string(&theirs).unwrap()).unwrap();
            }
        }
        m.record("01K2CCCCCCCCCCCCCCCCCCCCCC", Path::new("/c/.git"));
    })
    .unwrap();
    assert!(wrote, "the retry must succeed");

    let map = PathMap::load(&path);
    assert_eq!(map.projects.len(), 3, "no entry may be lost: {map:?}");
    assert_eq!(
        map.lookup(Path::new("/b/.git")),
        Some("01K2BBBBBBBBBBBBBBBBBBBBBB")
    );
    assert_eq!(
        map.lookup(Path::new("/c/.git")),
        Some("01K2CCCCCCCCCCCCCCCCCCCCCC")
    );
}

#[test]
fn a_corrupt_path_map_is_a_cache_miss_not_an_error() {
    let w = World::new("ident-corrupt");
    let path = w.dir.join("paths.toml");
    std::fs::write(&path, b"this is not toml {{{").unwrap();
    assert!(PathMap::load(&path).projects.is_empty());
    assert!(
        update_path_map(&path, |m| {
            m.record("01K2AAAAAAAAAAAAAAAAAAAAAA", Path::new("/a/.git"));
        })
        .unwrap()
    );
    assert_eq!(PathMap::load(&path).projects.len(), 1);
}

#[test]
fn a_checkout_reports_its_default_name_and_normalized_remote() {
    let w = World::new("ident-checkout");
    let repo = w.repo("My-Repo", Some("git@github.com:Me/My-Repo.git"));
    let checkout = Checkout::detect(&repo).expect("a git checkout");
    assert_eq!(checkout.default_name(), "My-Repo");
    assert_eq!(checkout.remote.as_deref(), Some("github.com/Me/My-Repo"));
    assert!(Checkout::detect(&w.plain_dir("nope")).is_none());
}
