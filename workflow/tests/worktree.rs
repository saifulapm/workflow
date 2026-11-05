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
    assert!(
        has_branch(&main, &format!("worktree-{NAME}")),
        "a branch it found stays"
    );
}

#[test]
fn a_failed_add_takes_back_the_branch_it_made() {
    let w = Scratch::new("wt-create-stray");
    let main = repo(&w, "repo");
    w.put(&format!("repo/.claude/worktrees/{NAME}/stray.txt"), "x\n");
    let out = create(&main);
    assert_ne!(out.status.code(), Some(0), "{out:?}");
    assert!(!has_branch(&main, &format!("worktree-{NAME}")));
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

#[test]
fn generated_types_come_with_the_trunks_bytes() {
    let w = Scratch::new("wt-types");
    let main = pnpm_repo(&w, "repo");
    w.put("repo/apps/admin/package.json", "{}\n");
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "Admin"]);
    w.put("repo/apps/web/.react-router/types/+routes.ts", "routes\n");
    w.put("repo/apps/web/worker-configuration.d.ts", "worker\n");
    w.put("repo/apps/admin/.cloudflare/types/env.d.ts", "env\n");
    let out = create(&main);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let dir = printed(&out);
    let read = |rel: &str| std::fs::read_to_string(dir.join(rel)).unwrap();
    assert_eq!(read("apps/web/.react-router/types/+routes.ts"), "routes\n");
    assert_eq!(read("apps/web/worker-configuration.d.ts"), "worker\n");
    assert_eq!(read("apps/admin/.cloudflare/types/env.d.ts"), "env\n");
}

#[test]
fn a_tracked_file_of_the_same_name_is_never_overwritten() {
    let w = Scratch::new("wt-types-tracked");
    let main = repo(&w, "repo");
    w.put("repo/package.json", "{}\n");
    w.put("repo/worker-configuration.d.ts", "tracked\n");
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "Worker types"]);
    w.put("repo/worker-configuration.d.ts", "regenerated\n");
    let out = create(&main);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let dir = printed(&out);
    assert_eq!(
        std::fs::read_to_string(dir.join("worker-configuration.d.ts")).unwrap(),
        "tracked\n"
    );
}

fn remove(dir: &Path) -> Output {
    let input = serde_json::json!({
        "hook_event_name": "WorktreeRemove",
        "cwd": dir,
        "worktree_path": dir,
    });
    hook("remove", &input.to_string())
}

fn has_branch(repo: &Path, name: &str) -> bool {
    !git(repo, &["branch", "--list", name]).is_empty()
}

#[test]
fn a_worktree_holding_no_work_goes_with_its_branch_and_the_trunk_keeps_its_installs() {
    let w = Scratch::new("wt-remove");
    let main = pnpm_repo(&w, "repo");
    let dir = printed(&create(&main));
    let out = remove(&dir);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!dir.exists());
    assert!(!has_branch(&main, &format!("worktree-{NAME}")));
    assert!(main.join("node_modules/.pnpm/react/index.js").exists());
}

#[test]
fn a_worktree_with_uncommitted_work_stays_and_says_so() {
    for (tag, rel, text) in [
        ("untracked", "new.txt", "wip\n"),
        ("modified", "a.txt", "changed\n"),
    ] {
        let w = Scratch::new(&format!("wt-remove-{tag}"));
        let main = repo(&w, "repo");
        let dir = printed(&create(&main));
        std::fs::write(dir.join(rel), text).unwrap();
        let out = remove(&dir);
        assert_ne!(out.status.code(), Some(0), "{tag}: {out:?}");
        assert!(dir.join(rel).exists(), "{tag}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("kept") && err.contains(&*dir.to_string_lossy()),
            "{tag}: {err}"
        );
        assert!(has_branch(&main, &format!("worktree-{NAME}")), "{tag}");
    }
}

#[test]
fn a_branch_with_commits_no_other_branch_has_stays_and_says_so() {
    let w = Scratch::new("wt-remove-commit");
    let main = repo(&w, "repo");
    let dir = printed(&create(&main));
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();
    git(&dir, &["add", "b.txt"]);
    git(&dir, &["commit", "-qm", "Second file"]);
    let out = remove(&dir);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!dir.exists());
    let branch = format!("worktree-{NAME}");
    assert!(has_branch(&main, &branch));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&branch), "{err}");
}

/// The command of the cloud kit's one entry for a worktree hook event.
fn entry(event: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cloud/settings-hooks.json");
    let hooks: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let entries = hooks[event].as_array().expect("an entry list");
    assert_eq!(entries.len(), 1, "{hooks}");
    entries[0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The entry run by sh in `cwd`, with `bin` first on a PATH that has no
/// workflow of its own.
fn run_entry(event: &str, bin: &Path, cwd: &Path, input: &str) -> Output {
    let mut child = Command::new("/bin/sh")
        .args(["-c", &entry(event)])
        .current_dir(cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
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

#[test]
fn the_hook_entries_make_and_remove_a_worktree_with_any_workflow() {
    assert!(
        !Path::new("/usr/bin/workflow").exists() && !Path::new("/bin/workflow").exists(),
        "the test's PATH must hold no workflow"
    );
    let w = Scratch::new("wt-entry");
    let real = w.mkdir("real");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_workflow"), real.join("workflow")).unwrap();
    let missing = w.mkdir("missing");
    // A workflow from before the worktree command: clap refuses it, exit 2.
    let old = w.mkdir("old");
    w.script(
        "old/workflow",
        "echo \"error: unrecognized subcommand '$1'\" >&2; exit 2",
    );
    for (tag, bin) in [("real", &real), ("missing", &missing), ("old", &old)] {
        let main = pnpm_repo(&w, &format!("{tag}-repo"));
        let mut input: Value =
            serde_json::from_str(include_str!("fixtures/worktree-create.json")).unwrap();
        input["cwd"] = main.to_str().unwrap().into();
        let out = run_entry("WorktreeCreate", bin, &main, &input.to_string());
        assert_eq!(out.status.code(), Some(0), "{tag}: {out:?}");
        let dir = printed(&out);
        assert_eq!(dir, main.join(".claude/worktrees").join(NAME), "{tag}");
        assert_eq!(
            git(&dir, &["branch", "--show-current"]),
            format!("worktree-{NAME}")
        );

        let input = serde_json::json!({
            "hook_event_name": "WorktreeRemove",
            "cwd": main,
            "worktree_path": dir,
        });
        let out = run_entry("WorktreeRemove", bin, &main, &input.to_string());
        assert_eq!(out.status.code(), Some(0), "{tag}: {out:?}");
        assert!(!dir.exists(), "{tag}");
    }
}
