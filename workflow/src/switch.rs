//! `workflow switch --store <url>` -- join this machine's memory to the
//! Cloudflare store: a fresh machine, or one whose local.env lost its token.
//!
//! chezmoi runs it on apply, beside running sessions: a session's write
//! between the first seed and the switch reaches the store through the second
//! seed, and one after it through the outbox. On a machine that already
//! switched it moves the store's URL and mints the machine's token again.
//! Every program it runs is found on PATH (mem also by `$WORKFLOW_MEM`), so a
//! test can put a script in each one's place.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::{exit, machine, memcli, paths, warn};

/// The store object the hub shares through; the walk keeps its own.
const STORE_NAME: &str = "main";

/// Fails a step: the message, and the exit code it ends with.
type Step<T> = Result<T, (i32, String)>;

fn fail<T>(msg: impl Into<String>) -> Step<T> {
    Err((exit::FAILED, msg.into()))
}

fn refuse<T>(msg: impl Into<String>) -> Step<T> {
    Err((exit::USAGE, msg.into()))
}

pub fn cmd_switch(url: &str) -> i32 {
    match switch(url) {
        Ok(()) => exit::OK,
        Err((code, msg)) => {
            warn(msg);
            code
        }
    }
}

fn switch(url: &str) -> Step<()> {
    if !store_url(url) {
        return refuse(format!(
            "--store must be a ws:// or wss:// URL ending in /store/share, not {url}"
        ));
    }
    let Some(name) = machine::name() else {
        return fail("cannot tell this machine's name; set WORKFLOW_MACHINE");
    };
    if switched() {
        return move_store(url, &name);
    }

    // Every secret is read before anything changes, so a missing pass entry
    // stops the switch on a machine that is still whole. The store's names
    // sit behind Cloudflare Access, which lets a machine through with the
    // service token alone.
    let admin = pass("workflow/hubweb-store-token")?;
    let cloud = pass("workflow/hub-cloud-token")?;
    let access_id = pass("workflow/cloud-next-access-id")?;
    let access_secret = pass("workflow/cloud-next-access-secret")?;
    let access = [
        ("HUB_STORE_ACCESS_ID", access_id.as_str()),
        ("HUB_STORE_ACCESS_SECRET", access_secret.as_str()),
    ];
    let token = mint(url, &name, &machine::mem_name(), &admin, &access)?;

    // The import goes out on the shared bearer: it carries other machines'
    // items and global rules, which the store takes from no machine's token.
    let store = [
        ("HUB_STORE_URL", url),
        ("HUB_STORE_NAME", STORE_NAME),
        ("HUB_STORE_TOKEN", admin.as_str()),
        access[0],
        access[1],
    ];
    match status("hub", &["share", "seed"], &store)? {
        0 => {}
        3 => {
            return refuse(
                "the store's import budget for today is spent and the import is not complete; \
                 run workflow switch again after 06:00 Dhaka",
            );
        }
        code => return fail(format!("hub share seed exited {code}")),
    }

    run(&memcli::bin(), &["share", "switch"], &[])
        .map_err(|e| (exit::FAILED, format!("mem share switch: {e}")))?;

    // A write made between the first seed and the switch reaches the store
    // only through this second one. If it fails, the switch has already
    // happened: the outbox holds any new write and the restarted hub sends it.
    let reseeded = match status("hub", &["share", "seed"], &store)? {
        0 => true,
        code => {
            warn(format!(
                "the second hub share seed exited {code}; new writes wait in the outbox \
                 and the restarted hub sends them"
            ));
            false
        }
    };

    let env = env_file()?;
    write_env(
        &env,
        &[
            ("HUB_STORE_URL", url),
            ("HUB_STORE_NAME", STORE_NAME),
            ("HUB_STORE_TOKEN", &token),
            ("HUB_CLOUD_TOKEN", &cloud),
            access[0],
            access[1],
        ],
    )
    .map_err(|e| (exit::FAILED, format!("cannot write {}: {e}", env.display())))?;

    run("systemctl", &["--user", "restart", "hub-local"], &[])
        .map_err(|e| (exit::FAILED, format!("restarting hub-local: {e}")))?;

    println!("switched {name} to {url}");
    println!("  memory shares through the local hub");
    if !reseeded {
        println!("  the second seed did not finish; the outbox holds new writes");
    }
    println!(
        "  wrote HUB_STORE_URL, HUB_STORE_NAME, HUB_STORE_TOKEN, HUB_CLOUD_TOKEN and the \
         Access service token to {}",
        env.display()
    );
    println!("  restarted hub-local");
    Ok(())
}

/// Has `mem share switch` run here? It leaves a flag file.
fn switched() -> bool {
    let base = match std::env::var("XDG_STATE_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => match paths::home() {
            Some(home) => home.join(".local/state"),
            None => return false,
        },
    };
    base.join("mem/share/switched").exists()
}

/// A re-run after the switch: the memory is already in the store, so the
/// URL moves and the machine's own token is minted again; the cloud token
/// stays as local.env has it.
fn move_store(url: &str, name: &str) -> Step<()> {
    let env = env_file()?;
    // A token minted before tokens named a machine writes nothing, so the
    // machine is minted again on the store it moves to.
    let admin = pass("workflow/hubweb-store-token")?;
    let access_id = pass("workflow/cloud-next-access-id")?;
    let access_secret = pass("workflow/cloud-next-access-secret")?;
    let access = [
        ("HUB_STORE_ACCESS_ID", access_id.as_str()),
        ("HUB_STORE_ACCESS_SECRET", access_secret.as_str()),
    ];
    let token = mint(url, name, &machine::mem_name(), &admin, &access)?;
    write_env(
        &env,
        &[
            ("HUB_STORE_URL", url),
            ("HUB_STORE_NAME", STORE_NAME),
            ("HUB_STORE_TOKEN", &token),
            access[0],
            access[1],
        ],
    )
    .map_err(|e| (exit::FAILED, format!("cannot write {}: {e}", env.display())))?;
    run("systemctl", &["--user", "restart", "hub-local"], &[])
        .map_err(|e| (exit::FAILED, format!("restarting hub-local: {e}")))?;
    println!("moved this machine's store to {url}");
    println!(
        "  wrote HUB_STORE_URL, HUB_STORE_NAME and a new HUB_STORE_TOKEN to {}",
        env.display()
    );
    println!("  restarted hub-local");
    Ok(())
}

/// A ws:// or wss:// URL with a host, ending in /store/share, the way the hub
/// reads `HUB_STORE_URL`.
pub fn store_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("wss://")
        .or_else(|| url.strip_prefix("ws://"));
    rest.and_then(|r| r.strip_suffix("/store/share"))
        .is_some_and(|host| !host.is_empty() && !host.contains('/'))
}

/// A pass entry's first line. Never printed.
fn pass(entry: &str) -> Step<String> {
    let out = Command::new("pass")
        .args(["show", entry])
        .stderr(Stdio::inherit())
        .output()
        .or_else(|e| fail(format!("cannot run pass: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let secret = text.lines().next().unwrap_or("").trim();
    if !out.status.success() || secret.is_empty() {
        return fail(format!("pass show {entry} gave no secret"));
    }
    Ok(secret.to_string())
}

/// This machine's store token, for its runner name and the name mem writes
/// as: the store's admin token goes to `hub share mint` on stdin, and the
/// token comes back on its stdout, so neither is ever in an argv.
fn mint(
    url: &str,
    runner: &str,
    writes_as: &str,
    admin: &str,
    access: &[(&str, &str)],
) -> Step<String> {
    let mut child = Command::new("hub")
        .args(["share", "mint", runner, writes_as])
        .envs(access.iter().copied())
        .env("HUB_STORE_URL", url)
        .env("HUB_STORE_NAME", STORE_NAME)
        .env_remove("HUB_STORE_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .or_else(|e| fail(format!("cannot run hub: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{admin}");
    }
    let out = child
        .wait_with_output()
        .or_else(|e| fail(format!("hub share mint: {e}")))?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || token.is_empty() {
        return fail(format!("hub share mint {runner} {writes_as} gave no token"));
    }
    Ok(token)
}

/// A program's exit code. Its stdout goes to our stderr, so stdout carries
/// only the summary.
fn status(program: &str, args: &[&str], env: &[(&str, &str)]) -> Step<i32> {
    let status = Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .status()
        .or_else(|e| fail(format!("cannot run {program}: {e}")))?;
    status
        .code()
        .map_or_else(|| fail(format!("{program} was killed")), Ok)
}

fn run(program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<(), String> {
    match status(program, args, env) {
        Ok(0) => Ok(()),
        Ok(code) => Err(format!("exited {code}")),
        Err((_, msg)) => Err(msg),
    }
}

/// `$XDG_CONFIG_HOME/hub/local.env`, else `~/.config/hub/local.env`.
fn env_file() -> Step<PathBuf> {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => match paths::home() {
            Some(home) => home.join(".config"),
            None => return fail("HOME is not set"),
        },
    };
    Ok(base.join("hub/local.env"))
}

/// Set `keys` in the env file, each in its own line's place or appended, and
/// keep every other line. A temp file in the same directory renamed over it
/// means the hub never reads half a file.
fn write_env(path: &PathBuf, keys: &[(&str, &str)]) -> std::io::Result<()> {
    let old = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = old.lines().map(str::to_string).collect();
    for (key, value) in keys {
        let line = format!("{key}={value}");
        let prefix = format!("{key}=");
        match lines.iter_mut().find(|l| l.starts_with(&prefix)) {
            Some(existing) => *existing = line,
            None => lines.push(line),
        }
    }
    let dir = path.parent().expect("the env file has a directory");
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".local.env.{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    // A temp file left by an earlier run keeps its old mode otherwise.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    file.write_all(format!("{}\n", lines.join("\n")).as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}
