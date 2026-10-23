//! `hub pair --web` asks the store for a single-use code with the store
//! token and prints the hub web's pairing link, on HUB_WEB_URL: the store's
//! name admits only machines, so a browser cannot open a link on it.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

use common::{TempDir, hub_bin};

const STORE_TOKEN: &str = "store-token-for-tests-fedcba9876543210";

/// A store answering `POST /store/pair` with a code; it keeps each request's
/// first line and headers.
fn fake_store() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            let length = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0usize);
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            kept.lock().unwrap().push(head);
            let mut stream = stream;
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nK7Q2ZD",
            );
        }
    });
    (port, seen)
}

#[test]
fn pair_web_prints_the_hub_webs_link_for_a_code_the_store_made() {
    let dir = TempDir::new("pair-web");
    let (port, seen) = fake_store();
    let out = Command::new(hub_bin())
        .args(["pair", "--web"])
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env(
            "HUB_STORE_URL",
            format!("ws://127.0.0.1:{port}/store/share"),
        )
        .env("HUB_STORE_TOKEN", STORE_TOKEN)
        .env("HUB_WEB_URL", "https://hub.example/")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "https://hub.example/pair/K7Q2ZD"
    );
    let seen = seen.lock().unwrap();
    assert!(seen[0].starts_with("POST /store/pair "), "{}", seen[0]);
    assert!(
        seen[0].contains(&format!("Bearer {STORE_TOKEN}")),
        "{}",
        seen[0]
    );
}

#[test]
fn pair_web_without_the_store_settings_says_so() {
    let dir = TempDir::new("pair-web-unset");
    let out = Command::new(hub_bin())
        .args(["pair", "--web"])
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env_remove("HUB_STORE_URL")
        .env_remove("HUB_STORE_TOKEN")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("HUB_STORE_URL"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn pair_web_without_the_hub_webs_address_says_so() {
    let dir = TempDir::new("pair-web-no-web");
    let (port, seen) = fake_store();
    let out = Command::new(hub_bin())
        .args(["pair", "--web"])
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env(
            "HUB_STORE_URL",
            format!("ws://127.0.0.1:{port}/store/share"),
        )
        .env("HUB_STORE_TOKEN", STORE_TOKEN)
        .env_remove("HUB_WEB_URL")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("HUB_WEB_URL"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // No code is spent on a link that cannot be printed.
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn pair_web_reads_what_the_environment_lacks_from_local_env() {
    let dir = TempDir::new("pair-web-env-file");
    let (port, seen) = fake_store();
    std::fs::create_dir_all(dir.join("config/hub")).unwrap();
    std::fs::write(
        dir.join("config/hub/local.env"),
        format!(
            "HUB_PEER_TOKEN=peer\n\
             HUB_STORE_URL=ws://127.0.0.1:{port}/store/share\n\
             HUB_STORE_TOKEN=machine-token\n\
             HUB_WEB_URL=https://hub.example\n"
        ),
    )
    .unwrap();
    let run = |token: Option<&str>| {
        let mut command = Command::new(hub_bin());
        command
            .args(["pair", "--web"])
            .env("HOME", dir.join("home"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env_remove("HUB_STORE_URL")
            .env_remove("HUB_STORE_TOKEN")
            .env_remove("HUB_WEB_URL");
        if let Some(token) = token {
            command.env("HUB_STORE_TOKEN", token);
        }
        command.output().unwrap()
    };

    let out = run(None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "https://hub.example/pair/K7Q2ZD"
    );
    // A key the environment does set wins over the file's.
    assert!(run(Some(STORE_TOKEN)).status.success());
    let seen = seen.lock().unwrap();
    assert!(seen[0].contains("Bearer machine-token"), "{}", seen[0]);
    assert!(
        seen[1].contains(&format!("Bearer {STORE_TOKEN}")),
        "{}",
        seen[1]
    );
}
