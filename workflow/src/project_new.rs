//! `workflow project-new <project>`: the few steps that put a project into
//! a Claude Project on the work and the main account, which only Saiful can
//! take on claude.ai, and one line for each thing a machine can check. It
//! changes nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::memcli::{self, ProjectRow};
use crate::{cloud, device, exit, machine, warn};

/// The accounts a Project is made on, as the headings name them.
const ACCOUNTS: [&str; 2] = ["work", "main"];

/// `owner/repo` of a normalized remote: `github.com/saifulapm/workflow` is
/// `saifulapm/workflow`, as is an ssh alias host's
/// `github.com-lareys/saifulapm/workflow`.
fn slug(remote: &str) -> String {
    let remote = remote.trim().trim_end_matches('/');
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let mut parts = remote.rsplit(['/', ':']);
    let repo = parts.next().unwrap_or_default();
    match parts.next() {
        Some(owner) => format!("{owner}/{repo}"),
        None => repo.to_string(),
    }
}

/// The repo a `.gitmodules` url names: one relative to the remote, as
/// `../dactyl.git` from `lareysbd/shopify_apps` is `lareysbd/dactyl`, or
/// any absolute url's last two segments.
fn submodule_slug(remote: &str, url: &str) -> String {
    let url = url.trim();
    if !(url.starts_with("../") || url.starts_with("./")) {
        return slug(url);
    }
    let mut path: Vec<String> = slug(remote).split('/').map(str::to_string).collect();
    for part in url.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                path.pop();
            }
            name => path.push(name.strip_suffix(".git").unwrap_or(name).to_string()),
        }
    }
    path.join("/")
}

/// The root of the checkout `dir` of `row`: the folder itself for a root
/// project, the folder above its subdir for a child.
fn top_of(row: &ProjectRow, dir: &str) -> PathBuf {
    let dir = Path::new(dir);
    match &row.subdir {
        Some(sub) => {
            let up = Path::new(sub).components().count();
            dir.ancestors().nth(up).unwrap_or(dir).to_path_buf()
        }
        None => dir.to_path_buf(),
    }
}

/// The urls of the repo's submodules, from the `.gitmodules` of a checkout
/// of it on this machine; `None` when there is no checkout here to read.
fn submodule_urls(repo: &[&ProjectRow]) -> Option<Vec<String>> {
    let top = repo
        .iter()
        .flat_map(|row| row.checkouts.iter().map(move |dir| top_of(row, dir)))
        .find(|top| top.is_dir())?;
    let text = std::fs::read_to_string(top.join(".gitmodules")).unwrap_or_default();
    Some(
        text.lines()
            .filter_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "url").then(|| value.trim().to_string())
            })
            .collect(),
    )
}

/// `a`, `a and b`, `a, b and c`.
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn once_steps(theme: bool) -> Vec<String> {
    let mut steps = vec![
        "Give mem the project's git remote, `mem project set remote` for one registered before it had one, and write the Cloud section of its verify page.".to_string(),
    ];
    if theme {
        steps.push("`pass insert` the theme's token and store password, and list them in its cloud-secrets: `mem project set cloud-secrets`, one NAME=<pass entry> a line, and SHOPIFY_FLAG_STORE=<store>.myshopify.com.".to_string());
    }
    steps.push("For a project tied to one machine, such as a mobile app with its phone on the nuc: `mem project set device <machine>`.".to_string());
    steps
}

fn account_steps(project: &str, repo: &str, submodules: &[String]) -> Vec<String> {
    let mut repos = vec![repo.to_string()];
    repos.extend(submodules.iter().cloned());
    let added = match submodules {
        [] => format!("add the repository {repo}"),
        _ => format!(
            "add the repository {repo} and {}, which it holds as a submodule",
            listed(submodules)
        ),
    };
    vec![
        format!("The Claude GitHub App reaches {}.", listed(&repos)),
        format!("New Project: {added}, and pick the environment `workflow`."),
        format!("Paste the output of `workflow project-text {project}` into Settings, Memory."),
    ]
}

fn print_steps(heading: &str, steps: &[String]) {
    println!("{heading}:");
    for (n, step) in steps.iter().enumerate() {
        println!("  {}. {step}", n + 1);
    }
    println!();
}

/// A theme's store, which `cloud-secrets` names in place of a pass entry.
const STORE: &str = "SHOPIFY_FLAG_STORE";

/// The device `row` runs on: its own, when a device wrote that key, as the
/// hub follows it, else the global choice of this machine's local hub.
fn device_of(row: &ProjectRow) -> Option<String> {
    let own = row.device.as_deref().filter(|device| {
        device::short_name(device).is_some()
            && row
                .device_by
                .as_deref()
                .is_some_and(|by| device::short_name(by).is_some())
    });
    own.map(str::to_string).or_else(device::choice)
}

/// How one device is asked: this machine's own files, or ssh to the
/// device's tailnet name (`WORKFLOW_SSH` in the place of ssh).
enum Device {
    Here,
    Ssh(&'static str),
}

impl Device {
    fn of(machine: &str) -> Device {
        match device::short_name(machine) {
            Some(host) if machine != machine::mem_name() => Device::Ssh(host),
            _ => Device::Here,
        }
    }

    /// `args` run on the device over ssh: its exit code, and its stdout.
    fn ssh(host: &str, args: &[&str]) -> Option<(i32, String)> {
        let program = std::env::var("WORKFLOW_SSH")
            .ok()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "ssh".to_string());
        let out = Command::new(program)
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", host])
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let code = out.status.code()?;
        Some((code, String::from_utf8_lossy(&out.stdout).to_string()))
    }
}

/// Whether `other` is `row`'s project or another project of its repo.
fn same_repo(row: &ProjectRow, other: &ProjectRow) -> bool {
    other.name == row.name || (remote_of(row).is_some() && remote_of(other) == remote_of(row))
}

fn remote_of(row: &ProjectRow) -> Option<&str> {
    row.remote
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
}

/// Whether the device has a checkout of `row`'s repo: its own, or one of
/// another project of that repo, since an app's folder may exist only on its
/// own branch. `here` is this machine's rows; `Err` when the device did not
/// answer.
fn has_checkout(on: &Device, row: &ProjectRow, here: &[ProjectRow]) -> Result<bool, ()> {
    match on {
        Device::Here => Ok(here
            .iter()
            .filter(|other| same_repo(row, other))
            .any(|other| {
                other
                    .checkouts
                    .iter()
                    .any(|dir| top_of(other, dir).is_dir())
            })),
        Device::Ssh(host) => {
            let (code, out) = Device::ssh(host, &["mem", "projects", "--json"]).ok_or(())?;
            let rows = (code == 0)
                .then(|| memcli::parse_projects(&out))
                .flatten()
                .ok_or(())?;
            Ok(rows
                .iter()
                .any(|other| same_repo(row, other) && !other.checkouts.is_empty()))
        }
    }
}

/// A pass entry's name as `cloud-secrets` may hold one, which can go on an
/// ssh command line unquoted.
fn entry_name(entry: &str) -> bool {
    !entry.is_empty()
        && !entry.starts_with(['/', '.', '-'])
        && !entry.split('/').any(|part| part.is_empty() || part == "..")
        && entry
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
}

/// Whether the device's pass has `entry`, by its file: `Err` when it did not
/// answer.
fn has_entry(on: &Device, entry: &str) -> Result<bool, ()> {
    match on {
        Device::Here => {
            let store = std::env::var("PASSWORD_STORE_DIR")
                .ok()
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .or_else(|| crate::paths::home().map(|h| h.join(".password-store")))
                .ok_or(())?;
            Ok(store.join(format!("{entry}.gpg")).is_file())
        }
        Device::Ssh(host) => {
            let file = format!("~/.password-store/{entry}.gpg");
            match Device::ssh(host, &["test", "-f", &file]) {
                Some((0, _)) => Ok(true),
                Some((1, _)) => Ok(false),
                _ => Err(()),
            }
        }
    }
}

/// The check lines for `row`, each `ok: ...` or `missing: ...`.
fn checks(
    row: &ProjectRow,
    projects: &[ProjectRow],
    remote: Option<&str>,
    theme: bool,
) -> Vec<String> {
    let mut lines = vec![match remote {
        Some(remote) => format!("ok: remote {}", slug(remote)),
        None => "missing: remote".to_string(),
    }];
    let Some(machine) = device_of(row) else {
        lines.push("missing: checkout (this machine's local hub names no device)".to_string());
        return lines;
    };
    let on = Device::of(&machine);
    lines.push(match has_checkout(&on, row, projects) {
        Ok(true) => format!("ok: checkout on {machine}"),
        Ok(false) => format!("missing: checkout on {machine}"),
        Err(()) => format!("missing: checkout on {machine} ({machine} did not answer)"),
    });
    if !theme {
        return lines;
    }
    let Some(secrets) = row.cloud_secrets.as_deref() else {
        lines.push("missing: cloud-secrets".to_string());
        return lines;
    };
    for (n, line) in secrets.lines().map(str::trim).enumerate() {
        let Some((name, entry)) = line.split_once('=') else {
            if !line.is_empty() {
                lines.push(format!(
                    "missing: cloud-secrets line {} is not NAME=<pass entry>",
                    n + 1
                ));
            }
            continue;
        };
        if name.trim() == STORE {
            continue;
        }
        let entry = entry.trim();
        if !entry_name(entry) {
            lines.push(format!(
                "missing: cloud-secrets line {} names no pass entry",
                n + 1
            ));
            continue;
        }
        lines.push(match has_entry(&on, entry) {
            Ok(true) => format!("ok: pass entry {entry}"),
            Ok(false) => format!("missing: pass entry {entry}"),
            Err(()) => format!("missing: pass entry {entry} ({machine} did not answer)"),
        });
    }
    lines
}

/// `workflow project-new [<project>]`.
pub fn cmd_project_new(project: Option<&str>) -> i32 {
    let Some(projects) = memcli::projects() else {
        warn("project-new: mem cannot list its projects");
        return exit::FAILED;
    };
    let Some(name) = project
        .map(str::to_string)
        .or_else(|| memcli::project_current().map(|p| p.name))
    else {
        warn("project-new: name a project, or run it in a project's checkout");
        return exit::USAGE;
    };
    let Some(row) = projects.iter().find(|row| row.name == name) else {
        warn(format!("project-new: mem lists no {name}"));
        return exit::USAGE;
    };
    let remote = row.remote.as_deref().filter(|r| !r.trim().is_empty());
    let theme = row
        .subdir
        .as_deref()
        .is_some_and(|sub| sub.starts_with("themes/"));
    // One Project serves a repo, so an app inside one gets no Project of its own.
    let child = remote.filter(|_| row.subdir.is_some());
    if let Some(remote) = child {
        println!(
            "{name} is a mem project of {}: its Project takes the start line \"{}\"",
            slug(remote),
            cloud::start_line(&name)
        );
        println!();
    }
    print_steps("Once, on any device", &once_steps(theme));
    let mut unread = None;
    if let Some(remote) = remote.filter(|_| child.is_none()) {
        let repo: Vec<&ProjectRow> = projects
            .iter()
            .filter(|other| other.remote.as_deref().map(str::trim) == Some(remote.trim()))
            .collect();
        let urls = submodule_urls(&repo);
        if urls.is_none() {
            unread = Some(format!(
                "missing: submodules of {} (no checkout on this machine to read .gitmodules)",
                slug(remote)
            ));
        }
        let submodules: Vec<String> = urls
            .unwrap_or_default()
            .iter()
            .map(|url| submodule_slug(remote, url))
            .collect();
        let steps = account_steps(&name, &slug(remote), &submodules);
        for account in ACCOUNTS {
            print_steps(&format!("On the {account} account"), &steps);
        }
    }
    let mut lines = checks(row, &projects, remote, theme);
    lines.extend(unread);
    for line in &lines {
        println!("{line}");
    }
    if lines.iter().all(|line| line.starts_with("ok: ")) {
        exit::OK
    } else {
        exit::FAILED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_submodule_url_is_resolved_against_the_remote() {
        let remote = "github.com/lareysbd/shopify_apps";
        assert_eq!(submodule_slug(remote, "../dactyl.git"), "lareysbd/dactyl");
        assert_eq!(
            submodule_slug(remote, "../../saifulapm/kit"),
            "saifulapm/kit"
        );
        assert_eq!(
            submodule_slug(remote, "git@github.com:saifulapm/kit.git"),
            "saifulapm/kit"
        );
        assert_eq!(
            submodule_slug(remote, "https://github.com/saifulapm/kit"),
            "saifulapm/kit"
        );
    }

    #[test]
    fn an_alias_host_still_gives_owner_and_repo() {
        assert_eq!(
            slug("github.com-lareys/lareysbd/lareys.dev"),
            "lareysbd/lareys.dev"
        );
    }
}
