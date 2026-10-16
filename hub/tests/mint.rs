//! `hub share mint <runner name>` sends the shared store bearer, read from
//! stdin, to the store's mint endpoint and prints only the machine's new
//! token.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use common::{TempDir, hub_bin};

const SHARED: &str = "shared-store-bearer-for-tests-0123456789";

/// A store answering every request with a minted token; it keeps each
/// request's first line and headers.
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
                b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nab12cd34ef",
            );
        }
    });
    (port, seen)
}

#[test]
fn share_mint_sends_the_bearer_from_stdin_and_prints_only_the_new_token() {
    let dir = TempDir::new("share-mint");
    let (port, seen) = fake_store();
    let mut child = Command::new(hub_bin())
        .args(["share", "mint", "mini"])
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env(
            "HUB_STORE_URL",
            format!("ws://127.0.0.1:{port}/store/share"),
        )
        .env("HUB_STORE_NAME", "main")
        .env_remove("HUB_STORE_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{SHARED}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ab12cd34ef\n");
    assert!(!stderr.contains(SHARED), "{stderr}");
    let seen = seen.lock().unwrap();
    assert!(
        seen[0].starts_with("POST /store/machines/mini "),
        "{}",
        seen[0]
    );
    assert!(
        seen[0].contains(&format!("Bearer {SHARED}\r\n")),
        "{}",
        seen[0]
    );
    assert!(
        seen[0]
            .to_ascii_lowercase()
            .contains("x-hub-store: main\r\n"),
        "the token is minted in the store object the machine will use: {}",
        seen[0]
    );
}
