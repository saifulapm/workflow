//! The routing table (spec §3), and the one object the routes share.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::html::{self, Banner};
use crate::http::{Request, Response};
use crate::memcli::MemCli;
use crate::model::View;
use crate::origin::Guard;
use crate::{api, model};

/// Every path hub answers, and the one method it answers it with. Anything
/// else on a known path is a 405 that says so; anything else at all is a 404.
const ROUTES: &[(&str, &str)] = &[
    ("/", "GET"),
    ("/answer", "POST"),
    ("/api/questions", "GET"),
    ("/api/activity", "GET"),
    ("/api/projects", "GET"),
    ("/api/presence", "GET"),
    ("/api/notify", "POST"),
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
    /// One answer at a time, per question id (review B-1).
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
/// `mem` child can be killed on a deadline (review M-3) that is reachable.
enum Pending {
    Yes,
    No,
    Unreadable(String),
}

impl App {
    pub fn new(config: Config, port: u16, machine: String) -> App {
        let guard = Guard::new(&config, port, &machine);
        App {
            config,
            port,
            machine,
            mem: Arc::new(MemCli::new()),
            guard,
            answering: Mutex::new(HashMap::new()),
        }
    }

    pub fn handle(&self, request: &Request) -> Response {
        // §9.2. On every route, not only the write: a rebinding attack that
        // reaches `GET /` has read the question text.
        if !self.guard.host_allowed(request.header("host")) {
            return Response::text(403, "not a host this hub answers to");
        }
        let Some((_, allowed)) = ROUTES.iter().find(|(path, _)| *path == request.path) else {
            return Response::not_found();
        };
        if request.method != *allowed {
            return Response::method_not_allowed(allowed);
        }
        match request.path.as_str() {
            "/" => self.dashboard(request),
            "/answer" => self.answer(request),
            "/api/questions" => {
                Response::json(api::questions(&model::questions(&self.mem, self.now_ms())))
            }
            "/api/activity" => {
                Response::json(api::activity(&model::activity(&self.mem, self.now_ms())))
            }
            "/api/projects" => {
                Response::json(api::projects(&model::projects(&self.mem, self.now_ms())))
            }
            "/api/presence" => {
                Response::json(api::presence(&crate::presence::sample(), &self.machine))
            }
            "/api/notify" => self.notify(request),
            "/subscribe" => self.subscribe(),
            _ => Response::not_found(),
        }
    }

    pub fn view(&self) -> View {
        View::build(&self.mem, &self.machine, self.now_ms())
    }

    fn now_ms(&self) -> i64 {
        jiff::Timestamp::now().as_millisecond()
    }

    fn dashboard(&self, request: &Request) -> Response {
        let banner = Banner::from_query(&request.query);
        Response::html(html::page(&self.view(), &self.config, &banner))
    }

    /// §3 and §9. The order matters: nothing runs `mem answer` until the
    /// origin has been checked, and the redirect is a 303 so a reload of the
    /// result page cannot answer twice.
    fn answer(&self, request: &Request) -> Response {
        if !self.guard.may_write(request) {
            return Response::text(403, "cross-origin writes are refused");
        }
        let form = request.form();
        let id = form.get("id").unwrap_or("").trim();
        // `mem ask` prints `#RK4B2PBW`, and that is what a human copies. The
        // page's own form supplies the bare id, so the phone path never met
        // this; anything typed or pasted from a terminal did, and got
        // `?unknown=1` with the question still pending.
        let id = id.strip_prefix('#').unwrap_or(id);
        let text = form.get("text").unwrap_or("").trim();
        if id.is_empty() || text.is_empty() {
            return self.back(Banner::Empty);
        }

        let response = {
            let lock = self.lock_for(id);
            // Held across all three steps. Dropping it between the read and the
            // write is precisely B-1.
            let _held = lock.lock().unwrap_or_else(|e| e.into_inner());

            // The queue as it is right now, not as it was up to five seconds
            // ago. Without this the answered question stays on screen for the
            // rest of the TTL and a second tap writes a second answer — which
            // mem accepts, and which the waiting agent never sees, because the
            // first one won (review M-2).
            self.mem.invalidate();
            match self.is_pending(id) {
                Pending::No => self.back(Banner::Unknown),
                Pending::Unreadable(why) => {
                    eprintln!("hub: answer: could not read the queue: {why}");
                    self.back(Banner::Failed)
                }
                Pending::Yes => {
                    let run = self.mem.answer(id, text);
                    self.mem.invalidate();
                    match run.code {
                        Some(0) => self.back(Banner::Answered(short(id))),
                        // §4a: exit 1 with a plain-text stderr is "no such
                        // question", and §3 says that is a banner, not a 500.
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

    /// The lock for one question id, created on first use.
    fn lock_for(&self, id: &str) -> Arc<Mutex<()>> {
        let mut map = self.answering.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(map.entry(id.to_string()).or_default())
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
            .iter()
            .any(|row| row["id"] == id || row["short_id"] == id);
        if waiting { Pending::Yes } else { Pending::No }
    }

    fn back(&self, banner: Banner) -> Response {
        Response::see_other(&format!("/{}", banner.query()))
    }

    /// A sibling hub handing this machine the bell (spec: presence routing).
    /// The body is the doorbell's own minimal message; it is capped and passed
    /// as one argv, never through a shell.
    fn notify(&self, request: &Request) -> Response {
        if !self.guard.may_machine_write(request) {
            return Response::text(403, "browser writes are refused here");
        }
        let form = request.form();
        let body = form.get("body").unwrap_or("").trim().to_string();
        if body.is_empty() {
            return Response::text(400, "empty body");
        }
        let body: String = body.chars().take(300).collect();
        if crate::presence::notify_send(&body) {
            Response::text(200, "notified")
        } else {
            Response::text(503, "notify-send failed")
        }
    }

    /// §3: the topic as plain text and as both links. No QR, no image crate.
    fn subscribe(&self) -> Response {
        Response::html(html::subscribe_page(&self.config, &self.machine))
    }
}

/// mem's own short id: the last eight characters of a ULID. A banner reading
/// `#28J1TSD1` matches what `mem ask` printed and what the page shows; the full
/// 26-character id matches nothing a human has looked at.
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
