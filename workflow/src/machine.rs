//! Which machine this is: the first label of its tailnet MagicDNS name.

use std::process::{Command, Stdio};

/// This machine's name. `WORKFLOW_MACHINE`, when set and not empty, wins;
/// otherwise the tailnet name from `tailscale status --self --json`. `None`
/// when neither says.
pub fn name() -> Option<String> {
    if let Ok(v) = std::env::var("WORKFLOW_MACHINE")
        && !v.is_empty()
    {
        return Some(v);
    }
    let out = Command::new("tailscale")
        .args(["status", "--self", "--json"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_status(&String::from_utf8_lossy(&out.stdout))
}

/// The machine name in `tailscale status --self --json`: `.Self.DNSName` is
/// `mini.taila27604.ts.net.`, and the name is its first label.
pub fn parse_status(text: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(text).ok()?;
    let dns = doc.get("Self")?.get("DNSName")?.as_str()?;
    let label = dns.split('.').next()?;
    (!label.is_empty()).then(|| label.to_string())
}

/// The name mem stamps on this machine's writes: `qshell/machine` under
/// `$XDG_CONFIG_HOME` (else `~/.config`), else `uname -n`.
pub fn mem_name() -> String {
    let config = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => Some(std::path::PathBuf::from(v)),
        _ => crate::paths::home().map(|home| home.join(".config")),
    };
    if let Some(text) = config.and_then(|c| std::fs::read_to_string(c.join("qshell/machine")).ok())
    {
        let name = text.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Ok(out) = Command::new("uname").arg("-n").output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_first_label_of_the_dns_name() {
        let status = r#"{"Self":{"DNSName":"mini.taila27604.ts.net.","HostName":"x"}}"#;
        assert_eq!(parse_status(status).as_deref(), Some("mini"));
        assert_eq!(parse_status(r#"{"Self":{"DNSName":""}}"#), None);
        assert_eq!(parse_status("not json"), None);
    }
}
