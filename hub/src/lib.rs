//! hub — mem's phone-facing view.
//!
//! Everything hub knows it learned by running the `mem` binary; it never
//! touches mem's store. `config` reads the config file and the doorbell topic,
//! `memcli` runs `mem`, `http` is the hand-rolled server, the page modules and
//! `answer` render and accept answers, and `doorbell` rings the phone.

pub mod access;
pub mod agents;
pub mod api;
pub mod app;
pub mod assets;
pub mod bin_upload;
pub mod config;
pub mod device;
pub mod device_amx;
pub mod device_secrets;
pub mod device_tmux;
pub mod device_worktree;
pub mod doorbell;
pub mod fleet;
pub mod form;
pub mod held;
pub mod html;
pub mod http;
pub mod live;
pub mod local;
pub mod localdevice;
pub mod localroute;
pub mod mask;
pub mod mcp;
pub mod memcli;
pub mod memserve;
pub mod mint;
pub mod model;
pub mod multipart;
pub mod nudge;
pub mod open;
pub mod origin;
pub mod page_control;
pub mod page_evidence;
pub mod page_home;
pub mod page_roadmap;
pub mod pages;
pub mod pair;
pub mod parked;
pub mod presence;
pub mod proc;
pub mod proxy;
pub mod route;
pub mod runs;
pub mod seed;
pub mod share;
pub mod slack;
pub mod stalls;

/// How long `tailscale status` may take. It runs once, before the listener is
/// serving, so a wedged `tailscaled` must not become the reason hub never
/// starts.
const TAILSCALE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// `~/.config/qshell/machine` else `uname -n` — the same rule as mem, so the
/// name hub prints and the `machine` field mem stamps on an item cannot drift
/// apart.
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

/// Whether a tailnet peer is a cloud thread's VM: it holds no device, so a
/// device hub takes no device choice from it and sends it no device call.
/// A VM names itself `cloud-<name>`, but a thread can rename it, so the
/// `tag:cloud` its auth key gives it decides too (`cloud_in_status`). An
/// explicit `HUB_PEERS` list, the suite's seam, is known by name only, and
/// loopback is this machine.
pub fn cloud_peer(host: &str) -> bool {
    let host = bare_host(host);
    if host.starts_with("cloud-") {
        return true;
    }
    if std::env::var_os("HUB_PEERS").is_some()
        || matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
    {
        return false;
    }
    tailscale_status().is_none_or(|status| cloud_in_status(&host, &status))
}

/// Whether `tailscale status --json` shows `host` (its MagicDNS name, the
/// name's first label, or one of its tailnet IPs, with or without a port) as
/// a node tagged `tag:cloud`. A host it does not list at all counts as cloud
/// too: only a node the tailnet vouches for gets the device token.
pub fn cloud_in_status(host: &str, status: &serde_json::Value) -> bool {
    let host = bare_host(host);
    let nodes: Vec<&serde_json::Value> = std::iter::once(&status["Self"])
        .chain(
            status["Peer"]
                .as_object()
                .into_iter()
                .flat_map(|peers| peers.values()),
        )
        .filter(|node| {
            let name = node["DNSName"].as_str().unwrap_or_default();
            let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
            let ips = node["TailscaleIPs"].as_array().into_iter().flatten();
            (!name.is_empty() && (name == host || name.split('.').next() == Some(host.as_str())))
                || ips.filter_map(|ip| ip.as_str()).any(|ip| ip == host)
        })
        .collect();
    nodes.is_empty()
        || nodes.iter().any(|node| {
            let tags = node["Tags"].as_array().into_iter().flatten();
            tags.filter_map(|tag| tag.as_str())
                .any(|tag| tag == "tag:cloud")
        })
}

/// `host`, `host:port` or an IP, lowercased, without its port or a trailing dot.
fn bare_host(target: &str) -> String {
    let target = target.trim();
    let host = if target.parse::<std::net::IpAddr>().is_ok() {
        target
    } else {
        match target.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => host,
            _ => target,
        }
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// `tailscale status --json`, or `None` when there is no tailnet to ask.
fn tailscale_status() -> Option<serde_json::Value> {
    let mut command = std::process::Command::new("tailscale");
    command.args(["status", "--json"]);
    let proc::Ended::Exited(done) = proc::output_within(&mut command, TAILSCALE_TIMEOUT) else {
        return None;
    };
    if done.code != Some(0) {
        return None;
    }
    serde_json::from_slice(&done.stdout).ok()
}

/// The local hubs this one tells about its questions: `HUB_PEERS` (a comma
/// list of `host[:port]`, empty for none) when set, else every online peer
/// `tailscale status --json` lists, phones aside, by MagicDNS name. A peer
/// that runs no local hub refuses the connection and costs nothing.
pub fn tailnet_peers() -> Vec<String> {
    if let Some(peers) = std::env::var_os("HUB_PEERS") {
        return peers
            .to_string_lossy()
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
    }
    let Some(status) = tailscale_status() else {
        return Vec::new();
    };
    status["Peer"]
        .as_object()
        .into_iter()
        .flat_map(|peers| peers.values())
        .filter(|peer| peer["Online"] == true)
        // A phone runs no local hub.
        .filter(|peer| !matches!(peer["OS"].as_str(), Some("iOS" | "android")))
        .filter_map(|peer| peer["DNSName"].as_str())
        .map(|name| name.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}
