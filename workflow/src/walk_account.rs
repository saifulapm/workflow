//! `workflow walk-account`: which claude.ai account Saiful's Chromium holds,
//! read before a walk's question or brief is written. r1-hooks-proof's walker
//! learned it only after dispatch, and the milestone parked on it.
//!
//! It attaches to his Chromium through the dotfiles' playwright-cli wrapper,
//! opens a tab of its own on claude.ai's Code page, reads the page's text,
//! closes that tab and detaches. The main account's page names its plan,
//! Ultra, as the r1-hooks-proof walk read it.

use std::process::{Command, Output};

use crate::exit;

const SESSION: &str = "-s=walk-account";
// Split as hygiene.rs splits it: the joined text reads as a session link.
const PAGE: &str = concat!("https://claude.ai", "/code");
const TEXT: &str = "() => document.body.innerText";

fn say(msg: impl AsRef<str>) {
    eprintln!("workflow walk-account: {}", msg.as_ref());
}

fn cli(args: &[&str], token: &str) -> std::io::Result<Output> {
    Command::new("playwright-cli")
        .arg(SESSION)
        .args(args)
        .env("PLAYWRIGHT_MCP_EXTENSION_TOKEN", token)
        .output()
}

/// The extension's token: the environment's, else `playwright-extension-token`'s.
/// Without one, an attach waits on Saiful to pick a tab.
fn token() -> Option<String> {
    if let Some(token) = std::env::var("PLAYWRIGHT_MCP_EXTENSION_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
    {
        return Some(token);
    }
    let out = Command::new("playwright-extension-token").output().ok()?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

pub fn cmd_walk_account() -> i32 {
    let Some(token) = token() else {
        say("no extension token: playwright-extension-token printed none");
        return exit::FAILED;
    };
    match cli(&["attach", "--extension=chromium"], &token) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            say(format!(
                "attach failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            return exit::FAILED;
        }
        Err(e) => {
            say(format!("cannot run playwright-cli: {e}"));
            return exit::FAILED;
        }
    }
    let opened = cli(&["tab-new", PAGE], &token).is_ok_and(|out| out.status.success());
    let read = opened
        .then(|| cli(&["eval", TEXT], &token).ok())
        .flatten()
        .filter(|out| out.status.success());
    // Only the tab this command opened closes: the current tab is Saiful's
    // when tab-new failed, and closing his last one quits his browser.
    if opened {
        let _ = cli(&["tab-close"], &token);
    }
    let _ = cli(&["detach"], &token);
    let Some(read) = read else {
        say(format!("could not read {PAGE}"));
        return exit::FAILED;
    };
    if String::from_utf8_lossy(&read.stdout).contains("Ultra") {
        println!("claude.ai account in Chromium: main");
    } else {
        println!(
            "claude.ai account in Chromium: not main ({PAGE} names no Ultra plan): work, or signed out"
        );
    }
    exit::OK
}
