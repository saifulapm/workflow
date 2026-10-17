//! Sharing: the local hub holds this machine's one socket to the store and
//! shuttles JSON between it and mem, which owns the replica and the outbox.
//!
//! mem is run as `mem share hello`, `mem share pending` and `mem share apply`
//! with the hub's own environment, argv only. The store token and the Access
//! service token come from the environment and travel only in headers, never
//! in a log line. Evidence and raw files skip the socket: the hub puts them
//! to the project's files store over plain HTTP(S).

use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;

use crate::localroute::Stamped;
use tungstenite::Message;
use tungstenite::client::IntoClientRequest;
use tungstenite::error::UrlError;
use tungstenite::stream::MaybeTlsStream;

use crate::proc::{self, Ended};
use crate::slack::Slack;

const BACKOFF_START: Duration = Duration::from_secs(1);
/// The outbox should empty within 10 s of the store coming back, so the wait
/// between tries stops growing at 8 s.
const BACKOFF_CAP: Duration = Duration::from_secs(8);
/// Short, so a kick waits at most this long behind a read.
const READ_TICK: Duration = Duration::from_millis(150);
/// Liveness is a WebSocket protocol ping, never a message: Cloudflare's
/// runtime answers it without calling webSocketMessage or waking a
/// hibernated store ("Automatic ping/pong handling" in
/// developers.cloudflare.com/durable-objects/best-practices/websockets), so
/// an idle machine costs the store no requests.
const PING_EVERY: Duration = Duration::from_secs(30);

/// `PING_EVERY`, or `HUB_SHARE_PING_MS` when set, so a test can watch an idle
/// hub ping without waiting minutes. Read once.
fn ping_every() -> Duration {
    static EVERY: OnceLock<Duration> = OnceLock::new();
    *EVERY.get_or_init(|| {
        std::env::var("HUB_SHARE_PING_MS")
            .ok()
            .and_then(|ms| ms.trim().parse().ok())
            .map_or(PING_EVERY, Duration::from_millis)
    })
}
/// Two pings and more without a pong: the peer is gone, though the kernel has
/// not said so yet (a laptop that slept, a dropped NAT entry).
pub(crate) const QUIET: Duration = Duration::from_secs(75);
/// Frames are handed to `mem share apply` once the socket goes quiet, or once
/// they have waited this long.
const APPLY_WITHIN: Duration = Duration::from_millis(300);

/// The next 00:00 UTC after `now`, which is 06:00 in Dhaka (UTC+6, no DST):
/// when the store's day counters start again, and Cloudflare's if it is
/// on a plan that has daily limits.
pub fn next_reset(now: jiff::Timestamp) -> jiff::Timestamp {
    const DAY: i64 = 86_400;
    jiff::Timestamp::from_second((now.as_second().div_euclid(DAY) + 1) * DAY)
        .expect("a day after a valid time is in range")
}

/// What Saiful is told when the store answers `limit`; the hubweb home shows
/// the same line.
const LIMIT_NOTICE: &str = "Cloudflare limit reached, resets 06:00";
/// The same, when the `limit` frame says `cap`: the store's own day cap.
const CAP_NOTICE: &str = "The store's day cap is reached, writes wait until 06:00";

pub(crate) type Socket = crate::proxy::Socket;

/// The store could not be reached at all: no answer, no route, or a proxy
/// that would not open the way.
#[derive(Debug)]
struct Unreachable;

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the store cannot be reached")
    }
}

impl std::error::Error for Unreachable {}

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
    /// The store object to reach, `main` for the real store; unset, the
    /// Worker picks it by host.
    name: Option<String>,
    access: Option<(String, String)>,
    /// The cloud VM's HTTP proxy, which the socket tunnels through.
    proxy: Option<String>,
    pub(crate) agent: ureq::Agent,
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
        name: read("HUB_STORE_NAME"),
        access,
        proxy: crate::proxy::from_env(),
        agent,
    })
}

impl Store {
    /// What every store request carries, the socket's handshake included.
    pub(crate) fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![("Authorization", format!("Bearer {}", self.token))];
        if let Some(name) = &self.name {
            headers.push(("X-Hub-Store", name.clone()));
        }
        if let Some((id, secret)) = &self.access {
            headers.push(("CF-Access-Client-Id", id.clone()));
            headers.push(("CF-Access-Client-Secret", secret.clone()));
        }
        headers
    }

    /// `<files base>/store/files/<path>`.
    pub(crate) fn file_url(&self, path: &str) -> Result<String> {
        Ok(format!("{}/store/files/{}", self.base()?, encode(path)))
    }

    /// The store's web address, the socket's scheme and host: `ws://h:p/store/share`
    /// gives `http://h:p`.
    pub(crate) fn base(&self) -> Result<String> {
        let (scheme, rest) = match self.url.split_once("://") {
            Some(("wss", rest)) => ("https", rest),
            Some(("ws", rest)) => ("http", rest),
            _ => bail!("HUB_STORE_URL is neither ws:// nor wss://"),
        };
        let host = rest.split('/').next().unwrap_or(rest);
        Ok(format!("{scheme}://{host}"))
    }

    /// A single-use link that pairs a device with the hub web, from a code
    /// the store makes for the holder of its token.
    pub fn pair_link(&self) -> Result<String> {
        let base = self.base()?;
        let mut request = self.agent.post(format!("{base}/store/pair"));
        for (name, value) in self.headers() {
            request = request.header(name, value);
        }
        let mut answer = request
            .send_empty()
            .context("asking the store for a code")?;
        let status = answer.status().as_u16();
        let code = answer.body_mut().read_to_string().unwrap_or_default();
        if status != 200 {
            bail!("the store answered {status} to /store/pair");
        }
        Ok(format!("{base}/pair/{}", code.trim()))
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
    pub(crate) fn put_file(&self, path: &str, local: &str) -> Result<()> {
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
/// after 1, 2, 4, then 8 s, and a `ready` from the store resets the wait; a
/// store that cannot be reached at all is tried every second.
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
            // Cut off costs the store nothing, so it is tried every second:
            // the outbox goes out, and other writes come in, within 2 s of
            // the way back.
            Err(e) if e.chain().any(|c| c.is::<Unreachable>()) => {
                eprintln!("hub: share: disconnected: {e:#}");
                std::thread::sleep(BACKOFF_START);
                continue;
            }
            Err(e) => eprintln!("hub: share: disconnected: {e:#}"),
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(BACKOFF_CAP);
    }
}

/// The socket to the store, its handshake carrying the bearer and the
/// Access service token.
pub(crate) fn connect(store: &Store) -> Result<Socket> {
    let url = store.url.as_str();
    let mut request = url.into_client_request().context("reading HUB_STORE_URL")?;
    for (name, value) in store.headers() {
        let value = value
            .parse()
            .with_context(|| format!("the {name} header is not a header value"))?;
        request.headers_mut().insert(name, value);
    }
    let (socket, _) = crate::proxy::connect(request, store.proxy.as_deref())
        .map_err(|error| match error {
            // A host that cannot be reached, or a proxy that will not reach
            // it: the store is cut off, not refusing.
            tungstenite::Error::Url(UrlError::UnableToConnect(_)) => anyhow!(Unreachable),
            tungstenite::Error::Io(e) => anyhow!(e).context(Unreachable),
            error => anyhow!(error),
        })
        .context("connecting to the store")?;
    eprintln!("hub: share: connected to {url}");
    Ok(socket)
}

/// `hub share sync`: one connection that brings the replica up to the
/// store's `ready` and ends, landing every project's writes, since a cloud
/// session's replica starts with no project to say it shares one. Nothing
/// of the outbox is sent. Returns the frames applied.
pub fn sync(store: &Store) -> Result<usize> {
    /// Frames handed to mem at once, so a long replay is not one stdin.
    const BATCH: usize = 500;
    let mut socket = connect(store)?;
    read_timeout(&mut socket, QUIET).context("timing store reads")?;
    let hello = mem(&["share", "hello"], None)?;
    let hello = hello.lines().next().unwrap_or_default().trim();
    if hello.is_empty() {
        bail!("mem share hello printed nothing");
    }
    socket
        .send(Message::text(hello))
        .context("sending the hello")?;
    let mut held = Vec::new();
    let mut applied = 0;
    loop {
        let text = match socket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => {
                bail!("the store closed the socket before it was ready")
            }
            Ok(_) => continue,
            Err(e) => return Err(e).context("reading the store"),
        };
        // Routes and commands are the long-running hub's business.
        let what = kind(&text);
        if what == "route" || what == "cmd" {
            continue;
        }
        held.push(text);
        if what == "ready" || held.len() >= BATCH {
            let mut input = held.join("\n");
            input.push('\n');
            mem(&["share", "apply", "--all"], Some(input.as_bytes()))?;
            applied += held.len();
            held.clear();
        }
        if what == "ready" {
            let _ = socket.close(None);
            return Ok(applied);
        }
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
    let mut socket = connect(store)?;
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

    // The route this socket last carried either way; a newer one goes out.
    let mut told: Option<Stamped> = None;
    // The number of the last report this socket carried; a new socket
    // carries the newest at once.
    let mut reported = 0;
    // Commands heard behind recs not yet applied.
    let mut commands: Vec<String> = Vec::new();
    let mut heard = Instant::now();
    let mut pinged = Instant::now();
    let mut held: Vec<String> = Vec::new();
    let mut held_since = Instant::now();
    loop {
        let quiet = match socket.read() {
            Ok(message) => {
                heard = Instant::now();
                match message {
                    Message::Text(text) if kind(&text) == "route" => {
                        told = take_route(&text).or(told);
                    }
                    // A command runs after the recs that came before it,
                    // which it may read (an approval, then Start).
                    Message::Text(text) if kind(&text) == "cmd" => {
                        if held.is_empty() {
                            crate::runs::take(&text);
                        } else {
                            commands.push(text.to_string());
                        }
                    }
                    Message::Text(text) => {
                        say_why(&text);
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
            for command in commands.drain(..) {
                crate::runs::take(&command);
            }
            if kinds.iter().any(|k| k == "ready") {
                *backoff = BACKOFF_START;
            }
            if kinds.iter().any(|k| k == "limit") {
                let at = now();
                *paused_until = Some(next_reset(at));
                let cap = frames.iter().any(|f| {
                    serde_json::from_str::<Value>(f)
                        .is_ok_and(|v| v["t"] == "limit" && v["cap"] == true)
                });
                notify_once_a_day(slack, at, if cap { CAP_NOTICE } else { LIMIT_NOTICE });
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
        if let Some(route) = route_to_tell(told.as_ref()) {
            socket
                .send(Message::text(route_message(&route)))
                .context("sending the route")?;
            told = Some(route);
        }
        // A report is no write: it goes out past a Free limit too, as the
        // store keeps it off its log.
        for done in crate::runs::dones() {
            socket.send(Message::text(done)).context("sending a done")?;
        }
        if let Some((n, report)) = crate::fleet::to_send(reported) {
            socket
                .send(Message::text(report))
                .context("sending the report")?;
            reported = n;
        }
        if heard.elapsed() >= QUIET {
            bail!("the store was silent for {QUIET:?}");
        }
        if pinged.elapsed() >= ping_every() {
            socket
                .send(Message::Ping(Default::default()))
                .context("pinging the store")?;
            pinged = Instant::now();
        }
    }
}

/// The clock. `HUB_SHARE_NOW_FILE` names a file holding an RFC 3339 time that
/// stands in for it, so a test can move past the reset without waiting.
pub(crate) fn now() -> Timestamp {
    std::env::var("HUB_SHARE_NOW_FILE")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or_else(Timestamp::now)
}

/// Posts `notice` to Saiful's DM unless it already went out this Dhaka
/// day. The day is kept in `$XDG_STATE_HOME/hub/limit-notice`, so a restart
/// does not post again, and only once Slack took the post.
fn notify_once_a_day(slack: Option<&Slack>, at: Timestamp, notice: &str) {
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
    if let Err(e) = slack.post_text(notice) {
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

/// Logs why the store answered with an error or refused a write: mem keeps
/// the one in its outbox and the other as a conflict copy, and says
/// nothing of the reason.
fn say_why(frame: &str) {
    let Ok(message) = serde_json::from_str::<Value>(frame) else {
        return;
    };
    let Some(why) = message["why"].as_str() else {
        return;
    };
    match message["t"].as_str() {
        Some("error") => eprintln!("hub: share: the store said: {why}"),
        Some("refused") => eprintln!(
            "hub: share: the store refused {}: {why}",
            message["id"].as_str().unwrap_or("a write")
        ),
        _ => {}
    }
}

/// Applies what arrived before the socket ended, then ends the session.
fn closing(held: &[String], end: Result<()>) -> Result<()> {
    if !held.is_empty() {
        apply(held)?;
    }
    end
}

/// Run after each batch mem applies: the local hub ends its waits early, so a
/// waiting mem finds an answer that came through the store at once.
static APPLIED: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// Sets what runs after each applied batch; the first call wins.
pub fn when_applied(f: impl Fn() + Send + Sync + 'static) {
    let _ = APPLIED.set(Box::new(f));
}

/// Hands the store's messages to mem, one per line, in the order they came.
fn apply(frames: &[String]) -> Result<()> {
    let mut input = frames.join("\n");
    input.push('\n');
    mem(&["share", "apply"], Some(input.as_bytes()))?;
    if let Some(applied) = APPLIED.get() {
        applied();
    }
    Ok(())
}

/// The local hubs' route: how to read it, and how to offer one the store
/// sent, which is taken only when it is newer.
struct Routes {
    get: Box<dyn Fn() -> Stamped + Send + Sync>,
    offer: Box<dyn Fn(Stamped) -> bool + Send + Sync>,
}

static ROUTES: OnceLock<Routes> = OnceLock::new();

/// Lets the socket carry this machine's route to the store and take the
/// store's; the first call wins. Without it no route travels.
pub fn when_routed(
    get: impl Fn() -> Stamped + Send + Sync + 'static,
    offer: impl Fn(Stamped) -> bool + Send + Sync + 'static,
) {
    let _ = ROUTES.set(Routes {
        get: Box::new(get),
        offer: Box::new(offer),
    });
}

/// Offers the store's route to this machine, and returns the route the hub
/// now holds when it took it, so it is not sent back.
fn take_route(frame: &str) -> Option<Stamped> {
    let routes = ROUTES.get()?;
    let theirs: Stamped = serde_json::from_str(frame).ok()?;
    theirs.at?;
    (routes.offer)(theirs).then(|| (routes.get)())
}

/// This machine's route when it was chosen (it has a stamp) and differs from
/// the one the socket last carried.
fn route_to_tell(told: Option<&Stamped>) -> Option<Stamped> {
    let now = (ROUTES.get()?.get)();
    (now.at.is_some_and(|at| at > 0) && Some(&now) != told).then_some(now)
}

fn route_message(route: &Stamped) -> String {
    let mut message = serde_json::to_value(route).unwrap_or_default();
    message["t"] = "route".into();
    message.to_string()
}

pub(crate) fn kind(frame: &str) -> String {
    serde_json::from_str::<Value>(frame)
        .ok()
        .and_then(|v| v["t"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Runs `mem <args>` and returns its stdout, or why it failed.
pub(crate) fn mem(args: &[&str], input: Option<&[u8]>) -> Result<String> {
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

pub(crate) fn read_timeout(socket: &mut Socket, timeout: Duration) -> std::io::Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(timeout)),
        MaybeTlsStream::Rustls(stream) => stream.get_mut().set_read_timeout(Some(timeout)),
        _ => Ok(()),
    }
}
