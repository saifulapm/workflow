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
fn a_child_project_round_trips_parent_and_subdir() {
    let toml_text = r#"
id = "01K2CCCCCCCCCCCCCCCCCCCCCC"
name = "splitroute"
remote = "github.com/me/apps"
created = "2026-08-26T00:00:00Z"
parent = "01K2AAAAAAAAAAAAAAAAAAAAAA"
subdir = "apps/splitroute"
"#;
    let p: mem::project::Project = toml::from_str(toml_text).unwrap();
    assert_eq!(p.parent.as_deref(), Some("01K2AAAAAAAAAAAAAAAAAAAAAA"));
    assert_eq!(p.subdir.as_deref(), Some("apps/splitroute"));
    let out = toml::to_string(&p).unwrap();
    assert!(out.contains("parent = "), "{out}");
    assert!(out.contains("subdir = "), "{out}");

    // A project that never heard of children serializes neither key.
    let root: mem::project::Project = toml::from_str(
        r#"
id = "01K2AAAAAAAAAAAAAAAAAAAAAA"
name = "apps"
created = "2026-08-26T00:00:00Z"
"#,
    )
    .unwrap();
    assert_eq!(root.parent, None);
    assert_eq!(root.subdir, None);
    let out = toml::to_string(&root).unwrap();
    assert!(!out.contains("parent"), "{out}");
    assert!(!out.contains("subdir"), "{out}");
}

#[test]
fn children_of_returns_only_that_roots_children() {
    let w = World::new("ident-children");
    let store = w.store();
    let dirs = w.dirs();
    let a = w.repo("appsrepo", Some("git@github.com:me/appsrepo.git"));
    let b = w.repo("otherrepo", Some("git@github.com:me/otherrepo.git"));
    let root_a = resolve(&a, &store, &dirs, None, Mode::Write)
        .unwrap()
        .id()
        .unwrap()
        .to_string();
    let root_b = resolve(&b, &store, &dirs, None, Mode::Write)
        .unwrap()
        .id()
        .unwrap()
        .to_string();

    let registry = Registry::load(&store);
    let mut child = registry.by_id(&root_a).unwrap().clone();
    child.id = "01K2DDDDDDDDDDDDDDDDDDDDDD".to_string();
    child.name = "splitroute".to_string();
    child.parent = Some(root_a.clone());
    child.subdir = Some("apps/splitroute".to_string());
    mem::project::write_project(&store, &child).unwrap();

    let registry = Registry::load(&store);
    let children = registry.children_of(&root_a);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "splitroute");
    assert!(registry.children_of(&root_b).is_empty());
}

/// Registers `root_id`'s child named `name` at `subdir`, the way
/// `mem project add` will: same remote, parent set, subdir recorded.
fn add_child(store: &Store, root_id: &str, child_id: &str, name: &str, subdir: &str) {
    let registry = Registry::load(store);
    let mut child = registry.by_id(root_id).unwrap().clone();
    child.id = child_id.to_string();
    child.name = name.to_string();
    child.parent = Some(root_id.to_string());
    child.subdir = Some(subdir.to_string());
    mem::project::write_project(store, &child).unwrap();
    std::fs::create_dir_all(store.project_items(child_id)).unwrap();
}

#[test]
fn the_deepest_subdir_child_wins_and_the_root_takes_the_rest() {
    let w = World::new("ident-subdir");
    let store = w.store();
    let dirs = w.dirs();
    let repo = w.repo("mono", Some("git@github.com:me/mono.git"));
    let root = resolve(&repo, &store, &dirs, None, Mode::Write)
        .unwrap()
        .id()
        .unwrap()
        .to_string();
    add_child(&store, &root, "01K2DDDDDDDDDDDDDDDDDDDDDD", "x", "apps/x");
    add_child(&store, &root, "01K2EEEEEEEEEEEEEEEEEEEEEE", "x2", "apps/x2");

    for dir in ["apps/x", "apps/x/deep/er"] {
        let cwd = repo.join(dir);
        std::fs::create_dir_all(&cwd).unwrap();
        let got = resolve(&cwd, &store, &dirs, None, Mode::Read).unwrap();
        assert_eq!(got.name(), Some("x"), "{dir}");
    }

    // A component boundary, not a string prefix: apps/x2 is not inside apps/x.
    let x2 = repo.join("apps/x2/nested");
    std::fs::create_dir_all(&x2).unwrap();
    assert_eq!(
        resolve(&x2, &store, &dirs, None, Mode::Read).unwrap().name(),
        Some("x2")
    );
    let xy = repo.join("apps/xy");
    std::fs::create_dir_all(&xy).unwrap();
    assert_eq!(
        resolve(&xy, &store, &dirs, None, Mode::Read).unwrap().name(),
        Some("mono"),
        "apps/xy shares a string prefix with apps/x but no component"
    );

    // The root everywhere else, writes included: no accidental registration.
    assert_eq!(
        resolve(&repo, &store, &dirs, None, Mode::Write)
            .unwrap()
            .name(),
        Some("mono")
    );
    assert_eq!(Registry::load(&store).projects.len(), 3);

    // A write from inside the child's subdir belongs to the child.
    let deep = repo.join("apps/x/src");
    std::fs::create_dir_all(&deep).unwrap();
    assert_eq!(
        resolve(&deep, &store, &dirs, None, Mode::Write)
            .unwrap()
            .name(),
        Some("x")
    );
}

#[test]
fn a_fresh_machine_resolves_a_child_through_the_parents_remote() {
    let w = World::new("ident-subdir-remote");
    let store = w.store();
    let dirs = w.dirs();
    let first = w.repo("mono", Some("git@github.com:me/mono.git"));
    let root = resolve(&first, &store, &dirs, None, Mode::Write)
        .unwrap()
        .id()
        .unwrap()
        .to_string();
    add_child(&store, &root, "01K2DDDDDDDDDDDDDDDDDDDDDD", "x", "apps/x");

    // Another checkout of the same remote, no paths.toml entry: the ladder
    // goes by remote, and it must land on the ROOT before picking the child.
    let clone = w.repo("mono-clone", Some("https://github.com/me/mono"));
    let sub = clone.join("apps/x");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::remove_file(dirs.paths_toml()).unwrap();
    let got = resolve(&sub, &store, &dirs, None, Mode::Read).unwrap();
    assert_eq!(got.name(), Some("x"));

    // The remote hit recorded the ROOT project's path, not the child's.
    let map = PathMap::load(&dirs.paths_toml());
    assert!(map.projects.contains_key(&root), "{map:?}");
    assert!(
        !map.projects.contains_key("01K2DDDDDDDDDDDDDDDDDDDDDD"),
        "paths.toml stays root-only: {map:?}"
    );
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
