//! hub — mem's phone-facing view (spec `specs/hub-v1.md`, draft 2).
//!
//! Everything hub knows it learned by running the `mem` binary; it never
//! touches mem's store. The modules are the spec's sections: `config` is §6,
//! `memcli` is §4, `http` is §8, `page` and `answer` are §3 and §9, `doorbell`
//! is §5.

pub mod api;
pub mod app;
pub mod config;
pub mod doorbell;
pub mod form;
pub mod html;
pub mod http;
pub mod memcli;
pub mod model;
pub mod origin;
pub mod proc;

/// How long `tailscale status` may take. It runs once, before the listener is
/// serving, so a wedged `tailscaled` must not become the reason hub never
/// starts.
const TAILSCALE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// `~/.config/qshell/machine` else `uname -n` — the same rule as mem §7, so the
/// name hub prints and the `machine` field mem stamps on an item cannot drift
/// apart (review m-10).
pub fn machine_name() -> String {
    if let Ok(config) = config::config_home()
        && let Ok(text) = std::fs::read_to_string(config.join("qshell/machine"))
    {
        let name = text.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Ok(out) = std::process::Command::new("uname").arg("-n").output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    "unknown".to_string()
}

/// This machine's MagicDNS name, from `tailscale status --json`, lowercased and
/// without its trailing dot — or `None` when there is no tailnet to ask.
///
/// The doorbell's click URL used to derive from `machine_name()`, and that name
/// is whatever `~/.config/qshell/machine` says: on this machine `macbook-m2`,
/// which resolves nowhere. The tailnet name is the one name that is true from
/// the phone, which is the only client the doorbell has.
pub fn tailnet_name() -> Option<String> {
    // A seam in the style of `HUB_POLL_MS`: set it and `tailscale` is not run
    // at all. The suite sets it empty, so no test on a tailnet-connected
    // machine can come to depend on that machine's real name.
    if let Some(name) = std::env::var_os("HUB_TAILNET_NAME") {
        let name = name.to_string_lossy().trim().to_ascii_lowercase();
        return (!name.is_empty()).then_some(name);
    }
    let mut command = std::process::Command::new("tailscale");
    command.args(["status", "--json"]);
    let proc::Ended::Exited(done) = proc::output_within(&mut command, TAILSCALE_TIMEOUT) else {
        return None;
    };
    if done.code != Some(0) {
        return None;
    }
    let status: serde_json::Value = serde_json::from_slice(&done.stdout).ok()?;
    let name = status.get("Self")?.get("DNSName")?.as_str()?;
    let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    (!name.is_empty()).then_some(name)
}
