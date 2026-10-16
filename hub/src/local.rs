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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine as _;

use crate::http::{Request, Response};
use crate::localroute::{Kind, LocalRoute, Stamped};
use crate::open::{Known, Open};
use crate::share::Store;
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
    /// Every open question this hub knows, asked here or on a peer.
    known: Arc<Known>,
    /// The Slack message (channel, ts) of each question this hub posted, so
    /// an answer from anywhere takes its buttons away.
    posted: Arc<Mutex<HashMap<String, (String, String)>>>,
    /// Where this machine's person questions ring.
    route_now: LocalRoute,
    /// Answers by question short id, kept for a wait that comes later.
    answers: Mutex<HashMap<String, String>>,
    answered: Condvar,
    /// Bumped, under the answers lock, when the store may have gained an
    /// answer some other way; a wait then ends early so mem looks again.
    nudged: AtomicU64,
    /// Set by mem's `POST /share/kick`, taken by the sharing thread.
    kick: Arc<AtomicBool>,
    /// The store, while sharing is on; mem's file fetches go there.
    store: Option<Store>,
}

impl Local {
    pub fn new(machine: String, host: String, token: String, slack: Option<Arc<Slack>>) -> Local {
        let remote = std::env::var_os("MEM_REMOTE").is_some_and(|v| !v.is_empty());
        Local {
            machine,
            known: Arc::new(Known::load(host.clone())),
            posted: Arc::new(Mutex::new(HashMap::new())),
            route_now: LocalRoute::load(),
            host,
            token,
            slack,
            records: !remote,
            asked: Mutex::new(HashMap::new()),
            answers: Mutex::new(HashMap::new()),
            answered: Condvar::new(),
            nudged: AtomicU64::new(0),
            kick: Arc::new(AtomicBool::new(false)),
            store: None,
        }
    }

    pub fn with_store(mut self, store: Option<Store>) -> Local {
        self.store = store;
        self
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

    /// A request on mem's socket. Only mem may kick the sharing thread or
    /// fetch a shared file, so the peer port serves neither.
    pub fn mem(&self, request: &Request) -> Response {
        match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/share/kick") => {
                self.kick.store(true, Ordering::SeqCst);
                // mem kicks after every write, an answer written straight
                // to the store among them.
                self.nudge();
                Response::new(204, "text/plain; charset=utf-8", "")
            }
            ("POST", "/answer") => self.answer_from_mem(request),
            // Asked only here, by mem: a peer could otherwise register a
            // question under any id, a roadmap review's among them.
            ("POST", "/ask") => match serde_json::from_slice::<Asked>(&request.body) {
                Ok(asked) => {
                    let open = Open {
                        id: asked.id.clone(),
                        project: asked.project.clone(),
                        machine: self.host.clone(),
                        text: asked.text.clone(),
                        options: asked.options.clone(),
                        person: asked.audience.as_deref() != Some("orchestrator"),
                        ring: None,
                    };
                    self.known.add(open.clone());
                    tell_peers(
                        &self.host,
                        &self.token,
                        "/question",
                        serde_json::to_vec(&open).unwrap_or_default(),
                        Some((Arc::clone(&self.known), open.id.clone())),
                    );
                    self.post(&asked, open);
                    self.asked
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(asked.id.clone(), asked);
                    Response::text(200, "ok")
                }
                Err(e) => Response::text(400, &format!("not a question: {e}")),
            },
            // A route chosen on this machine; a peer's comes stamped.
            ("POST", "/route") => match serde_json::from_slice::<Stamped>(&request.body) {
                Ok(chosen) if chosen.at.is_none() => {
                    let chosen = self.route_now.choose(chosen, &self.host);
                    let body = serde_json::to_vec(&chosen).unwrap_or_default();
                    self.tell_peers("/route", body.clone());
                    Response::new(200, "application/json", body)
                }
                Ok(_) => self.route(request),
                Err(e) => Response::text(400, &format!("route to phone or device: {e}")),
            },
            ("GET", "/share/file") => self.file(request),
            _ => self.route(request),
        }
    }

    /// A shared project's evidence or raw file from the store, for mem to
    /// keep in its replica: 404 while sharing is off or when the store has
    /// none, 502 when the store cannot be reached or answers otherwise.
    fn file(&self, request: &Request) -> Response {
        let Some(store) = &self.store else {
            return Response::not_found();
        };
        let Some(path) = request
            .query
            .get("path")
            .filter(|path| crate::share::is_file_path(path))
        else {
            return Response::text(400, "expected projects/<id>/evidence|raw/<name>");
        };
        match store.get_file(path) {
            Ok(Some(bytes)) => Response::new(200, "application/octet-stream", bytes),
            Ok(None) => Response::not_found(),
            Err(e) => {
                eprintln!("hub: local: fetching {path}: {e:#}");
                Response::text(502, "the store could not be reached")
            }
        }
    }

    fn route(&self, request: &Request) -> Response {
        let method = request.method.as_str();
        match (method, request.path.as_str()) {
            ("GET", "/ping") => Response::text(200, &self.machine),
            (_, "/ping") => Response::method_not_allowed("GET"),
            // The asking hub offers its question to this, the route device.
            ("POST", "/ring") => match serde_json::from_slice::<Open>(&request.body) {
                Ok(open) if !plain_id(&open.id) => Response::text(400, "not a question id"),
                // This hub rings what it asked itself; a ring in its name for
                // a question it never asked is a forgery.
                Ok(open) if self.asks(&open.id) || open.machine == self.host => {
                    Response::text(409, "this hub asked it, and rings it itself")
                }
                Ok(_) if crate::presence::sample().away() => Response::text(409, "away"),
                Ok(open) => {
                    if self.known.take(open, &self.host) {
                        Response::text(200, "taken")
                    } else {
                        Response::text(409, "it rings elsewhere now")
                    }
                }
                Err(e) => Response::text(400, &format!("not a question: {e}")),
            },
            ("POST", "/question") => match serde_json::from_slice::<Open>(&request.body) {
                Ok(open) if !plain_id(&open.id) => Response::text(400, "not a question id"),
                Ok(open) => {
                    // This hub alone speaks for the questions it asked; its
                    // copy is what tells a roadmap review after a restart.
                    let mine = open.machine == self.host || self.asked_here(&open.id).is_some();
                    if !mine {
                        self.known.add(open);
                    }
                    Response::text(200, "ok")
                }
                Err(e) => Response::text(400, &format!("not a question: {e}")),
            },
            ("POST", "/closed") => match serde_json::from_slice::<Closed>(&request.body) {
                // This hub closes its own questions, when they are answered.
                Ok(closed) if self.asks(&closed.question) => Response::text(200, "ok"),
                Ok(closed) => {
                    self.known.close(&closed.question);
                    Response::text(200, "ok")
                }
                Err(e) => Response::text(400, &format!("not a question id: {e}")),
            },
            ("GET", "/route") => Response::new(
                200,
                "application/json",
                serde_json::to_string(&self.route_now.get()).unwrap_or_default(),
            ),
            // A change made on this machine carries no stamp; a peer's does.
            ("POST", "/route") => match serde_json::from_slice::<Stamped>(&request.body) {
                Ok(theirs) if theirs.at.is_some() => {
                    self.route_now.offer(theirs);
                    Response::text(200, "ok")
                }
                Ok(_) => Response::text(400, "a peer's route carries the time it was chosen"),
                Err(e) => Response::text(400, &format!("route to phone or device: {e}")),
            },
            ("GET", "/questions") => {
                Response::new(200, "application/json", self.known.doc().to_string())
            }
            // A photo Saiful posted alone in the thread of a question asked
            // here, from the hub whose Slack connection got it.
            ("POST", "/evidence") => {
                let given =
                    serde_json::from_slice::<serde_json::Value>(&request.body).unwrap_or_default();
                let (Some(id), Some(data)) = (given["question"].as_str(), given["data"].as_str())
                else {
                    return Response::text(400, "expected a question and its photo's data");
                };
                let Ok(photo) = base64::engine::general_purpose::STANDARD.decode(data) else {
                    return Response::text(400, "the photo is not base64");
                };
                let here = self
                    .asked
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(id)
                    || self.asked_here(id).is_some();
                if !here {
                    return Response::not_found();
                }
                self.on_reply(crate::slack::Reply {
                    channel: String::new(),
                    thread_ts: String::new(),
                    question: id.to_string(),
                    machine: self.host.clone(),
                    text: String::new(),
                    photos: vec![photo],
                });
                Response::text(200, "ok")
            }
            ("POST", "/answer") => match serde_json::from_slice::<Answer>(&request.body) {
                // Only the surfaces a hub passes on travel between hubs; an
                // answer claiming any other would be recorded under a name
                // nobody checked.
                Ok(answer) if !TRAVELS.contains(&answer.by.as_str()) => Response::text(
                    400,
                    "only a Slack, popup or terminal answer travels between hubs",
                ),
                Ok(answer) if self.is_review(&answer.question) => {
                    Response::text(403, REVIEW_REFUSAL)
                }
                // Nowhere to keep it: a tap on an old message whose asker
                // forgot it must not read as taken.
                Ok(answer) if !self.asks(&answer.question) => Response::not_found(),
                Ok(answer) => {
                    let shots = match answer.shots().iter().map(|s| s.bytes()).collect() {
                        Ok(shots) => shots,
                        Err(e) => return Response::text(400, &e),
                    };
                    match self.deliver(&answer.question, &answer.answer, &answer.by, shots) {
                        Ok(()) => Response::text(200, "ok"),
                        // The answer that stands, so the tapping hub can show it.
                        Err(standing) => Response::text(409, &standing),
                    }
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

    /// Whether `question`, asked here, is a roadmap review: answering it
    /// starts work, so it is answered on the hub and never by a tap.
    fn is_review(&self, question: &str) -> bool {
        if let Some(asked) = self
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(question)
        {
            return asked.is_review();
        }
        self.asked_here(question)
            .is_some_and(|open| is_review(&open.text, &open.options))
    }

    /// Whether this hub asked `question`, now or before a restart.
    fn asks(&self, question: &str) -> bool {
        self.asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(question)
            || self.asked_here(question).is_some()
    }

    /// The open question `question`, when this hub asked it: the copy kept
    /// across a restart, for a question asked before it.
    fn asked_here(&self, question: &str) -> Option<Open> {
        self.known.get(question).filter(|o| o.machine == self.host)
    }

    /// An answer given on this machine (`mem answer`, the popup): kept here
    /// when the question was asked here, else passed to the hub that asked
    /// it. A 404 tells mem to write it to the store itself.
    fn answer_from_mem(&self, asked: &Request) -> Response {
        let given = match serde_json::from_slice::<Given>(&asked.body) {
            Ok(given) if !given.by.trim().is_empty() => given,
            Ok(_) => return Response::text(400, "an answer is signed by its surface"),
            Err(e) => return Response::text(400, &format!("not an answer: {e}")),
        };
        let mut shots = Vec::new();
        for path in given.screenshot.iter().chain(&given.screenshots) {
            match std::fs::read(path) {
                Ok(bytes) => shots.push(bytes),
                Err(e) => return Response::text(400, &format!("reading {}: {e}", path.display())),
            }
        }
        let here = self
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&given.question)
            || self.asked_here(&given.question).is_some();
        if here {
            if self.is_review(&given.question) {
                return Response::text(403, REVIEW_REFUSAL);
            }
            return match self.deliver(&given.question, &given.answer, &given.by, shots) {
                Ok(()) => Response::text(200, "ok"),
                Err(standing) => Response::text(409, &standing),
            };
        }
        let Some(open) = self.known.get(&given.question) else {
            return Response::not_found();
        };
        if !TRAVELS.contains(&given.by.as_str()) {
            return Response::text(
                403,
                &format!(
                    "#{} was asked on {}; only a slack, popup or cli answer travels \
                     between machines: answer there, or here with --by cli",
                    given.question, open.machine
                ),
            );
        }
        let mut body = serde_json::json!({
            "question": given.question,
            "answer": given.answer,
            "by": given.by,
        });
        if !shots.is_empty() {
            body["screenshots"] = encoded(&shots);
        }
        match request(
            &open.machine,
            "POST",
            "/answer",
            &self.token,
            body.to_string().as_bytes(),
        ) {
            Ok((status @ (200 | 403 | 409), text)) => Response::text(status, text.trim()),
            Ok((status, _)) => {
                eprintln!(
                    "hub: local: {} answered {status} to the answer to #{}",
                    open.machine, given.question
                );
                Response::text(502, "the asking machine refused the answer")
            }
            Err(e) => {
                eprintln!(
                    "hub: local: the answer to #{} did not reach {}: {e:#}",
                    given.question, open.machine
                );
                Response::text(502, "the asking machine could not be reached")
            }
        }
    }

    /// The route this hub holds.
    pub fn route_now(&self) -> Stamped {
        self.route_now.get()
    }

    /// A route from the store, taken only when it is newer than this hub's.
    pub fn offer_route(&self, theirs: Stamped) -> bool {
        self.route_now.offer(theirs)
    }

    /// Ends every wait early, so each mem waiting looks at its store again.
    pub fn nudge(&self) {
        let _answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        self.nudged.fetch_add(1, Ordering::SeqCst);
        self.answered.notify_all();
    }

    /// Rings a person's question on its route, on a thread of its own so
    /// mem is not kept waiting. An orchestrator's question never rings, and
    /// a roadmap review stays on the hub.
    fn post(&self, asked: &Asked, open: Open) {
        if !open.person || asked.is_review() {
            return;
        }
        let ringer = Ringer {
            slack: self.slack.clone(),
            known: Arc::clone(&self.known),
            posted: Arc::clone(&self.posted),
            host: self.host.clone(),
            token: self.token.clone(),
            question: Question {
                id: asked.id.clone(),
                project: asked.project.clone().unwrap_or_default(),
                machine: self.host.clone(),
                text: asked.text.clone(),
                options: asked.options.clone(),
            },
        };
        let route = self.route_now.get();
        std::thread::spawn(move || ringer.ring(&route, open));
    }

    /// Saiful's tap: the answer goes to the hub that asked, this one or a
    /// peer, and the message loses its buttons once that hub has it.
    pub fn on_tap(&self, tap: Tap) {
        // The answer the message ends up showing: this tap's, or the one
        // that already stood, so a late tap still clears the buttons.
        let shown = if tap.machine == self.host {
            match self.deliver(&tap.question, &tap.answer, "slack", Vec::new()) {
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

    /// Saiful's reply in a question's thread, for the hub that asked, this
    /// one or a peer. Words are the answer, with the first photo as its
    /// screenshot, and the message loses its buttons; a photo alone is
    /// evidence on the question.
    pub fn on_reply(&self, reply: crate::slack::Reply) {
        let id = reply.question;
        if reply.text.trim().is_empty() {
            for photo in &reply.photos {
                if reply.machine != self.host {
                    let body = serde_json::json!({
                        "question": id,
                        "data": base64::engine::general_purpose::STANDARD.encode(photo),
                    })
                    .to_string();
                    match request(
                        &reply.machine,
                        "POST",
                        "/evidence",
                        &self.token,
                        body.as_bytes(),
                    ) {
                        Ok((200, _)) => {}
                        Ok((status, _)) => eprintln!(
                            "hub: local: {} answered {status} to a photo for #{id}",
                            reply.machine
                        ),
                        Err(e) => eprintln!(
                            "hub: local: a photo for #{id} did not reach {}: {e:#}",
                            reply.machine
                        ),
                    }
                    continue;
                }
                let Some(file) = save_shot(&id, 0, photo) else {
                    continue;
                };
                let task = format!("#{id}");
                // In the question's project: the hub runs from $HOME, where
                // mem could not tell which project is meant.
                let project = self
                    .asked
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&id)
                    .and_then(|a| a.project.clone())
                    .or_else(|| self.asked_here(&id).and_then(|o| o.project));
                let mut command = std::process::Command::new("mem");
                if let Some(project) = &project {
                    command.arg(format!("--project={project}"));
                }
                match command
                    .args(["evidence", "add", "--task", &task])
                    .arg(&file)
                    .args(["--note", &format!("photo from Slack for {task}")])
                    .stdin(std::process::Stdio::null())
                    .output()
                {
                    Ok(out) if out.status.success() => {}
                    Ok(out) => eprintln!(
                        "hub: local: mem evidence add for {task} failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                    Err(e) => eprintln!("hub: local: mem evidence add for {task}: {e}"),
                }
                let _ = std::fs::remove_file(file);
            }
            return;
        }
        // Read before the answer closes the question here.
        let text = self
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(|a| a.text.clone())
            .or_else(|| self.known.get(&id).map(|o| o.text))
            .unwrap_or_else(|| format!("#{id}"));
        let shots = reply.photos;
        let shown = if reply.machine == self.host {
            match self.deliver(&id, &reply.text, "slack", shots) {
                Ok(()) => Some(reply.text.clone()),
                Err(standing) => Some(standing),
            }
        } else {
            let mut body = serde_json::json!({
                "question": id,
                "answer": reply.text,
                "by": "slack",
            });
            if !shots.is_empty() {
                body["screenshots"] = encoded(&shots);
            }
            match request(
                &reply.machine,
                "POST",
                "/answer",
                &self.token,
                body.to_string().as_bytes(),
            ) {
                Ok((200, _)) => Some(reply.text.clone()),
                Ok((409, standing)) => Some(standing.trim().to_string()),
                Ok((status, _)) => {
                    eprintln!(
                        "hub: local: {} answered {status} to the answer to #{id}",
                        reply.machine
                    );
                    None
                }
                Err(e) => {
                    eprintln!(
                        "hub: local: the answer to #{id} did not reach {}: {e:#}",
                        reply.machine
                    );
                    None
                }
            }
        };
        let Some(shown) = shown else {
            return;
        };
        eprintln!("hub: local: #{id} answered in its Slack thread");
        if let Some(slack) = &self.slack
            && let Err(e) = slack.mark_answered(&reply.channel, &reply.thread_ts, &shown, &text)
        {
            eprintln!("hub: local: #{id} not marked answered: {e:#}");
        }
    }

    /// Reads what Saiful wrote in the threads of the questions still open
    /// here while no hub was connected: in each thread, the first reply
    /// with words answers. A photo posted alone was filed when it came, so
    /// it is not filed again.
    pub fn catch_up_replies(&self) {
        let Some(slack) = &self.slack else {
            return;
        };
        let replies = slack.replies_in_history(|question, machine| {
            machine == self.host && self.asked_here(question).is_some()
        });
        let replies = match replies {
            Ok(replies) => replies,
            Err(e) => {
                eprintln!("hub: local: reading the Slack DM: {e:#}");
                return;
            }
        };
        let mut answered = std::collections::HashSet::new();
        for reply in replies {
            if reply.text.trim().is_empty() || answered.contains(&reply.question) {
                continue;
            }
            answered.insert(reply.question.clone());
            self.on_reply(reply);
        }
    }

    /// Keeps the answer and wakes every wait on it, then records it in this
    /// machine's store when the question was asked here, signed by the
    /// surface that took it. The first answer stands: when the question
    /// already has one, here or in the store (a terminal, the hub web), nothing
    /// is done and that answer comes back as the error.
    pub fn deliver(
        &self,
        question: &str,
        answer: &str,
        by: &str,
        shots: Vec<Vec<u8>>,
    ) -> Result<(), String> {
        let project = self
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(question)
            .map(|a| a.project.clone())
            .or_else(|| self.asked_here(question).map(|o| o.project));
        let asked_here = project.is_some();
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(standing) = answers.get(question).cloned() {
            drop(answers);
            // A second answer comes from a surface that still showed the
            // question, perhaps a peer that missed the close: close it again.
            if asked_here {
                self.closed(question, &standing);
            }
            return Err(standing);
        }
        if self.records
            && let Some(project) = &project
            && let Some(standing) = answered_in_store(project.as_deref(), question)
        {
            answers.insert(question.to_string(), standing.clone());
            self.answered.notify_all();
            drop(answers);
            self.closed(question, &standing);
            return Err(standing);
        }
        answers.insert(question.to_string(), answer.to_string());
        self.answered.notify_all();
        drop(answers);
        if asked_here {
            self.closed(question, answer);
        }
        if self.records && asked_here {
            let mut argv = vec!["answer".to_string(), "--by".into(), by.into()];
            let files: Vec<PathBuf> = shots
                .iter()
                .enumerate()
                .filter_map(|(n, bytes)| save_shot(question, n, bytes))
                .collect();
            for file in &files {
                argv.extend(["--screenshot".into(), file.display().to_string()]);
            }
            // `--`, so an option that starts with a dash is not read as a flag.
            argv.extend(["--".into(), question.into(), answer.into()]);
            let question = question.to_string();
            std::thread::spawn(move || {
                // Straight to the store: this hub is where mem's answer
                // would be handed, so handing it here again would loop.
                match std::process::Command::new("mem")
                    .args(&argv)
                    .env("MEM_ANSWER_DIRECT", "1")
                    .stdin(std::process::Stdio::null())
                    .output()
                {
                    Ok(out) if out.status.success() => {}
                    Ok(out) => eprintln!(
                        "hub: local: mem answer {question} failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                    Err(e) => eprintln!("hub: local: mem answer {question}: {e}"),
                }
                for file in files {
                    let _ = std::fs::remove_file(file);
                }
            });
        }
        Ok(())
    }

    /// Drops an answered question here and on every peer, so no bar counts
    /// it, and takes the buttons off its Slack message.
    fn closed(&self, question: &str, answer: &str) {
        let text = self.known.get(question).map(|o| o.text);
        self.known.close(question);
        let body = serde_json::json!({ "question": question }).to_string();
        self.tell_peers("/closed", body.into_bytes());
        let posted = self
            .posted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(question);
        if let (Some((channel, ts)), Some(slack)) = (posted, self.slack.clone()) {
            let text = text.unwrap_or_else(|| format!("#{question}"));
            let (question, answer) = (question.to_string(), answer.to_string());
            std::thread::spawn(move || {
                if let Err(e) = slack.mark_answered(&channel, &ts, &answer, &text) {
                    eprintln!("hub: local: #{question} not marked answered: {e:#}");
                }
            });
        }
    }

    fn tell_peers(&self, path: &'static str, body: Vec<u8>) {
        tell_peers(&self.host, &self.token, path, body, None);
    }

    /// Asks every peer for its open questions, once at start, so a hub that
    /// was away learns what was asked meanwhile and forgets what was answered.
    pub fn learn_from_peers(&self) {
        self.catch_up(false);
    }

    /// Takes the newest route and the open questions from every online peer.
    /// At start a peer's news of every question is taken, for an asker that
    /// is away; in a sweep only the questions each peer asked itself, so a
    /// peer that missed a close cannot pass the question on again. A sweep
    /// says nothing of a peer it cannot reach: the start said it once.
    fn catch_up(&self, sweeping: bool) {
        let say = |line: String| {
            if !sweeping {
                eprintln!("{line}");
            }
        };
        for peer in crate::tailnet_peers() {
            if peer == self.host {
                continue;
            }
            match request(&peer, "GET", "/route", &self.token, b"") {
                Ok((200, body)) => match serde_json::from_str::<Stamped>(&body) {
                    Ok(theirs) => {
                        self.route_now.offer(theirs);
                    }
                    Err(e) => say(format!("hub: local: {peer} sent no route: {e}")),
                },
                Ok((status, _)) => say(format!("hub: local: {peer} answered {status} to /route")),
                Err(e) => {
                    say(format!("hub: local: /route did not reach {peer}: {e:#}"));
                    continue;
                }
            }
            match request(&peer, "GET", "/questions", &self.token, b"") {
                Ok((200, body)) => match serde_json::from_str::<Listed>(&body) {
                    Ok(mut listed) => {
                        if sweeping {
                            listed.questions.retain(|o| o.machine == listed.host);
                        }
                        self.known.reconcile(&listed.host, listed.questions);
                    }
                    Err(e) => say(format!("hub: local: {peer} sent no question list: {e}")),
                },
                Ok((status, _)) => {
                    say(format!(
                        "hub: local: {peer} answered {status} to /questions"
                    ));
                }
                Err(e) => say(format!(
                    "hub: local: /questions did not reach {peer}: {e:#}"
                )),
            }
        }
    }

    /// Every `HUB_SWEEP_MS` (30 s), catches up with the peers, then closes
    /// each question asked here that the store already holds an answer for:
    /// one written straight to it, or one from the hub web, which never
    /// passed this hub.
    pub fn sweep(&self) -> ! {
        let every = std::env::var("HUB_SWEEP_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .map_or(Duration::from_secs(30), Duration::from_millis);
        loop {
            std::thread::sleep(every);
            // A machine that slept through questions, listed offline so no
            // asker reached it, catches up here.
            self.catch_up(true);
            if !self.records {
                continue;
            }
            for open in self.known.asked_by(&self.host) {
                let Some(standing) = answered_in_store(open.project.as_deref(), &open.id) else {
                    continue;
                };
                let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
                answers.entry(open.id.clone()).or_insert(standing.clone());
                self.answered.notify_all();
                drop(answers);
                self.closed(&open.id, &standing);
            }
        }
    }

    /// The answer to `question`, waiting up to `hold` for it.
    fn wait(&self, question: &str, hold: Duration) -> Option<String> {
        let deadline = Instant::now() + hold;
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        let nudged = self.nudged.load(Ordering::SeqCst);
        loop {
            if let Some(answer) = answers.get(question) {
                return Some(answer.clone());
            }
            if self.nudged.load(Ordering::SeqCst) != nudged {
                return None;
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

/// How long a send to a peer that could not be reached is tried again.
const RETRY_FOR: Duration = Duration::from_secs(60 * 60);

/// How often an unreached peer or Slack is tried again: 3 s, so a question
/// goes out within seconds of the network coming back, or `HUB_RETRY_MS`.
fn retry_every() -> Duration {
    std::env::var("HUB_RETRY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(Duration::from_secs(3), Duration::from_millis)
}

/// POSTs `body` to `path` on every peer but `host`, off this thread. A
/// round sends to every peer at once, so one that is listed but off holds up
/// nobody; peers that could not be reached are tried again each
/// `retry_every`, for `RETRY_FOR`, and for a question only while `open`
/// still lists it.
fn tell_peers(
    host: &str,
    token: &str,
    path: &'static str,
    body: Vec<u8>,
    open: Option<(Arc<Known>, String)>,
) {
    let host = host.to_string();
    let token = token.to_string();
    std::thread::spawn(move || {
        let until = Instant::now() + RETRY_FOR;
        let mut heard: Vec<String> = Vec::new();
        // One line for each peer that cannot be reached, not one a round.
        let mut told: Vec<String> = Vec::new();
        loop {
            let peers: Vec<String> = crate::tailnet_peers()
                .into_iter()
                .filter(|p| *p != host && !heard.contains(p))
                .collect();
            let reached: Vec<(String, Result<(), String>)> = std::thread::scope(|scope| {
                let sends: Vec<_> = peers
                    .into_iter()
                    .map(|peer| {
                        let (token, body) = (&token, &body);
                        scope.spawn(move || {
                            let reached = match request(&peer, "POST", path, token, body) {
                                Ok((200, _)) => Ok(()),
                                Ok((status, _)) => {
                                    eprintln!("hub: local: {peer} answered {status} to {path}");
                                    Ok(())
                                }
                                // Nothing listens there: a machine with no
                                // local hub, nothing to try again.
                                Err(e) if refused(&e) => Ok(()),
                                Err(e) => Err(format!("{e:#}")),
                            };
                            (peer, reached)
                        })
                    })
                    .collect();
                sends.into_iter().filter_map(|s| s.join().ok()).collect()
            });
            // No peers at all is most likely this machine off the network.
            let mut missed = reached.is_empty() && heard.is_empty();
            for (peer, reached) in reached {
                match reached {
                    Ok(()) => heard.push(peer),
                    Err(e) => {
                        missed = true;
                        if !told.contains(&peer) {
                            eprintln!("hub: local: {path} did not reach {peer}, trying again: {e}");
                            told.push(peer);
                        }
                    }
                }
            }
            if !missed || Instant::now() >= until {
                return;
            }
            std::thread::sleep(retry_every());
            let still_open = open
                .as_ref()
                .is_none_or(|(known, id)| known.get(id).is_some());
            if !still_open {
                return;
            }
        }
    });
}

/// Whether `id` could be one of mem's short ids: letters and digits only.
fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Whether Slack could not be reached at all, as against answering no.
fn unreachable(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|cause| cause.downcast_ref::<ureq::Error>().is_some())
}

/// Whether a connection was refused: the machine is there, its hub is not.
fn refused(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
    })
}

/// How long the route device has to take a question before it goes to
/// Phone: 10 s, or `HUB_RING_MS`.
fn ring_timeout() -> Duration {
    std::env::var("HUB_RING_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(Duration::from_secs(10), Duration::from_millis)
}

/// What the asking hub needs, off its own thread, to ring one question.
struct Ringer {
    slack: Option<Arc<Slack>>,
    known: Arc<Known>,
    posted: Arc<Mutex<HashMap<String, (String, String)>>>,
    host: String,
    token: String,
    question: Question,
}

impl Ringer {
    /// Device when the route device takes it, else Phone; then every peer
    /// hears where it rings, so the route device drops a popup it showed
    /// too late.
    fn ring(&self, route: &Stamped, open: Open) {
        let device = match (route.route, &route.machine) {
            (Kind::Device, Some(machine)) => Some(machine.as_str()),
            _ => None,
        };
        let taken = device.filter(|machine| self.taken_by(machine, &open));
        let ring = taken.unwrap_or("phone").to_string();
        if let Some(machine) = device
            && taken.is_none()
        {
            eprintln!(
                "hub: local: #{} not taken by {machine}; ringing Phone",
                open.id
            );
        }
        // Answered while the route device was asked: leave it closed.
        let Some(open) = self.known.set_ring(&open.id, &ring) else {
            return;
        };
        tell_peers(
            &self.host,
            &self.token,
            "/question",
            serde_json::to_vec(&open).unwrap_or_default(),
            Some((Arc::clone(&self.known), open.id.clone())),
        );
        if taken.is_none() {
            self.post_to_slack();
        }
    }

    /// Whether the route device took the question: this machine when someone
    /// is at it, else a peer the tailnet lists online that says so in time.
    fn taken_by(&self, machine: &str, open: &Open) -> bool {
        if machine == self.host {
            return !crate::presence::sample().away();
        }
        if !crate::tailnet_peers().iter().any(|p| p == machine) {
            eprintln!("hub: local: {machine} is offline");
            return false;
        }
        let body = serde_json::to_vec(open).unwrap_or_default();
        match request_within(machine, "POST", "/ring", &self.token, &body, ring_timeout()) {
            Ok((200, _)) => true,
            Ok((status, why)) => {
                eprintln!(
                    "hub: local: {machine} answered {status} to /ring: {}",
                    why.trim()
                );
                false
            }
            Err(e) => {
                eprintln!("hub: local: /ring did not reach {machine}: {e:#}");
                false
            }
        }
    }

    /// Posts the question, trying again while it is open and Slack cannot
    /// be reached: asked with no network, it goes out once there is one.
    fn post_to_slack(&self) {
        let Some(slack) = &self.slack else { return };
        let id = &self.question.id;
        loop {
            match slack.post_question(&self.question) {
                Ok(posted) => {
                    eprintln!("hub: local: #{id} posted to Slack");
                    self.posted
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(id.clone(), (posted.channel, posted.ts));
                    return;
                }
                // Slack's own refusal (a bad token, a message too long)
                // comes back the same however often it is tried.
                Err(e) if !unreachable(&e) => {
                    eprintln!("hub: local: #{id} not posted to Slack: {e:#}");
                    return;
                }
                Err(e) => eprintln!("hub: local: #{id} not posted to Slack yet: {e:#}"),
            }
            std::thread::sleep(retry_every());
            if self.known.get(id).is_none() {
                return;
            }
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
        is_review(&self.text, &self.options)
    }
}

/// The hub's own rule for a roadmap review, as the connector applies it.
fn is_review(text: &str, options: &[String]) -> bool {
    let text = text.trim();
    crate::page_control::is_approval(text)
        || (options.iter().any(|o| o == "approve") && text.to_lowercase().contains("roadmap"))
}

/// The surfaces whose answers one local hub passes to another.
const TRAVELS: [&str; 3] = ["slack", "popup", "cli"];

const REVIEW_REFUSAL: &str = "a roadmap review is Saiful's to answer, on the hub";

/// An answer given on this machine, as mem hands it over: each image is a
/// file here. `screenshot`, one image, is what an older mem sends.
#[derive(Debug, serde::Deserialize)]
struct Given {
    question: String,
    answer: String,
    by: String,
    screenshot: Option<PathBuf>,
    #[serde(default)]
    screenshots: Vec<PathBuf>,
}

/// Images as they travel between hubs, in order.
fn encoded(shots: &[Vec<u8>]) -> serde_json::Value {
    let all: Vec<_> = shots
        .iter()
        .map(|bytes| serde_json::json!({"data": base64::engine::general_purpose::STANDARD.encode(bytes)}))
        .collect();
    serde_json::Value::Array(all)
}

/// A screenshot as it travels between hubs.
#[derive(Debug, serde::Deserialize)]
struct Shot {
    data: String,
}

impl Shot {
    fn bytes(&self) -> Result<Vec<u8>, String> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.data)
            .map_err(|e| format!("the screenshot is not base64: {e}"))
    }
}

/// Keeps a screenshot where mem can file it, 0600 under the state
/// directory, until mem has.
fn save_shot(question: &str, n: usize, bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    // A question id names the file, so it may name nothing else.
    if question.is_empty() || !question.chars().all(|c| c.is_ascii_alphanumeric()) {
        eprintln!("hub: local: not keeping a screenshot for the id {question:?}");
        return None;
    }
    let dir = crate::config::state_home().ok()?.join("hub/screenshots");
    std::fs::create_dir_all(&dir).ok()?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("{question}-{nanos}-{n}.png"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| file.write_all(bytes));
    match written {
        Ok(()) => Some(path),
        Err(e) => {
            eprintln!("hub: local: keeping the screenshot for #{question}: {e}");
            None
        }
    }
}

/// An answer as a peer hands it in, with the surface that took it.
#[derive(Debug, serde::Deserialize)]
struct Answer {
    question: String,
    answer: String,
    by: String,
    /// One image, as an older hub sends it.
    screenshot: Option<Shot>,
    #[serde(default)]
    screenshots: Vec<Shot>,
}

impl Answer {
    fn shots(&self) -> Vec<&Shot> {
        self.screenshot.iter().chain(&self.screenshots).collect()
    }
}

/// A question a peer says was answered.
#[derive(Debug, serde::Deserialize)]
struct Closed {
    question: String,
}

/// A peer's open questions, as `GET /questions` gives them.
#[derive(Debug, serde::Deserialize)]
struct Listed {
    #[serde(rename = "self")]
    host: String,
    questions: Vec<Open>,
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
    request_within(target, method, path, token, body, PEER_TIMEOUT)
}

/// `request`, given up after `within` for the connection and for each read
/// and write.
pub fn request_within(
    target: &str,
    method: &str,
    path: &str,
    token: &str,
    body: &[u8],
    within: Duration,
) -> Result<(u16, String)> {
    let (host, port) = split_target(target)?;
    let mut stream = match std::env::var("HUB_SOCKS5").ok().filter(|v| !v.is_empty()) {
        Some(proxy) => socks5_connect(&proxy, &host, port)?,
        None => {
            connect_within(&host, port, within).with_context(|| format!("connecting {target}"))?
        }
    };
    stream.set_read_timeout(Some(within))?;
    stream.set_write_timeout(Some(within))?;
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

/// A connect that gives up after `within`, so a peer that is listed
/// but off costs seconds, not the system's two minutes.
fn connect_within(host: &str, port: u16, within: Duration) -> std::io::Result<TcpStream> {
    use std::net::ToSocketAddrs;
    let mut last = std::io::Error::other("no address");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, within) {
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
    let mut stream = connect_within(&proxy_host, proxy_port, PEER_TIMEOUT)
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
