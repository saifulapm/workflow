//! `workflow shopify theme-dev <theme>` and `workflow shopify deploy <app>`:
//! Shopify CLI on a cloud thread, with the theme's or app's own variables
//! from the Project's environment.
//!
//! One environment holds every theme's and app's secrets, so each is named
//! after its theme or app: `<THEME>_SHOPIFY_CLI_THEME_TOKEN`,
//! `<THEME>_SHOPIFY_FLAG_STORE`, `<THEME>_STORE_PASSWORD` and
//! `<APP>_SHOPIFY_SERVICE_ACCOUNT_TOKEN`, in capitals with `-` as `_`. This
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

/// The variables a project at `subdir` of a Shopify repo reads from its
/// Project's environment: a theme's three under `themes/`, an app's token
/// under `apps/`.
pub fn variable_names(subdir: &str) -> Vec<String> {
    let Some((kind, name)) = subdir.trim_matches('/').split_once('/') else {
        return Vec::new();
    };
    let whats: &[&str] = match kind {
        "themes" => &[
            "SHOPIFY_CLI_THEME_TOKEN",
            "SHOPIFY_FLAG_STORE",
            "STORE_PASSWORD",
        ],
        "apps" => &["SHOPIFY_SERVICE_ACCOUNT_TOKEN"],
        _ => &[],
    };
    whats
        .iter()
        .map(|what| format!("{}_{what}", prefix(name)))
        .collect()
}

/// Whether the checkout `root` holds Shopify `kind`s: `apps/<app>` with its
/// shopify.app.toml, or `themes/<theme>` with its layout/theme.liquid.
pub fn holds(root: &Path, kind: &str) -> bool {
    let file = match kind {
        "apps" => "shopify.app.toml",
        "themes" => "layout/theme.liquid",
        _ => return false,
    };
    std::fs::read_dir(root.join(kind))
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().join(file).is_file())
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

/// A string value from an app config: `key` at the top when `section` is
/// None, else under `[section]`. App configs keep these on one line each.
fn toml_value(text: &str, section: Option<&str>, key: &str) -> Option<String> {
    let mut here: Option<&str> = None;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            here = Some(name.trim());
            continue;
        }
        if here != section {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() == key {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

pub fn cmd_shopify(verb: &str, name: &str) -> i32 {
    match verb {
        "theme-dev" => theme_dev(name),
        "deploy" => deploy(name),
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
    // theme dev syncs the cloud theme whole, deletes included: before the
    // build it would strip the store's copy of every built asset.
    let built = std::fs::read_dir(dir.join("assets")).is_ok_and(|mut d| d.next().is_some());
    if !built {
        say(&format!(
            "themes/{theme}/assets is missing or empty: build the theme first"
        ));
        return exit::USAGE;
    }
    let token = match named(theme, "SHOPIFY_CLI_THEME_TOKEN") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let store = match named(theme, "SHOPIFY_FLAG_STORE") {
        Ok(v) => v,
        Err(code) => return code,
    };
    // Development stores keep a password page, which a walk would judge.
    let password = match named(theme, "STORE_PASSWORD") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let cli = |args: &[&str]| {
        let mut cmd = Command::new("shopify");
        cmd.args(args)
            .arg("--no-input")
            .arg("--path")
            .arg(&dir)
            .env("SHOPIFY_CLI_THEME_TOKEN", &token)
            .env("SHOPIFY_FLAG_STORE", &store)
            .env("SHOPIFY_FLAG_STORE_PASSWORD", &password);
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
            "{}\n{}",
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

/// A `--json` error's message, else the output as it came. The error is one
/// line of stdout; stderr, after it, may add a line of its own.
fn error_message(out: &str) -> String {
    out.lines()
        .find_map(|line| {
            serde_json::from_str::<Value>(line.trim())
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        })
        .unwrap_or_else(|| out.trim().to_string())
}

/// The one config a cloud thread deploys. Production stays Saiful's
/// go-ahead, so no other is ever named.
const STAGING: &str = "staging";

fn deploy(app: &str) -> i32 {
    let Some(dir) = folder("apps", app) else {
        say(&format!(
            "no apps/{app} in this folder or one above it: run this in the apps repo"
        ));
        return exit::USAGE;
    };
    let config = dir.join(format!("shopify.app.{STAGING}.toml"));
    let Ok(staging) = std::fs::read_to_string(&config) else {
        say(&format!(
            "{} does not exist: {app}'s staging app is Saiful's to make and link (shopify app config link --config {STAGING})",
            config.display()
        ));
        return NEEDS_SAIFUL;
    };
    let production = std::fs::read_to_string(dir.join("shopify.app.toml")).unwrap_or_default();
    let id = toml_value(&staging, None, "client_id");
    if id.is_some() && id == toml_value(&production, None, "client_id") {
        say(&format!(
            "{} has production's client_id: a staging deploy would deploy production, which stays Saiful's go-ahead",
            config.display()
        ));
        return NEEDS_SAIFUL;
    }
    // Its application_url, redirect, proxy and pixel URLs alike: any left on
    // production's host sends the staging app's traffic to production.
    if let Some(url) = toml_value(&production, None, "application_url")
        .filter(|url| !url.is_empty() && staging.contains(url.as_str()))
    {
        say(&format!(
            "{} holds the application_url of {}, {url}: the staging app's admin frame, webhooks and pixel would reach production's Worker",
            config.display(),
            dir.join("shopify.app.toml").display(),
        ));
        return NEEDS_SAIFUL;
    }
    let store = toml_value(&staging, Some("build"), "dev_store_url");
    let handle = toml_value(&staging, None, "handle");
    let (Some(store), Some(handle)) = (store, handle) else {
        say(&format!(
            "{} needs handle and [build] dev_store_url (the dev store it is installed on): no admin page to walk",
            config.display()
        ));
        return NEEDS_SAIFUL;
    };
    let name = format!("{}_SHOPIFY_SERVICE_ACCOUNT_TOKEN", prefix(app));
    let Some(token) = var(&name) else {
        say(&format!(
            "{name} is not set: add it to the Project's environment"
        ));
        return NEEDS_SAIFUL;
    };
    let worker = match staging_worker(&dir, app) {
        Ok(worker) => worker,
        Err(code) => return code,
    };
    if let Err(code) = cloudflare_token(&dir) {
        return code;
    }
    if let Err(code) = deploy_worker(&dir, app, &worker) {
        return code;
    }
    // shopify app deploy builds each Rust Function for wasm, a target a
    // fresh thread's toolchain lacks; the release adds it the same way.
    let rust = std::fs::read_dir(dir.join("extensions"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().join("Cargo.toml").is_file());
    if rust {
        let mut target = Command::new("rustup");
        target.args(["target", "add", "wasm32-unknown-unknown"]);
        if let Err(code) = step(target, "adding the wasm target for Rust Functions") {
            return code;
        }
    }
    let out = Command::new("shopify")
        .args([
            "app",
            "deploy",
            "--config",
            STAGING,
            "--allow-updates",
            "--no-input",
        ])
        .arg("--path")
        .arg(&dir)
        .env("SHOPIFY_SERVICE_ACCOUNT_TOKEN", &token)
        // The CLI fails when either is set beside a service account token.
        .env_remove("SHOPIFY_APP_AUTOMATION_TOKEN")
        .env_remove("SHOPIFY_CLI_PARTNERS_TOKEN")
        .output();
    let out = match out {
        Ok(out) => out,
        Err(e) => {
            say(&format!("cannot run shopify: {e}"));
            return exit::FAILED;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The walk line stands on a line of its own.
    if stdout.is_empty() || stdout.ends_with('\n') {
        print!("{stdout}");
    } else {
        println!("{stdout}");
    }
    eprint!("{stderr}");
    if !out.status.success() {
        if words(&format!("{stdout}{stderr}"))
            .contains("SHOPIFY_SERVICE_ACCOUNT_TOKEN was rejected")
        {
            say(&format!(
                "{name} was rejected: it was revoked, has expired (1, 3 or 6 months) or is not valid; make a new token for {app}'s service account"
            ));
            return NEEDS_SAIFUL;
        }
        say(&format!("shopify app deploy failed for {app}"));
        return exit::FAILED;
    }
    let store = store
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .trim_end_matches(".myshopify.com");
    println!("walk: https://admin.shopify.com/store/{store}/apps/{handle}");
    exit::OK
}

/// The words of the CLI's error box on one line: it wraps to the
/// terminal's width, so a phrase may span its lines.
fn words(out: &str) -> String {
    out.split(|c: char| c.is_whitespace() || "│╭╮╰╯─".contains(c))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// wrangler.jsonc read as JSON: its comments and trailing commas dropped,
/// the only ways wrangler's JSONC differs.
pub fn jsonc(text: &str) -> Option<Value> {
    let chars: Vec<char> = text.chars().collect();
    let mut plain = String::with_capacity(text.len());
    let mut i = 0;
    let mut quoted = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if quoted {
            plain.push(c);
            if c == '\\' {
                if let Some(n) = next {
                    plain.push(n);
                }
                i += 2;
                continue;
            }
            quoted = c != '"';
            i += 1;
            continue;
        }
        match (c, next) {
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            (',', _) => {
                let rest = chars[i + 1..].iter().collect::<String>();
                if !strip_comments_ahead(&rest).starts_with([']', '}']) {
                    plain.push(c);
                }
                i += 1;
            }
            _ => {
                quoted = c == '"';
                plain.push(c);
                i += 1;
            }
        }
    }
    serde_json::from_str(&plain).ok()
}

/// What follows a comma once blanks and comments are skipped, enough to see
/// whether it closes a list or an object.
fn strip_comments_ahead(rest: &str) -> &str {
    let mut rest = rest.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else {
            return rest;
        }
    }
}

/// The paths of the Cloudflare API a staging deploy of `account` calls,
/// for the network secret that carries its token. The secret replaces the
/// Authorization header of every request on its paths, so the assets
/// upload, which carries its own upload token, must not be among them, nor
/// the bare /accounts, which is a prefix of it.
pub fn staging_api_paths(account: &str) -> Vec<String> {
    let mut paths: Vec<String> = [
        "/client/v4/user",
        "/client/v4/memberships",
        "/client/v4/zones",
    ]
    .map(str::to_string)
    .to_vec();
    for under in [
        "d1",
        "queues",
        "workflows",
        "workers/scripts",
        "workers/services",
        "workers/subdomain",
        "workers/workers",
        "workers/durable_objects",
    ] {
        paths.push(format!("/client/v4/accounts/{account}/{under}"));
    }
    paths
}

/// The staging Worker of the app at `dir`, when the app has a staging
/// config: its name, its account, and the secrets it needs, which are the
/// names its .dev.vars.example lists that env.staging does not set as vars.
/// Wrangler's own `secrets.required` would say them, but in one environment
/// it changes the types wrangler generates for all of them.
pub fn staging_worker_of(dir: &Path) -> Option<(String, Option<String>, Vec<String>)> {
    if !dir.join(format!("shopify.app.{STAGING}.toml")).is_file() {
        return None;
    }
    let config = jsonc(&std::fs::read_to_string(dir.join("wrangler.jsonc")).ok()?)?;
    let env = &config["env"][STAGING];
    let example = std::fs::read_to_string(dir.join(".dev.vars.example")).unwrap_or_default();
    let secrets = example
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .filter(|name| {
            !name.is_empty() && !name.starts_with('#') && env["vars"].get(name).is_none()
        })
        .map(str::to_string)
        .collect();
    Some((
        env["name"].as_str()?.to_string(),
        env["account_id"].as_str().map(str::to_string),
        secrets,
    ))
}

/// The app's staging Worker, as its wrangler.jsonc's `env.staging` names it.
struct Worker {
    name: String,
    databases: Vec<String>,
}

fn staging_worker(dir: &Path, app: &str) -> Result<Worker, i32> {
    let config = std::fs::read_to_string(dir.join("wrangler.jsonc"))
        .ok()
        .and_then(|text| jsonc(&text))
        .unwrap_or_default();
    let env = &config["env"][STAGING];
    let Some(name) = env["name"].as_str() else {
        say(&format!(
            "{app} has no env.{STAGING} in wrangler.jsonc: its staging Worker is not set up"
        ));
        return Err(NEEDS_SAIFUL);
    };
    let ids = |of: &Value| -> Vec<String> {
        of["d1_databases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|db| db["database_id"].as_str().map(str::to_string))
            .collect()
    };
    // With no account wrangler lists the token's, and the thread's token
    // reaches only one account's paths.
    if env["account_id"].as_str().is_none_or(str::is_empty) {
        say(&format!(
            "env.{STAGING} in {app}'s wrangler.jsonc names no account_id: give it the Cloudflare account the staging Worker lives in"
        ));
        return Err(NEEDS_SAIFUL);
    }
    let production = config["name"].as_str();
    if production == Some(name) {
        say(&format!(
            "env.{STAGING} in {app}'s wrangler.jsonc names the Worker {name}, production's: give staging a Worker of its own"
        ));
        return Err(NEEDS_SAIFUL);
    }
    // Each app's thread touches only its own `<app>-staging`, so two may
    // deploy at once, and an env.staging copied from another app is caught.
    let own = format!("{app}-{STAGING}");
    if name != own {
        say(&format!(
            "env.{STAGING} in {app}'s wrangler.jsonc names the Worker {name}: {app}'s staging Worker is {own}"
        ));
        return Err(NEEDS_SAIFUL);
    }
    let theirs = ids(&config);
    if let Some(id) = ids(env).into_iter().find(|id| theirs.contains(id)) {
        say(&format!(
            "env.{STAGING} in {app}'s wrangler.jsonc binds production's D1 database {id}: give staging a database of its own"
        ));
        return Err(NEEDS_SAIFUL);
    }
    let databases: Vec<String> = env["d1_databases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|db| db["database_name"].as_str().map(str::to_string))
        .collect();
    // wrangler finds a binding with no id by its name.
    if let Some(db) = databases.iter().find(|db| **db != own) {
        say(&format!(
            "env.{STAGING} in {app}'s wrangler.jsonc binds the D1 database {db}: {app}'s staging database is {own}"
        ));
        return Err(NEEDS_SAIFUL);
    }
    Ok(Worker {
        name: name.to_string(),
        databases,
    })
}

/// The app's own wrangler with the proxy's Cloudflare token: wrangler's
/// fetch reaches the proxy only with NODE_USE_ENV_PROXY, and sends nothing
/// for it to replace without a token variable. A token already set, as on
/// Saiful's machine, is used as it is.
fn wrangler(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(dir.join("node_modules/.bin/wrangler"));
    cmd.args(args)
        .current_dir(dir)
        .env("NODE_USE_ENV_PROXY", "1")
        .env(
            "CLOUDFLARE_API_TOKEN",
            var("CLOUDFLARE_API_TOKEN").unwrap_or_else(|| "proxy-injected".to_string()),
        );
    cmd
}

/// Whether a Cloudflare token reaches wrangler. In a thread it is the
/// environment's network secret for api.cloudflare.com, which only Saiful
/// can add; without it the proxy adds nothing and every call is refused.
/// Listing the account's D1 databases stays inside the paths the secret is
/// given; whoami would list every account.
fn cloudflare_token(dir: &Path) -> Result<(), i32> {
    let mut list = wrangler(
        dir,
        &[
            "d1",
            "list",
            "--json",
            "--env",
            STAGING,
            "--config",
            "wrangler.jsonc",
        ],
    );
    match list.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            eprint!("{}", String::from_utf8_lossy(&out.stdout));
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            say(
                "no Cloudflare token reaches wrangler: add api.cloudflare.com as a network secret of the shopify_apps environment",
            );
            Err(NEEDS_SAIFUL)
        }
        Err(e) => {
            say(&format!(
                "wrangler could not start ({e}): run pnpm install first"
            ));
            Err(exit::FAILED)
        }
    }
}

/// pnpm without the proxy switch, which hangs it.
fn pnpm(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("pnpm");
    cmd.args(args)
        .current_dir(dir)
        .env_remove("NODE_USE_ENV_PROXY");
    cmd
}

/// Runs `cmd` with its output shown; a failure stops the deploy there.
fn step(mut cmd: Command, what: &str) -> Result<(), i32> {
    match cmd.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => {
            say(&format!("{what} failed: nothing after it was deployed"));
            Err(exit::FAILED)
        }
        Err(e) => {
            say(&format!(
                "{what} could not start ({e}): run pnpm install first"
            ));
            Err(exit::FAILED)
        }
    }
}

/// The release's order for staging: the D1 migrations, then the build, then
/// the Worker.
fn deploy_worker(dir: &Path, app: &str, worker: &Worker) -> Result<(), i32> {
    for db in &worker.databases {
        // The source config, not a build's: a production build left in the
        // folder holds no staging database.
        step(
            wrangler(
                dir,
                &[
                    "d1",
                    "migrations",
                    "apply",
                    db,
                    "--remote",
                    "--env",
                    STAGING,
                    "--config",
                    "wrangler.jsonc",
                ],
            ),
            &format!("the staging D1 migrations of {db}"),
        )?;
    }
    let package = std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| app.to_string());
    let root = dir.parent().and_then(Path::parent).unwrap_or(dir);
    step(
        pnpm(
            root,
            &["turbo", "run", "build", &format!("--filter={package}^...")],
        ),
        &format!("the build of {app}'s packages"),
    )?;
    // Outside turbo: its cache keys no environment, so it would hand back
    // production's build. The Vite plugin picks the environment here.
    let mut build = pnpm(dir, &["run", "build"]);
    build.env("CLOUDFLARE_ENV", STAGING);
    step(build, &format!("the staging build of {app}"))?;
    if let Some(built) = built_worker(dir)
        && built != worker.name
    {
        say(&format!(
            "the build is for the Worker {built}, not {}: nothing was deployed",
            worker.name
        ));
        return Err(exit::FAILED);
    }
    step(
        wrangler(dir, &["deploy", "--env", STAGING]),
        "the staging Worker deploy",
    )
}

/// The Worker a Vite build is for, from the config it hands wrangler, when
/// the app builds with the Cloudflare plugin; one for no environment is
/// production's whatever it is called.
fn built_worker(dir: &Path) -> Option<String> {
    let deploy = dir.join(".wrangler/deploy");
    let pointer: Value =
        serde_json::from_str(&std::fs::read_to_string(deploy.join("config.json")).ok()?).ok()?;
    let built: Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.join(pointer["configPath"].as_str()?)).ok()?,
    )
    .ok()?;
    let name = built["name"].as_str().unwrap_or_default();
    Some(if built["targetEnvironment"].as_str() == Some(STAGING) {
        name.to_string()
    } else {
        format!("{name} (built for no environment)")
    })
}

#[cfg(test)]
mod tests {
    use super::jsonc;

    /// Wrangler's JSONC as apps write it: comments with quotes in them,
    /// `//` inside strings, and trailing commas, some before a comment.
    #[test]
    fn jsonc_drops_comments_and_trailing_commas_only() {
        let text = r#"{
  // the app's "handle" lives here
  "url": "https://a.lareys.dev/a/sc", /* a "block" */
  "list": [1, 2, ],
  "vars": { "Q": "say \"hi\" // not a comment", // after
  },
}"#;
        let v = jsonc(text).unwrap();
        assert_eq!(v["url"], "https://a.lareys.dev/a/sc");
        assert_eq!(v["list"], serde_json::json!([1, 2]));
        assert_eq!(v["vars"]["Q"], "say \"hi\" // not a comment");
    }
}
