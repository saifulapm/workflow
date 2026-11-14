//! The folder a thread's tmux session works in: a git worktree of the
//! project's repo at `~/.cache/workflow-cloud/<repo>`, detached at origin's
//! `cloud/<project>-<milestone>`, one per repo on this device and kept
//! between sessions, so an untracked node_modules/ stays.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::device::{HELPER_TIMEOUT, Outcome, find_in, user_command, user_env};
use crate::memcli::MemCli;
use crate::proc::{Ended, Finished, output_within};

/// How long a fetch of the branch may take; the call has 90 s in all.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// When the worktree must be checked out by, from the fetch's start.
const CHECKOUT_BY: Duration = Duration::from_secs(80);

pub(crate) struct Worktree {
    pub(crate) folder: PathBuf,
    /// Saiful's own checkout of the repo, which the worktree was made from.
    pub(crate) checkout: PathBuf,
    /// What the hub does with the repo's submodules before the session.
    pub(crate) submodules: Submodules,
}

/// Fetches the branch of session `name` (`cloud-<project>-<milestone>`) into
/// the checkout of `project`'s repo on `machine`, and puts the worktree at
/// its tip. The checkout keeps its own branch, index and files.
pub(crate) fn prepare(machine: &str, project: &str, name: &str) -> Result<Worktree, Outcome> {
    let rows = MemCli::new().read(&["projects", "--json"]).rows("projects");
    let row = rows
        .iter()
        .find(|row| row["name"].as_str() == Some(project));
    let Some(remote) = row.and_then(|row| row["remote"].as_str()) else {
        return Err(Outcome::refuse(format!("{project} has no remote")));
    };
    let slug = remote.split_once('/').map_or(remote, |(_, path)| path);
    let repo = remote.rsplit('/').next().unwrap_or(remote);
    // The folder's name, so never one that leaves ~/.cache/workflow-cloud.
    let named = !repo.is_empty()
        && repo != "."
        && repo != ".."
        && repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if !named {
        return Err(Outcome::refuse(format!("{remote} cannot name a folder")));
    }
    let git = Git::here()?;
    // The project's own checkouts first, then any of a project of the same repo.
    let owners = row
        .into_iter()
        .chain(rows.iter().filter(|r| r["remote"].as_str() == Some(remote)));
    let top = owners
        .flat_map(|row| row["checkouts"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c.as_str().map(PathBuf::from))
        .find_map(|c| {
            let done = git
                .run(&c, &["rev-parse", "--show-toplevel"], HELPER_TIMEOUT)
                .ok()?;
            (done.code == Some(0))
                .then(|| PathBuf::from(String::from_utf8_lossy(&done.stdout).trim()))
        });
    let Some(top) = top else {
        return Err(Outcome::refuse(format!(
            "no checkout of {slug} on {machine}"
        )));
    };
    let branch = format!("cloud/{}", name.strip_prefix("cloud-").unwrap_or(name));
    let tracking = format!("refs/remotes/origin/{branch}");
    let refspec = format!("+refs/heads/{branch}:{tracking}");
    let started = Instant::now();
    let fetched = git.run(&top, &["fetch", "-q", "origin", &refspec], FETCH_TIMEOUT)?;
    if fetched.code != Some(0) {
        // Told apart by ls-remote, not by fetch's message, which may be in
        // the user's language. In what is left of the fetch's minute.
        let listed = git.run(
            &top,
            &[
                "ls-remote",
                "--exit-code",
                "origin",
                &format!("refs/heads/{branch}"),
            ],
            FETCH_TIMEOUT.saturating_sub(started.elapsed()),
        );
        if listed.is_ok_and(|listed| listed.code == Some(2)) {
            return Err(Outcome::refuse(format!(
                "no branch {branch} on origin of {slug}"
            )));
        }
        return Err(failed("fetch", &fetched));
    }
    let sha = git.ok(&top, &["rev-parse", &format!("{tracking}^{{commit}}")])?;
    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();
    let Some(home) = std::env::var_os("HOME") else {
        return Err(Outcome::fail(126, "device: HOME is not set\n".to_string()));
    };
    let folder = PathBuf::from(home).join(".cache/workflow-cloud").join(repo);
    // A folder with no .git is not the worktree: git in it would reach up
    // to whatever repo holds it.
    // A big checkout can take more than a helper's seconds, and git killed
    // mid-checkout leaves its index locked; whatever the fetch left of the
    // call's time.
    let left = CHECKOUT_BY.saturating_sub(started.elapsed());
    if folder.join(".git").exists() {
        git.ok_within(
            &folder,
            &["checkout", "-q", "--force", "--detach", &sha],
            left,
        )?;
    } else {
        // -f: a folder deleted by hand is still on the checkout's list. Not
        // a prune, which would drop any other missing worktree of Saiful's.
        let to = folder.to_string_lossy();
        git.ok_within(
            &top,
            &["worktree", "add", "-f", "-q", "--detach", &to, &sha],
            left,
        )?;
    }
    used(repo, &top);
    let submodules = match std::fs::read(folder.join(".gitmodules")) {
        Err(_) => Submodules::None,
        Ok(listed) => {
            unplant(&git, &folder)?;
            // The update runs with Saiful's logins: only on the URLs his own
            // checkout names, never ones a thread's branch chose.
            if std::fs::read(top.join(".gitmodules")).is_ok_and(|own| own == listed) {
                Submodules::Update
            } else {
                Submodules::Differ
            }
        }
    };
    Ok(Worktree {
        submodules,
        folder,
        checkout: top,
    })
}

/// What the hub does with a worktree's submodules before its session.
#[derive(PartialEq)]
pub(crate) enum Submodules {
    None,
    /// Updates them outside the wall, in the pane, where a first clone may
    /// take longer than a call.
    Update,
    /// Leaves them: the branch's .gitmodules is not the checkout's.
    Differ,
}

/// Removes the folder of each submodule the worktree's commit names, so
/// the hub's update, outside the wall, makes it again from the git folder
/// the wall keeps read-only, never through a .git file a session wrote.
fn unplant(git: &Git, folder: &Path) -> Result<(), Outcome> {
    let listed = git.ok(folder, &["ls-files", "--stage", "-z"])?;
    for entry in listed.stdout.split(|b| *b == 0) {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let inside = Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if meta.starts_with("160000 ") && inside {
            remove_inside(folder, Path::new(path));
        }
    }
    Ok(())
}

/// Removes `path` under `folder`, never what a link on the way leads to:
/// a link a session made is removed itself, and the walk stops there.
fn remove_inside(folder: &Path, path: &Path) {
    let mut at = folder.to_path_buf();
    for part in path.components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Err(_) => return,
            Ok(meta) if meta.file_type().is_symlink() => {
                let _ = std::fs::remove_file(&at);
                return;
            }
            Ok(_) => {}
        }
    }
    let _ = std::fs::remove_dir_all(&at);
}

/// Under the state folder, one file a worktree, named for it: the checkout
/// it was made from, written each time a session starts in it.
const USED: &str = "hub/device-worktrees";

/// How long a worktree no session starts in is kept.
const DEFAULT_UNUSED: Duration = Duration::from_secs(14 * 24 * 60 * 60);

fn used(repo: &str, top: &Path) {
    let Ok(dir) = crate::config::state_home().map(|home| home.join(USED)) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Err(e) = std::fs::write(dir.join(repo), top.to_string_lossy().as_bytes()) {
        eprintln!("hub: device: cannot mark the worktree {repo} used: {e}");
    }
}

/// Removes each worktree no session has started in for the limit
/// (`HUB_DEVICE_WORKTREE_UNUSED_MS`, 14 days by default), and takes it off
/// its checkout's list. Called by the tmux sweep only while no job runs, so
/// never under a live session.
pub(crate) fn sweep() {
    let limit = std::env::var("HUB_DEVICE_WORKTREE_UNUSED_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(DEFAULT_UNUSED, Duration::from_millis);
    let Ok(dir) = crate::config::state_home().map(|home| home.join(USED)) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    for entry in entries.flatten() {
        let unused = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age >= limit);
        if !unused {
            continue;
        }
        let repo = entry.file_name();
        let folder = PathBuf::from(&home)
            .join(".cache/workflow-cloud")
            .join(&repo);
        // Only a linked worktree, whose .git is a file, never a checkout.
        // git removes it and its entry; one with submodules it refuses, so
        // that goes by hand and its entry waits for the next add's -f.
        if folder.join(".git").is_file() {
            let removed = std::fs::read_to_string(entry.path())
                .ok()
                .zip(Git::here().ok())
                .is_some_and(|(top, git)| {
                    let to = folder.to_string_lossy();
                    git.run(
                        Path::new(top.trim()),
                        &["worktree", "remove", "--force", "--force", &to],
                        FETCH_TIMEOUT,
                    )
                    .is_ok_and(|done| done.code == Some(0))
                });
            if !removed && let Err(e) = std::fs::remove_dir_all(&folder) {
                eprintln!("hub: device: cannot remove {}: {e}", folder.display());
                continue;
            }
        }
        let _ = std::fs::remove_file(entry.path());
        eprintln!(
            "hub: device: removed the unused worktree {}",
            folder.display()
        );
    }
}

/// git on the user manager's PATH, with its env, which holds the user's
/// tokens and not the hub's.
struct Git {
    bin: PathBuf,
    env: Vec<(String, String)>,
}

impl Git {
    fn here() -> Result<Git, Outcome> {
        let env = user_env();
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let Some(bin) = find_in(&path, "git") else {
            return Err(Outcome::fail(
                127,
                "device: git was not found\n".to_string(),
            ));
        };
        Ok(Git { bin, env })
    }

    fn run(&self, dir: &Path, args: &[&str], within: Duration) -> Result<Finished, Outcome> {
        let mut command = user_command(&self.bin, &self.env, false);
        command.arg("-C").arg(dir).args(args);
        match output_within(&mut command, within) {
            Ended::Exited(done) => Ok(done),
            Ended::TimedOut => Err(Outcome::fail(
                124,
                format!("device: git {} took too long\n", args[0]),
            )),
            Ended::Failed(e) => Err(Outcome::fail(
                126,
                format!("device: git did not start: {e}\n"),
            )),
        }
    }

    /// `run` in HELPER_TIMEOUT, failing unless git exited 0.
    fn ok(&self, dir: &Path, args: &[&str]) -> Result<Finished, Outcome> {
        self.ok_within(dir, args, HELPER_TIMEOUT)
    }

    /// `run` in `within`, failing unless git exited 0.
    fn ok_within(&self, dir: &Path, args: &[&str], within: Duration) -> Result<Finished, Outcome> {
        let done = self.run(dir, args, within)?;
        if done.code == Some(0) {
            Ok(done)
        } else {
            Err(failed(args[0], &done))
        }
    }
}

fn failed(what: &str, done: &Finished) -> Outcome {
    Outcome::fail(
        done.code.unwrap_or(128),
        format!(
            "device: git {what} failed: {}",
            String::from_utf8_lossy(&done.stderr)
        ),
    )
}
