//! Pairing a device. `hub pair` writes a single-use code and prints a link to
//! `/pair/<code>`; the device opens it and presses Pair, and that POST trades
//! the code for a cookie the hub knows again for a year.
//!
//! The page and its POST are two steps so that a chat app's link preview,
//! which fetches the URL with a GET, cannot spend the code before the person
//! does. Both files live in the hub's state directory at 0600: the devices
//! file holds every token that `paired` accepts.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::{self, Config};
use crate::html::esc;
use crate::http::{Request, Response};
use crate::pages::{PageCtx, page_shell};

/// How long a printed link works.
pub const CODE_TTL: Duration = Duration::from_secs(10 * 60);
pub const COOKIE: &str = "hub_device";
/// A year, in seconds, as `Max-Age` counts.
const COOKIE_MAX_AGE: u32 = 365 * 24 * 60 * 60;
const DEFAULT_POLL: Duration = Duration::from_millis(500);

/// Pending codes, one `<code> <expires_unix_ms>` per line.
pub fn pair_path() -> Result<PathBuf> {
    Ok(config::state_home()?.join("hub/pair"))
}

/// Paired devices, one `<token> <created rfc3339> <name>` per line.
pub fn devices_path() -> Result<PathBuf> {
    Ok(config::state_home()?.join("hub/devices"))
}

/// The hub's address as a device should open it: the config's `hub_url`, else
/// the tailnet name, else the machine name, which is the doorbell's rule.
pub fn base_url(config: &Config, port: u16, machine: &str) -> String {
    if let Some(url) = &config.hub_url {
        return url.clone();
    }
    let host = crate::tailnet_name().unwrap_or_else(|| machine.to_string());
    format!("http://{host}:{port}/")
}

/// The link `hub pair` prints. A `hub_url` written without its trailing slash
/// still gets exactly one before `pair/`.
pub fn link(base: &str, code: &str) -> String {
    format!("{}/pair/{code}", base.trim_end_matches('/'))
}

/// Six Crockford base32 characters, so a code read off a screen survives
/// being typed. They are the last six of 26 because the first carries only
/// three random bits and each of the others carries five.
pub fn new_code() -> Result<String> {
    let full = config::base32(&config::urandom::<16>()?);
    Ok(full[20..].to_string())
}

/// Writes a pending code that `/pair/<code>` accepts until `expires_ms`.
pub fn add_code(path: &Path, code: &str, expires_ms: i64) -> Result<()> {
    append_private(path, &format!("{code} {expires_ms}\n"))
}

/// Whether `code` is written down and has not expired by `now_ms`.
pub fn is_pending(path: &Path, code: &str, now_ms: i64) -> bool {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().any(|line| match line.split_once(' ') {
        Some((known, expires)) => {
            known == code && expires.trim().parse::<i64>().is_ok_and(|at| at > now_ms)
        }
        None => false,
    })
}

/// Whether `code` is still written down at all, expired or not.
fn is_written(path: &Path, code: &str) -> bool {
    is_pending(path, code, i64::MIN)
}

pub fn remove_code(path: &Path, code: &str) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let kept: String = text
        .lines()
        .filter(|line| line.split(' ').next() != Some(code))
        .map(|line| format!("{line}\n"))
        .collect();
    rewrite_private(path, &kept)
}

/// The name `hub pair` reports, from the device's User-Agent. The phones come
/// first because an iPhone's agent also says "Mac OS X" and Android's says
/// "Linux".
pub fn device_name(user_agent: Option<&str>) -> &'static str {
    let agent = user_agent.unwrap_or_default().to_ascii_lowercase();
    [
        ("iphone", "iphone"),
        ("ipad", "ipad"),
        ("android", "android"),
        ("macintosh", "mac"),
        ("mac os", "mac"),
        ("windows", "windows"),
        ("linux", "linux"),
    ]
    .into_iter()
    .find(|(needle, _)| agent.contains(needle))
    .map_or("device", |(_, name)| name)
}

/// The name on the devices file's last line: the device paired most recently.
pub fn newest_device(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().rfind(|line| !line.trim().is_empty())?;
    line.splitn(3, ' ').nth(2).map(str::to_string)
}

/// Waits for `/pair/<code>` to spend the code, and returns the device it
/// paired, or `None` once the code expires, which removes it.
pub fn wait(code: &str, expires_ms: i64) -> Result<Option<String>> {
    let pair = pair_path()?;
    let devices = devices_path()?;
    let poll = poll_interval();
    loop {
        if !is_written(&pair, code) {
            return newest_device(&devices)
                .map(Some)
                .context("the code is spent but no device is paired");
        }
        if now_ms() >= expires_ms {
            remove_code(&pair, code)?;
            return Ok(None);
        }
        std::thread::sleep(poll);
    }
}

/// 500 ms, or `HUB_PAIR_POLL_MS`, so a test does not wait half a second per
/// look at the file.
fn poll_interval() -> Duration {
    std::env::var("HUB_PAIR_POLL_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .map(|poll| poll.max(Duration::from_millis(10)))
        .unwrap_or(DEFAULT_POLL)
}

/// `GET /pair/<code>`: the Pair button, or a 404 page when the code is not
/// pending. Nothing is spent here.
pub fn get(ctx: &PageCtx) -> Response {
    let code = ctx.rest;
    let pending = pair_path().is_ok_and(|path| is_pending(&path, code, now_ms()));
    if !pending {
        return gone();
    }
    let body = format!(
        "<p>Pair this device with the hub on {}. It keeps a cookie for a year.</p>\n\
         <form method=\"post\" action=\"/pair/{}\">\n\
         <button type=\"submit\">Pair this device</button>\n\
         </form>\n",
        esc(&ctx.app.machine),
        esc(code),
    );
    Response::html(page_shell("pair", None, &body))
}

/// `POST /pair/<code>`: spends the code and hands the device its cookie.
/// The lock keeps two presses of one link from both finding it pending.
pub fn post(ctx: &PageCtx) -> Response {
    let lock = ctx.app.lock_for("pair");
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    match spend(ctx.rest, ctx.request.header("user-agent")) {
        Ok(Some(token)) => Response::see_other("/").header(
            "Set-Cookie",
            // No Secure: the tailnet URL is plain http, and a Secure cookie
            // would never be sent back over it.
            &format!(
                "{COOKIE}={token}; Path=/; Max-Age={COOKIE_MAX_AGE}; HttpOnly; SameSite=Strict"
            ),
        ),
        Ok(None) => gone(),
        Err(e) => {
            eprintln!("hub: pairing failed: {e:#}");
            Response::text(500, "pairing failed; see the hub's log")
        }
    }
}

/// The token for a new device, or `None` when the code is not pending.
fn spend(code: &str, user_agent: Option<&str>) -> Result<Option<String>> {
    let pair = pair_path()?;
    if !is_pending(&pair, code, now_ms()) {
        return Ok(None);
    }
    let token = config::base32(&config::urandom::<16>()?);
    let created = jiff::Timestamp::now();
    // The device goes in before the code comes out: `hub pair` reads the
    // newest device the moment it sees its code gone.
    append_private(
        &devices_path()?,
        &format!("{token} {created} {}\n", device_name(user_agent)),
    )?;
    remove_code(&pair, code)?;
    Ok(Some(token))
}

fn gone() -> Response {
    let body = "<p>This pairing link is spent, unknown or expired. \
                Run <code>hub pair</code> again for a new one.</p>\n";
    Response::new(
        404,
        "text/html; charset=utf-8",
        page_shell("pair", None, body),
    )
}

/// Whether the request carries the cookie of a paired device.
pub fn paired(request: &Request) -> bool {
    let devices = devices_path()
        .and_then(|path| Ok(std::fs::read_to_string(path)?))
        .unwrap_or_default();
    cookie_matches(request.header("cookie"), &devices)
}

fn cookie_matches(cookie: Option<&str>, devices: &str) -> bool {
    let Some(cookie) = cookie else {
        return false;
    };
    let known: Vec<&str> = devices
        .lines()
        .filter_map(|line| line.split(' ').next())
        .filter(|token| !token.is_empty())
        .collect();
    cookie
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(name, _)| *name == COOKIE)
        .map(|(_, token)| token.trim())
        .filter(|token| !token.is_empty())
        .any(|token| known.iter().any(|k| same(token.as_bytes(), k.as_bytes())))
}

/// Byte equality that looks at every byte, so how long a guess takes to fail
/// says nothing about how much of it was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// Appends at 0600, the mode set when the file is created rather than after.
fn append_private(path: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    create_parent(path)?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_all()?;
    Ok(())
}

/// Replaces the file through a 0600 temp file and a rename, so `hub pair`'s
/// poll never reads it half written.
fn rewrite_private(path: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    create_parent(path)?;
    let temp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .with_context(|| format!("creating {}", temp.display()))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)
        .with_context(|| format!("renaming {} into place", temp.display()))?;
    Ok(())
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str = "\
        AAAAAAAAAAAAAAAAAAAAAAAAAA 2026-10-08T10:00:00Z iphone\n\
        BBBBBBBBBBBBBBBBBBBBBBBBBB 2026-10-08T11:00:00Z mac\n";

    #[test]
    fn a_known_token_among_other_cookies_is_paired() {
        assert!(cookie_matches(
            Some("theme=dark; hub_device=BBBBBBBBBBBBBBBBBBBBBBBBBB; lang=en"),
            DEVICES
        ));
        assert!(cookie_matches(
            Some("hub_device=AAAAAAAAAAAAAAAAAAAAAAAAAA"),
            DEVICES
        ));
        assert!(cookie_matches(
            Some("  theme=dark ;  hub_device=AAAAAAAAAAAAAAAAAAAAAAAAAA  "),
            DEVICES
        ));
    }

    #[test]
    fn a_missing_or_wrong_token_is_not_paired() {
        assert!(!cookie_matches(None, DEVICES));
        assert!(!cookie_matches(Some(""), DEVICES));
        assert!(!cookie_matches(Some("theme=dark"), DEVICES));
        assert!(!cookie_matches(
            Some("hub_device=CCCCCCCCCCCCCCCCCCCCCCCCCC"),
            DEVICES
        ));
        // A prefix of a real token, and a real token under another name.
        assert!(!cookie_matches(Some("hub_device=AAAAAAAAAAAAA"), DEVICES));
        assert!(!cookie_matches(
            Some("other=AAAAAAAAAAAAAAAAAAAAAAAAAA"),
            DEVICES
        ));
        assert!(!cookie_matches(
            Some("hub_device=AAAAAAAAAAAAAAAAAAAAAAAAAA"),
            ""
        ));
    }

    #[test]
    fn an_empty_token_never_matches_a_blank_line() {
        assert!(!cookie_matches(Some("hub_device="), "\n \n"));
        assert!(!cookie_matches(Some("hub_device="), DEVICES));
    }

    #[test]
    fn the_device_is_named_from_its_user_agent() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X)";
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8)";
        let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";
        assert_eq!(device_name(Some(iphone)), "iphone");
        assert_eq!(device_name(Some("Mozilla/5.0 (iPad; CPU OS 17_5)")), "ipad");
        assert_eq!(device_name(Some(android)), "android");
        assert_eq!(device_name(Some(mac)), "mac");
        assert_eq!(
            device_name(Some("Mozilla/5.0 (X11; Linux aarch64)")),
            "linux"
        );
        assert_eq!(
            device_name(Some("Mozilla/5.0 (Windows NT 10.0)")),
            "windows"
        );
        assert_eq!(device_name(Some("curl/8.9")), "device");
        assert_eq!(device_name(None), "device");
    }

    #[test]
    fn the_link_has_one_slash_before_pair() {
        assert_eq!(
            link("http://box:8787/", "Q7K2MZ"),
            "http://box:8787/pair/Q7K2MZ"
        );
        assert_eq!(
            link("https://box.tail1234.ts.net", "Q7K2MZ"),
            "https://box.tail1234.ts.net/pair/Q7K2MZ"
        );
    }

    #[test]
    fn a_code_is_six_unambiguous_characters() {
        let code = new_code().unwrap();
        assert_eq!(code.len(), 6);
        assert!(
            code.bytes()
                .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
        );
    }
}
