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
        }
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
        Ok(Some(Slack::new(bot, app, user, api)))
    }

    /// Posts to the bot's DM with Saiful, one button per option.
    pub fn post_question(&self, q: &Question) -> Result<Posted> {
        let buttons: Vec<Value> = q
            .options
            .iter()
            .enumerate()
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
        let body = json!({
            "channel": self.user_id,
            "text": format!("{} on {}: {}", q.project, q.machine, q.text),
            "blocks": [
                {"type": "section", "text": {"type": "mrkdwn", "text": format!(
                    "*{}* on `{}`\n{}",
                    escape(&q.project),
                    escape(&q.machine),
                    escape(&q.text)
                )}},
                {"type": "actions", "elements": buttons},
            ],
        });
        let reply = self.call("chat.postMessage", &self.bot_token, Some(&body))?;
        Ok(Posted {
            channel: reply["channel"].as_str().unwrap_or_default().to_string(),
            ts: reply["ts"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// Replaces the buttons with the answer, so the message offers no second tap.
    pub fn mark_answered(&self, channel: &str, ts: &str, answer: &str, text: &str) -> Result<()> {
        let line = format!("{}\n*Answered:* {}", escape(text), escape(answer));
        let body = json!({
            "channel": channel,
            "ts": ts,
            "text": format!("{text} Answered: {answer}"),
            "blocks": [{"type": "section", "text": {"type": "mrkdwn", "text": line}}],
        });
        self.call("chat.update", &self.bot_token, Some(&body))?;
        Ok(())
    }

    /// Holds a Socket Mode connection for good, calling `on_tap` for each of
    /// Saiful's button taps. Slack's `disconnect` reconnects at once; an
    /// error reconnects after a backoff that doubles up to a minute.
    pub fn run_socket(&self, mut on_tap: impl FnMut(Tap)) -> ! {
        let mut backoff = BACKOFF_START;
        loop {
            if let Err(error) = self.session(&mut on_tap, &mut backoff) {
                (self.log)(&format!("slack: socket: {error:#}"));
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }

    /// One connection, until Slack asks for a new one (`Ok`) or it fails.
    fn session(&self, on_tap: &mut impl FnMut(Tap), backoff: &mut Duration) -> Result<()> {
        let reply = self.call("apps.connections.open", &self.app_token, None)?;
        let url = reply["url"]
            .as_str()
            .context("apps.connections.open returned no url")?;
        // The URL carries a one-use ticket, and tungstenite's connect error
        // quotes the URL whole, so that error is replaced.
        let (mut socket, _) = tungstenite::connect(url)
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
    if option.chars().count() <= LABEL_MAX {
        return option.to_string();
    }
    let mut cut: String = option.chars().take(LABEL_MAX - 1).collect();
    cut.push('…');
    cut
}

/// Slack's mrkdwn reads `&`, `<` and `>` as markup.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
