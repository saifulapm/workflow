//! The routing table, and the one object the routes share.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agents::{Agents, Command};
use crate::config::Config;
use crate::dulali::{self, Dulali, Held};
use crate::html::{self, Banner};
use crate::http::{Request, Response};
use crate::memcli::{MemCli, Run};
use crate::origin::Guard;
use crate::route::Route;
use crate::{api, model, pages};

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

/// Whether a question is still waiting — and the third answer, which matters:
/// "mem could not tell us" is not "no such question". Answering the second way
/// would report an unknown id for a store that is merely slow, and now that a
/// `mem` child can be killed on a deadline, that is reachable. A waiting one
/// carries its row, whose project and session the step after the answer needs.
enum Pending {
    Yes(Value),
    No,
    Unreadable(String),
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

    /// The detail routes under `GET /p/<project>/`: everything after the
    /// project is checked against one of the known shapes — `plan`,
    /// `plan/<slug>`, `item/<id>` — before it reaches an argv.
    /// Anything else is a 404, as `/wiki/` already does.
    pub fn project_page(&self, rest: &str) -> Response {
        let Some((project, sub)) = rest.split_once('/') else {
            return Response::not_found();
        };
        if project.is_empty() || !model::is_known_project(&self.mem, project) {
            return Response::not_found();
        }
        match sub {
            "plan" => self.project_plan(project),
            sub => {
                if let Some(slug) = sub.strip_prefix("plan/") {
                    self.project_plan_slug(project, slug)
                } else if let Some(id) = sub.strip_prefix("item/") {
                    self.project_item(project, id)
                } else {
                    Response::not_found()
                }
            }
        }
    }

    /// `GET /p/<project>/plan` — the current plan's whole text, from one
    /// read. A project with no current plan still gets the page, empty.
    fn project_plan(&self, project: &str) -> Response {
        let plan = model::plan_text(&self.mem, project);
        Response::html(html::plan_page(
            project,
            None,
            plan.value.as_deref(),
            plan.degraded.as_deref(),
        ))
    }

    /// `GET /p/<project>/plan/<slug>` — one stored plan, whole. A broken mem
    /// renders degraded; a slug mem simply does not have is a 404.
    fn project_plan_slug(&self, project: &str, slug: &str) -> Response {
        if !model::is_slug(slug) {
            return Response::not_found();
        }
        let plan = model::plan_slug_text(&self.mem, project, slug);
        if plan.value.is_none() && plan.degraded.is_none() {
            return Response::not_found();
        }
        Response::html(html::plan_page(
            project,
            Some(slug),
            plan.value.as_deref(),
            plan.degraded.as_deref(),
        ))
    }

    /// `GET /p/<project>/item/<id>` — one item, whole. `mem show` has no
    /// `--project`, so an id that resolves under another project 404s here
    /// rather than leaking that project's item.
    fn project_item(&self, project: &str, id: &str) -> Response {
        if !model::is_item_id(id) {
            return Response::not_found();
        }
        let item = model::item_detail(&self.mem, id);
        match &item.value {
            Some(detail) if detail.project.as_deref() == Some(project) => Response::html(
                html::item_page(project, Some(detail), item.degraded.as_deref()),
            ),
            Some(_) => Response::not_found(),
            None if item.degraded.is_some() => {
                Response::html(html::item_page(project, None, item.degraded.as_deref()))
            }
            None => Response::not_found(),
        }
    }

    /// The order matters: nothing runs `mem answer` until the
    /// origin has been checked, and the redirect is a 303 so a reload of the
    /// result page cannot answer twice.
    pub fn answer(&self, request: &Request) -> Response {
        if !self.guard.may_write(request) {
            return Response::text(403, "cross-origin writes are refused");
        }
        let form = request.form();
        let id = form.get("id").unwrap_or("").trim();
        // `mem ask` prints the id behind a `#`, and that is what a human
        // copies. The page's own form supplies the bare id, so the phone path
        // never met this; anything typed or pasted from a terminal did, and got
        // `?unknown=1` with the question still pending.
        let id = id.strip_prefix('#').unwrap_or(id);
        let text = form.get("text").unwrap_or("").trim();
        if id.is_empty() || text.is_empty() {
            return self.back(Banner::Empty);
        }

        let response = {
            let lock = self.lock_for(id);
            // Held across all three steps. Dropping it between the read and the
            // write is the race.
            let _held = lock.lock().unwrap_or_else(|e| e.into_inner());

            // The queue as it is right now, not as it was up to five seconds
            // ago. Without this the answered question stays on screen for the
            // rest of the TTL and a second tap writes a second answer — which
            // mem accepts, and which the waiting agent never sees, because the
            // first one won.
            self.mem.invalidate();
            match self.is_pending(id) {
                Pending::No => match self.dulali.find(id) {
                    Some((full, held)) if held.written.is_some() && !held.replaced => {
                        self.replace_dulali(&full, &held, text)
                    }
                    _ => self.back(Banner::Unknown),
                },
                Pending::Unreadable(why) => {
                    eprintln!("hub: answer: could not read the queue: {why}");
                    self.back(Banner::Failed)
                }
                Pending::Yes(row) => {
                    let run = self.mem.answer(id, text, "saiful", None);
                    self.mem.invalidate();
                    match run.code {
                        Some(0) => {
                            // Inside Dulali's window, Saiful's answer is the
                            // one that stands; the held one never reaches mem.
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
                            self.back(Banner::Answered(short(id)))
                        }
                        // Exit 1 with a plain-text stderr is "no such
                        // question", which is a banner, not a 500.
                        Some(1) => self.back(Banner::Unknown),
                        _ => {
                            if !run.stderr.is_empty() {
                                eprintln!("hub: mem answer: {}", run.stderr);
                            }
                            self.back(Banner::Failed)
                        }
                    }
                }
            }
        };
        self.forget_lock();
        response
    }

    /// A write a page makes through mem. One that took queues a sync for
    /// every machine, so each one's mem store sees it within one poll.
    pub fn write_through(&self, args: &[&str]) -> Run {
        let run = self.mem.write_through(args);
        if run.ok() {
            self.agents.queue_sync_all();
        }
        run
    }

    /// After an answer took, and after its sync was queued: when it was the
    /// last open question `session` asked, one `resume` of that session for
    /// the project's runner machine, carrying every answer it got. The sync
    /// sits ahead of it, so the agent's mem holds the answers first.
    pub fn after_answer(&self, project: &str, session: Option<&str>) {
        let Some(session) = session else {
            return;
        };
        let mut asked: Vec<Value> = self
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
        let runner = self
            .mem
            .projects()
            .rows("projects")
            .iter()
            .map(crate::page_home::summary_of)
            .find(|s| s.name == project)
            .and_then(|s| s.runner);
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
        // A second tap on the same last question finds the first resume still
        // waiting, and adds nothing.
        let queued = self.agents.pending(&runner.machine).iter().any(|c| {
            matches!(c, Command::Resume { project: p, id, .. } if p == project && id == session)
        });
        if queued {
            return;
        }
        // A session that parks again was resumed with the answers before;
        // they are in its context already.
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
        self.agents.queue_resume(
            &runner.machine,
            Command::Resume {
                project: project.to_string(),
                id: session.to_string(),
                message,
            },
            &ids,
        );
    }

    /// Saiful's answer to a question Dulali's answer already closed: it
    /// supersedes Dulali's in mem, and reaches the running session as a new
    /// message.
    fn replace_dulali(&self, id: &str, held: &Held, text: &str) -> Response {
        let earlier = held.written.as_deref();
        let run = self.mem.answer(id, text, "saiful", earlier);
        self.mem.invalidate();
        if !run.ok() {
            if !run.stderr.is_empty() {
                eprintln!("hub: mem answer: {}", run.stderr);
            }
            return self.back(Banner::Failed);
        }
        self.dulali.replaced(id);
        self.agents.queue_sync_all();
        let short_id = short(id);
        let message = format!(
            "Saiful replaced Dulali's answer to #{short_id} ({}). Dulali had answered: {}\n\n\
             Saiful's answer, which stands: {text}",
            held.title,
            dulali::ring_answer(&held.answer),
        );
        self.follow_up(&held.project, held.session.as_deref(), message);
        self.back(Banner::Answered(short_id))
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
        let id = said.id.trim();
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
        let outcome = self.mem.questions();
        if let Some(why) = model::list_fault(&outcome, "questions") {
            return Pending::Unreadable(why);
        }
        let waiting = outcome
            .rows("questions")
            .into_iter()
            .find(|row| row["id"] == id || row["short_id"] == id);
        match waiting {
            Some(row) => Pending::Yes(row),
            None => Pending::No,
        }
    }

    fn back(&self, banner: Banner) -> Response {
        Response::see_other(&format!("/{}", banner.query()))
    }

    /// The topic as plain text and as both links. No QR, no image crate.
    fn subscribe(&self) -> Response {
        Response::html(html::subscribe_page(&self.config, &self.machine))
    }
}

/// mem's own short id: the last eight characters of a ULID. A banner showing
/// those eight behind a `#` matches what `mem ask` printed and what the page
/// shows; the full 26-character id matches nothing a human has looked at.
fn short(id: &str) -> String {
    // `is_ascii` as well as the length: the id came off a form, and slicing a
    // 26-byte string that is not 26 characters would panic on the connection
    // thread.
    if id.len() == 26 && id.is_ascii() {
        id[18..].to_string()
    } else {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_ulid_shortens_to_the_id_mem_prints() {
        assert_eq!(short("01M0BF1F8BY8FXGZS428J1TSD1"), "28J1TSD1");
        assert_eq!(short("28J1TSD1"), "28J1TSD1");
        assert_eq!(short(""), "");
        // Twenty-six bytes, thirteen characters: sliced blindly, this panics.
        let multibyte = "é".repeat(13);
        assert_eq!(short(&multibyte), multibyte);
    }
}
