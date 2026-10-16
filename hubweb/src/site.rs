//! The hub's pages drawn from the store: memview answers the pages' mem
//! reads from the store's files, and a host that runs nothing stands in for
//! the hub's machines. The Worker hands this the files and the request; the
//! host can test it with files in memory.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hubpages::form::Form;
use hubpages::host::{Command, Held, Kind, NoMachine, Polled, Saved};
use hubpages::http::{Request, Response};
use hubpages::live::Runner;
use hubpages::pages::{Site, route_page};
use hubpages::{Fleet, Host, Outcome, Source};
use serde_json::Value;

use crate::memview::{self, Files, Reply};
use crate::webwrite::{self, After, Clock, Written};

/// The name the pages draw as the hub's own.
pub const MACHINE: &str = "next-hub";

/// The line the hub posts to Slack when the store hits a Free daily limit.
const LIMIT_LINE: &str = "Cloudflare limit reached, resets 06:00";

/// Whether the store's flag (the UTC day number it was set on) is still
/// today's: the Free limits reset at 00:00 UTC, so an older flag is stale.
pub fn limit_is_today(day: Option<u64>, now_ms: u64) -> bool {
    day == Some(now_ms / 86_400_000)
}

/// The routes the hub web offers.
const ROUTES: [Kind; 2] = [Kind::Phone, Kind::Device];

/// Saiful reads the hub in Dhaka, which keeps +06:00 all year.
pub fn dhaka() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::fixed(jiff::tz::offset(6))
}

/// What a page request brings beside the files.
pub struct Ask<'a> {
    pub request: &'a Request,
    pub now: jiff::Timestamp,
    /// The route the store keeps, as its route message.
    pub route: Option<Value>,
    pub paired: bool,
    pub may_write: bool,
    /// The store hit a Free daily limit today, which home says.
    pub limited: bool,
    /// What the store has seen, as its /live socket names it: a page drawn
    /// at another stamp is out of date.
    pub stamp: String,
}

/// The page for `ask.request` and the writes it made, or None for a path no
/// page owns. Every page listens on /live, so an open one reloads when the
/// store changes.
pub fn page(
    files: &impl Files,
    ask: Ask,
    random: &mut dyn FnMut() -> [u8; 10],
) -> Option<(Response, Written)> {
    let shared = Shared {
        written: RefCell::new(Written::default()),
        clock: RefCell::new(Clock {
            now: ask.now,
            random,
        }),
    };
    let mem = StoreMem {
        files,
        shared: &shared,
        read: RefCell::new(HashMap::new()),
    };
    let host = Web {
        route: saved(ask.route.as_ref()),
        device: ask.route.as_ref().and_then(crate::share::last_device),
        files,
        shared: &shared,
    };
    let site = Site {
        mem: &mem,
        host: &host,
        machine: MACHINE,
        siblings: &[],
        by: "hub-web",
        paired: ask.paired,
        pair_how: crate::pair::HOW,
        may_write: ask.may_write,
        now: ask.now,
        tz: dhaka(),
        routes: &ROUTES,
        poll: false,
    };
    let mut response = route_page(&site, ask.request)?;
    if response.content_type.starts_with("text/html") {
        let mut html = String::from_utf8_lossy(&response.body).into_owned();
        if ask.limited && ask.request.path == "/" {
            html = html.replacen(
                "</nav>\n",
                &format!("</nav>\n<p class=\"banner warn\">{LIMIT_LINE}</p>\n"),
                1,
            );
        }
        response.body = listening(&html, &ask.stamp);
    }
    drop((mem, host));
    Some((response, shared.written.into_inner()))
}

/// A request as the pages read it, from its parts.
pub fn request(
    method: &str,
    target: &str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> Request {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Request {
        method: method.to_string(),
        path: hubpages::form::decode_path(path),
        query: Form::parse(query),
        headers: headers
            .into_iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
            .collect(),
        body,
        peer: "cloudflare".to_string(),
    }
}

/// The page's own socket. The store sends its stamp when the socket opens
/// and after every change; a stamp other than the one the page was drawn at
/// reloads the page, so a change made before the socket opened, or while a
/// phone kept the page in the background, is not missed. Never while a
/// field has focus, holds text of its own or text is selected: the page
/// reloads once the field is left. A closed socket comes back after 1 s,
/// then 2, 4, up to a minute, and at once when the page is shown again.
fn listen(stamp: &str) -> String {
    format!(
        "<script>(function(){{var seen=\"{stamp}\",due=false,t,wait=1000,s;\
function busy(){{var a=document.activeElement;\
if(a&&/^(TEXTAREA|INPUT|SELECT)$/.test(a.tagName))return true;\
if(String(window.getSelection())!=='')return true;\
var f=document.querySelectorAll('textarea,input:not([type=hidden])');\
for(var i=0;i<f.length;i++){{if(f[i].value!==f[i].defaultValue)return true;}}return false;}}\
function go(){{if(busy()){{due=true;return;}}location.reload();}}\
document.addEventListener('focusout',function(){{setTimeout(function(){{if(due)go();}},0);}});\
document.addEventListener('selectionchange',function(){{if(due&&!busy())go();}});\
function open(){{clearTimeout(t);s=new WebSocket((location.protocol==='https:'?'wss://':'ws://')+location.host+'/live');\
s.onopen=function(){{wait=1000;}};\
s.onmessage=function(e){{var m={{}};try{{m=JSON.parse(e.data);}}catch(x){{}}\
if(m.stamp!==seen){{clearTimeout(t);t=setTimeout(go,150);}}}};\
s.onclose=function(){{t=setTimeout(open,wait);wait=Math.min(wait*2,60000);}};}}\
document.addEventListener('visibilitychange',function(){{\
if(!document.hidden&&s&&s.readyState>1)open();}});open();}})();</script>\n"
    )
}

/// Opened with `?timing`, a page reports how long its own load took to the
/// store (`/api/timing`), so a load on the phone is a record, not a reading.
const TIMING: &str = "<script>if(/[?&]timing\\b/.test(location.search))\
addEventListener('load',function(){setTimeout(function(){\
var n=performance.getEntriesByType('navigation')[0];if(!n)return;\
navigator.sendBeacon('/api/timing',JSON.stringify({path:location.pathname,\
load:Math.round(n.loadEventEnd),ttfb:Math.round(n.responseStart),\
dns:Math.round(n.domainLookupEnd-n.domainLookupStart),\
connect:Math.round(n.connectEnd-n.connectStart),size:n.transferSize,\
ua:navigator.userAgent}));},0);});</script>\n";

fn listening(html: &str, stamp: &str) -> Vec<u8> {
    let scripts = format!("{}{TIMING}", listen(stamp));
    match html.rfind("</body>") {
        Some(at) => format!("{}{scripts}{}", &html[..at], &html[at..]).into_bytes(),
        None => format!("{html}{scripts}").into_bytes(),
    }
}

/// The store's route message as the pages draw it; Phone when there is none.
fn saved(route: Option<&Value>) -> Saved {
    let Some(route) = route else {
        return Saved::default();
    };
    Saved {
        route: match route["route"].as_str() {
            Some("device") => Kind::Device,
            _ => Kind::Phone,
        },
        machine: route["machine"].as_str().map(str::to_string),
    }
}

/// What a request's pages and host write, and the clock they write by.
struct Shared<'a> {
    written: RefCell<Written>,
    clock: RefCell<Clock<'a>>,
}

/// mem's reads, answered from the store's files with this request's own
/// writes on top; and mem's writes, kept for the store to add to its log.
/// One page asks some reads twice, so each is kept until the next write.
struct StoreMem<'a, 'c, F: Files> {
    files: &'a F,
    shared: &'a Shared<'c>,
    read: RefCell<HashMap<String, Arc<Outcome>>>,
}

impl<F: Files> Source for StoreMem<'_, '_, F> {
    fn read(&self, argv: &[&str]) -> Arc<Outcome> {
        let key = argv.join("\u{0}");
        if let Some(kept) = self.read.borrow().get(&key) {
            return Arc::clone(kept);
        }
        let written = self.shared.written.borrow();
        let now = self.shared.clock.borrow().now;
        let view = After {
            files: self.files,
            written: &written.files,
        };
        let reply = memview::read(&view, argv, now);
        let outcome = Arc::new(match (self.files.fault(), reply) {
            // A read that failed is no answer, never an empty one.
            (Some(why), _) => Outcome::Broken(format!("the store could not be read: {why}")),
            (None, Reply::Json(doc)) => Outcome::Json(doc),
            (None, Reply::Absent) => Outcome::Absent,
            (None, Reply::Unknown) => {
                Outcome::Broken(format!("the store does not answer {argv:?}"))
            }
        });
        self.read.borrow_mut().insert(key, Arc::clone(&outcome));
        outcome
    }

    fn write(&self, argv: &[&str], _input: Option<&[u8]>) -> hubpages::Written {
        self.read.borrow_mut().clear();
        let mut written = self.shared.written.borrow_mut();
        let mut clock = self.shared.clock.borrow_mut();
        let view = After {
            files: self.files,
            written: &written.files.clone(),
        };
        let applied = webwrite::apply(&view, &mut clock, &mut written, argv);
        hubpages::Written {
            ok: applied.is_ok(),
            stdout: String::new(),
            stderr: applied.err().unwrap_or_default(),
        }
    }

    fn invalidate(&self) {
        self.read.borrow_mut().clear();
    }
}

/// The hub web's host: no machines report to it and it runs nothing; the
/// route is the store's, and a pause or a photo is a write like mem's.
struct Web<'a, 'c, F: Files> {
    route: Saved,
    /// The machine that last took Device.
    device: Option<String>,
    files: &'a F,
    shared: &'a Shared<'c>,
}

/// No machine reports to the hub web.
struct NoFleet;

impl Fleet for NoFleet {
    fn online(&self, _machine: &str) -> bool {
        false
    }
    fn report(&self, _machine: &str) -> Option<Polled> {
        None
    }
    fn machines(&self) -> Vec<String> {
        Vec::new()
    }
    fn pending(&self, _machine: &str) -> Vec<Command> {
        Vec::new()
    }
}

impl<F: Files> Host for Web<'_, '_, F> {
    fn fleet(&self) -> &dyn Fleet {
        &NoFleet
    }
    fn queue(&self, _machine: &str, _command: Command) {}
    fn wrote(&self) {}
    fn lock(&self, _key: &str) -> Arc<Mutex<()>> {
        // A Durable Object runs one request at a time.
        Arc::new(Mutex::new(()))
    }
    fn forget_locks(&self) {}
    fn route(&self) -> Saved {
        self.route.clone()
    }
    fn set_route(&self, kind: Kind) -> Result<(), NoMachine> {
        let chosen = match kind {
            Kind::Phone => "phone",
            Kind::Device if self.device.is_some() => "device",
            _ => return Err(NoMachine),
        };
        self.shared.written.borrow_mut().route = Some(chosen);
        Ok(())
    }
    fn replaceable(&self, _now_ms: i64) -> Vec<(String, Held)> {
        Vec::new()
    }
    fn held_written(&self, _id: &str) -> Option<(String, Held)> {
        None
    }
    fn superseded(&self, _id: &str, _held: &Held, _text: &str) {}
    fn answered(&self, _row: &Value) {}
    fn after_answer(&self, _project: &str, _session: Option<&str>) {}
    fn replied_in_app(&self, _project: &str, _runner: &Runner) {}
    fn pause(&self, verb: &str, project: &str) -> Result<(), String> {
        let mut written = self.shared.written.borrow_mut();
        let clock = self.shared.clock.borrow();
        let view = After {
            files: self.files,
            written: &written.files.clone(),
        };
        webwrite::pause(&view, &clock, &mut written, verb, project)
    }
    fn stash_photo(&self, kind: &str, bytes: &[u8]) -> Result<String, String> {
        let mut written = self.shared.written.borrow_mut();
        let mut clock = self.shared.clock.borrow_mut();
        Ok(webwrite::keep_photo(&mut clock, &mut written, kind, bytes))
    }
    fn drop_photo(&self, _path: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Mem(BTreeMap<String, String>);

    impl Files for Mem {
        fn read(&self, path: &str) -> Option<String> {
            self.0.get(path).cloned()
        }
        fn list(&self, prefix: &str) -> Vec<(String, String)> {
            self.0
                .range(prefix.to_string()..)
                .take_while(|(path, _)| path.starts_with(prefix))
                .map(|(p, b)| (p.clone(), b.clone()))
                .collect()
        }
        fn projects(&self) -> Vec<String> {
            self.0
                .keys()
                .filter_map(|p| p.strip_prefix("projects/")?.strip_suffix("/project.toml"))
                .map(str::to_string)
                .collect()
        }
    }

    const P: &str = "01K0SHAREWA7KM20000000000P";

    fn store() -> Mem {
        let project = format!(
            "id = \"{P}\"\nname = \"walk\"\ncreated = \"2026-10-16T09:00:00Z\"\nshare = \"true\"\n"
        );
        let question = "+++\ncreated = \"2026-10-16T10:00:00Z\"\nid = \"01M52000000000000000QWAKT1\"\n\
             kind = \"question\"\nmachine = \"mac\"\nmodified = \"2026-10-16T10:00:00Z\"\n\
             options = [\"yes\", \"no\"]\nproject = \"walk\"\ntitle = \"Ship the walk?\"\n+++\n\nShip the walk?\n";
        Mem(BTreeMap::from([
            (format!("projects/{P}/project.toml"), project),
            (
                format!("projects/{P}/items/01M52000000000000000QWAKT1.md"),
                question.to_string(),
            ),
        ]))
    }

    fn get(path: &str) -> Request {
        request("GET", path, vec![], vec![])
    }

    fn ask(request: &Request) -> Ask<'_> {
        Ask {
            request,
            now: "2026-10-16T12:00:00Z".parse().unwrap(),
            route: None,
            paired: true,
            may_write: true,
            limited: false,
            stamp: "7.0".into(),
        }
    }

    fn draw(files: &Mem, ask: Ask) -> Option<Response> {
        let mut random = || [3; 10];
        page(files, ask, &mut random).map(|(response, _)| response)
    }

    fn html(response: Response) -> String {
        String::from_utf8(response.body).unwrap()
    }

    #[test]
    fn home_lists_the_store_project_and_its_waiting_question_and_listens() {
        let page = html(draw(&store(), ask(&get("/"))).expect("home"));
        assert!(page.contains("<a href=\"/p/walk\">walk</a>"), "{page}");
        assert!(page.contains("Ship the walk?"), "{page}");
        assert!(!page.contains("Nothing waiting"), "{page}");
        assert!(page.contains("name=\"text\" value=\"yes\""), "{page}");
        assert!(page.contains("/live"), "{page}");
        assert!(!page.to_lowercase().contains("dulali"), "{page}");
    }

    #[test]
    fn the_home_shows_the_limit_line_only_when_the_limit_is_flagged() {
        let line = "Cloudflare limit reached, resets 06:00";
        let request = get("/");
        let mut limited = ask(&request);
        limited.limited = true;
        assert!(html(draw(&store(), limited).unwrap()).contains(line));
        assert!(!html(draw(&store(), ask(&request)).unwrap()).contains(line));
        let other = get("/p/walk/questions");
        let mut limited = ask(&other);
        limited.limited = true;
        assert!(!html(draw(&store(), limited).unwrap()).contains(line));
    }

    #[test]
    fn a_limit_flag_counts_only_on_the_utc_day_it_was_set() {
        // 2026-10-16T13:00:00Z is UTC day 20742.
        let now = 1_792_155_600_000;
        assert!(limit_is_today(Some(20742), now));
        assert!(!limit_is_today(Some(20741), now));
        assert!(!limit_is_today(None, now));
    }

    #[test]
    fn a_page_knows_the_stamp_it_was_drawn_at_and_reloads_on_another() {
        let page = html(draw(&store(), ask(&get("/"))).unwrap());
        assert!(page.contains("var seen=\"7.0\""), "{page}");
        assert!(page.contains("if(m.stamp!==seen)"), "{page}");
        // A form's own default (the finding's step) is no text being typed.
        assert!(page.contains("f[i].value!==f[i].defaultValue"), "{page}");
    }

    #[test]
    fn the_questions_page_draws_from_the_store() {
        let response = draw(&store(), ask(&get("/p/walk/questions"))).expect("a page");
        assert_eq!(response.status, 200);
        let page = html(response);
        assert!(page.contains("Ship the walk?"), "{page}");
    }

    #[test]
    fn a_paired_answer_writes_one_answer_signed_hub_web() {
        let files = store();
        let form = b"id=01M52000000000000000QWAKT1&text=yes".to_vec();
        let request = request(
            "POST",
            "/answer",
            vec![(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            form,
        );
        let mut random = || [5; 10];
        let (response, written) = page(&files, ask(&request), &mut random).unwrap();
        assert_eq!(response.status, 303);
        let location = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("location"));
        assert!(
            location.is_some_and(|(_, v)| v.contains("answered")),
            "{location:?}"
        );
        assert_eq!(written.files.len(), 1);
        let answer = written.files[0].1.as_deref().unwrap();
        assert!(
            answer.contains("by = \"hub-web\"")
                && answer.contains("answers = \"01M52000000000000000QWAKT1\""),
            "{answer}"
        );

        let mut unpaired = ask(&request);
        unpaired.paired = false;
        let (refused, nothing) = page(&files, unpaired, &mut random).unwrap();
        assert_eq!(refused.status, 403);
        assert!(nothing.files.is_empty());
    }

    #[test]
    fn the_route_buttons_choose_phone_or_the_last_device() {
        let files = store();
        let mut random = || [5; 10];
        let post = |route: &str| {
            request(
                "POST",
                "/route",
                vec![(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                )],
                format!("route={route}").into_bytes(),
            )
        };
        let device = post("device");
        let (response, written) = page(&files, ask(&device), &mut random).unwrap();
        assert_eq!(response.status, 303);
        assert_eq!(written.route, None, "no machine has taken Device yet");
        let mut known = ask(&device);
        known.route = Some(
            serde_json::json!({"t":"route","route":"phone","machine":null,"at":5,"device":"nuc"}),
        );
        let (_, written) = page(&files, known, &mut random).unwrap();
        assert_eq!(written.route, Some("device"));
        let phone = post("phone");
        let (_, written) = page(&files, ask(&phone), &mut random).unwrap();
        assert_eq!(written.route, Some("phone"));
        let dulali = post("dulali");
        let (refused, written) = page(&files, ask(&dulali), &mut random).unwrap();
        assert_eq!((refused.status, written.route), (400, None));
    }

    /// Files whose every read failed, as the store's are past a Free limit.
    struct Failing;

    impl Files for Failing {
        fn read(&self, _path: &str) -> Option<String> {
            None
        }
        fn list(&self, _prefix: &str) -> Vec<(String, String)> {
            Vec::new()
        }
        fn projects(&self) -> Vec<String> {
            Vec::new()
        }
        fn fault(&self) -> Option<String> {
            Some("rows read limit exceeded".into())
        }
    }

    #[test]
    fn a_store_that_cannot_be_read_never_says_nothing_waits() {
        let request = get("/");
        let mut random = || [1; 10];
        let (response, _) = page(&Failing, ask(&request), &mut random).unwrap();
        let page = html(response);
        assert!(page.contains("rows read limit exceeded"), "{page}");
        assert!(!page.contains("Nothing waiting"), "{page}");
    }

    #[test]
    fn a_project_the_store_lacks_is_not_found() {
        let response = draw(&store(), ask(&get("/p/nope/questions"))).expect("a page");
        assert_eq!(response.status, 404);
    }

    #[test]
    fn the_route_is_the_stores() {
        let request = get("/");
        let mut asked = ask(&request);
        asked.route =
            Some(serde_json::json!({"t":"route","route":"device","machine":"nuc","at":5}));
        let page = html(draw(&store(), asked).unwrap());
        assert!(page.contains("Route: Device (nuc)"), "{page}");
    }
}
