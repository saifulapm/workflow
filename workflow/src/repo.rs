//! Standing where the hooks stand.

use std::path::{Path, PathBuf};

use crate::gitcmd::Git;
use crate::{paths, warn};

/// The toplevel, and a git that will keep answering correctly from there.
///
/// `GIT_INDEX_FILE` may be relative -- a partial commit's temporary index is --
/// so the git environment is made absolute before any move, and no move happens
/// at all when we are already at the toplevel (spec §7).
pub fn goto_toplevel() -> Option<(Git, PathBuf)> {
    let top = Git::here().toplevel()?;
    let mut git = Git::here();

    let here = paths::realpath(paths::cwd()).unwrap_or_default();
    let there = paths::realpath(&top).unwrap_or_default();
    if here != there {
        for key in ["GIT_INDEX_FILE", "GIT_DIR"] {
            if let Ok(v) = std::env::var(key)
                && !v.is_empty()
            {
                git = git.with_env(key, paths::realpath_m(&v).to_string_lossy().to_string());
            }
        }
        std::env::set_current_dir(&top).ok()?;
    }
    Some((git, top))
}

/// Check a worktree's submodules out from the main checkout's own, with no
/// network: `git worktree add` leaves every submodule directory empty, and
/// the upstream a `.gitmodules` names may be private, offline or gone
/// (friction #H8QF4ES3). A submodule the main checkout never initialised
/// fails here like any other, with a warning; nothing about a dispatch
/// waits on it.
pub fn submodules(main: &Path, wt: &Path) {
    if !wt.join(".gitmodules").is_file() {
        return;
    }
    let git = Git::at(wt);
    let listed = git
        .out(&[
            "config",
            "-f",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ])
        .unwrap_or_default();
    for line in listed.lines() {
        let Some((key, path)) = line.split_once(' ') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("submodule.")
            .and_then(|k| k.strip_suffix(".path"))
        else {
            continue;
        };
        let url = format!("submodule.{name}.url={}", main.join(path).display());
        let ok = git.quiet(&[
            "-c",
            "protocol.file.allow=always",
            "-c",
            &url,
            "submodule",
            "update",
            "--init",
            "--recursive",
            "--",
            path,
        ]);
        if !ok {
            warn(format!(
                "submodule {path} could not be checked out in {} from {}",
                wt.display(),
                main.join(path).display()
            ));
        }
    }
}
