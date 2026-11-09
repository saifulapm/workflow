//! `workflow shopify theme-dev <theme>`: Shopify CLI on a cloud thread, with
//! the theme's own variables from the Project's environment.
//!
//! One environment holds every theme's secrets, so each is named after its
//! theme: `<THEME>_SHOPIFY_CLI_THEME_TOKEN`, `<THEME>_SHOPIFY_FLAG_STORE` and
//! `<THEME>_STORE_PASSWORD`, `<THEME>` in capitals with `-` as `_`. This
//! command hands them to the CLI under its own names. A name that is not set,
//! or a token Shopify refuses, exits 3: only Saiful can make a token, so the
//! task parks on it.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::exit;

/// A name that is not set, or a token Shopify refused.
pub const NEEDS_SAIFUL: i32 = 3;

/// Where theme dev serves: the CLI's own default host, on its usual port.
const HOST: &str = "127.0.0.1";
const PORT: &str = "9292";

fn say(msg: &str) {
    eprintln!("shopify: {msg}");
}

/// `pawsome` is `PAWSOME`, `lareys-trendsi` is `LAREYS_TRENDSI`.
fn prefix(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `<kind>/<name>` in the current folder or the nearest folder above it
/// that has one, so a thread may stand at the repo's top or inside the
/// theme.
fn folder(kind: &str, name: &str) -> Option<PathBuf> {
    let here = std::env::current_dir().ok()?;
    let rel = Path::new(kind).join(name);
    here.ancestors()
        .find_map(|dir| Some(dir.join(&rel)).filter(|p| p.is_dir()))
}

/// The value of `<prefix>_<what>`, or the line that says it is missing.
fn named(theme: &str, what: &str) -> Result<String, i32> {
    let name = format!("{}_{what}", prefix(theme));
    var(&name).ok_or_else(|| {
        say(&format!(
            "{what} is not set for {theme}: add {name} to the Project's environment"
        ));
        NEEDS_SAIFUL
    })
}

pub fn cmd_shopify(verb: &str, name: &str) -> i32 {
    match verb {
        "theme-dev" => theme_dev(name),
        _ => {
            say(&format!("unknown verb {verb}"));
            exit::USAGE
        }
    }
}

/// The name of the unpublished theme a cloud thread's theme dev runs on. A
/// fresh VM has no development theme of its own and would make another each
/// time; this one is made once, and theme dev syncs it whole from the
/// checkout at every start, so nothing an earlier VM left on it stays.
fn cloud_theme(theme: &str) -> String {
    format!("{theme} (cloud)")
}

fn theme_dev(theme: &str) -> i32 {
    let Some(dir) = folder("themes", theme) else {
        say(&format!(
            "no themes/{theme} in this folder or one above it: run this in the themes repo"
        ));
        return exit::USAGE;
    };
    let token = match named(theme, "SHOPIFY_CLI_THEME_TOKEN") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let store = match named(theme, "SHOPIFY_FLAG_STORE") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let password = var(&format!("{}_STORE_PASSWORD", prefix(theme)));
    let cli = |args: &[&str]| {
        let mut cmd = Command::new("shopify");
        cmd.args(args)
            .arg("--no-input")
            .arg("--path")
            .arg(&dir)
            .env("SHOPIFY_CLI_THEME_TOKEN", &token)
            .env("SHOPIFY_FLAG_STORE", &store);
        match &password {
            Some(p) => cmd.env("SHOPIFY_FLAG_STORE_PASSWORD", p),
            None => cmd.env_remove("SHOPIFY_FLAG_STORE_PASSWORD"),
        };
        cmd
    };
    let name = cloud_theme(theme);
    let refused = |out: &str| {
        let message = error_message(out);
        if message.contains("Theme Access password was rejected") {
            say(&format!(
                "{}_SHOPIFY_CLI_THEME_TOKEN was rejected by {store}: make a new Theme Access password on that store ({message})",
                prefix(theme)
            ));
            NEEDS_SAIFUL
        } else {
            say(&format!("shopify theme failed on {store}: {message}"));
            exit::FAILED
        }
    };
    let listed = match run(cli(&["theme", "list", "--name", &name, "--json"])) {
        Ok(out) => out,
        Err(out) => return refused(&out),
    };
    let id = match find(&listed, &name) {
        Some(id) => id,
        None => {
            let pushed = match run(cli(&[
                "theme",
                "push",
                "--unpublished",
                "--theme",
                &name,
                "--json",
            ])) {
                Ok(out) => out,
                Err(out) => return refused(&out),
            };
            match serde_json::from_str::<Value>(&pushed)
                .ok()
                .and_then(|v| v["theme"]["id"].as_str().map(str::to_string))
            {
                Some(id) => id,
                None => {
                    say(&format!("theme push gave no theme id: {pushed}"));
                    return exit::FAILED;
                }
            }
        }
    };
    say(&format!(
        "{theme} on {store}, theme {id} ({name}), at http://{HOST}:{PORT}"
    ));
    let err = cli(&[
        "theme", "dev", "--theme", &id, "--host", HOST, "--port", PORT,
    ])
    .exec();
    say(&format!("cannot run shopify: {err}"));
    exit::FAILED
}

/// The CLI's stdout when it exits 0, else everything it printed.
fn run(mut cmd: Command) -> Result<String, String> {
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => Err(format!("cannot run shopify: {e}")),
    }
}

/// The id of the theme named exactly `name` in `theme list --json`.
fn find(listed: &str, name: &str) -> Option<String> {
    let v: Value = serde_json::from_str(listed).ok()?;
    v["themes"].as_array()?.iter().find_map(|t| {
        (t["name"].as_str() == Some(name))
            .then(|| t["id"].as_str().map(str::to_string))
            .flatten()
    })
}

/// A `--json` error's message, else the output as it came.
fn error_message(out: &str) -> String {
    serde_json::from_str::<Value>(out.trim())
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| out.trim().to_string())
}
