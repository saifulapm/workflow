//! The Worker: one entry point for every request, the walk store Durable
//! Object, and the home page built from it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use serde_json::{Value, json};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::wasm_bindgen_futures::JsFuture;
use worker::*;

use crate::access::{self, Access, Jwt};
use crate::evidence;
use crate::files;
use crate::item;
use crate::latest::{self, Change};
use crate::mcp;
use crate::memview;
use crate::pair;
use crate::share::{self, Fault, Log, Rec};
use crate::site;
use crate::webwrite;
use hubpages::form::encode_component;
use hubpages::page_evidence::image_kind;

/// The store's binding, and the one instance every request reaches.
const STORE: &str = "STORE";
const STORE_NAME: &str = "walk";
/// Saiful reads the hub from Dhaka, so the store is created in Asia-Pacific.
const STORE_LOCATION: &str = "apac";
/// The tags the store's sockets carry: a machine's sharing socket, or an
/// open page's, which hears only that the store changed.
const MACHINE: &str = "machine";
const PAGE: &str = "page";
/// Each project's files, one Durable Object per project id.
const FILES: &str = "FILES";

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    handle(req, env).await
}

/// A JWKS lookup for a kid the cached keys lack happens at most this often,
/// so a stream of made-up kids cannot turn into a stream of fetches.
const REFETCH_EVERY_MS: u64 = 60_000;

/// Every request passes through here, so the Access JWT is checked in this
/// one place.
async fn handle(req: Request, env: Env) -> Result<Response> {
    let path = req.path();
    // The store write carries its own bearer STORE_TOKEN, so a machine can
    // send records with no Access login.
    if req.method() == Method::Put && path == "/store/records" {
        return put_records(req, &env).await;
    }
    // The sharing socket takes the same bearer, and the store answers the
    // upgrade itself so it can keep the socket while it hibernates.
    if req.method() == Method::Get && path == "/store/share" {
        if !bearer(&req, &env)? {
            return Response::error("Unauthorized", 401);
        }
        return store(&env)?.fetch_with_request(req).await;
    }
    // A project's files take the same bearer, so a machine uploads and
    // fetches them with no Access login.
    if matches!(req.method(), Method::Put | Method::Get)
        && let Some(raw) = path.strip_prefix("/store/files/")
    {
        if !bearer(&req, &env)? {
            return Response::error("Unauthorized", 401);
        }
        let Some(project) = files::decode(raw)
            .as_deref()
            .and_then(files::project_of)
            .map(str::to_string)
        else {
            return Response::error("Expected projects/<id>/evidence|raw/<name>", 400);
        };
        return env
            .durable_object(FILES)?
            .get_by_name_with_location_hint(&project, STORE_LOCATION)?
            .fetch_with_request(req)
            .await;
    }
    // A machine with the store token asks for a pairing code.
    if req.method() == Method::Post && path == "/store/pair" {
        if !bearer(&req, &env)? {
            return Response::error("Unauthorized", 401);
        }
        return store(&env)?.fetch_with_request(req).await;
    }
    if let Err(why) = guard(&req, &env).await {
        console_log!("hubweb: refused {:?} {path}: {why}", req.method());
        return Response::error("Unauthorized", 401);
    }
    match (req.method(), path.as_str()) {
        // An open page's socket, which hears only that the store changed.
        (Method::Get, "/live") => store(&env)?.fetch_with_request(req).await,
        (Method::Get, _) if evidence_file(&path).is_some() => {
            let (project, id) = evidence_file(&path).unwrap_or_default();
            serve_evidence(project, id, &env).await
        }
        (Method::Post, "/" | "/mcp") => {
            // claude.ai calls the connector from its servers. A browser
            // says where its request comes from, and a form can send only
            // a few content types: refusing both keeps a page signed in to
            // Access from writing through the connector.
            if req.headers().get("Sec-Fetch-Site")?.is_some() {
                return Response::error("the connector does not take a browser's request", 403);
            }
            let json = req
                .headers()
                .get("Content-Type")?
                .is_some_and(|kind| kind.trim_start().starts_with("application/json"));
            if !json {
                return Response::error("the connector takes application/json", 415);
            }
            serve_mcp(req, &env).await
        }
        (_, "/mcp") => Response::error("Method Not Allowed", 405),
        (Method::Get | Method::Post, _) => page(req, &env).await,
        _ => Response::error("Method Not Allowed", 405),
    }
}

/// `Ok` only for a token one of this Worker's Access apps would have issued,
/// and never while ACCESS_AUD or ACCESS_TEAM is unset. The error says why,
/// for the log.
async fn guard(req: &Request, env: &Env) -> std::result::Result<(), &'static str> {
    let setting = |name| env.secret(name).map(|value| value.to_string()).ok();
    let access = Access::new(
        setting("ACCESS_AUD").as_deref(),
        setting("ACCESS_TEAM").as_deref(),
    )
    .ok_or("ACCESS_AUD or ACCESS_TEAM is unset")?;
    let token = req.headers().get("Cf-Access-Jwt-Assertion").ok().flatten();
    let jwt = access::parse(token.as_deref())?;
    let jwk = key(&access, &jwt.kid).await.ok_or("unknown kid")?;
    if !verify(&jwk, &jwt).await.unwrap_or(false) {
        return Err("bad signature");
    }
    let now = (Date::now().as_millis() / 1000) as i64;
    access.claims(&jwt, now)
}

/// The team's keys, kept for as long as the isolate lives.
#[derive(Default)]
struct Keys {
    by_kid: HashMap<String, Value>,
    fetched: Option<u64>,
}

thread_local! {
    static KEYS: RefCell<Keys> = RefCell::default();
}

/// The JWK for `kid`, fetching the team's keys when none are cached yet or,
/// at most once a minute, when `kid` is new (Access rotates keys).
async fn key(access: &Access, kid: &str) -> Option<Value> {
    if let Some(jwk) = KEYS.with_borrow(|keys| keys.by_kid.get(kid).cloned()) {
        return Some(jwk);
    }
    let now = Date::now().as_millis();
    let due = KEYS.with_borrow_mut(|keys| {
        if keys.fetched.is_some_and(|at| now - at < REFETCH_EVERY_MS) {
            return false;
        }
        keys.fetched = Some(now);
        true
    });
    if !due {
        return None;
    }
    match fetch_keys(access).await {
        Ok(fresh) => KEYS.with_borrow_mut(|keys| keys.by_kid = fresh),
        Err(why) => console_error!("hubweb: fetching Access keys: {why}"),
    }
    KEYS.with_borrow(|keys| keys.by_kid.get(kid).cloned())
}

/// A Request, not a `Url`: `Url::parse` brings the url crate's IDNA tables,
/// about 200 KB of wasm.
async fn fetch_keys(access: &Access) -> std::result::Result<HashMap<String, Value>, String> {
    let request = Request::new(&access.certs_url(), Method::Get).map_err(|e| e.to_string())?;
    let mut answer = Fetch::Request(request)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&answer.status_code()) {
        return Err(format!("status {}", answer.status_code()));
    }
    let body = answer.bytes().await.map_err(|e| e.to_string())?;
    access::jwks(&body)
}

/// Checks the token's RS256 signature with the runtime's WebCrypto.
async fn verify(jwk: &Value, jwt: &Jwt<'_>) -> std::result::Result<bool, JsValue> {
    const RS256: &str = "RSASSA-PKCS1-v1_5";
    let object = |value: &Value| {
        js_sys::JSON::parse(&value.to_string()).map(JsCast::unchecked_into::<js_sys::Object>)
    };
    let crypto: web_sys::Crypto =
        js_sys::Reflect::get(&js_sys::global(), &"crypto".into())?.unchecked_into();
    let subtle = crypto.subtle();
    let algorithm = object(&json!({"name": RS256, "hash": "SHA-256"}))?;
    let usages = js_sys::Array::of1(&"verify".into());
    let imported =
        subtle.import_key_with_object("jwk", &object(jwk)?, &algorithm, false, &usages)?;
    let key: web_sys::CryptoKey = JsFuture::from(imported).await?.unchecked_into();
    let verified = subtle.verify_with_str_and_u8_array_and_u8_array(
        RS256,
        &key,
        &jwt.signature,
        jwt.signed.as_bytes(),
    )?;
    Ok(JsFuture::from(verified).await?.as_bool() == Some(true))
}

/// One JSON-RPC message in, its JSON reply out, served by the store from
/// its files; a write the connector makes becomes the store's own.
async fn serve_mcp(mut req: Request, env: &Env) -> Result<Response> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(req.bytes().await?.into()));
    store(env)?
        .fetch_with_request(Request::new_with_init("https://store/mcp", &init)?)
        .await
}

fn store(env: &Env) -> Result<Stub> {
    env.durable_object(STORE)?
        .get_by_name_with_location_hint(STORE_NAME, STORE_LOCATION)
}

/// The project and id of `/p/<project>/file/<id>`, as the path spells them.
fn evidence_file(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/p/")?;
    let (project, id) = rest.split_once("/file/")?;
    (!project.is_empty() && !id.is_empty() && !project.contains('/') && !id.contains('/'))
        .then_some((project, id))
}

/// One evidence file: the store names its path, the project's Files object
/// holds the bytes. A Durable Object cannot fetch another one synchronously,
/// so the page code cannot do this and the Worker does.
async fn serve_evidence(project: &str, id: &str, env: &Env) -> Result<Response> {
    let mut named = store(env)?
        .fetch_with_str(&format!("https://store/evidence/{project}/{id}"))
        .await?;
    if named.status_code() != 200 {
        return Response::error("Not Found", 404);
    }
    let path = named.text().await?;
    let Some(owner) = files::project_of(&path) else {
        return Response::error("Not Found", 404);
    };
    let encoded: Vec<String> = path.split('/').map(encode_component).collect();
    let mut answer = env
        .durable_object(FILES)?
        .get_by_name_with_location_hint(owner, STORE_LOCATION)?
        .fetch_with_str(&format!("https://files/store/files/{}", encoded.join("/")))
        .await?;
    if answer.status_code() != 200 {
        return Response::error("Not Found", 404);
    }
    let bytes = answer.bytes().await?;
    let kind = image_kind(&bytes).unwrap_or("text/plain; charset=utf-8");
    let mut response = Response::from_bytes(bytes)?;
    let headers = response.headers_mut();
    headers.set("Content-Type", kind)?;
    headers.set("Cache-Control", "private, max-age=3600")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    Ok(response)
}

/// A hub page, which the store draws from its files. Server-Timing carries
/// the store's own build time and the whole round trip to it.
async fn page(mut req: Request, env: &Env) -> Result<Response> {
    let start = Date::now().as_millis();
    // The store sees the request under its own host, so whether the POST
    // came from the hub web's own pages is decided here, and any such
    // header the browser sent is replaced.
    let headers = req.headers().clone();
    let asked = site::request(
        req.method().as_ref(),
        &req.path(),
        headers.entries().collect(),
        Vec::new(),
    );
    headers.set(MAY_WRITE, if pair::may_write(&asked) { "1" } else { "0" })?;
    let mut init = RequestInit::new();
    // A page's See Other is the browser's to follow, not this fetch's.
    init.with_method(req.method())
        .with_headers(headers)
        .with_redirect(RequestRedirect::Manual);
    if req.method() == Method::Post {
        init.with_body(Some(req.bytes().await?.into()));
    }
    let target = format!("https://store/page{}", path_and_query(&req));
    let mut answer = store(env)?
        .fetch_with_request(Request::new_with_init(&target, &init)?)
        .await?;
    let bytes = answer.bytes().await?;
    let took = Date::now().as_millis() - start;
    // The store's response is immutable, so its parts go into a new one.
    let mut page = Response::from_bytes(bytes)?.with_status(answer.status_code());
    for (name, value) in answer.headers().entries() {
        let value = match name.as_str() {
            "server-timing" => format!("{value}, store;dur={took}"),
            _ => value,
        };
        // A new response has a content type of its own: set replaces it,
        // where append would send both and a phone would offer a download.
        // A cookie is the one header that may come more than once.
        match name.as_str() {
            "set-cookie" => page.headers_mut().append(&name, &value)?,
            _ => page.headers_mut().set(&name, &value)?,
        }
    }
    Ok(page)
}

/// The header the Worker tells the store with whether a POST came from the
/// hub web's own pages.
const MAY_WRITE: &str = "X-Hub-May-Write";

/// The path and query of a request, from its URL string: `Request::url`
/// brings the url crate's IDNA tables, about 200 KB of wasm.
fn path_and_query(req: &Request) -> String {
    let url = req.inner().url();
    let after_scheme = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
    match after_scheme.find('/') {
        Some(at) => after_scheme[at..].to_string(),
        None => "/".to_string(),
    }
}

/// Hands the records to the store when the request carries the bearer
/// STORE_TOKEN; 401 otherwise, and 401 while no token is configured.
async fn put_records(mut req: Request, env: &Env) -> Result<Response> {
    if !bearer(&req, env)? {
        return Response::error("Unauthorized", 401);
    }
    let body = req.text().await?;
    let mut init = RequestInit::new();
    init.with_method(Method::Put).with_body(Some(body.into()));
    store(env)?
        .fetch_with_request(Request::new_with_init("https://store/records", &init)?)
        .await
}

/// True when the request carries `Authorization: Bearer <STORE_TOKEN>`;
/// false while no token is configured.
fn bearer(req: &Request, env: &Env) -> Result<bool> {
    let token = env.secret("STORE_TOKEN").map(|t| t.to_string()).ok();
    let given = req.headers().get("Authorization")?;
    Ok(match (token, given) {
        (Some(token), Some(given)) => {
            !token.is_empty() && same(given.as_bytes(), format!("Bearer {token}").as_bytes())
        }
        _ => false,
    })
}

/// Compares two byte strings in time that depends on their length only, so
/// a wrong token does not tell how much of it matched.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The walk store: records keyed by project, kind and key, and the shared
/// mem write log, in the Durable Object's SQLite.
#[durable_object]
pub struct Store {
    state: State,
    env: Env,
    sql: SqlStorage,
    /// The UTC day number a write last hit a Free limit, while the object is
    /// awake. Each open socket's attachment holds it through hibernation.
    limit_day: Cell<Option<u64>>,
}

impl Store {
    /// The newest limit day this object hit, from memory or from any open
    /// socket's attachment.
    fn limit_day(&self) -> Result<Option<u64>> {
        let mut attachments = Vec::new();
        for socket in self.state.get_websockets_with_tag(MACHINE) {
            attachments.push(attachment(&socket)?);
        }
        Ok(share::newest_limit_day(&attachments).max(self.limit_day.get()))
    }

    /// Draws the page the Worker forwarded as `/page<path>`, timing the
    /// build in Server-Timing.
    async fn page(&self, mut req: Request) -> Result<Response> {
        let target = path_and_query(&req)["/page".len()..].to_string();
        let method = req.method().to_string();
        let headers: Vec<(String, String)> = req.headers().entries().collect();
        let body = if req.method() == Method::Post {
            req.bytes().await?
        } else {
            Vec::new()
        };
        let req_may_write = req.headers().get(MAY_WRITE)?.as_deref() == Some("1");
        let start = Date::now().as_millis();
        let request = site::request(&method, &target, headers, body);
        let route = SqlLog(&self.sql)
            .route()
            .map_err(|fault| Error::RustError(format!("{fault:?}")))?;
        let ask = site::Ask {
            request: &request,
            now: jiff::Timestamp::from_millisecond(now_ms() as i64).unwrap_or_default(),
            route,
            paired: self.paired(&request)?,
            may_write: req_may_write,
            limited: site::limit_is_today(self.limit_day()?, now_ms()),
            window_ms: self
                .env
                .secret("HUB_AGENT_WINDOW_MS")
                .ok()
                .and_then(|window| window.to_string().trim().parse().ok())
                .unwrap_or(site::AGENT_WINDOW_MS),
            stamp: self.stamp()?,
        };
        let mut random = || random().unwrap_or_default();
        let Some((drawn, written, queued)) =
            site::page(&SqlFiles::new(&self.sql), ask, &mut random)
        else {
            return Response::error("Not Found", 404);
        };
        let took = Date::now().as_millis() - start;
        self.keep(webwrite::MACHINE, written.files, written.bytes)
            .await?;
        if let Some(kind) = written.route {
            self.route(kind)?;
        }
        self.send_commands(queued)?;
        let mut response = Response::from_bytes(drawn.body)?.with_status(drawn.status);
        let headers = response.headers_mut();
        headers.set("Content-Type", &drawn.content_type)?;
        for (name, value) in &drawn.headers {
            headers.append(name, value)?;
        }
        headers.set("Server-Timing", &format!("build;dur={took}"))?;
        Ok(response)
    }

    /// Adds writes the store made itself (the hub web's, the connector's) to
    /// its log: each file's bytes to its project's Files store first, so a
    /// machine that hears of an item finds its file; then each write, which
    /// every machine hears as a rec; then one nudge to the open pages.
    async fn keep(
        &self,
        machine: &str,
        writes: Vec<(String, Option<String>)>,
        bytes: Vec<(String, Vec<u8>)>,
    ) -> Result<()> {
        for (path, data) in bytes {
            let Some(project) = files::project_of(&path) else {
                return Err(Error::RustError(format!("{path} is not a project's file")));
            };
            let mut init = RequestInit::new();
            init.with_method(Method::Put).with_body(Some(data.into()));
            let target = format!("https://files/store/files/{}", encode_component(&path));
            let answer = self
                .env
                .durable_object(FILES)?
                .get_by_name_with_location_hint(project, STORE_LOCATION)?
                .fetch_with_request(Request::new_with_init(&target, &init)?)
                .await?;
            if answer.status_code() != 204 {
                return Err(Error::RustError(format!(
                    "the files store answered {} for {path}",
                    answer.status_code()
                )));
            }
        }
        if writes.is_empty() {
            return Ok(());
        }
        for (path, body) in writes {
            let id = item::ulid(now_ms(), random()?);
            let rec = share::local_write(&mut SqlLog(&self.sql), &id, &path, body, machine)
                .map_err(|fault| Error::RustError(format!("{fault:?}")))?;
            for socket in self.state.get_websockets_with_tag(MACHINE) {
                // A closing socket's machine gets it from the replay.
                let _ = socket.send_with_str(&rec);
            }
        }
        self.nudge_pages();
        Ok(())
    }

    /// Keeps the commands a page queued and sends each at once as a cmd
    /// frame to every machine socket; a machine that is away gets it from
    /// the replay. Then one nudge, so open pages read `waiting for`.
    fn send_commands(&self, queued: site::Queued) -> Result<()> {
        if queued.is_empty() {
            return Ok(());
        }
        for (machine, command) in queued {
            let id = item::ulid(now_ms(), random()?);
            let body = serde_json::to_value(&command)?;
            let frame = share::queue_command(&mut SqlLog(&self.sql), &id, &machine, &body)
                .map_err(|fault| Error::RustError(format!("{fault:?}")))?;
            for socket in self.state.get_websockets_with_tag(MACHINE) {
                let _ = socket.send_with_str(&frame);
            }
        }
        self.nudge_pages();
        Ok(())
    }

    /// Takes the route the hub web chose and hands it to every machine.
    fn route(&self, kind: &str) -> Result<()> {
        let chosen = share::choose(&mut SqlLog(&self.sql), kind, now_ms() as i64)
            .map_err(|fault| Error::RustError(format!("{fault:?}")))?;
        if let Some(message) = chosen {
            for socket in self.state.get_websockets_with_tag(MACHINE) {
                let _ = socket.send_with_str(&message);
            }
            self.nudge_pages();
        }
        Ok(())
    }

    /// A page's own load time, from the hub web's own pages: the last 100
    /// are kept. No pairing: a timing is no mem write, and a cold load is
    /// measured in a private tab, which holds no pair cookie.
    async fn timing(&self, mut req: Request) -> Result<Response> {
        if req.headers().get(MAY_WRITE)?.as_deref() != Some("1") {
            return Response::error("cross-origin writes are refused", 403);
        }
        let body = req.text().await?;
        if body.len() > 2048 || serde_json::from_str::<Value>(&body).is_err() {
            return Response::error("Expected a small JSON timing", 400);
        }
        self.sql.exec(
            "INSERT INTO timings(at, body) VALUES (?, ?)",
            vec![(now_ms() as i64).into(), body.into()],
        )?;
        self.sql.exec(
            "DELETE FROM timings WHERE at NOT IN (SELECT at FROM timings ORDER BY at DESC LIMIT 100)",
            None,
        )?;
        Response::empty().map(|r| r.with_status(204))
    }

    /// Whether the request carries a paired device's cookie.
    fn paired(&self, request: &hubpages::http::Request) -> Result<bool> {
        let Some(token) = pair::token(request) else {
            return Ok(false);
        };
        let rows: Vec<Value> = self
            .sql
            .exec("SELECT 1 FROM devices WHERE token = ?", vec![token.into()])?
            .to_array()?;
        Ok(!rows.is_empty())
    }

    /// `GET /pair/<code>`: the Pair button while the code is pending.
    /// `POST /pair/<code>`, from the hub web's own page: spends the code and
    /// hands the device its cookie.
    async fn pair(&self, req: Request) -> Result<Response> {
        let code = path_and_query(&req)["/page/pair/".len()..].to_string();
        let now = now_ms() as i64;
        let pending = pair::is_code(&code)
            && !self
                .sql
                .exec(
                    "SELECT 1 FROM pair_codes WHERE code = ? AND expires > ?",
                    vec![code.as_str().into(), now.into()],
                )?
                .to_array::<Value>()?
                .is_empty();
        if !pending {
            return Ok(Response::from_html(pair::gone())?.with_status(404));
        }
        match req.method() {
            Method::Get => Response::from_html(pair::page(&code)),
            Method::Post if req.headers().get(MAY_WRITE)?.as_deref() == Some("1") => {
                let token = pair::new_token(random()?);
                let agent = req.headers().get("User-Agent")?;
                self.sql.exec(
                    "INSERT INTO devices(token, created, name) VALUES (?, ?, ?)",
                    vec![
                        token.as_str().into(),
                        jiff::Timestamp::from_millisecond(now)
                            .unwrap_or_default()
                            .to_string()
                            .into(),
                        pair::device_name(agent.as_deref()).into(),
                    ],
                )?;
                self.sql.exec(
                    "DELETE FROM pair_codes WHERE code = ?",
                    vec![code.as_str().into()],
                )?;
                let mut response = Response::empty()?.with_status(303);
                response.headers_mut().set("Location", "/")?;
                response
                    .headers_mut()
                    .set("Set-Cookie", &pair::cookie(&token))?;
                Ok(response)
            }
            Method::Post => Response::error("cross-origin writes are refused", 403),
            _ => Response::error("Method Not Allowed", 405),
        }
    }

    /// Tells every open page the store changed, and nothing more: the page
    /// reloads itself when no one is typing.
    fn nudge_pages(&self) {
        let Ok(changed) = self.changed() else {
            return;
        };
        for page in self.state.get_websockets_with_tag(PAGE) {
            // A page that is closing misses it; its next socket hears the stamp.
            let _ = page.send_with_str(&changed);
        }
    }

    /// What the store has seen, for a page to compare with the stamp it was
    /// drawn at: the newest write's seq, the route's time, and when a
    /// machine's report or its commands last changed.
    fn stamp(&self) -> Result<String> {
        let log = SqlLog(&self.sql);
        let fault = |fault: Fault| Error::RustError(format!("{fault:?}"));
        let seq = log.last().map_err(fault)?;
        let at = log
            .route()
            .map_err(fault)?
            .and_then(|route| route["at"].as_i64())
            .unwrap_or(0);
        let fleet = SqlFiles::new(&self.sql)
            .rows("SELECT max(changed) AS changed FROM machines", None)
            .first()
            .and_then(|row| row["changed"].as_i64())
            .unwrap_or(0);
        Ok(format!("{seq}.{at}.{fleet}"))
    }

    /// The message an open page hears: the store changed, and its stamp.
    fn changed(&self) -> Result<String> {
        Ok(json!({"t":"changed","stamp":self.stamp()?}).to_string())
    }
}

/// A socket's attachment, Null when it has none.
fn attachment(socket: &WebSocket) -> Result<Value> {
    Ok(socket.deserialize_attachment()?.unwrap_or(Value::Null))
}

impl DurableObject for Store {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        sql.exec(
            "CREATE TABLE IF NOT EXISTS records(project TEXT, kind TEXT, key TEXT, body TEXT, \
             PRIMARY KEY(project, kind, key))",
            None,
        )
        .expect("create the records table");
        // share::handle sets seq one above the highest, and no row is ever
        // deleted, so a seq is never handed out twice.
        sql.exec(
            "CREATE TABLE IF NOT EXISTS writes(seq INTEGER PRIMARY KEY, id TEXT UNIQUE NOT NULL, \
             path TEXT NOT NULL, body TEXT, machine TEXT)",
            None,
        )
        .expect("create the writes table");
        sql.exec(
            "CREATE INDEX IF NOT EXISTS writes_by_path ON writes(path, seq)",
            None,
        )
        .expect("index the writes by path");
        // The newest body of each project path, which the pages read.
        sql.exec(
            "CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY, project TEXT NOT NULL, \
             kind TEXT, seq INTEGER NOT NULL, body TEXT NOT NULL)",
            None,
        )
        .expect("create the files table");
        sql.exec(
            "CREATE INDEX IF NOT EXISTS files_by_kind ON files(project, kind)",
            None,
        )
        .expect("index the files by kind");
        fill_files(&sql).expect("fill the files table from the log");
        sql.exec(
            "CREATE INDEX IF NOT EXISTS files_by_kind_alone ON files(kind)",
            None,
        )
        .expect("index the files by kind alone");
        mark_projects(&sql).expect("mark each project.toml");
        sql.exec(
            "CREATE TABLE IF NOT EXISTS timings(at INTEGER NOT NULL, body TEXT NOT NULL)",
            None,
        )
        .expect("create the timings table");
        sql.exec(
            "CREATE TABLE IF NOT EXISTS pair_codes(code TEXT PRIMARY KEY, expires INTEGER NOT NULL)",
            None,
        )
        .expect("create the pair_codes table");
        sql.exec(
            "CREATE TABLE IF NOT EXISTS devices(token TEXT PRIMARY KEY, created TEXT NOT NULL, \
             name TEXT NOT NULL)",
            None,
        )
        .expect("create the devices table");
        sql.exec(
            "CREATE TABLE IF NOT EXISTS route(one INTEGER PRIMARY KEY CHECK (one = 1), \
             body TEXT NOT NULL)",
            None,
        )
        .expect("create the route table");
        // Each machine's newest report, by runner name: `at` moves with
        // every report, `changed` only when what it says does, so a page's
        // stamp holds still while machines report the same.
        sql.exec(
            "CREATE TABLE IF NOT EXISTS machines(name TEXT PRIMARY KEY, at INTEGER NOT NULL, \
             changed INTEGER NOT NULL, body TEXT NOT NULL)",
            None,
        )
        .expect("create the machines table");
        sql.exec(
            "CREATE TABLE IF NOT EXISTS commands(id TEXT PRIMARY KEY, machine TEXT NOT NULL, \
             at INTEGER NOT NULL, body TEXT NOT NULL)",
            None,
        )
        .expect("create the commands table");
        Self {
            state,
            env,
            sql,
            limit_day: Cell::new(None),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        match (req.method(), req.path().as_str()) {
            (Method::Put, "/records") => {
                let records: Vec<Value> = match req.json().await {
                    Ok(records) => records,
                    Err(_) => return Response::error("Expected a JSON array of records", 400),
                };
                let mut rows = Vec::with_capacity(records.len());
                for record in &records {
                    let field = |name: &str| record[name].as_str().map(str::to_string);
                    // A project's body is its `mem projects --json` row; the
                    // column holds it as JSON text, as it holds a handoff's text.
                    let body = match &record["body"] {
                        Value::String(text) => Some(text.clone()),
                        Value::Null => None,
                        other => Some(other.to_string()),
                    };
                    match (field("project"), field("kind"), field("key"), body) {
                        (Some(project), Some(kind), Some(key), Some(body)) => {
                            rows.push([project, kind, key, body])
                        }
                        _ => {
                            return Response::error(
                                "Each record needs project, kind, key and body",
                                400,
                            );
                        }
                    }
                }
                for [project, kind, key, body] in rows {
                    self.sql.exec(
                        "INSERT INTO records(project, kind, key, body) VALUES (?, ?, ?, ?) \
                         ON CONFLICT(project, kind, key) DO UPDATE SET body = excluded.body",
                        vec![project.into(), kind.into(), key.into(), body.into()],
                    )?;
                }
                Ok(Response::empty()?.with_status(204))
            }
            (_, path) if path.starts_with("/page/pair/") => self.pair(req).await,
            // A bare round trip, the baseline a page's build is measured by.
            (Method::Get, "/page/api/ping") => {
                let mut response = Response::empty()?.with_status(204);
                response.headers_mut().set("Server-Timing", "build;dur=0")?;
                Ok(response)
            }
            (Method::Post, "/page/api/timing") => self.timing(req).await,
            (Method::Get, "/page/api/timing") => {
                let rows: Vec<Value> = self
                    .sql
                    .exec(
                        "SELECT at, body FROM timings ORDER BY at DESC LIMIT 50",
                        None,
                    )?
                    .to_array()?;
                Response::from_json(&rows)
            }
            (Method::Get, path) if path.starts_with("/evidence/") => {
                let Some((project, id)) = path["/evidence/".len()..].split_once('/') else {
                    return Response::error("Not Found", 404);
                };
                let project = hubpages::form::decode_path(project);
                match evidence::store_path(&SqlFiles::new(&self.sql), &project, id) {
                    Some(path) => Response::ok(path),
                    None => Response::error("Not Found", 404),
                }
            }
            (_, path) if path.starts_with("/page/") => self.page(req).await,
            (Method::Post, "/store/pair") => {
                let code = pair::new_code(random()?);
                self.sql.exec(
                    "DELETE FROM pair_codes WHERE expires <= ?",
                    vec![(now_ms() as i64).into()],
                )?;
                self.sql.exec(
                    "INSERT INTO pair_codes(code, expires) VALUES (?, ?)",
                    vec![
                        code.as_str().into(),
                        (now_ms() as i64 + pair::CODE_TTL_MS).into(),
                    ],
                )?;
                Response::ok(code)
            }
            (Method::Post, "/mcp") => {
                let body = req.bytes().await?;
                let mut random = || random().unwrap_or_default();
                let served = mcp::serve(
                    &body,
                    mcp::Store {
                        files: &SqlFiles::new(&self.sql),
                        now: jiff::Timestamp::from_millisecond(now_ms() as i64).unwrap_or_default(),
                        random: &mut random,
                    },
                );
                self.keep(mcp::MACHINE, served.writes, served.files).await?;
                let mut response = Response::ok(served.body)?.with_status(served.status);
                response
                    .headers_mut()
                    .set("Content-Type", "application/json")?;
                Ok(response)
            }
            (Method::Get, "/store/share") => {
                if req.headers().get("Upgrade")?.as_deref() != Some("websocket") {
                    return Response::error("Expected a WebSocket upgrade", 426);
                }
                let pair = WebSocketPair::new()?;
                self.state
                    .accept_websocket_with_tags(&pair.server, &[MACHINE]);
                Response::from_websocket(pair.client)
            }
            (Method::Get, "/live") => {
                if req.headers().get("Upgrade")?.as_deref() != Some("websocket") {
                    return Response::error("Expected a WebSocket upgrade", 426);
                }
                let pair = WebSocketPair::new()?;
                self.state.accept_websocket_with_tags(&pair.server, &[PAGE]);
                // The page compares this with the stamp it was drawn at, so
                // a change before its socket opened is not missed.
                pair.server.send_with_str(&self.changed()?)?;
                Response::from_websocket(pair.client)
            }
            _ => Response::error("Not Found", 404),
        }
    }

    /// Nothing here awaits, so the order the store numbers writes in is the
    /// order their recs go out on every socket.
    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        // A page only listens: nothing it sends is a machine's message.
        if self.state.get_tags(&ws).iter().any(|tag| tag == PAGE) {
            return Ok(());
        }
        let WebSocketIncomingMessage::String(text) = message else {
            ws.send_with_str(r#"{"t":"error","why":"messages are JSON text frames"}"#)?;
            return Ok(());
        };
        // The machine's name lives on the socket, which outlasts hibernation.
        let (machine, _) = share::attached(&attachment(&ws)?);
        let outcome = share::handle(&mut SqlLog(&self.sql), machine.as_deref(), &text);
        if let Some(machine) = &outcome.hello {
            ws.serialize_attachment(share::attachment(Some(machine), self.limit_day()?))?;
        }
        for reply in &outcome.sender {
            ws.send_with_str(reply)?;
        }
        if outcome.limit {
            let day = utc_day();
            self.limit_day.set(Some(day));
            // The object forgets its memory when it hibernates, so every
            // socket carries the day for the home to find. A socket that is
            // closing may refuse it; the others still hold the day.
            for socket in self.state.get_websockets_with_tag(MACHINE) {
                if let Ok(value) = attachment(&socket) {
                    let (machine, _) = share::attached(&value);
                    let _ = socket
                        .serialize_attachment(share::attachment(machine.as_deref(), Some(day)));
                }
            }
        }
        if let Some(rec) = &outcome.others {
            for other in self.state.get_websockets_with_tag(MACHINE) {
                if other.as_ref() != ws.as_ref() {
                    // A socket that is closing misses the rec; its machine
                    // gets it from the replay after its next hello.
                    let _ = other.send_with_str(rec);
                }
            }
            self.nudge_pages();
        } else if outcome.changed {
            self.nudge_pages();
        }
        Ok(())
    }

    async fn websocket_close(
        &self,
        _ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        Ok(())
    }

    async fn websocket_error(&self, _ws: WebSocket, _error: Error) -> Result<()> {
        Ok(())
    }
}

/// One project's evidence and raw files, each stored as rows of at most
/// `files::CHUNK` bytes in the object's SQLite.
#[durable_object]
pub struct Files {
    sql: SqlStorage,
}

impl DurableObject for Files {
    fn new(state: State, _env: Env) -> Self {
        let sql = state.storage().sql();
        sql.exec(
            "CREATE TABLE IF NOT EXISTS chunks(path TEXT, n INTEGER, data BLOB, \
             PRIMARY KEY(path, n))",
            None,
        )
        .expect("create the chunks table");
        Self { sql }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let Some(path) = req
            .path()
            .strip_prefix("/store/files/")
            .and_then(files::decode)
            .filter(|path| files::project_of(path).is_some())
        else {
            return Response::error("Expected projects/<id>/evidence|raw/<name>", 400);
        };
        let answer = match req.method() {
            Method::Put => {
                // Every await comes before the first write, so the delete and
                // the inserts commit together and two PUTs never interleave.
                let bytes = req.bytes().await?;
                self.put(&path, &bytes)
                    .and_then(|()| Ok(Response::empty()?.with_status(204)))
            }
            Method::Get => match self.get(&path) {
                Ok(rows) if rows.is_empty() => Response::error("No such file", 404),
                Ok(rows) => Response::from_bytes(files::join(rows)),
                Err(error) => Err(error),
            },
            _ => Response::error("Method Not Allowed", 405),
        };
        // A limit is the Free plan's daily cap: the hub keeps the file and
        // tries again later.
        match answer {
            Err(error) if names_limit(&error.to_string()) => {
                Response::error("Cloudflare limit reached", 503)
            }
            answer => answer,
        }
    }
}

impl Files {
    /// Replaces every row of `path` with the rows of `bytes`.
    fn put(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.sql
            .exec("DELETE FROM chunks WHERE path = ?", vec![path.into()])?;
        for (n, row) in files::split(bytes).into_iter().enumerate() {
            self.sql.exec(
                "INSERT INTO chunks(path, n, data) VALUES (?, ?, ?)",
                vec![
                    path.into(),
                    (n as i64).into(),
                    SqlStorageValue::Blob(row.to_vec()),
                ],
            )?;
        }
        Ok(())
    }

    /// The rows of `path` in order; none when it is not stored. A BLOB comes
    /// back as an ArrayBuffer, which only the raw cursor turns into bytes.
    fn get(&self, path: &str) -> Result<Vec<Vec<u8>>> {
        self.sql
            .exec(
                "SELECT data FROM chunks WHERE path = ? ORDER BY n",
                vec![path.into()],
            )?
            .raw()
            .map(|row| match row?.pop() {
                Some(SqlStorageValue::Blob(bytes)) => Ok(bytes),
                other => Err(Error::RustError(format!(
                    "a chunk is {other:?}, not a blob"
                ))),
            })
            .collect()
    }
}

/// The store's files table, as memview reads files. A failed query is kept,
/// so the page says the store could not be read instead of drawing an empty
/// one.
struct SqlFiles<'a> {
    sql: &'a SqlStorage,
    fault: RefCell<Option<String>>,
}

impl<'a> SqlFiles<'a> {
    fn new(sql: &'a SqlStorage) -> Self {
        SqlFiles {
            sql,
            fault: RefCell::new(None),
        }
    }

    fn rows(&self, query: &str, bindings: Option<Vec<SqlStorageValue>>) -> Vec<Value> {
        match self
            .sql
            .exec(query, bindings)
            .and_then(|cursor| cursor.to_array())
        {
            Ok(rows) => rows,
            Err(error) => {
                self.fault.borrow_mut().get_or_insert(error.to_string());
                Vec::new()
            }
        }
    }

    fn pairs(&self, query: &str, bindings: Vec<SqlStorageValue>) -> Vec<(String, String)> {
        let rows = self.rows(query, Some(bindings));
        rows.iter()
            .filter_map(|row| {
                Some((
                    row["path"].as_str()?.to_string(),
                    row["body"].as_str()?.to_string(),
                ))
            })
            .collect()
    }
}

impl memview::Files for SqlFiles<'_> {
    fn fault(&self) -> Option<String> {
        self.fault.borrow().clone()
    }

    fn read(&self, path: &str) -> Option<String> {
        self.pairs(
            "SELECT path, body FROM files WHERE path = ?",
            vec![path.into()],
        )
        .pop()
        .map(|(_, body)| body)
    }

    fn list(&self, prefix: &str) -> Vec<(String, String)> {
        // A range on the primary key: every path that starts with `prefix`.
        self.pairs(
            "SELECT path, body FROM files WHERE path >= ? AND path < ? ORDER BY path",
            vec![prefix.into(), format!("{prefix}\u{10FFFF}").into()],
        )
    }

    fn projects(&self) -> Vec<String> {
        let rows = self.rows(
            &format!(
                "SELECT project FROM files WHERE kind = '{}' ORDER BY project",
                latest::PROJECT
            ),
            None,
        );
        rows.iter()
            .filter_map(|row| row["project"].as_str().map(str::to_string))
            .collect()
    }

    fn reports(&self) -> Vec<(String, i64, String)> {
        let rows = self.rows("SELECT name, at, body FROM machines ORDER BY name", None);
        rows.iter()
            .filter_map(|row| {
                Some((
                    row["name"].as_str()?.to_string(),
                    row["at"].as_i64()?,
                    row["body"].as_str()?.to_string(),
                ))
            })
            .collect()
    }

    fn queued(&self) -> Vec<(String, String)> {
        let rows = self.rows("SELECT machine, body FROM commands ORDER BY at, id", None);
        rows.iter()
            .filter_map(|row| {
                Some((
                    row["machine"].as_str()?.to_string(),
                    row["body"].as_str()?.to_string(),
                ))
            })
            .collect()
    }

    fn items(&self, project: &str, kinds: &[&str]) -> Vec<(String, String)> {
        if kinds.is_empty() {
            return self.list(&format!("projects/{project}/items/"));
        }
        let marks = vec!["?"; kinds.len()].join(", ");
        let mut bindings: Vec<SqlStorageValue> = vec![project.into()];
        bindings.extend(kinds.iter().map(|kind| (*kind).into()));
        self.pairs(
            &format!(
                "SELECT path, body FROM files WHERE project = ? AND kind IN ({marks}) ORDER BY path"
            ),
            bindings,
        )
    }
}

/// The write log in the store's SQLite.
struct SqlLog<'a>(&'a SqlStorage);

const COLUMNS: &str = "SELECT seq, id, path, body, machine FROM writes";

impl SqlLog<'_> {
    fn rows(
        &self,
        query: &str,
        bindings: Vec<SqlStorageValue>,
    ) -> std::result::Result<Vec<Rec>, Fault> {
        let rows: Vec<Value> = self
            .0
            .exec(query, Some(bindings))
            .and_then(|cursor| cursor.to_array())
            .map_err(fault)?;
        Ok(rows
            .iter()
            .map(|row| Rec {
                seq: row["seq"].as_i64().unwrap_or_default(),
                id: row["id"].as_str().unwrap_or_default().to_string(),
                path: row["path"].as_str().unwrap_or_default().to_string(),
                body: row["body"].as_str().map(str::to_string),
                machine: row["machine"].as_str().map(str::to_string),
            })
            .collect())
    }
}

impl Log for SqlLog<'_> {
    fn seq_of(&self, id: &str) -> std::result::Result<Option<i64>, Fault> {
        let rows = self.rows(&format!("{COLUMNS} WHERE id = ?"), vec![id.into()])?;
        Ok(rows.first().map(|rec| rec.seq))
    }

    fn head(&self, path: &str) -> std::result::Result<Option<Rec>, Fault> {
        let query = format!("{COLUMNS} WHERE path = ? ORDER BY seq DESC LIMIT 1");
        Ok(self.rows(&query, vec![path.into()])?.pop())
    }

    fn after(&self, seq: i64) -> std::result::Result<Vec<Rec>, Fault> {
        let query = format!("{COLUMNS} WHERE seq > ? ORDER BY seq");
        self.rows(&query, vec![seq.into()])
    }

    fn last(&self) -> std::result::Result<i64, Fault> {
        let query = format!("{COLUMNS} ORDER BY seq DESC LIMIT 1");
        Ok(self.rows(&query, vec![])?.first().map_or(0, |rec| rec.seq))
    }

    fn append(&mut self, rec: &Rec) -> std::result::Result<(), Fault> {
        self.0
            .exec(
                "INSERT INTO writes(seq, id, path, body, machine) VALUES (?, ?, ?, ?, ?)",
                vec![
                    rec.seq.into(),
                    rec.id.as_str().into(),
                    rec.path.as_str().into(),
                    rec.body.clone().into(),
                    rec.machine.clone().into(),
                ],
            )
            .map_err(fault)?;
        keep_file(self.0, rec).map_err(fault)
    }

    fn route(&self) -> std::result::Result<Option<Value>, Fault> {
        let rows: Vec<Value> = self
            .0
            .exec("SELECT body FROM route", None)
            .and_then(|cursor| cursor.to_array())
            .map_err(fault)?;
        Ok(rows
            .first()
            .and_then(|row| row["body"].as_str())
            .and_then(|body| serde_json::from_str(body).ok()))
    }

    fn set_route(&mut self, route: &Value) -> std::result::Result<(), Fault> {
        self.0
            .exec(
                "INSERT INTO route(one, body) VALUES (1, ?) ON CONFLICT(one) DO UPDATE SET \
                 body = excluded.body",
                vec![route.to_string().into()],
            )
            .map(|_| ())
            .map_err(fault)
    }

    fn now_ms(&self) -> i64 {
        now_ms() as i64
    }

    fn report(&self, machine: &str) -> std::result::Result<Option<Value>, Fault> {
        let rows: Vec<Value> = self
            .0
            .exec(
                "SELECT body FROM machines WHERE name = ?",
                vec![machine.into()],
            )
            .and_then(|cursor| cursor.to_array())
            .map_err(fault)?;
        Ok(rows
            .first()
            .and_then(|row| row["body"].as_str())
            .and_then(|body| serde_json::from_str(body).ok()))
    }

    fn set_report(
        &mut self,
        machine: &str,
        at: i64,
        report: &Value,
    ) -> std::result::Result<(), Fault> {
        self.0
            .exec(
                "INSERT INTO machines(name, at, changed, body) VALUES (?1, ?2, ?2, ?3) \
                 ON CONFLICT(name) DO UPDATE SET at = excluded.at, \
                 changed = CASE WHEN body = excluded.body THEN changed ELSE excluded.at END, \
                 body = excluded.body",
                vec![machine.into(), at.into(), report.to_string().into()],
            )
            .map(|_| ())
            .map_err(fault)
    }

    fn commands(&self, machine: &str) -> std::result::Result<Vec<(String, Value)>, Fault> {
        let rows: Vec<Value> = self
            .0
            .exec(
                "SELECT id, body FROM commands WHERE machine = ? ORDER BY at, id",
                vec![machine.into()],
            )
            .and_then(|cursor| cursor.to_array())
            .map_err(fault)?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                let id = row["id"].as_str()?.to_string();
                let body = serde_json::from_str(row["body"].as_str()?).ok()?;
                Some((id, body))
            })
            .collect())
    }

    fn add_command(
        &mut self,
        id: &str,
        machine: &str,
        command: &Value,
    ) -> std::result::Result<(), Fault> {
        self.0
            .exec(
                "INSERT INTO commands(id, machine, at, body) VALUES (?, ?, ?, ?)",
                vec![
                    id.into(),
                    machine.into(),
                    (now_ms() as i64).into(),
                    command.to_string().into(),
                ],
            )
            .map_err(fault)?;
        // A page reading `waiting for <machine>` is to hear it start.
        self.0
            .exec(
                "UPDATE machines SET changed = ? WHERE name = ?",
                vec![(now_ms() as i64).into(), machine.into()],
            )
            .map(|_| ())
            .map_err(fault)
    }

    fn drop_command(&mut self, id: &str, machine: &str) -> std::result::Result<bool, Fault> {
        let gone = self
            .0
            .exec(
                "DELETE FROM commands WHERE id = ? AND machine = ?",
                vec![id.into(), machine.into()],
            )
            .map_err(fault)?
            .rows_written();
        if gone > 0 {
            // A page reading `waiting for <machine>` is to hear it stopped.
            self.0
                .exec(
                    "UPDATE machines SET changed = ? WHERE name = ?",
                    vec![(now_ms() as i64).into(), machine.into()],
                )
                .map_err(fault)?;
        }
        Ok(gone > 0)
    }
}

/// Puts `rec` in the files table: its body becomes the path's newest, or a
/// removal drops the path.
fn keep_file(sql: &SqlStorage, rec: &Rec) -> Result<()> {
    match latest::change(rec) {
        Some(Change::Keep {
            project,
            kind,
            body,
        }) => sql.exec(
            "INSERT INTO files(path, project, kind, seq, body) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(path) DO UPDATE SET project = excluded.project, kind = excluded.kind, \
             seq = excluded.seq, body = excluded.body",
            vec![
                rec.path.as_str().into(),
                project.into(),
                kind.into(),
                rec.seq.into(),
                body.into(),
            ],
        )?,
        Some(Change::Drop) => sql.exec(
            "DELETE FROM files WHERE path = ?",
            vec![rec.path.as_str().into()],
        )?,
        None => return Ok(()),
    };
    Ok(())
}

/// Marks the project.toml rows a store kept before they had a kind of their
/// own, once: while one is marked, this is one indexed lookup.
fn mark_projects(sql: &SqlStorage) -> Result<()> {
    let marked: Vec<Value> = sql
        .exec(
            "SELECT 1 FROM files WHERE kind = ? LIMIT 1",
            vec![latest::PROJECT.into()],
        )?
        .to_array()?;
    if marked.is_empty() {
        sql.exec(
            "UPDATE files SET kind = ? WHERE path = 'projects/' || project || '/project.toml'",
            vec![latest::PROJECT.into()],
        )?;
    }
    Ok(())
}

/// Fills an empty files table from the whole log, once: a store that kept
/// writes before the table existed.
fn fill_files(sql: &SqlStorage) -> Result<()> {
    let kept: Vec<Value> = sql.exec("SELECT 1 FROM files LIMIT 1", None)?.to_array()?;
    if !kept.is_empty() {
        return Ok(());
    }
    let log = SqlLog(sql)
        .after(0)
        .map_err(|fault| Error::RustError(format!("{fault:?}")))?;
    for rec in latest::newest(log) {
        keep_file(sql, &rec)?;
    }
    Ok(())
}

/// Ten random bytes from the runtime's WebCrypto.
fn random() -> Result<[u8; 10]> {
    let crypto: web_sys::Crypto =
        js_sys::Reflect::get(&js_sys::global(), &"crypto".into())?.unchecked_into();
    let mut bytes = [0u8; 10];
    crypto.get_random_values_with_u8_array(&mut bytes)?;
    Ok(bytes)
}

fn names_limit(why: &str) -> bool {
    why.contains("limit") || why.contains("free tier")
}

fn now_ms() -> u64 {
    Date::now().as_millis()
}

/// The UTC day number, the unit the store's limit flag is kept in.
fn utc_day() -> u64 {
    now_ms() / 86_400_000
}

/// A storage error that names a limit is the Free plan's daily cap, which
/// the machine answers by keeping the write; anything else is an error.
fn fault(error: Error) -> Fault {
    let why = error.to_string();
    if names_limit(&why) {
        Fault::Limit
    } else {
        Fault::Broken(why)
    }
}
