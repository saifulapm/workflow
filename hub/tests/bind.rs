//! H1/AC4 — hub binds loopback and nothing else.

mod common;

use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use common::{Hub, TempDir};

/// Every global IPv4/IPv6 address this machine holds, per `ip -o addr`.
fn global_addresses() -> Vec<IpAddr> {
    let Ok(out) = std::process::Command::new("ip")
        .args(["-o", "addr", "show", "scope", "global"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let address = fields.nth(3)?;
            address.split('/').next()?.parse().ok()
        })
        .collect()
}

#[test]
fn the_listener_is_loopback_only() {
    let dir = TempDir::new("bind");
    let home = dir.join("home");
    let hub = Hub::spawn(&home, &[], &["--port", "0"]);

    // Loopback answers.
    TcpStream::connect(("127.0.0.1", hub.port)).expect("loopback is reachable");

    // Nothing else does. An empty list is not a pass: say so.
    let addresses = global_addresses();
    if addresses.is_empty() {
        eprintln!("no global addresses on this machine; the negative half did not run");
    }
    for address in addresses {
        let target = SocketAddr::new(address, hub.port);
        let result = TcpStream::connect_timeout(&target, Duration::from_millis(500));
        assert!(
            result.is_err(),
            "hub answered on {target}; §7 says 127.0.0.1 only"
        );
    }
}

#[test]
fn the_port_flag_beats_the_config_file() {
    let dir = TempDir::new("bind-port");
    let home = dir.join("home");
    let config = dir.join("config.toml");
    std::fs::write(&config, "port = 1\n").unwrap();

    // Port 1 would need root; --port 0 is what actually gets bound.
    let hub = Hub::spawn(
        &home,
        &[],
        &["--config", config.to_str().unwrap(), "--port", "0"],
    );
    assert_ne!(hub.port, 1);
    TcpStream::connect(("127.0.0.1", hub.port)).expect("the flag's port is the bound one");
}

#[test]
fn the_config_flag_keeps_the_real_config_untouched() {
    let dir = TempDir::new("bind-config");
    let home = dir.join("home");
    let config = dir.join("elsewhere/config.toml");

    let _hub = Hub::spawn(
        &home,
        &[],
        &["--config", config.to_str().unwrap(), "--port", "0"],
    );

    assert!(config.is_file(), "hub created the config it was pointed at");
    assert!(
        !home.join("config/hub").exists(),
        "and nothing at the default location"
    );
}
