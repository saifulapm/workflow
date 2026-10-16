//! `hub local`: the local hub's peer port and its socket for mem.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{Hub, TempDir, body_of, hub_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

fn home_named(dir: &TempDir, machine: &str) -> std::path::PathBuf {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    home
}

fn spawn_local(dir: &TempDir, machine: &str) -> Hub {
    let home = home_named(dir, machine);
    let socket = dir.join("local.sock");
    Hub::spawn_env(
        &home,
        &[],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
        ],
    )
}

fn ping(home: &Path, target: &str, env: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(hub_bin());
    command
        .args(["ping", target])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .stdin(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("run hub ping")
}

#[test]
fn a_peer_with_the_token_gets_the_machine_name() {
    let dir = TempDir::new("local-ping");
    let hub = spawn_local(&dir, "testbox");
    let caller = TempDir::new("local-ping-caller");
    let out = ping(
        caller.path(),
        &format!("127.0.0.1:{}", hub.port),
        &[("HUB_PEER_TOKEN", TOKEN)],
    );
    assert!(
        out.status.success(),
        "hub ping failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "testbox");
}

#[test]
fn a_request_without_the_right_token_is_refused() {
    let dir = TempDir::new("local-refuse");
    let hub = spawn_local(&dir, "testbox");
    let none = hub.raw("GET /ping HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status_of(&none), 401, "{none}");
    let wrong = hub.raw("GET /ping HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer nope\r\n\r\n");
    assert_eq!(status_of(&wrong), 401, "{wrong}");
    assert!(!body_of(&wrong).contains("testbox"));

    let caller = TempDir::new("local-refuse-caller");
    let out = ping(
        caller.path(),
        &format!("127.0.0.1:{}", hub.port),
        &[("HUB_PEER_TOKEN", "a-different-token")],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("401"));
}

#[test]
fn the_local_hub_will_not_start_without_a_peer_token() {
    let dir = TempDir::new("local-notoken");
    let home = home_named(&dir, "testbox");
    let out = Command::new(hub_bin())
        .args(["local", "--port", "0"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("HUB_LOCAL_SOCKET", dir.join("local.sock"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("HUB_PEER_TOKEN"));
}

#[test]
fn mem_reaches_the_local_hub_on_its_socket_without_a_token() {
    let dir = TempDir::new("local-socket");
    let _hub = spawn_local(&dir, "testbox");
    let socket = dir.join("local.sock");
    let mode = std::fs::metadata(&socket).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(mode.permissions().mode() & 0o777, 0o600);
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .write_all(b"GET /ping HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response).trim(), "testbox");
}

/// A cloud VM runs Tailscale in userspace, where a tailnet address is reached
/// only through tailscaled's SOCKS5 listener.
#[test]
fn hub_ping_goes_through_a_socks5_proxy_when_one_is_named() {
    let dir = TempDir::new("local-socks");
    let hub = spawn_local(&dir, "testbox");
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_port = proxy.local_addr().unwrap().port();
    let hub_port = hub.port;
    let asked = std::thread::spawn(move || {
        let (mut client, _) = proxy.accept().unwrap();
        let mut greeting = [0u8; 3];
        client.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [5, 1, 0]);
        client.write_all(&[5, 0]).unwrap();
        let mut head = [0u8; 5];
        client.read_exact(&mut head).unwrap();
        assert_eq!(&head[..4], &[5, 1, 0, 3], "a CONNECT by name");
        let mut name = vec![0u8; head[4] as usize];
        client.read_exact(&mut name).unwrap();
        let mut port = [0u8; 2];
        client.read_exact(&mut port).unwrap();
        client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
        let mut upstream = TcpStream::connect(("127.0.0.1", hub_port)).unwrap();
        let mut up = upstream.try_clone().unwrap();
        let mut down = client.try_clone().unwrap();
        let pump = std::thread::spawn(move || std::io::copy(&mut client, &mut up));
        std::io::copy(&mut upstream, &mut down).unwrap();
        down.shutdown(std::net::Shutdown::Both).unwrap();
        let _ = pump.join();
        (String::from_utf8(name).unwrap(), u16::from_be_bytes(port))
    });

    let caller = TempDir::new("local-socks-caller");
    let out = ping(
        caller.path(),
        &format!("mini.tailnet:{hub_port}"),
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_SOCKS5", &format!("127.0.0.1:{proxy_port}")),
        ],
    );
    assert!(
        out.status.success(),
        "hub ping failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "testbox");
    let (name, port) = asked.join().unwrap();
    assert_eq!(name, "mini.tailnet");
    assert_eq!(port, hub_port);
}
