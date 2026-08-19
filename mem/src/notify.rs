//! Desktop notification for a blocking question (spec §7b). Best effort by
//! design: a machine with no notification daemon, or none at all, must still be
//! able to ask.

/// The command mem shells out to. Overridable so a test can observe the call
/// without a desktop.
pub fn command() -> Option<Vec<String>> {
    if let Ok(custom) = std::env::var("MEM_NOTIFY_CMD") {
        if custom.trim().is_empty() {
            return None;
        }
        return Some(custom.split_whitespace().map(|s| s.to_string()).collect());
    }
    if cfg!(target_os = "macos") {
        Some(vec!["osascript".into(), "-e".into()])
    } else {
        Some(vec!["notify-send".into(), "-a".into(), "mem".into()])
    }
}

/// Fires and forgets. Never returns an error: nothing here is worth failing a
/// write that already landed.
pub fn notify(summary: &str, body: &str) {
    let Some(parts) = command() else { return };
    let Some((program, args)) = parts.split_first() else {
        return;
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    if cfg!(target_os = "macos") && std::env::var("MEM_NOTIFY_CMD").is_err() {
        cmd.arg(format!(
            "display notification {} with title {}",
            quote_applescript(body),
            quote_applescript(summary)
        ));
    } else {
        cmd.arg(summary).arg(body);
    }
    let _ = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn quote_applescript(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}
