//! `workflow project-new <project>`: the few steps that put a project into
//! a Claude Project on the work and the main account, which only Saiful can
//! take on claude.ai, and one line for each thing a machine can check. It
//! changes nothing.

use std::path::{Path, PathBuf};

use crate::memcli::{self, ProjectRow};
use crate::{exit, warn};

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
    print_steps("Once, on any device", &once_steps(theme));
    if let Some(remote) = remote {
        let repo: Vec<&ProjectRow> = projects
            .iter()
            .filter(|other| other.remote.as_deref().map(str::trim) == Some(remote.trim()))
            .collect();
        let submodules: Vec<String> = submodule_urls(&repo)
            .unwrap_or_default()
            .iter()
            .map(|url| submodule_slug(remote, url))
            .collect();
        let steps = account_steps(&name, &slug(remote), &submodules);
        for account in ACCOUNTS {
            print_steps(&format!("On the {account} account"), &steps);
        }
    }
    exit::OK
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
