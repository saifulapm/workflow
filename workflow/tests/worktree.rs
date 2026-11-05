//! `workflow worktree create`, the WorktreeCreate hook, through the real
//! binary. `fixtures/worktree-create.json` is the input a real
//! `isolation: worktree` spawn handed the hook (Claude Code 2.1.316), its
//! ids and folders scrubbed: `name` is `agent-` and the Agent tool's id.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use scratch::Scratch;
use serde_json::Value;

const NAME: &str = "agent-a248710c831042813";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository at `rel` with one commit on main.
fn repo(w: &Scratch, rel: &str) -> PathBuf {
    let dir = w.mkdir(rel);
    git(&dir, &["init", "-q", "-b", "main"]);
    w.put(&format!("{rel}/a.txt"), "a\n");
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "First file"]);
    dir.canonicalize().unwrap()
}

fn hook(action: &str, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["worktree", action])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn create(cwd: &Path) -> Output {
    let mut input: Value =
        serde_json::from_str(include_str!("fixtures/worktree-create.json")).unwrap();
    input["cwd"] = cwd.to_str().unwrap().into();
    hook("create", &input.to_string())
}

/// The last non-empty line of stdout, which Claude Code reads as the path.
fn printed(out: &Output) -> PathBuf {
    let text = String::from_utf8_lossy(&out.stdout);
    PathBuf::from(text.lines().rev().find(|l| !l.trim().is_empty()).unwrap())
}

#[test]
fn a_new_worktree_branches_from_head_where_orchestrate_looks() {
    let w = Scratch::new("wt-create");
    let main = repo(&w, "repo");
    let out = create(&main);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let dir = printed(&out);
    assert_eq!(dir, main.join(".claude/worktrees").join(NAME));
    assert_eq!(
        git(&dir, &["branch", "--show-current"]),
        format!("worktree-{NAME}")
    );
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD"]),
        git(&main, &["rev-parse", "HEAD"])
    );
}

#[test]
fn a_session_in_another_apps_worktree_branches_from_that_checkout() {
    let w = Scratch::new("wt-create-app");
    let main = repo(&w, "repo");
    let app = w.at("app");
    git(
        &main,
        &["worktree", "add", "-q", "-b", "app", app.to_str().unwrap()],
    );
    w.put("app/b.txt", "b\n");
    git(&app, &["add", "b.txt"]);
    git(&app, &["commit", "-qm", "Second file"]);

    let out = create(&app);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let dir = printed(&out);
    assert_eq!(dir, main.join(".claude/worktrees").join(NAME));
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD"]),
        git(&app, &["rev-parse", "HEAD"])
    );
}

#[test]
fn a_failed_git_worktree_add_fails_with_gits_message_and_no_folder() {
    let w = Scratch::new("wt-create-fail");
    let main = repo(&w, "repo");
    git(&main, &["branch", &format!("worktree-{NAME}")]);
    let out = create(&main);
    assert_ne!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("already exists"), "{err}");
    assert!(!main.join(".claude/worktrees").join(NAME).exists());
}

/// A pnpm monorepo's trunk checkout with its installs: the root
/// node_modules holds the store, and each workspace package's own
/// node_modules links its siblings by relative links, as pnpm makes them.
fn pnpm_repo(w: &Scratch, rel: &str) -> PathBuf {
    let dir = repo(w, rel);
    w.put(&format!("{rel}/.gitignore"), "node_modules\nvendor\n");
    w.put(&format!("{rel}/package.json"), "{}\n");
    w.put(&format!("{rel}/apps/web/package.json"), "{}\n");
    w.put(&format!("{rel}/packages/ui/package.json"), "{}\n");
    w.put(&format!("{rel}/packages/ui/index.js"), "export {}\n");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "Workspace"]);
    w.put(&format!("{rel}/node_modules/.pnpm/react/index.js"), "\n");
    w.put(&format!("{rel}/packages/ui/node_modules/.bin/tool"), "\n");
    w.mkdir(&format!("{rel}/apps/web/node_modules/@scope"));
    std::os::unix::fs::symlink(
        "../../../../packages/ui",
        dir.join("apps/web/node_modules/@scope/ui"),
    )
    .unwrap();
    dir
}

#[test]
fn the_root_node_modules_links_the_trunks_and_nested_ones_are_copied() {
    let w = Scratch::new("wt-pnpm");
    let main = pnpm_repo(&w, "repo");
    let out = create(&main);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let dir = printed(&out);

    assert_eq!(
        std::fs::read_link(dir.join("node_modules")).unwrap(),
        main.join("node_modules")
    );
    for nested in ["apps/web/node_modules", "packages/ui/node_modules"] {
        let meta = std::fs::symlink_metadata(dir.join(nested)).unwrap();
        assert!(meta.is_dir(), "{nested} is a real folder");
    }
    assert!(dir.join("packages/ui/node_modules/.bin/tool").exists());
    assert_eq!(
        dir.join("apps/web/node_modules/@scope/ui")
            .canonicalize()
            .unwrap(),
        dir.join("packages/ui")
    );
}

#[test]
fn a_session_in_another_apps_worktree_links_that_checkouts_node_modules() {
    let w = Scratch::new("wt-pnpm-app");
    let main = pnpm_repo(&w, "repo");
    let app = w.at("app");
    git(
        &main,
        &["worktree", "add", "-q", "-b", "app", app.to_str().unwrap()],
    );
    w.put("app/node_modules/.pnpm/react/index.js", "\n");
    let out = create(&app);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        std::fs::read_link(printed(&out).join("node_modules")).unwrap(),
        app.join("node_modules")
    );
}

#[test]
fn vendor_is_a_real_folder() {
    let w = Scratch::new("wt-composer");
    let main = repo(&w, "repo");
    w.put("repo/.gitignore", "vendor\n");
    w.put("repo/composer.json", "{}\n");
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "Composer"]);
    w.put("repo/vendor/autoload.php", "<?php\n");
    let out = create(&main);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let dir = printed(&out);
    assert!(
        std::fs::symlink_metadata(dir.join("vendor"))
            .unwrap()
            .is_dir()
    );
    assert!(dir.join("vendor/autoload.php").exists());
}

#[test]
fn a_repo_with_no_installs_gets_none() {
    let w = Scratch::new("wt-bare");
    let main = repo(&w, "repo");
    w.put("repo/package.json", "{}\n");
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "Package"]);
    let out = create(&main);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let dir = printed(&out);
    assert!(std::fs::symlink_metadata(dir.join("node_modules")).is_err());
    assert!(std::fs::symlink_metadata(dir.join("vendor")).is_err());
}
