//! The routing table, and the one object the routes share.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use hubpages::page_questions::{Pending, pending, short};

use crate::agents::{Agents, Command};
use crate::config::Config;
use crate::dulali::{self, Dulali, Held};
use crate::html;
use crate::http::{Request, Response};
use crate::live::Runner;
use crate::memcli::MemCli;
use crate::origin::Guard;
use crate::route::{Kind, NoMachine, Route, Saved};
use crate::{api, pages};

/// How long `workflow pause|unpause` may take: a few mem reads and a write.
const PAUSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Every path hub answers, and the methods it answers it with, as the
/// `Allow` header lists them. Anything else on a known path is a 405 that says
/// so; anything else at all is a 404. The pages are `pages::route_page`'s;
/// these are the rest.
const ROUTES: &[(&str, &str)] = &[
    ("/api/agent", "POST"),
    ("/api/route", "GET, POST"),
    ("/api/agents", "GET"),
    ("/api/presence", "GET"),
    ("/api/stamp", "GET"),
    ("/subscribe", "GET"),
];

pub struct App {
    pub config: Config,
    /// The port actually bound, which is not always `config.port`: `--port`
    /// overrides it and `--port 0` means the kernel chose. Every origin and
    /// Host check has to use this one.
    pub port: u16,
    pub machine: String,
    /// Shared with the doorbell thread, so its fifteen-second poll warms the
    /// same cache the page reads from.
    pub mem: Arc<MemCli>,
    pub guard: Guard,
    /// The device agents' reports and command queues.
    pub agents: Arc<Agents>,
    /// Where questions and rings reach the owner.
    pub route: Arc<Route>,
    /// Dulali's answers and passes, and its webhook.
    pub dulali: Arc<Dulali>,
    /// One answer at a time, per question id.
    ///
    /// `MemCli`'s gate serialises the child *processes*; it cannot serialise
    /// the sequence around them, because it is released between "is this still
    /// pending" and "answer it" by construction. So without this, every racer's
    /// `is_pending` ran before any racer's `answer`, all of them saw the
    /// question waiting, and all of them wrote — twelve rounds out of twelve,
    /// each one told the phone it had worked, and `mem questions --wait`
    /// unblocked the orchestrator with whichever landed first.
    ///
    /// Per id rather than one lock for the whole route, so two different
    /// questions still answer at the same time.
    answering: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl App {
    pub fn new(config: Config, port: u16, machine: String) -> App {
        let guard = Guard::new(&config, port, &machine);
        // With no HOME the path is relative to the working directory; a save
        // that fails there says so in the journal, and the queue lives on in
        // memory.
        let state = crate::config::state_home().unwrap_or_default();
        let path = state.join("hub/agents.json");
        let dulali = Dulali::open(state.join("hub/dulali.json"), config.dulali_webhook.clone());
        App {
            config,
            port,
            machine,
            mem: Arc::new(MemCli::new()),
            guard,
            agents: Arc::new(Agents::open(path, crate::agents::window())),
            route: Arc::new(Route::open(state.join("hub/route.json"))),
            dulali: Arc::new(dulali),
            answering: Mutex::new(HashMap::new()),
        }
    }

    pub fn handle(&self, request: &Request) -> Response {
        // On every route, not only the write: a rebinding attack that
        // reaches `GET /` has read the question text.
        if !self.guard.host_allowed(request.header("host")) {
            return Response::text(403, "not a host this hub answers to");
        }
        if let Some(response) = pages::route_page(self, request) {
            return response;
        }
        let allowed = ROUTES
            .iter()
            .find(|(path, _)| *path == request.path)
            .map(|(_, allowed)| *allowed);
        let Some(allowed) = allowed else {
            return Response::not_found();
        };
        if !allowed.split(", ").any(|method| method == request.method) {
            return Response::method_not_allowed(allowed);
        }
        match request.path.as_str() {
            "/api/agent" => {
                crate::agents::poll_post(&self.agents, request, || self.route.object(&self.mem))
            }
            "/api/route" if request.method == "GET" => {
                crate::route::get(&self.route, &self.mem, request)
            }
            "/api/route" => crate::route::post(&self.route, &self.mem, request),
            "/api/agents" => crate::agents::list_get(&self.agents, request),
            "/api/presence" => {
                Response::json(api::presence(&crate::presence::sample(), &self.machine))
            }
            "/api/stamp" => Response::json(
                serde_json::json!({"stamp": crate::page_home::stamp(
                    &self.mem,
                    &self.route,
                    &self.dulali.replaceable(dulali::now_ms()),
                )})
                .to_string(),
            ),
            "/subscribe" => self.subscribe(),
            _ => Response::not_found(),
        }
    }

    /// After an answer took, and after its sync was queued: when it was the
    /// last open question `session` asked, one `resume` of that session for
    /// the project's runner machine, carrying every answer it got. The sync
    /// sits ahead of it, so the agent's mem holds the answers first.
    pub fn after_answer(&self, project: &str, session: Option<&str>) {
        let runner = self.runner_of(project);
        // A cloud run takes its answers over Tailscale, from its own hub.
        if runner.as_ref().is_some_and(|r| r.machine == "cloud") {
            return;
        }
        let Some(session) = session else {
            return;
        };
        let asked: Vec<Value> = self
            .mem
            .questions_all(project)
            .rows("questions")
            .into_iter()
            // An orchestrator's own question from its implementers carries
            // the same amx id, but nobody answers it here. A person's,
            // Saiful's included, is answered here.
            .filter(|row| row["session"] == session && row["audience"] != "orchestrator")
            .collect();
        if asked.is_empty() || asked.iter().any(|row| row["answered"] != true) {
            return;
        }
        let Some(runner) = runner else {
            eprintln!("hub: {project} has no runner to resume {session} on");
            return;
        };
        // An older orchestrator of the project would come back beside the
        // run, so only the runner's own session is resumed.
        if runner.id != session {
            eprintln!(
                "hub: {session} is not {project}'s runner ({}); not resumed",
                runner.id
            );
            return;
        }
        self.deliver(&runner.machine, project, session, asked, |message| {
            Command::Resume {
                project: project.to_string(),
                id: session.to_string(),
                message,
            }
        });
    }

    /// The runner record of `project`, if it has one.
    fn runner_of(&self, project: &str) -> Option<Runner> {
        self.mem
            .projects()
            .rows("projects")
            .iter()
            .map(crate::page_home::summary_of)
            .find(|s| s.name == project)
            .and_then(|s| s.runner)
    }

    /// Queues for `machine` the command `make` builds from the answers of
    /// `asked` that `session` has not been told, and records them as told.
    fn deliver(
        &self,
        machine: &str,
        project: &str,
        session: &str,
        mut asked: Vec<Value>,
        make: impl FnOnce(String) -> Command,
    ) {
        // A second tap on the same last question finds the first command
        // still waiting, and adds nothing.
        let queued = self.agents.pending(machine).iter().any(|c| match c {
            Command::Resume { project: p, id, .. } => p == project && id == session,
            _ => false,
        });
        if queued {
            return;
        }
        // A session that parks again was told the answers before; they are
        // in its context already.
        let told = self.agents.delivered(session);
        asked.retain(|row| !told.iter().any(|id| row["id"] == id.as_str()));
        if asked.is_empty() {
            return;
        }
        // mem lists newest first; a ULID sorts in the order it was made.
        asked.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let by_dulali = |row: &Value| row["answer_by"] == "dulali";
        let answers: Vec<String> = asked
            .iter()
            .map(|row| {
                format!(
                    "#{} {}\n{}: {}",
                    row["short_id"].as_str().unwrap_or(""),
                    row["body"].as_str().unwrap_or(""),
                    if by_dulali(row) {
                        "Dulali's answer, which Saiful let stand"
                    } else {
                        "Answer"
                    },
                    row["answer"].as_str().unwrap_or(""),
                )
            })
            .collect();
        let who = if asked.iter().any(by_dulali) {
            "Your questions were answered on the hub, by Saiful or by Dulali as each says"
        } else {
            "Saiful answered your questions on the hub"
        };
        let message = format!(
            "{who}. Each answer is below; \
             `mem questions` shows them once this machine's sync round lands.\n\n{}",
            answers.join("\n\n")
        );
        let ids: Vec<String> = asked
            .iter()
            .filter_map(|row| row["id"].as_str().map(str::to_string))
            .collect();
        self.agents.queue_resume(machine, make(message), &ids);
    }

    /// A message for a session that may still be running: the runner's
    /// agent sends it in, or resumes the session with it.
    fn follow_up(&self, project: &str, session: Option<&str>, message: String) {
        let Some(session) = session else {
            return;
        };
        let runner = self
            .mem
            .projects()
            .rows("projects")
            .iter()
            .map(crate::page_home::summary_of)
            .find(|s| s.name == project)
            .and_then(|s| s.runner);
        match runner {
            Some(runner) if runner.id == session => self.agents.queue(
                &runner.machine,
                Command::FollowUp {
                    project: project.to_string(),
                    id: session.to_string(),
                    message,
                },
            ),
            _ => eprintln!("hub: {session} is not {project}'s runner; no follow-up sent"),
        }
    }

    /// Dulali's socket, `POST /api/dulali`: its answer, held for the
    /// override window, or its pass, which rings the phone. Nothing else is
    /// served there, and nothing but Dulali's token is taken.
    pub fn dulali_post(&self, request: &Request) -> Response {
        if request.path != "/api/dulali" {
            return Response::not_found();
        }
        if request.method != "POST" {
            return Response::method_not_allowed("POST");
        }
        let authorized = dulali::token_path()
            .map(|path| dulali::authorized(request, &path))
            .unwrap_or(false);
        if !authorized {
            return crate::agents::unauthorized();
        }
        #[derive(serde::Deserialize)]
        struct Said {
            id: String,
            answer: Option<String>,
            #[serde(default)]
            pass: bool,
            checked: String,
        }
        let refuse =
            |status, error: &str| crate::agents::json(status, serde_json::json!({"error": error}));
        let said: Said = match serde_json::from_slice(&request.body) {
            Ok(said) => said,
            Err(e) => return refuse(400, &e.to_string()),
        };
        let checked = said.checked.trim();
        let answer = said.answer.as_deref().map(str::trim).unwrap_or("");
        if checked.is_empty() {
            return refuse(400, "say what you checked");
        }
        if !said.pass && answer.is_empty() {
            return refuse(400, "an answer or a pass");
        }
        // Locked on the full id, as Saiful's own answer is, whether Dulali
        // sent that or the short one.
        let id = match self.is_pending(said.id.trim()) {
            Pending::Yes(row) => row["id"].as_str().unwrap_or("").to_string(),
            _ => said.id.trim().to_string(),
        };
        let id = id.as_str();
        let response = {
            let lock = self.lock_for(id);
            let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
            self.mem.invalidate();
            match self.is_pending(id) {
                Pending::No => refuse(409, "that question is not waiting"),
                Pending::Unreadable(why) => {
                    eprintln!("hub: dulali: could not read the queue: {why}");
                    refuse(503, "the queue could not be read; try again")
                }
                Pending::Yes(row) => self.dulali_says(&row, said.pass, answer, checked),
            }
        };
        self.forget_lock();
        response
    }

    /// Under the question's lock, with its row.
    fn dulali_says(&self, row: &Value, pass: bool, answer: &str, checked: &str) -> Response {
        let full = row["id"].as_str().unwrap_or("");
        let sent = crate::config::state_home()
            .ok()
            .and_then(|state| crate::doorbell::load(&state.join("hub/seen")))
            .is_some_and(|seen| seen.contains(&format!("dulali {full}")));
        if !sent {
            return crate::agents::json(
                409,
                serde_json::json!({"error": "that question was not sent to Dulali"}),
            );
        }
        // The run reads the answer as the option it picked; Dulali's
        // reasons go in what it checked.
        let options: Vec<&str> = row["options"]
            .as_array()
            .map(|o| o.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !pass && !options.is_empty() && !options.contains(&answer) {
            return crate::agents::json(
                400,
                serde_json::json!({"error": format!(
                    "answer with one of the options, word for word: {}",
                    options.join(", ")
                )}),
            );
        }
        if self.dulali.has_spoken(full) {
            return crate::agents::json(
                409,
                serde_json::json!({"error": "Dulali already answered or passed it"}),
            );
        }
        let project = row["project"].as_str().unwrap_or("");
        let machine = row["machine"].as_str().unwrap_or(&self.machine);
        let link = crate::pair::base_url(&self.config, self.port, &self.machine);
        let now = dulali::now_ms();
        if pass {
            self.dulali.pass(full, now);
            crate::doorbell::push(
                &self.config.ntfy_url(),
                &format!("Dulali passed a question on {machine} ({project}) — {link}"),
            );
            return Response::json(serde_json::json!({"passed": true}).to_string());
        }
        let until = now + self.dulali.window.as_millis() as i64;
        self.dulali.hold(
            full,
            Held {
                answer: answer.to_string(),
                checked: checked.to_string(),
                project: project.to_string(),
                session: row["session"].as_str().map(str::to_string),
                machine: machine.to_string(),
                title: row["title"].as_str().unwrap_or("").to_string(),
                at_ms: now,
                until_ms: until,
                written: None,
                replaced: false,
            },
        );
        crate::doorbell::push(
            &self.config.ntfy_url(),
            &format!(
                "Dulali answered a question on {machine} ({project}): {} — override within 2 min: {link}",
                dulali::ring_answer(answer)
            ),
        );
        let until = jiff::Timestamp::from_millisecond(until)
            .map(|t| t.to_string())
            .unwrap_or_default();
        Response::json(serde_json::json!({"held_until": until}).to_string())
    }

    /// Each Dulali answer whose window has closed goes to mem as Dulali's,
    /// and the run resumes on it, unless Saiful answered first.
    pub fn dulali_expire(&self) {
        for (id, held) in self.dulali.due(dulali::now_ms()) {
            let lock = self.lock_for(&id);
            {
                let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
                self.mem.invalidate();
                match self.is_pending(&id) {
                    Pending::No => self.dulali.replaced(&id),
                    Pending::Unreadable(why) => {
                        eprintln!("hub: dulali: could not read the queue: {why}");
                    }
                    Pending::Yes(_) => {
                        let text = format!("{}\n\nDulali checked: {}", held.answer, held.checked);
                        let run = self.mem.answer(&id, &text, "dulali", None);
                        self.mem.invalidate();
                        let written = serde_json::from_slice::<Value>(&run.stdout)
                            .ok()
                            .and_then(|v| v["id"].as_str().map(str::to_string));
                        match (run.ok(), written) {
                            (true, Some(answer_id)) => {
                                self.dulali.written(&id, &answer_id);
                                self.agents.queue_sync_all();
                                self.after_answer(&held.project, held.session.as_deref());
                            }
                            _ => eprintln!("hub: dulali: mem answer: {}", run.stderr),
                        }
                    }
                }
            }
            drop(lock);
            self.forget_lock();
        }
    }

    /// The lock for one key, created on first use: a question id here, and
    /// whatever a page's write has to serialise on.
    pub fn lock_for(&self, key: &str) -> Arc<Mutex<()>> {
        let mut map = self.answering.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(map.entry(key.to_string()).or_default())
    }

    /// Drops every entry no thread is holding any more.
    ///
    /// The ids come off a form, so the map must not be somewhere an entry can
    /// be grown per distinct string. Called only once the caller's own guard
    /// *and* its `Arc` are gone, so its entry is a candidate; an entry another
    /// thread is waiting on has a strong count above one and stays, which is
    /// what keeps two racers on one id on the same mutex.
    fn forget_lock(&self) {
        let mut map = self.answering.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, held| Arc::strong_count(held) > 1);
    }

    fn is_pending(&self, id: &str) -> Pending {
        pending(&*self.mem, id)
    }

    /// The topic as plain text and as both links. No QR, no image crate.
    fn subscribe(&self) -> Response {
        Response::html(html::subscribe_page(&self.config, &self.machine))
    }
}

/// What the pages ask of this hub beyond mem: its agents, route, Dulali,
/// locks, the `workflow` binary and its state directory.
impl hubpages::Host for App {
    fn fleet(&self) -> &dyn hubpages::Fleet {
        &*self.agents
    }

    fn queue(&self, machine: &str, command: Command) {
        self.agents.queue(machine, command);
    }

    /// A sync for every machine, so each one's mem store sees the write
    /// within one poll.
    fn wrote(&self) {
        self.agents.queue_sync_all();
    }

    fn lock(&self, key: &str) -> Arc<Mutex<()>> {
        self.lock_for(key)
    }

    fn forget_locks(&self) {
        self.forget_lock();
    }

    fn route(&self) -> Saved {
        self.route.get()
    }

    fn set_route(&self, kind: Kind) -> Result<(), NoMachine> {
        self.route.set(kind, None)
    }

    fn replaceable(&self, now_ms: i64) -> Vec<(String, Held)> {
        self.dulali.replaceable(now_ms)
    }

    fn held_written(&self, id: &str) -> Option<(String, Held)> {
        self.dulali
            .find(id)
            .filter(|(_, held)| held.written.is_some() && !held.replaced)
    }

    /// Saiful's answer replaced Dulali's in mem; it reaches the running
    /// session as a new message.
    fn superseded(&self, id: &str, held: &Held, text: &str) {
        self.dulali.replaced(id);
        self.agents.queue_sync_all();
        let message = format!(
            "Saiful replaced Dulali's answer to #{} ({}). Dulali had answered: {}\n\n\
             Saiful's answer, which stands: {text}",
            short(id),
            held.title,
            dulali::ring_answer(&held.answer),
        );
        self.follow_up(&held.project, held.session.as_deref(), message);
    }

    fn answered(&self, row: &Value) {
        // Inside Dulali's window, Saiful's answer is the one that stands; the
        // held one never reaches mem.
        if let Some(full) = row["id"].as_str()
            && self.dulali.held(full).is_some()
        {
            self.dulali.replaced(full);
        }
        self.agents.queue_sync_all();
        self.after_answer(
            row["project"].as_str().unwrap_or(""),
            row["session"].as_str(),
        );
    }

    fn after_answer(&self, project: &str, session: Option<&str>) {
        App::after_answer(self, project, session);
    }

    /// `workflow pause|unpause`, which write mem and start nothing.
    fn pause(&self, verb: &str, project: &str) -> Result<(), String> {
        let mut command = std::process::Command::new("workflow");
        command.args([verb, project]);
        if verb == "pause" {
            command.args(["--by", "hub"]);
        }
        let ended = crate::proc::output_within(&mut command, PAUSE_TIMEOUT);
        self.mem.invalidate();
        match ended {
            crate::proc::Ended::Exited(done) if done.code == Some(0) => {
                self.agents.queue_sync_all();
                Ok(())
            }
            crate::proc::Ended::Exited(done) => {
                Err(String::from_utf8_lossy(&done.stderr).trim().to_string())
            }
            crate::proc::Ended::TimedOut => Err(format!("workflow {verb} did not finish in time.")),
            crate::proc::Ended::Failed(why) => Err(format!("cannot run workflow {verb}: {why}")),
        }
    }

    /// A photo under the hub's state directory, never /tmp with its small
    /// quota, named by the hub so no path comes from the request.
    fn stash_photo(&self, kind: &str, bytes: &[u8]) -> Result<String, String> {
        let stash = || -> anyhow::Result<std::path::PathBuf> {
            let dir = crate::config::state_home()?.join("hub/uploads");
            std::fs::create_dir_all(&dir)?;
            let ext = match kind {
                "image/png" => "png",
                "image/webp" => "webp",
                _ => "jpg",
            };
            let hex: String = crate::config::urandom::<4>()?
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let time = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ");
            let path = dir.join(format!("photo-{time}-{hex}.{ext}"));
            let mut file = std::fs::File::create_new(&path)?;
            if let Err(e) = std::io::Write::write_all(&mut file, bytes) {
                let _ = std::fs::remove_file(&path);
                return Err(e.into());
            }
            Ok(path)
        };
        stash()
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|e| format!("{e:#}"))
    }

    /// mem has copied the photo in, or refused it: either way it goes.
    fn drop_photo(&self, path: &str) {
        let _ = std::fs::remove_file(path);
    }
}
