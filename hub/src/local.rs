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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::http::{Request, Response};
use crate::slack::{Question, Slack, Tap};

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

/// The name other local hubs reach this one by: `HUB_HOST`, else this
/// machine's tailnet name, else its machine name, with the port when it is not
/// the usual one. A Slack button carries it, so a tap finds its way back.
pub fn host(machine: &str, port: u16) -> String {
    let name = std::env::var("HUB_HOST")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(crate::tailnet_name)
        .unwrap_or_else(|| machine.to_string());
    if port == PORT {
        name
    } else {
        format!("{name}:{port}")
    }
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
    /// What `host` gives: the name peers and Slack buttons use.
    host: String,
    token: String,
    slack: Option<Arc<Slack>>,
    /// Whether this machine's mem writes its own store; a cloud session's
    /// runs on the mem server, which refuses an answer.
    records: bool,
    /// The questions asked on this machine, by short id.
    asked: Mutex<HashMap<String, Asked>>,
    /// Answers by question short id, kept for a wait that comes later.
    answers: Mutex<HashMap<String, String>>,
    answered: Condvar,
    /// Set by mem's `POST /share/kick`, taken by the sharing thread.
    kick: Arc<AtomicBool>,
}

impl Local {
    pub fn new(machine: String, host: String, token: String, slack: Option<Arc<Slack>>) -> Local {
        let remote = std::env::var_os("MEM_REMOTE").is_some_and(|v| !v.is_empty());
        Local {
            machine,
            host,
            token,
            slack,
            records: !remote,
            asked: Mutex::new(HashMap::new()),
            answers: Mutex::new(HashMap::new()),
            answered: Condvar::new(),
            kick: Arc::new(AtomicBool::new(false)),
        }
    }

    /// What the sharing thread watches for mem's kicks.
    pub fn kick(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.kick)
    }

    /// A request on the peer port.
    pub fn peer(&self, request: &Request) -> Response {
        if !bearer_matches(request, &self.token) {
            return Response::text(401, "a peer token is needed");
        }
        self.route(request)
    }

    /// A request on mem's socket. Only mem may kick the sharing thread, so
    /// the peer port never serves `/share/kick`.
    pub fn mem(&self, request: &Request) -> Response {
        match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/share/kick") => {
                self.kick.store(true, Ordering::SeqCst);
                Response::new(204, "text/plain; charset=utf-8", "")
            }
            _ => self.route(request),
        }
    }

    fn route(&self, request: &Request) -> Response {
        let method = request.method.as_str();
        match (method, request.path.as_str()) {
            ("GET", "/ping") => Response::text(200, &self.machine),
            (_, "/ping") => Response::method_not_allowed("GET"),
            ("POST", "/ask") => match serde_json::from_slice::<Asked>(&request.body) {
                Ok(asked) => {
                    self.post(&asked);
                    self.asked
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(asked.id.clone(), asked);
                    Response::text(200, "ok")
                }
                Err(e) => Response::text(400, &format!("not a question: {e}")),
            },
            ("POST", "/answer") => match serde_json::from_slice::<Answer>(&request.body) {
                // Only a Slack tap travels between hubs; an answer claiming
                // any other surface would be recorded under a name nobody
                // checked.
                Ok(answer) if answer.by != "slack" => {
                    Response::text(400, "only a Slack tap travels between hubs")
                }
                Ok(answer) if self.is_review(&answer.question) => {
                    Response::text(403, "a roadmap review is Saiful's to answer, on the hub")
                }
                Ok(answer) => match self.deliver(&answer.question, &answer.answer, &answer.by) {
                    Ok(()) => Response::text(200, "ok"),
                    // The answer that stands, so the tapping hub can show it.
                    Err(standing) => Response::text(409, &standing),
                },
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

    /// Whether `question`, asked here, is a roadmap review: answering it
    /// starts work, so it is answered on the hub and never by a tap.
    fn is_review(&self, question: &str) -> bool {
        self.asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(question)
            .is_some_and(Asked::is_review)
    }

    /// Posts a person's question to Slack, on a thread of its own so mem is
    /// not kept waiting. An orchestrator's question never rings, and a
    /// roadmap review stays on the hub.
    fn post(&self, asked: &Asked) {
        let Some(slack) = self.slack.clone() else {
            return;
        };
        if asked.audience.as_deref() == Some("orchestrator") || asked.is_review() {
            return;
        }
        let question = Question {
            id: asked.id.clone(),
            project: asked.project.clone().unwrap_or_default(),
            machine: self.host.clone(),
            text: asked.text.clone(),
            options: asked.options.clone(),
        };
        std::thread::spawn(move || match slack.post_question(&question) {
            Ok(_) => eprintln!("hub: local: #{} posted to Slack", question.id),
            Err(e) => eprintln!("hub: local: #{} not posted to Slack: {e:#}", question.id),
        });
    }

    /// Saiful's tap: the answer goes to the hub that asked, this one or a
    /// peer, and the message loses its buttons once that hub has it.
    pub fn on_tap(&self, tap: Tap) {
        // The answer the message ends up showing: this tap's, or the one
        // that already stood, so a late tap still clears the buttons.
        let shown = if tap.machine == self.host {
            match self.deliver(&tap.question, &tap.answer, "slack") {
                Ok(()) => Some(tap.answer.clone()),
                Err(standing) => Some(standing),
            }
        } else {
            let body = serde_json::json!({
                "question": tap.question,
                "answer": tap.answer,
                "by": "slack",
            })
            .to_string();
            match request(
                &tap.machine,
                "POST",
                "/answer",
                &self.token,
                body.as_bytes(),
            ) {
                Ok((200, _)) => Some(tap.answer.clone()),
                Ok((409, standing)) => Some(standing.trim().to_string()),
                Ok((status, _)) => {
                    eprintln!(
                        "hub: local: {} answered {status} to the answer to #{}",
                        tap.machine, tap.question
                    );
                    None
                }
                Err(e) => {
                    eprintln!(
                        "hub: local: the answer to #{} did not reach {}: {e:#}",
                        tap.question, tap.machine
                    );
                    None
                }
            }
        };
        let Some(shown) = shown else {
            return;
        };
        eprintln!("hub: local: #{} answered from Slack", tap.question);
        if let Some(slack) = &self.slack {
            let text = self
                .asked
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&tap.question)
                .map(|a| a.text.clone())
                .unwrap_or_else(|| format!("#{}", tap.question));
            if let Err(e) = slack.mark_answered(&tap.channel, &tap.ts, &shown, &text) {
                eprintln!("hub: local: #{} not marked answered: {e:#}", tap.question);
            }
        }
    }

    /// Keeps the answer and wakes every wait on it, then records it in this
    /// machine's store when the question was asked here, signed by the
    /// surface that took it. The first answer stands: when the question
    /// already has one, here or in the store (a terminal, the hub web), nothing
    /// is done and that answer comes back as the error.
    pub fn deliver(&self, question: &str, answer: &str, by: &str) -> Result<(), String> {
        let project = self
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(question)
            .map(|a| a.project.clone());
        let asked_here = project.is_some();
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(standing) = answers.get(question) {
            return Err(standing.clone());
        }
        if self.records
            && let Some(project) = &project
            && let Some(standing) = answered_in_store(project.as_deref(), question)
        {
            answers.insert(question.to_string(), standing.clone());
            self.answered.notify_all();
            return Err(standing);
        }
        answers.insert(question.to_string(), answer.to_string());
        self.answered.notify_all();
        drop(answers);
        if self.records && asked_here {
            // `--`, so an option that starts with a dash is not read as a flag.
            let argv = ["answer", "--by", by, "--", question, answer].map(str::to_string);
            std::thread::spawn(move || {
                match std::process::Command::new("mem")
                    .args(&argv)
                    .stdin(std::process::Stdio::null())
                    .output()
                {
                    Ok(out) if out.status.success() => {}
                    Ok(out) => eprintln!(
                        "hub: local: mem answer {} failed: {}",
                        argv[4],
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                    Err(e) => eprintln!("hub: local: mem answer {}: {e}", argv[4]),
                }
            });
        }
        Ok(())
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

impl Asked {
    /// The hub's own rule for a roadmap review, as the connector applies it.
    fn is_review(&self) -> bool {
        let text = self.text.trim();
        crate::page_control::is_approval(text)
            || (self.options.iter().any(|o| o == "approve")
                && text.to_lowercase().contains("roadmap"))
    }
}

/// An answer as a peer hands it in, with the surface that took it.
#[derive(Debug, serde::Deserialize)]
struct Answer {
    question: String,
    answer: String,
    by: String,
}

/// The answer this machine's store already holds for `question`, if any.
fn answered_in_store(project: Option<&str>, question: &str) -> Option<String> {
    let mut command = std::process::Command::new("mem");
    if let Some(project) = project {
        command.arg(format!("--project={project}"));
    }
    let out = command
        .args(["questions", "--json"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    listed["questions"]
        .as_array()?
        .iter()
        .find(|q| q["short_id"] == question)
        .filter(|q| q["answered"] == true)
        .and_then(|q| q["answer"].as_str())
        .map(str::to_string)
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
        None => connect_within(&host, port).with_context(|| format!("connecting {target}"))?,
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

/// A connect that gives up after `PEER_TIMEOUT`, so a peer that is listed
/// but off costs seconds, not the system's two minutes.
fn connect_within(host: &str, port: u16) -> std::io::Result<TcpStream> {
    use std::net::ToSocketAddrs;
    let mut last = std::io::Error::other("no address");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, PEER_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e,
        }
    }
    Err(last)
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
    let (proxy_host, proxy_port) = split_target(proxy)?;
    let mut stream = connect_within(&proxy_host, proxy_port)
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
