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

pub(crate) struct Worktree {
    pub(crate) folder: PathBuf,
    /// Whether the repo has submodules, which the session updates itself:
    /// a first clone of one can take longer than a call may.
    pub(crate) submodules: bool,
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
    if folder.join(".git").exists() {
        git.ok(&folder, &["checkout", "-q", "--force", "--detach", &sha])?;
    } else {
        // A folder deleted by hand is still on the checkout's list, and
        // git refuses to add it again until the list is pruned.
        git.ok(&top, &["worktree", "prune"])?;
        let to = folder.to_string_lossy();
        git.ok(&top, &["worktree", "add", "-q", "--detach", &to, &sha])?;
    }
    used(repo, &top);
    Ok(Worktree {
        submodules: folder.join(".gitmodules").exists(),
        folder,
    })
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
        if folder.join(".git").is_file()
            && let Err(e) = std::fs::remove_dir_all(&folder)
        {
            eprintln!("hub: device: cannot remove {}: {e}", folder.display());
            continue;
        }
        if let (Ok(top), Ok(git)) = (std::fs::read_to_string(entry.path()), Git::here()) {
            let _ = git.run(
                Path::new(top.trim()),
                &["worktree", "prune"],
                HELPER_TIMEOUT,
            );
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
        let done = self.run(dir, args, HELPER_TIMEOUT)?;
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
