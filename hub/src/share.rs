//! Sharing: the local hub holds this machine's one socket to the store and
//! shuttles JSON between it and mem, which owns the replica and the outbox.
//!
//! mem is run as `mem share hello`, `mem share pending` and `mem share apply`
//! with the hub's own environment, argv only. The store token and the Access
//! service token come from the environment and travel only in headers, never
//! in a log line. Evidence and raw files skip the socket: the hub puts them
//! to the project's files store over plain HTTP(S).

use std::collections::HashSet;
use std::net::TcpStream;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use tungstenite::client::IntoClientRequest;
use tungstenite::error::UrlError;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::proc::{self, Ended};
use crate::slack::Slack;

const BACKOFF_START: Duration = Duration::from_secs(1);
/// The outbox should empty within 10 s of the store coming back, so the wait
/// between tries stops growing at 8 s.
const BACKOFF_CAP: Duration = Duration::from_secs(8);
/// Short, so a kick waits at most this long behind a read.
const READ_TICK: Duration = Duration::from_millis(150);
const PING_EVERY: Duration = Duration::from_secs(30);
/// Two pings and more without a pong: the peer is gone, though the kernel has
/// not said so yet (a laptop that slept, a dropped NAT entry).
const QUIET: Duration = Duration::from_secs(75);
/// Frames are handed to `mem share apply` once the socket goes quiet, or once
/// they have waited this long.
const APPLY_WITHIN: Duration = Duration::from_millis(300);

/// The next 00:00 UTC after `now`, which is 06:00 in Dhaka (UTC+6, no DST):
/// when Cloudflare's Free daily counters start again.
pub fn next_reset(now: jiff::Timestamp) -> jiff::Timestamp {
    const DAY: i64 = 86_400;
    jiff::Timestamp::from_second((now.as_second().div_euclid(DAY) + 1) * DAY)
        .expect("a day after a valid time is in range")
}

/// What Saiful is told when the store answers `limit`; the hubweb home shows
/// the same line.
const LIMIT_NOTICE: &str = "Cloudflare limit reached, resets 06:00";

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// A files request may move a 16.8 MB screenshot over a slow uplink.
const FILE_TIMEOUT: Duration = Duration::from_secs(300);
/// A Worker takes a request body of at most 100 MB, so no stored file is
/// larger.
const MAX_FILE: u64 = 100 * 1000 * 1000;

/// How the hub reaches the store: the socket's URL, the bearer, and the
/// Access service token that gets a request through Cloudflare Access at the
/// edge. No Debug, so neither secret can reach a log line.
#[derive(Clone)]
pub struct Store {
    url: String,
    token: String,
    access: Option<(String, String)>,
    agent: ureq::Agent,
}

/// `HUB_STORE_URL` and `HUB_STORE_TOKEN`, when both are set, with
/// `HUB_STORE_ACCESS_ID` and `HUB_STORE_ACCESS_SECRET` when both of those
/// are. From the environment only, so the secrets stay out of `ps`.
pub fn from_env() -> Option<Store> {
    let read = |name| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let access = read("HUB_STORE_ACCESS_ID").zip(read("HUB_STORE_ACCESS_SECRET"));
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(FILE_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    Some(Store {
        url: read("HUB_STORE_URL")?,
        token: read("HUB_STORE_TOKEN")?,
        access,
        agent,
    })
}

impl Store {
    /// What every store request carries, the socket's handshake included.
    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![("Authorization", format!("Bearer {}", self.token))];
        if let Some((id, secret)) = &self.access {
            headers.push(("CF-Access-Client-Id", id.clone()));
            headers.push(("CF-Access-Client-Secret", secret.clone()));
        }
        headers
    }

    /// `<files base>/store/files/<path>`, the base being the socket's scheme
    /// and host: `ws://h:p/store/share` gives `http://h:p`.
    fn file_url(&self, path: &str) -> Result<String> {
        let (scheme, rest) = match self.url.split_once("://") {
            Some(("wss", rest)) => ("https", rest),
            Some(("ws", rest)) => ("http", rest),
            _ => bail!("HUB_STORE_URL is neither ws:// nor wss://"),
        };
        let host = rest.split('/').next().unwrap_or(rest);
        Ok(format!("{scheme}://{host}/store/files/{}", encode(path)))
    }

    /// The bytes the store holds at `path`, None when it holds none.
    pub fn get_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let mut request = self.agent.get(self.file_url(path)?);
        for (name, value) in self.headers() {
            request = request.header(name, value);
        }
        let mut response = request.call().context("reaching the store")?;
        match response.status().as_u16() {
            200 => Ok(Some(
                response
                    .body_mut()
                    .with_config()
                    .limit(MAX_FILE)
                    .read_to_vec()
                    .context("reading the file")?,
            )),
            404 => Ok(None),
            status => bail!("the store answered {status}"),
        }
    }

    /// Stores the bytes at `local` as `path`.
    fn put_file(&self, path: &str, local: &str) -> Result<()> {
        let bytes = std::fs::read(local).with_context(|| format!("reading {local}"))?;
        let mut request = self.agent.put(self.file_url(path)?);
        for (name, value) in self.headers() {
            request = request.header(name, value);
        }
        let response = request.send(&bytes[..]).context("reaching the store")?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            status => bail!("the store answered {status}"),
        }
    }
}

/// Whether `path` is `projects/<id>/(evidence|raw)/...` made of plain
/// segments, the only paths the files store holds.
pub fn is_file_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let ["projects", project, "evidence" | "raw", rest @ ..] = parts.as_slice() else {
        return false;
    };
    let plain = |part: &&str| !matches!(*part, "" | "." | "..");
    plain(project) && !rest.is_empty() && rest.iter().all(plain)
}

/// `path` percent-encoded but for `A-Z a-z 0-9 - . _ ~ /`, as mem encodes it.
fn encode(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Uploads the files mem lists on a thread of its own, so a large one never
/// holds up the socket's reads, and one pass at a time, so a file is never
/// uploaded twice at once.
#[derive(Default)]
struct Uploads {
    pass: Mutex<Pass>,
}

#[derive(Default)]
struct Pass {
    running: bool,
    /// Asked for while a pass ran: the files mem lists may have changed.
    again: bool,
}

impl Uploads {
    fn start(self: &Arc<Self>, store: &Store) {
        let mut pass = self.pass.lock().unwrap_or_else(|e| e.into_inner());
        if pass.running {
            pass.again = true;
            return;
        }
        pass.running = true;
        drop(pass);
        let (uploads, store) = (Arc::clone(self), store.clone());
        let spawned = std::thread::Builder::new()
            .name("hub-share-upload".to_string())
            .spawn(move || {
                loop {
                    upload_pending(&store);
                    let mut pass = uploads.pass.lock().unwrap_or_else(|e| e.into_inner());
                    if !std::mem::take(&mut pass.again) {
                        pass.running = false;
                        return;
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("hub: share: starting an upload: {e}");
            self.pass.lock().unwrap_or_else(|e| e.into_inner()).running = false;
        }
    }
}

/// Puts each file mem lists, and tells mem of each one stored. A file that
/// fails stays listed, and the next kick or connection tries it again.
fn upload_pending(store: &Store) {
    let pending = match mem(&["share", "pending"], None) {
        Ok(pending) => pending,
        Err(e) => {
            eprintln!("hub: share: {e:#}");
            return;
        }
    };
    for line in pending.lines() {
        let Ok(file) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some("file"), Some(path), Some(local)) = (
            file["t"].as_str(),
            file["path"].as_str(),
            file["local"].as_str(),
        ) else {
            continue;
        };
        match store.put_file(path, local) {
            Ok(()) => {
                let path = serde_json::to_string(path).expect("a string serializes");
                // Left unmarked, the file stays listed and is put again.
                if let Err(e) = apply(&[format!(r#"{{"t":"stored","path":{path}}}"#)]) {
                    eprintln!("hub: share: {e:#}");
                }
            }
            Err(e) => eprintln!("hub: share: uploading {path}: {e:#}"),
        }
    }
}

/// Holds the socket for good. A dropped or failed connection is tried again
/// after 1, 2, 4, then 8 s, and a `ready` from the store resets the wait.
/// `kick` is set when mem has put a write in the outbox. After the store
/// answers `limit`, no write is sent until the next 00:00 UTC, on this
/// connection or the next; `slack` hears of it once per Dhaka day.
pub fn run(store: Store, kick: Arc<AtomicBool>, slack: Option<Arc<Slack>>) -> ! {
    let mut backoff = BACKOFF_START;
    let mut paused_until = None;
    let uploads = Arc::new(Uploads::default());
    loop {
        match session(
            &store,
            &uploads,
            &kick,
            &mut backoff,
            &mut paused_until,
            slack.as_deref(),
        ) {
            Ok(()) => eprintln!("hub: share: the store closed the socket"),
            Err(e) => eprintln!("hub: share: disconnected: {e:#}"),
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(BACKOFF_CAP);
    }
}

/// One connection, until the store closes it (`Ok`) or it fails.
fn session(
    store: &Store,
    uploads: &Arc<Uploads>,
    kick: &AtomicBool,
    backoff: &mut Duration,
    paused_until: &mut Option<Timestamp>,
    slack: Option<&Slack>,
) -> Result<()> {
    let url = store.url.as_str();
    let mut request = url.into_client_request().context("reading HUB_STORE_URL")?;
    for (name, value) in store.headers() {
        let value = value
            .parse()
            .with_context(|| format!("the {name} header is not a header value"))?;
        request.headers_mut().insert(name, value);
    }
    let (mut socket, _) = tungstenite::connect(request)
        .map_err(|error| match error {
            tungstenite::Error::Url(UrlError::UnableToConnect(_)) => anyhow!("unable to connect"),
            error => anyhow!(error),
        })
        .context("connecting to the store")?;
    eprintln!("hub: share: connected to {url}");
    read_timeout(&mut socket, READ_TICK).context("timing store reads")?;

    let hello = mem(&["share", "hello"], None)?;
    let hello = hello.lines().next().unwrap_or_default().trim();
    if hello.is_empty() {
        bail!("mem share hello printed nothing");
    }
    socket
        .send(Message::text(hello))
        .context("sending the hello")?;
    // A kick before this point is covered by the full send below.
    kick.store(false, Ordering::SeqCst);
    let mut sent = HashSet::new();
    if paused_until.is_none() {
        send_pending(&mut socket, &mut sent, store, uploads)?;
    }

    let mut heard = Instant::now();
    let mut pinged = Instant::now();
    let mut held: Vec<String> = Vec::new();
    let mut held_since = Instant::now();
    loop {
        let quiet = match socket.read() {
            Ok(message) => {
                heard = Instant::now();
                match message {
                    Message::Text(text) => {
                        if held.is_empty() {
                            held_since = Instant::now();
                        }
                        held.push(text.to_string());
                    }
                    Message::Close(_) => return closing(&held, Ok(())),
                    _ => {}
                }
                false
            }
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                true
            }
            Err(tungstenite::Error::ConnectionClosed) => return closing(&held, Ok(())),
            Err(e) => return closing(&held, Err(e).context("reading the store")),
        };
        if !held.is_empty() && (quiet || held_since.elapsed() >= APPLY_WITHIN) {
            let frames = std::mem::take(&mut held);
            let kinds: Vec<String> = frames.iter().map(|f| kind(f)).collect();
            // mem raised its `since` only for what it committed, so ending
            // here makes the next hello ask the store for the rest.
            apply(&frames)?;
            if kinds.iter().any(|k| k == "ready") {
                *backoff = BACKOFF_START;
            }
            if kinds.iter().any(|k| k == "limit") {
                let at = now();
                *paused_until = Some(next_reset(at));
                notify_once_a_day(slack, at);
            }
            // An ack or a refusal changes the outbox; mem may have more.
            if kinds.iter().any(|k| k == "ack" || k == "refused") {
                kick.store(true, Ordering::SeqCst);
            }
        }
        if paused_until.is_some_and(|until| now() >= until) {
            *paused_until = None;
            // The write the store refused is in `sent` too.
            sent.clear();
            kick.store(true, Ordering::SeqCst);
        }
        // A kick during the pause is dropped: the resume above sends it all.
        if kick.swap(false, Ordering::SeqCst) && paused_until.is_none() {
            send_pending(&mut socket, &mut sent, store, uploads)?;
        }
        if heard.elapsed() >= QUIET {
            bail!("the store was silent for {QUIET:?}");
        }
        if pinged.elapsed() >= PING_EVERY {
            socket
                .send(Message::Ping(Default::default()))
                .context("pinging the store")?;
            pinged = Instant::now();
        }
    }
}

/// The clock. `HUB_SHARE_NOW_FILE` names a file holding an RFC 3339 time that
/// stands in for it, so a test can move past the reset without waiting.
fn now() -> Timestamp {
    std::env::var("HUB_SHARE_NOW_FILE")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or_else(Timestamp::now)
}

/// Posts the limit line to Saiful's DM unless it already went out this Dhaka
/// day. The day is kept in `$XDG_STATE_HOME/hub/limit-notice`, so a restart
/// does not post again, and only once Slack took the post.
fn notify_once_a_day(slack: Option<&Slack>, at: Timestamp) {
    let Some(slack) = slack else { return };
    let Ok(state) = crate::config::state_home() else {
        return;
    };
    let dir = state.join("hub");
    let file = dir.join("limit-notice");
    let day = at
        .to_zoned(TimeZone::fixed(jiff::tz::offset(6)))
        .date()
        .to_string();
    if std::fs::read_to_string(&file).is_ok_and(|seen| seen.trim() == day) {
        return;
    }
    if let Err(e) = slack.post_text(LIMIT_NOTICE) {
        eprintln!("hub: share: posting the limit notice: {e:#}");
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&file, &day)) {
        eprintln!("hub: share: remembering the limit notice: {e}");
    }
}

/// Sends each outbox line not yet sent on this connection. A new connection
/// starts with an empty `sent`, so it resends everything; the store dedupes.
/// Files never go on the socket: when mem lists one, an upload pass starts.
fn send_pending(
    socket: &mut Socket,
    sent: &mut HashSet<String>,
    store: &Store,
    uploads: &Arc<Uploads>,
) -> Result<()> {
    let pending = match mem(&["share", "pending"], None) {
        Ok(pending) => pending,
        Err(e) => {
            eprintln!("hub: share: {e:#}");
            return Ok(());
        }
    };
    let mut files = false;
    for line in pending.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let message = serde_json::from_str::<Value>(line).ok();
        if message.as_ref().is_some_and(|m| m["t"] == "file") {
            files = true;
            continue;
        }
        let id = message
            .and_then(|v| v["id"].as_str().map(str::to_string))
            .unwrap_or_else(|| line.to_string());
        if sent.contains(&id) {
            continue;
        }
        socket
            .send(Message::text(line))
            .context("sending a write")?;
        sent.insert(id);
    }
    if files {
        uploads.start(store);
    }
    Ok(())
}

/// Applies what arrived before the socket ended, then ends the session.
fn closing(held: &[String], end: Result<()>) -> Result<()> {
    if !held.is_empty() {
        apply(held)?;
    }
    end
}

/// Hands the store's messages to mem, one per line, in the order they came.
fn apply(frames: &[String]) -> Result<()> {
    let mut input = frames.join("\n");
    input.push('\n');
    mem(&["share", "apply"], Some(input.as_bytes())).map(drop)
}

fn kind(frame: &str) -> String {
    serde_json::from_str::<Value>(frame)
        .ok()
        .and_then(|v| v["t"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Runs `mem <args>` and returns its stdout, or why it failed.
fn mem(args: &[&str], input: Option<&[u8]>) -> Result<String> {
    let mut command = Command::new("mem");
    command.args(args);
    match proc::output_fed(&mut command, input, crate::memcli::mem_timeout()) {
        Ended::Exited(done) if done.code == Some(0) => {
            Ok(String::from_utf8_lossy(&done.stdout).into_owned())
        }
        Ended::Exited(done) => bail!(
            "mem {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&done.stderr).trim()
        ),
        Ended::TimedOut => bail!("mem {} did not answer and was killed", args.join(" ")),
        Ended::Failed(why) => bail!("could not run mem {}: {why}", args.join(" ")),
    }
}

fn read_timeout(socket: &mut Socket, timeout: Duration) -> std::io::Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(timeout)),
        MaybeTlsStream::Rustls(stream) => stream.get_mut().set_read_timeout(Some(timeout)),
        _ => Ok(()),
    }
}
