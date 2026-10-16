//! The local hub: `hub local`, one per machine and one per cloud session.
//!
//! Local hubs reach each other on one TCP port over the tailnet, and every
//! request there carries the peer token. mem reaches its own
//! machine's local hub on a unix socket instead, so the token never reaches
//! mem. A cloud VM's userspace Tailscale hands tailnet connections in from
//! 127.0.0.1, so the address a request came from proves nothing; the token
//! and the socket's file mode are the only guards.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::http::{Request, Response};

/// The port local hubs talk to each other on. The hub web keeps 8787.
pub const PORT: u16 = 8789;

/// How long a peer may take to answer.
const PEER_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest a wait is held open; mem asks again.
const MAX_HOLD: Duration = Duration::from_secs(25);

/// The peer token, from the environment only: a flag would put it in `ps`.
pub fn token_from_env() -> Result<String> {
    let token = std::env::var("HUB_PEER_TOKEN").unwrap_or_default();
    let token = token.trim();
    if token.is_empty() {
        bail!("HUB_PEER_TOKEN is not set; the local hub will not take requests it cannot check");
    }
    Ok(token.to_string())
}

/// `HUB_LOCAL_SOCKET`, else `hub/local.sock` under the state directory. mem
/// works out the same path; a test that moves the state directory can never
/// reach the real local hub, and so never posts to Slack.
pub fn socket_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HUB_LOCAL_SOCKET").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(crate::config::state_home()?.join("hub/local.sock"))
}

pub struct Local {
    machine: String,
    token: String,
    /// Answers by question short id, kept for a wait that comes later.
    answers: Mutex<HashMap<String, String>>,
    answered: Condvar,
}

impl Local {
    pub fn new(machine: String, token: String) -> Local {
        Local {
            machine,
            token,
            answers: Mutex::new(HashMap::new()),
            answered: Condvar::new(),
        }
    }

    /// A request on the peer port.
    pub fn peer(&self, request: &Request) -> Response {
        if !bearer_matches(request, &self.token) {
            return Response::text(401, "a peer token is needed");
        }
        self.route(request)
    }

    /// A request on mem's socket.
    pub fn mem(&self, request: &Request) -> Response {
        self.route(request)
    }

    fn route(&self, request: &Request) -> Response {
        let method = request.method.as_str();
        match (method, request.path.as_str()) {
            ("GET", "/ping") => Response::text(200, &self.machine),
            (_, "/ping") => Response::method_not_allowed("GET"),
            ("POST", "/ask") => match serde_json::from_slice::<Asked>(&request.body) {
                Ok(_) => Response::text(200, "ok"),
                Err(e) => Response::text(400, &format!("not a question: {e}")),
            },
            ("POST", "/answer") => match serde_json::from_slice::<Answer>(&request.body) {
                Ok(answer) => {
                    self.deliver(&answer.question, answer.answer);
                    Response::text(200, "ok")
                }
                Err(e) => Response::text(400, &format!("not an answer: {e}")),
            },
            ("GET", path) if path.starts_with("/wait/") => {
                let id = &path["/wait/".len()..];
                let hold = request
                    .query
                    .get("s")
                    .and_then(|s| s.parse().ok())
                    .map(Duration::from_secs)
                    .unwrap_or(MAX_HOLD)
                    .min(MAX_HOLD);
                match self.wait(id, hold) {
                    Some(answer) => Response::new(200, "text/plain; charset=utf-8", answer),
                    None => Response::new(204, "text/plain; charset=utf-8", ""),
                }
            }
            _ => Response::not_found(),
        }
    }

    /// Keeps the answer and wakes every wait on it.
    pub fn deliver(&self, question: &str, answer: String) {
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        answers.insert(question.to_string(), answer);
        self.answered.notify_all();
    }

    /// The answer to `question`, waiting up to `hold` for it.
    fn wait(&self, question: &str, hold: Duration) -> Option<String> {
        let deadline = Instant::now() + hold;
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(answer) = answers.get(question) {
                return Some(answer.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            answers = self
                .answered
                .wait_timeout(answers, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// A question as mem hands it over.
#[derive(Debug, serde::Deserialize)]
pub struct Asked {
    pub id: String,
    pub project: Option<String>,
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    pub audience: Option<String>,
}

/// An answer as a peer hands it in.
#[derive(Debug, serde::Deserialize)]
struct Answer {
    question: String,
    answer: String,
}

/// Whether `Authorization: Bearer <token>` names `token`, compared in
/// constant time.
fn bearer_matches(request: &Request, token: &str) -> bool {
    let Some(given) = request
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (a, b) = (given.trim().as_bytes(), token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Sends one request to the local hub at `target` (`host` or `host:port`)
/// and returns its status and body. With `HUB_SOCKS5` set (a cloud VM's
/// tailscaled), the connection goes through that SOCKS5 listener, by name.
pub fn request(
    target: &str,
    method: &str,
    path: &str,
    token: &str,
    body: &[u8],
) -> Result<(u16, String)> {
    let (host, port) = split_target(target)?;
    let mut stream = match std::env::var("HUB_SOCKS5").ok().filter(|v| !v.is_empty()) {
        Some(proxy) => socks5_connect(&proxy, &host, port)?,
        None => TcpStream::connect((host.as_str(), port))
            .with_context(|| format!("connecting {target}"))?,
    };
    stream.set_read_timeout(Some(PEER_TIMEOUT))?;
    stream.set_write_timeout(Some(PEER_TIMEOUT))?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .with_context(|| format!("{target} sent no status line"))?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
    }
    let mut body = String::new();
    reader.read_to_string(&mut body)?;
    Ok((status, body))
}

fn split_target(target: &str) -> Result<(String, u16)> {
    match target.rsplit_once(':') {
        Some((host, port)) => Ok((
            host.to_string(),
            port.parse().with_context(|| format!("port in {target}"))?,
        )),
        None => Ok((target.to_string(), PORT)),
    }
}

/// A SOCKS5 CONNECT with no authentication, naming the host so the proxy
/// resolves it (tailscaled knows MagicDNS names; this machine may not).
fn socks5_connect(proxy: &str, host: &str, port: u16) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(proxy)
        .with_context(|| format!("connecting the SOCKS5 proxy {proxy}"))?;
    stream.set_read_timeout(Some(PEER_TIMEOUT))?;
    stream.write_all(&[5, 1, 0])?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice)?;
    if choice != [5, 0] {
        bail!("SOCKS5 proxy {proxy} wants authentication");
    }
    let name = host.as_bytes();
    if name.len() > 255 {
        bail!("host name too long for SOCKS5: {host}");
    }
    let mut connect = vec![5, 1, 0, 3, name.len() as u8];
    connect.extend_from_slice(name);
    connect.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&connect)?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply)?;
    if reply[1] != 0 {
        bail!(
            "SOCKS5 proxy {proxy} could not reach {host}:{port} (code {})",
            reply[1]
        );
    }
    let rest = match reply[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len)?;
            len[0] as usize + 2
        }
        other => bail!("SOCKS5 proxy {proxy} sent address type {other}"),
    };
    let mut skip = vec![0u8; rest];
    stream.read_exact(&mut skip)?;
    Ok(stream)
}
