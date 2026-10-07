//! `workflow install` through the real binary, with `$HOME` pointed at a
//! scratch directory.

#[path = "../src/scratch.rs"]
mod scratch;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use scratch::Scratch;

/// `workflow install` with `$HOME` set to the scratch directory.
fn install(home: &Scratch) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("install")
        .env("HOME", home.path())
        .output()
        .unwrap()
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn the_repo_files_land_in_home_and_a_second_run_has_nothing_to_do() {
    let home = Scratch::new("install-lands");

    let first = install(&home);
    assert!(first.status.success(), "{first:?}");
    let said = String::from_utf8_lossy(&first.stdout);
    assert!(
        said.contains("wrote ~/.claude/skills/mem/SKILL.md"),
        "{said}"
    );
    assert!(
        said.contains("wrote ~/.agents/skills/mem/SKILL.md"),
        "{said}"
    );
    assert!(
        said.contains("wrote ~/.config/git/hooks/pre-commit"),
        "{said}"
    );
    assert!(said.contains(" removed, 0 unchanged"), "{said}");

    for (from, to) in [
        ("skills/mem/SKILL.md", ".claude/skills/mem/SKILL.md"),
        ("skills/mem/SKILL.md", ".agents/skills/mem/SKILL.md"),
        ("hooks/pre-commit", ".config/git/hooks/pre-commit"),
        ("hooks/commit-msg", ".config/git/hooks/commit-msg"),
        ("hooks/pre-push", ".config/git/hooks/pre-push"),
    ] {
        assert_eq!(
            fs::read(repo().join(from)).unwrap(),
            fs::read(home.at(to)).unwrap(),
            "{to}"
        );
    }
    let hook = fs::metadata(home.at(".config/git/hooks/pre-commit")).unwrap();
    assert_eq!(hook.permissions().mode() & 0o777, 0o755);

    let second = install(&home);
    assert!(second.status.success(), "{second:?}");
    let said = String::from_utf8_lossy(&second.stdout);
    assert!(
        said.starts_with("installed: 0 written, 0 removed, "),
        "{said}"
    );
    assert_eq!(said.lines().count(), 1, "{said}");
}

#[test]
fn without_a_home_it_writes_nothing_and_says_so() {
    let out = Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("install")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("HOME is not set"),
        "{out:?}"
    );
    assert!(out.stdout.is_empty());
}
