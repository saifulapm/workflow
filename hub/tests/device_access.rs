//! Cloudflare Access's token on hub local's peer port, as the device tunnel
//! (device.on-rexa.com) brings a cloud session's `device` call: it opens
//! `POST /device` and nothing else, and only when this hub's device Access
//! app signed it.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::LazyLock;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};
use rsa::rand_core::{Infallible, TryCryptoRng, TryRng};
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TEAM: &str = "https://test-team.cloudflareaccess.com";
const AUD: &str = "eb8bc32242890cf431192a69faff951f4e82b8e3a3d7f98e9ae586ff184b0b68";
const OTHER_AUD: &str = "4156a52d77bb948a63b9b6d4dad534cc230280e9cf4235f464a8761b35a9c998";
const KID: &str = "test-kid-1";
const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

/// `/dev/urandom` as the keygen's RNG, so the tests need no RNG crate.
struct Urandom(std::fs::File);

impl TryRng for Urandom {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut b = [0u8; 4];
        self.try_fill_bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut b = [0u8; 8];
        self.try_fill_bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        self.0.read_exact(dst).expect("read /dev/urandom");
        Ok(())
    }
}

impl TryCryptoRng for Urandom {}

/// The team's signing key and a key the team never published.
static KEYS: LazyLock<(RsaPrivateKey, RsaPrivateKey)> = LazyLock::new(|| {
    let mut rng = Urandom(std::fs::File::open("/dev/urandom").unwrap());
    (
        RsaPrivateKey::new(&mut rng, 2048).unwrap(),
        RsaPrivateKey::new(&mut rng, 2048).unwrap(),
    )
});

fn b64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// What Access signs for a service token: no email, the client id as
/// common_name.
fn token_for(key: &RsaPrivateKey, aud: &str) -> String {
    let header = json!({"alg": "RS256", "kid": KID, "typ": "JWT"});
    let claims = json!({
        "aud": [aud],
        "common_name": "b03e54.access",
        "exp": now() + 300,
        "iat": now(),
        "nbf": now(),
        "iss": TEAM,
        "type": "app",
        "sub": "",
    });
    let input = format!(
        "{}.{}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes())
    );
    let digest = Sha256::digest(input.as_bytes());
    let signature = key.sign(Pkcs1v15Sign::new::<Sha256>(), &digest).unwrap();
    format!("{input}.{}", b64url(&signature))
}

/// A local hub named mini whose route is Device on itself, with the team's
/// keys served through a fixture curl.
fn spawn(dir: &TempDir) -> Hub {
    let home = dir.join("mini");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mini\n").unwrap();
    let bin = home.join("bin");
    let public = KEYS.0.to_public_key();
    let jwks = json!({
        "keys": [{
            "kid": KID, "alg": "RS256", "kty": "RSA", "use": "sig",
            "e": b64url(&public.e().to_be_bytes_trimmed_vartime()),
            "n": b64url(&public.n().to_be_bytes_trimmed_vartime()),
        }],
        "public_certs": [],
    });
    let jwks_path = home.join("jwks.json");
    std::fs::write(&jwks_path, jwks.to_string()).unwrap();
    fixture_bin(
        &bin,
        "curl",
        &format!(
            "case \"$*\" in *'{TEAM}/cdn-cgi/access/certs'*) cat '{}';; esac\nexit 0",
            jwks_path.display()
        ),
    );
    fixture_bin(&bin, "mem", "exit 0");
    let socket = home.join("local.sock");
    let hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("HUB_HOST", "127.0.0.1"),
            ("HUB_PEERS", ""),
            ("HUB_DEVICE_ACCESS_AUD", AUD),
            ("HUB_DEVICE_ACCESS_TEAM", TEAM),
        ],
    );
    choose_here(&socket);
    hub
}

/// Chooses this machine, the mini, as the device.
fn choose_here(socket: &Path) {
    let body = r#"{"device":"mini"}"#;
    let mut stream = UnixStream::connect(socket).unwrap();
    write!(
        stream,
        "POST /device/choice HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
}

fn send(hub: &Hub, method: &str, path: &str, jwt: &str) -> String {
    let body = r#"{"verb":"whoami","args":[],"project":"alpha"}"#;
    let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: device.on-rexa.com\r\n\
         Cf-Access-Jwt-Assertion: {jwt}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn the_device_app_s_token_opens_post_device() {
    let dir = TempDir::new("device-access-ok");
    let hub = spawn(&dir);
    let response = send(&hub, "POST", "/device", &token_for(&KEYS.0, AUD));
    assert_eq!(status_of(&response), 200, "{response}");
    let answer: Value = serde_json::from_str(body_of(&response)).unwrap();
    assert_eq!(answer["stdout"], "mini\n", "{answer}");
}

#[test]
fn another_app_s_token_or_a_stranger_s_key_is_refused() {
    let dir = TempDir::new("device-access-bad");
    let hub = spawn(&dir);
    for jwt in [
        token_for(&KEYS.0, OTHER_AUD),
        token_for(&KEYS.1, AUD),
        "not-a-jwt".to_string(),
    ] {
        let response = send(&hub, "POST", "/device", &jwt);
        assert_eq!(status_of(&response), 401, "{response}");
    }
}

/// The token is for device calls: every other path still wants the peer
/// token, and /device/here, which skips the route, is a peer's alone.
#[test]
fn the_token_opens_nothing_else() {
    let dir = TempDir::new("device-access-only");
    let hub = spawn(&dir);
    let jwt = token_for(&KEYS.0, AUD);
    for (method, path) in [
        ("POST", "/device/here"),
        ("GET", "/route"),
        ("GET", "/questions"),
        ("POST", "/answer"),
    ] {
        let response = send(&hub, method, path, &jwt);
        assert_eq!(status_of(&response), 401, "{method} {path}: {response}");
    }
}
