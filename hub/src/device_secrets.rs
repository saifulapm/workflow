//! `device with-secrets <project> -- <command>`: the pass entries a project's
//! `cloud-secrets` names, read here and handed to the thread, which sets them
//! for that one command. A theme's token and store password are the only
//! project secrets a cloud command needs; the key counts only when one of
//! the devices wrote it, and no value is printed or logged here.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::device::{DEVICES, Outcome, find_in, user_command, user_env};
use crate::memcli::MemCli;
use crate::proc::{Ended, output_within};

/// The answer the thread gets when pass cannot read without its passphrase.
pub(crate) const LOCKED: i32 = 7;

/// pass asks gpg-agent, which answers at once for a cached key.
const PASS_TIMEOUT: Duration = Duration::from_secs(20);

/// The entries of `project`'s `cloud-secrets`, by the name each is set as,
/// or the answer that says why there are none.
pub(crate) fn call(machine: &str, project: &str) -> (Outcome, BTreeMap<String, String>) {
    match read(machine, project) {
        Ok(secrets) => (Outcome::done(String::new()), secrets),
        Err(outcome) => (outcome, BTreeMap::new()),
    }
}

fn read(machine: &str, project: &str) -> Result<BTreeMap<String, String>, Outcome> {
    let rows = MemCli::new().read(&["projects", "--json"]).rows("projects");
    let row = rows
        .iter()
        .find(|row| row["name"].as_str() == Some(project));
    let Some(lines) = row.and_then(|row| row["cloud_secrets"].as_str()) else {
        return Err(Outcome::refuse(format!("{project} has no cloud-secrets")));
    };
    let by = row.and_then(|row| row["cloud_secrets_by"].as_str());
    if !by.is_some_and(|by| DEVICES.contains(&by)) {
        return Err(Outcome::refuse(format!(
            "cloud-secrets of {project} was not set from a device"
        )));
    }
    let env = user_env();
    let var = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let path = var("PATH").unwrap_or_default();
    let Some(pass) = find_in(&path, "pass") else {
        return Err(Outcome::fail(
            127,
            format!("device: pass was not found on {machine}\n"),
        ));
    };
    let store = var("PASSWORD_STORE_DIR")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".password-store")))
        .unwrap_or_default();
    let mut secrets = BTreeMap::new();
    for line in lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some((name, entry)) = line.split_once('=') else {
            return Err(Outcome::refuse(format!(
                "cloud-secrets of {project} has a line that is not NAME=<pass entry>"
            )));
        };
        // Its file first: a missing entry and a locked key both fail in
        // pass, and only the file tells them apart.
        if entry.contains("..") || !store.join(format!("{entry}.gpg")).is_file() {
            return Err(Outcome::refuse(format!(
                "no pass entry {entry} on {machine}"
            )));
        }
        let mut command = user_command(&pass, &env, false);
        // A key that is not cached fails at once rather than opening a
        // pinentry on the desktop, which no one may be at.
        command
            .env("PASSWORD_STORE_GPG_OPTS", "--pinentry-mode=error")
            .args(["show", entry]);
        let done = match output_within(&mut command, PASS_TIMEOUT) {
            Ended::Exited(done) => done,
            Ended::TimedOut => {
                return Err(Outcome::fail(
                    124,
                    format!("device: pass took too long on {machine}\n"),
                ));
            }
            Ended::Failed(e) => {
                return Err(Outcome::fail(
                    126,
                    format!("device: pass did not start on {machine}: {e}\n"),
                ));
            }
        };
        if done.code != Some(0) {
            let locked = String::from_utf8_lossy(&done.stderr).contains("No pinentry");
            return Err(if locked {
                Outcome::fail(LOCKED, format!("device: pass is locked on {machine}\n"))
            } else {
                Outcome::fail(
                    1,
                    format!("device: pass could not read {entry} on {machine}\n"),
                )
            });
        }
        // The password is the first line; pass keeps notes under it.
        let value = String::from_utf8_lossy(&done.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        secrets.insert(name.to_string(), value);
    }
    Ok(secrets)
}
