//! Slack: questions go to Saiful's DM as buttons, and his taps come back over
//! Socket Mode.
//!
//! The Web API goes through ureq and Socket Mode through tungstenite, both
//! blocking, so `run_socket` owns a thread of its own. The two tokens come
//! from the environment and travel only in `Authorization` headers: never in
//! a URL, an argument, a log line, an error or `Debug`.

use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tungstenite::error::UrlError;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const DEFAULT_API: &str = "https://slack.com/api";
/// Slack cuts a button's text at 75 characters.
const LABEL_MAX: usize = 75;
const API_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(60);
/// How long Socket Mode may stay silent, pings unanswered included, before
/// the connection is taken for dead: a laptop that slept, or a NAT entry that
/// was dropped, leaves a read that would otherwise block for good.
const QUIET: Duration = Duration::from_secs(90);
/// Slack's limits: a section's text, a message's text, and the buttons in
/// one actions block.
const SECTION_MAX: usize = 3000;
const TEXT_MAX: usize = 40_000;
const BUTTONS_MAX: usize = 25;
/// The `event_type` of the metadata that names a posted question.
const METADATA_TYPE: &str = "workflow_question";
/// The largest photo a reply may bring.
const PHOTO_MAX: u64 = 8 * 1024 * 1024;
/// How many of the DM's newest messages the catch-up reads.
const HISTORY: &str = "100";

pub struct Question {
    pub id: String,
    pub project: String,
    pub machine: String,
    pub text: String,
    pub options: Vec<String>,
}

/// Where a posted question lives, for `mark_answered`.
#[derive(Debug)]
pub struct Posted {
    pub channel: String,
    pub ts: String,
}

/// Saiful's tap on a button: the asking machine, the question and the option.
#[derive(Debug)]
pub struct Tap {
    pub machine: String,
    pub question: String,
    pub answer: String,
    pub channel: String,
    pub ts: String,
}

/// Saiful's reply in a question's thread: its words and the photos it brought.
#[derive(Debug)]
pub struct Reply {
    pub channel: String,
    /// The question's message.
    pub thread_ts: String,
    pub question: String,
    /// The asking machine.
    pub machine: String,
    pub text: String,
    pub photos: Vec<Vec<u8>>,
}

/// What a button's `value` carries, kept short for Slack's 2000-character cap.
#[derive(Serialize, Deserialize)]
struct ButtonValue {
    m: String,
    q: String,
    o: String,
}

type Log = Box<dyn Fn(&str) + Send + Sync>;

pub struct Slack {
    bot_token: String,
    app_token: String,
    user_id: String,
    api_url: String,
    agent: ureq::Agent,
    log: Log,
    quiet: Duration,
    /// An HTTP proxy Socket Mode tunnels through, as a cloud VM needs.
    proxy: Option<String>,
}

/// Written by hand so the tokens never reach a `{:?}`.
impl std::fmt::Debug for Slack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slack")
            .field("user_id", &self.user_id)
            .field("api_url", &self.api_url)
            .finish_non_exhaustive()
    }
}

impl Slack {
    /// Logs to stderr; `with_log` sends the lines elsewhere.
    pub fn new(bot_token: String, app_token: String, user_id: String, api_url: String) -> Slack {
        // The system's roots, not a bundled list: a cloud VM's proxy signs
        // with a CA only the system store holds. ureq takes that proxy from
        // the environment by itself.
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(API_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Slack {
            bot_token,
            app_token,
            user_id,
            api_url: api_url.trim_end_matches('/').to_string(),
            agent,
            log: Box::new(|line| eprintln!("{line}")),
            quiet: QUIET,
            proxy: None,
        }
    }

    /// Socket Mode through an HTTP proxy (`http://host:port`), with CONNECT.
    pub fn with_proxy(mut self, proxy: Option<String>) -> Slack {
        self.proxy = proxy;
        self
    }

    /// How long Socket Mode may stay silent; it is pinged at a third of that.
    pub fn with_quiet(mut self, quiet: Duration) -> Slack {
        self.quiet = quiet;
        self
    }

    pub fn with_log(mut self, log: impl Fn(&str) + Send + Sync + 'static) -> Slack {
        self.log = Box::new(log);
        self
    }

    /// `None` when either token is unset: a machine without Slack still runs
    /// its local hub.
    pub fn from_env() -> Result<Option<Slack>> {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        let (Some(bot), Some(app)) = (var("SLACK_BOT_TOKEN"), var("SLACK_APP_TOKEN")) else {
            return Ok(None);
        };
        let Some(user) = var("SLACK_USER_ID") else {
            bail!("SLACK_USER_ID is unset, so no tap could count");
        };
        let api = var("SLACK_API_URL").unwrap_or_else(|| DEFAULT_API.to_string());
        // ureq reads the proxy variables itself; tungstenite needs telling.
        let proxy = var("HTTPS_PROXY").or_else(|| var("https_proxy"));
        Ok(Some(Slack::new(bot, app, user, api).with_proxy(proxy)))
    }

    /// Posts to the bot's DM with Saiful, one button per option.
    pub fn post_question(&self, q: &Question) -> Result<Posted> {
        // Slack refuses a button with no label, and more than 25 in a block.
        let buttons: Vec<Value> = q
            .options
            .iter()
            .enumerate()
            .filter(|(_, option)| !option.trim().is_empty())
            .take(BUTTONS_MAX)
            .map(|(i, option)| {
                let value = ButtonValue {
                    m: q.machine.clone(),
                    q: q.id.clone(),
                    o: option.clone(),
                };
                json!({
                    "type": "button",
                    "action_id": i.to_string(),
                    "text": {"type": "plain_text", "text": label(option)},
                    "value": serde_json::to_string(&value).expect("strings serialize"),
                })
            })
            .collect();
        let mut blocks = vec![
            json!({"type": "section", "text": {"type": "mrkdwn", "text": cut(&format!(
                "*{}* on `{}`\n{}",
                escape(&q.project),
                escape(&q.machine),
                escape(&q.text)
            ), SECTION_MAX)}}),
        ];
        // Slack refuses an actions block with no elements; a question
        // without options is answered in its thread.
        if !buttons.is_empty() {
            blocks.push(json!({"type": "actions", "elements": buttons}));
        }
        // The metadata names the question for a reply in its thread, which
        // a question without buttons has no other way to do.
        let body = json!({
            "channel": self.user_id,
            "text": cut(&format!("{} on {}: {}", q.project, q.machine, q.text), TEXT_MAX),
            "blocks": blocks,
            "metadata": {
                "event_type": METADATA_TYPE,
                "event_payload": {"q": q.id, "m": q.machine},
            },
        });
        let reply = self.call("chat.postMessage", &self.bot_token, Some(&body))?;
        Ok(Posted {
            channel: reply["channel"].as_str().unwrap_or_default().to_string(),
            ts: reply["ts"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// Posts one plain line to the bot's DM with Saiful.
    pub fn post_text(&self, text: &str) -> Result<()> {
        let body = json!({"channel": self.user_id, "text": text});
        self.call("chat.postMessage", &self.bot_token, Some(&body))?;
        Ok(())
    }

    /// Replaces the buttons with the answer, so the message offers no second tap.
    pub fn mark_answered(&self, channel: &str, ts: &str, answer: &str, text: &str) -> Result<()> {
        // The answer whole, as far as it goes, and the question in what is
        // left: Slack refuses a section over its limit, and the buttons stay.
        let answered = cut(&format!("*Answered:* {}", escape(answer)), SECTION_MAX / 2);
        let room = SECTION_MAX - answered.chars().count() - 1;
        let line = format!("{}\n{answered}", cut(&escape(text), room));
        let body = json!({
            "channel": channel,
            "ts": ts,
            "text": cut(&format!("{text} Answered: {answer}"), TEXT_MAX),
            "blocks": [{"type": "section", "text": {"type": "mrkdwn", "text": line}}],
        });
        self.call("chat.update", &self.bot_token, Some(&body))?;
        Ok(())
    }

    /// Holds a Socket Mode connection for good, calling `on_tap` for each of
    /// Saiful's button taps. Slack's `disconnect` reconnects at once; an
    /// error reconnects after a backoff that doubles up to a minute.
    pub fn run_socket(&self, on_tap: impl FnMut(Tap)) -> ! {
        self.listen(on_tap, |_| {})
    }

    /// `run_socket`, calling `on_message` as well with each DM message event,
    /// for `reply` to read off the socket's thread: reading a reply's question
    /// and its photos takes calls to Slack.
    pub fn listen(&self, mut on_tap: impl FnMut(Tap), mut on_message: impl FnMut(Value)) -> ! {
        let mut backoff = BACKOFF_START;
        loop {
            if let Err(error) = self.session(&mut on_tap, &mut on_message, &mut backoff) {
                (self.log)(&format!("slack: socket: {error:#}"));
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }

    /// One connection, until Slack asks for a new one (`Ok`) or it fails.
    fn session(
        &self,
        on_tap: &mut impl FnMut(Tap),
        on_message: &mut impl FnMut(Value),
        backoff: &mut Duration,
    ) -> Result<()> {
        let reply = self.call("apps.connections.open", &self.app_token, None)?;
        let url = reply["url"]
            .as_str()
            .context("apps.connections.open returned no url")?;
        // The URL carries a one-use ticket, and tungstenite's connect error
        // quotes the URL whole, so that error is replaced.
        let connected = match &self.proxy {
            None => tungstenite::connect(url),
            Some(proxy) => through_proxy(proxy, url),
        };
        let (mut socket, _) = connected
            .map_err(|error| match error {
                tungstenite::Error::Url(UrlError::UnableToConnect(_)) => {
                    anyhow!("unable to connect")
                }
                error => anyhow!(error),
            })
            .context("connecting to Socket Mode")?;
        read_timeout(&mut socket, Some(self.quiet / 3)).context("timing Socket Mode reads")?;
        let mut heard = Instant::now();
        loop {
            let message = match socket.read() {
                Ok(message) => {
                    heard = Instant::now();
                    message
                }
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if heard.elapsed() >= self.quiet {
                        bail!("Socket Mode was silent for {:?}", self.quiet);
                    }
                    socket
                        .send(Message::Ping(Default::default()))
                        .context("pinging Socket Mode")?;
                    continue;
                }
                Err(e) => return Err(e).context("reading Socket Mode"),
            };
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(envelope) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            // Slack retries an envelope that is not acked within 3 s, so the
            // ack goes before anything else.
            if let Some(id) = envelope["envelope_id"].as_str() {
                let ack = json!({"envelope_id": id}).to_string();
                socket
                    .send(Message::text(ack))
                    .context("acking an envelope")?;
            }
            match envelope["type"].as_str() {
                Some("hello") => *backoff = BACKOFF_START,
                Some("disconnect") => return Ok(()),
                Some("interactive") => self.interactive(&envelope["payload"], on_tap),
                Some("events_api") if envelope["payload"]["event"]["type"] == "message" => {
                    on_message(envelope["payload"]["event"].clone());
                }
                _ => {}
            }
        }
    }

    fn interactive(&self, payload: &Value, on_tap: &mut impl FnMut(Tap)) {
        if payload["type"] != "block_actions" {
            return;
        }
        let Some(value) = payload["actions"][0]["value"]
            .as_str()
            .and_then(|v| serde_json::from_str::<ButtonValue>(v).ok())
        else {
            return;
        };
        let user = payload["user"]["id"].as_str().unwrap_or_default();
        if user != self.user_id {
            (self.log)(&format!(
                "slack: refused a tap by {user} on question {}",
                value.q
            ));
            return;
        }
        let container = &payload["container"];
        on_tap(Tap {
            machine: value.m,
            question: value.q,
            answer: value.o,
            channel: container["channel_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            ts: container["message_ts"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        });
    }

    /// A DM message event, when it is Saiful's reply in the thread of a
    /// posted question.
    pub fn reply(&self, event: &Value) -> Option<Reply> {
        if event["type"] != "message" || !self.by_saiful(event) {
            return None;
        }
        let channel = event["channel"].as_str()?;
        // A parent with replies carries its own ts as `thread_ts`.
        let thread_ts = event["thread_ts"]
            .as_str()
            .filter(|t| Some(*t) != event["ts"].as_str())?;
        let parent = self.form(
            "conversations.replies",
            &[
                ("channel", channel),
                ("ts", thread_ts),
                ("limit", "1"),
                ("include_all_metadata", "true"),
            ],
        );
        let parent = match parent {
            Ok(parent) => parent,
            Err(error) => {
                (self.log)(&format!("slack: reading a reply's question: {error:#}"));
                return None;
            }
        };
        let (question, machine) = question_of(&parent["messages"][0])?;
        Some(self.reply_in(channel, thread_ts, question, machine, event))
    }

    /// Whether `message` is Saiful's own words: never the bot's post, and
    /// never an edit or a deletion.
    fn by_saiful(&self, message: &Value) -> bool {
        message.get("bot_id").is_none()
            && message["user"] == self.user_id.as_str()
            && matches!(message["subtype"].as_str(), None | Some("file_share"))
    }

    fn reply_in(
        &self,
        channel: &str,
        thread_ts: &str,
        question: String,
        machine: String,
        message: &Value,
    ) -> Reply {
        let photos = message["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|file| {
                file["mimetype"]
                    .as_str()
                    .is_some_and(|m| m.starts_with("image/"))
            })
            .filter_map(|file| {
                file["url_private_download"]
                    .as_str()
                    .or(file["url_private"].as_str())
            })
            .filter_map(|url| match self.download(url) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    (self.log)(&format!("slack: a photo for #{question}: {error:#}"));
                    None
                }
            })
            .collect();
        Reply {
            channel: channel.to_string(),
            thread_ts: thread_ts.to_string(),
            question,
            machine,
            text: unescape(message["text"].as_str().unwrap_or_default()),
            photos,
        }
    }

    /// Saiful's replies, oldest first, in the threads of the DM's newest
    /// messages whose question `wanted(question, machine)` picks: what he
    /// wrote while no hub was connected. Slack keeps the DM, so nothing
    /// needs saving here.
    pub fn replies_in_history(&self, wanted: impl Fn(&str, &str) -> bool) -> Result<Vec<Reply>> {
        let opened = self.form("conversations.open", &[("users", &self.user_id)])?;
        let channel = opened["channel"]["id"]
            .as_str()
            .context("conversations.open returned no channel")?;
        let history = self.form(
            "conversations.history",
            &[
                ("channel", channel),
                ("limit", HISTORY),
                ("include_all_metadata", "true"),
            ],
        )?;
        let mut replies = Vec::new();
        for parent in history["messages"].as_array().into_iter().flatten() {
            if parent["reply_count"].as_u64().unwrap_or(0) == 0 {
                continue;
            }
            let Some((question, machine)) = question_of(parent).filter(|(q, m)| wanted(q, m))
            else {
                continue;
            };
            let ts = parent["ts"].as_str().unwrap_or_default();
            let thread = self.form("conversations.replies", &[("channel", channel), ("ts", ts)])?;
            for message in thread["messages"].as_array().into_iter().flatten() {
                if message["ts"] != ts && self.by_saiful(message) {
                    replies.push(self.reply_in(
                        channel,
                        ts,
                        question.clone(),
                        machine.clone(),
                        message,
                    ));
                }
            }
        }
        Ok(replies)
    }

    /// A file Saiful shared, fetched with the bot token (files:read).
    fn download(&self, url: &str) -> Result<Vec<u8>> {
        // The token goes to Slack's file host only, or to the API's own
        // origin, which is where a test's fake Slack serves its files.
        let api_origin = format!("{}/", origin(&self.api_url));
        if !url.starts_with("https://files.slack.com/") && !url.starts_with(&api_origin) {
            bail!("a file outside Slack");
        }
        let mut response = self
            .agent
            .get(url)
            .header("Authorization", &format!("Bearer {}", self.bot_token))
            .call()
            .context("fetching a file")?;
        if !response.status().is_success() {
            bail!("fetching a file: {}", response.status());
        }
        response
            .body_mut()
            .with_config()
            .limit(PHOTO_MAX)
            .read_to_vec()
            .context("reading a file")
    }

    /// POSTs a read method its arguments as a form, which every method
    /// takes. They are ids, timestamps and numbers: nothing to escape.
    fn form(&self, method: &str, args: &[(&str, &str)]) -> Result<Value> {
        let body: Vec<String> = args.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let sent = self
            .agent
            .post(&format!("{}/{method}", self.api_url))
            .header("Authorization", &format!("Bearer {}", self.bot_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(&body.join("&"));
        read_reply(method, sent)
    }

    /// POSTs one Web API method. Slack answers 200 with `ok: false` on most
    /// failures, so `ok` decides.
    fn call(&self, method: &str, token: &str, body: Option<&Value>) -> Result<Value> {
        let url = format!("{}/{method}", self.api_url);
        let request = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {token}"));
        let sent = match body {
            Some(body) => request
                .header("Content-Type", "application/json; charset=utf-8")
                .send(&body.to_string()),
            None => request
                .header("Content-Type", "application/x-www-form-urlencoded")
                .send_empty(),
        };
        read_reply(method, sent)
    }
}

/// A Web API reply, when its `ok` is true.
fn read_reply(
    method: &str,
    sent: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<Value> {
    let mut response = sent.with_context(|| format!("slack {method}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("slack {method}: reading the reply"))?;
    let reply: Value = serde_json::from_str(&text)
        .with_context(|| format!("slack {method}: a reply that is not JSON"))?;
    if reply["ok"] != true {
        let error = reply["error"].as_str().unwrap_or("no error given");
        bail!("slack {method}: {error}");
    }
    Ok(reply)
}

/// The question a posted message asks, and the machine that asked it: from
/// its metadata, else from a button, as a message posted before the
/// metadata carries it.
fn question_of(message: &Value) -> Option<(String, String)> {
    let metadata = &message["metadata"];
    if metadata["event_type"] == METADATA_TYPE {
        let payload = &metadata["event_payload"];
        return Some((
            payload["q"].as_str()?.to_string(),
            payload["m"].as_str()?.to_string(),
        ));
    }
    message["blocks"]
        .as_array()?
        .iter()
        .flat_map(|block| block["elements"].as_array().into_iter().flatten())
        .find_map(|element| serde_json::from_str::<ButtonValue>(element["value"].as_str()?).ok())
        .map(|value| (value.q, value.m))
}

/// A message's text as typed: Slack escapes `&`, `<` and `>`, and `&` last
/// so an escape Saiful typed stays as he typed it.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `scheme://host[:port]` of `url`.
fn origin(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    match url[after_scheme..].find('/') {
        Some(slash) => &url[..after_scheme + slash],
        None => url,
    }
}

/// Opens `url` through the HTTP proxy at `proxy` with CONNECT, then speaks
/// TLS (against the system's roots, which hold a cloud proxy's CA) and the
/// WebSocket handshake over the tunnel.
fn through_proxy(
    proxy: &str,
    url: &str,
) -> tungstenite::Result<(
    WebSocket<MaybeTlsStream<TcpStream>>,
    tungstenite::handshake::client::Response,
)> {
    use std::io::{BufRead, BufReader, Write};

    let unable = || tungstenite::Error::Url(UrlError::UnableToConnect(String::new()));
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(unable)?;
    let authority = rest.split('/').next().unwrap_or_default();
    let target = if authority.contains(':') {
        authority.to_string()
    } else if url.starts_with("wss://") {
        format!("{authority}:443")
    } else {
        format!("{authority}:80")
    };
    let proxy_at = proxy
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let mut stream = TcpStream::connect(&proxy_at)?;
    write!(
        stream,
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"
    )?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(unable());
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
    }
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|b| {
            use rustls_platform_verifier::BuilderVerifierExt;
            b.with_platform_verifier()
        })
        .map_err(|e| tungstenite::Error::Tls(e.into()))?
        .with_no_client_auth();
    let connector = tungstenite::Connector::Rustls(std::sync::Arc::new(config));
    tungstenite::client_tls_with_config(url, stream, None, Some(connector)).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => e,
        tungstenite::HandshakeError::Interrupted(_) => unable(),
    })
}

fn read_timeout(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Option<Duration>,
) -> std::io::Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
        MaybeTlsStream::Rustls(stream) => stream.get_mut().set_read_timeout(timeout),
        _ => Ok(()),
    }
}

fn label(option: &str) -> String {
    cut(option, LABEL_MAX)
}

/// `text` whole when it fits in `max` characters, else cut to fit with `…`.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// Slack's mrkdwn reads `&`, `<` and `>` as markup.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
