//! `hub wall-check`: each tool a project uses prints the same version
//! inside the wall as outside it, in the project's folder.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{TempDir, fixture_bin, hub_bin};

struct Rig {
    home: PathBuf,
    bin: PathBuf,
    _dir: TempDir,
}

/// A device whose user PATH has `tools` (name, script) in a folder of the
/// home, which the wall shows, and `outside` in a folder it does not; shop's
/// checkout is a repo of its own.
fn rig(tools: &[(&str, &str)], outside: &[(&str, &str)]) -> Rig {
    let dir = TempDir::new("wall-check");
    let home = dir.join("home");
    let bin = dir.join("bin");
    let shown = home.join(".local/bin");
    let hidden = dir.join("outside");
    for (name, body) in tools {
        fixture_bin(&shown, name, body);
    }
    for (name, body) in outside {
        fixture_bin(&hidden, name, body);
    }
    std::fs::create_dir_all(&shown).unwrap();
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    let path = format!("{}:{}:/usr/bin:/bin", shown.display(), hidden.display());
    fixture_bin(&bin, "systemctl", &format!("echo 'PATH={path}'"));
    let repo = dir.join("shop");
    std::fs::create_dir_all(&repo).unwrap();
    let init = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["-c", "core.hooksPath=/dev/null", "init", "-q"])
        .status();
    assert!(init.unwrap().success());
    let projects = serde_json::json!({"projects": [
        {"name": "shop", "remote": "github.com/x/shop", "checkouts": [repo]},
        {"name": "elsewhere", "remote": "github.com/x/gone", "checkouts": ["/nowhere"]},
        {"name": "notes", "remote": null, "checkouts": [repo]},
    ]});
    std::fs::write(dir.join("projects.json"), projects.to_string()).unwrap();
    fixture_bin(
        &bin,
        "mem",
        &format!("cat '{}'", dir.join("projects.json").display()),
    );
    Rig {
        home,
        bin,
        _dir: dir,
    }
}

/// `hub wall-check`: its exit code and what it printed.
fn check(rig: &Rig) -> (i32, String) {
    let out = Command::new(hub_bin())
        .arg("wall-check")
        .env_clear()
        .env("HOME", &rig.home)
        .env("PATH", format!("{}:/usr/bin:/bin", rig.bin.display()))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.is_empty() || out.status.code() == Some(1),
        "{stderr}"
    );
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn a_tool_that_prints_the_same_version_on_both_sides_is_ok() {
    let rig = rig(&[("node", "echo v24.21.0")], &[]);
    let (code, out) = check(&rig);
    assert!(out.lines().any(|l| l == "ok: shop node v24.21.0"), "{out}");
    assert_eq!(code, 0, "{out}");
}

#[test]
fn a_tool_that_prints_another_version_inside_differs() {
    // The wall hides ~/Documents, so the fake reads as another version there.
    let pnpm = "if [ -d \"$HOME/Documents\" ]; then echo 10.2.0; else echo 10.1.0; fi";
    let rig = rig(&[("node", "echo v24.21.0"), ("pnpm", pnpm)], &[]);
    let (code, out) = check(&rig);
    assert!(
        out.lines()
            .any(|l| l == "differs: shop pnpm 10.1.0 inside, 10.2.0 outside"),
        "{out}"
    );
    assert_eq!(code, 1, "{out}");
}

#[test]
fn a_tool_the_wall_does_not_show_is_missing() {
    let rig = rig(&[("node", "echo v24.21.0")], &[("cf", "echo 'cf 1.4.0'")]);
    let (code, out) = check(&rig);
    assert!(out.lines().any(|l| l == "missing: shop cf"), "{out}");
    assert_eq!(code, 1, "{out}");
}

#[test]
fn only_projects_with_a_remote_and_a_checkout_here_are_checked() {
    let rig = rig(&[("node", "echo v24.21.0")], &[]);
    let (_, out) = check(&rig);
    for project in ["elsewhere", "notes"] {
        assert!(!out.contains(&format!(" {project} ")), "{project}: {out}");
    }
}

#[test]
fn a_version_is_the_first_word_that_looks_like_one() {
    let composer = "echo 'Composer version 2.8.4 2024-12-11 10:57:47'";
    let rig = rig(&[("node", "echo v24.21.0"), ("composer", composer)], &[]);
    let (_, out) = check(&rig);
    assert!(out.lines().any(|l| l == "ok: shop composer 2.8.4"), "{out}");
}

/// mise's settings are a link into the dotfiles: the wall shows the file
/// the link leads to, so a tool that reads them prints the same inside.
#[test]
fn a_tool_whose_settings_are_a_link_elsewhere_reads_them_inside() {
    let node = "cat \"$HOME/.config/mise/config.toml\" 2>/dev/null || echo v24.18.0";
    let rig = rig(&[("node", node)], &[]);
    let dotfiles = rig.home.join("Codes/dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("config.toml"), "v24.21.0\n").unwrap();
    std::fs::create_dir_all(rig.home.join(".config/mise")).unwrap();
    std::os::unix::fs::symlink(
        dotfiles.join("config.toml"),
        rig.home.join(".config/mise/config.toml"),
    )
    .unwrap();
    let (code, out) = check(&rig);
    assert!(out.lines().any(|l| l == "ok: shop node v24.21.0"), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// pnpm keeps its store on the project's own mount, and the wall shows the
/// store and the worktree as two: this pnpm does as pnpm does, so the wall
/// must name the store for it.
#[test]
fn pnpm_s_store_is_the_same_folder_inside_and_outside() {
    let pnpm = "store=\"$HOME/.local/share/pnpm/store\"\n\
                if [ \"$1\" != store ]; then echo 11.17.0; exit; fi\n\
                if [ -n \"$pnpm_config_store_dir\" ]; then echo \"$pnpm_config_store_dir/v11\"\n\
                elif [ \"$(stat -c %m \"$store\")\" = \"$(stat -c %m .)\" ]; then echo \"$store/v11\"\n\
                else echo \"$PWD/.pnpm-store/v11\"; fi";
    let rig = rig(&[("pnpm", pnpm)], &[]);
    std::fs::create_dir_all(rig.home.join(".local/share/pnpm/store")).unwrap();
    let (code, out) = check(&rig);
    let store = rig.home.join(".local/share/pnpm/store/v11");
    let line = format!("ok: shop pnpm-store {}", store.display());
    assert!(out.lines().any(|l| l == line), "{line}\n{out}");
    assert_eq!(code, 0, "{out}");
}
