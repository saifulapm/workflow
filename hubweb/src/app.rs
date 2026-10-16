//! The Worker: one entry point for every request, the walk store Durable
//! Object, and the home page built from it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use serde_json::{Value, json};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::wasm_bindgen_futures::JsFuture;
use worker::*;

use crate::access::{self, Access, Jwt};
use crate::files;
use crate::latest::{self, Change};
use crate::mcp::{self, Step};
use crate::page;
use crate::share::{self, Fault, Log, Rec};

/// The store's binding, and the one instance every request reaches.
const STORE: &str = "STORE";
const STORE_NAME: &str = "walk";
/// Saiful reads the hub from Dhaka, so the store is created in Asia-Pacific.
const STORE_LOCATION: &str = "apac";
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
    if let Err(why) = guard(&req, &env).await {
        console_log!("hubweb: refused {:?} {path}: {why}", req.method());
        return Response::error("Unauthorized", 401);
    }
    match (req.method(), path.as_str()) {
        (Method::Get, "/") => home(&env).await,
        (Method::Post, "/" | "/mcp") => serve_mcp(req, &env).await,
        (_, "/mcp") => Response::error("Method Not Allowed", 405),
        _ => Response::error("Not Found", 404),
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

/// One JSON-RPC message in, its JSON reply out; a `handoff` call reads the
/// project's handoff record from the store.
async fn serve_mcp(mut req: Request, env: &Env) -> Result<Response> {
    let body = req.bytes().await?;
    let (status, reply) = match mcp::handle(&body) {
        Step::Reply(status, reply) => (status, reply),
        Step::Handoff { id, project } => {
            let mut answer = store(env)?
                .fetch_with_str(&format!("https://store/handoff/{project}"))
                .await?;
            let text = match answer.status_code() {
                200 => Some(answer.text().await?),
                _ => None,
            };
            (200, mcp::handoff_reply(id, &project, text.as_deref()))
        }
    };
    let mut response = Response::ok(reply)?.with_status(status);
    response
        .headers_mut()
        .set("Content-Type", "application/json")?;
    Ok(response)
}

fn store(env: &Env) -> Result<Stub> {
    env.durable_object(STORE)?
        .get_by_name_with_location_hint(STORE_NAME, STORE_LOCATION)
}

/// The home, with the time the Worker took to build it (store read
/// included) in Server-Timing.
async fn home(env: &Env) -> Result<Response> {
    let start = Date::now().as_millis();
    let (projects, limited) = match store(env)?.fetch_with_str("https://store/projects").await {
        Ok(mut answer) => {
            let flag = answer.headers().get("X-Store-Limit")?;
            let bodies: Vec<String> = answer.json().await?;
            let projects: Vec<Value> = bodies
                .iter()
                .filter_map(|body| serde_json::from_str(body).ok())
                .collect();
            (projects, page::limit_is_today(flag.as_deref(), now_ms()))
        }
        // The read itself hit the limit: the home says so, with no cards.
        Err(error) if names_limit(&error.to_string()) => (Vec::new(), true),
        Err(error) => return Err(error),
    };
    let html = page::render_home(&projects, limited);
    let took = Date::now().as_millis() - start;
    let mut response = Response::from_html(html)?;
    response
        .headers_mut()
        .set("Server-Timing", &format!("build;dur={took}"))?;
    Ok(response)
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
        for socket in self.state.get_websockets() {
            attachments.push(attachment(&socket)?);
        }
        Ok(share::newest_limit_day(&attachments).max(self.limit_day.get()))
    }
}

/// A socket's attachment, Null when it has none.
fn attachment(socket: &WebSocket) -> Result<Value> {
    Ok(socket.deserialize_attachment()?.unwrap_or(Value::Null))
}

impl DurableObject for Store {
    fn new(state: State, _env: Env) -> Self {
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
        Self {
            state,
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
            (Method::Get, "/projects") => {
                let rows: Vec<Value> = self
                    .sql
                    .exec(
                        "SELECT body FROM records WHERE kind = 'project' ORDER BY key",
                        None,
                    )?
                    .to_array()?;
                let bodies: Vec<&str> = rows.iter().filter_map(|r| r["body"].as_str()).collect();
                let mut response = Response::from_json(&bodies)?;
                if let Some(day) = self.limit_day()? {
                    response
                        .headers_mut()
                        .set("X-Store-Limit", &day.to_string())?;
                }
                Ok(response)
            }
            (Method::Get, path) if path.starts_with("/handoff/") => {
                let project = &path["/handoff/".len()..];
                let rows: Vec<Value> = self
                    .sql
                    .exec(
                        "SELECT body FROM records WHERE project = ? AND kind = 'handoff' \
                         AND key = ?",
                        vec![project.into(), project.into()],
                    )?
                    .to_array()?;
                match rows.first().and_then(|row| row["body"].as_str()) {
                    Some(text) => Response::ok(text),
                    None => Response::error("No handoff", 404),
                }
            }
            (Method::Get, "/store/share") => {
                if req.headers().get("Upgrade")?.as_deref() != Some("websocket") {
                    return Response::error("Expected a WebSocket upgrade", 426);
                }
                let pair = WebSocketPair::new()?;
                self.state.accept_web_socket(&pair.server);
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
            for socket in self.state.get_websockets() {
                if let Ok(value) = attachment(&socket) {
                    let (machine, _) = share::attached(&value);
                    let _ = socket
                        .serialize_attachment(share::attachment(machine.as_deref(), Some(day)));
                }
            }
        }
        if let Some(rec) = &outcome.others {
            for other in self.state.get_websockets() {
                if other.as_ref() != ws.as_ref() {
                    // A socket that is closing misses the rec; its machine
                    // gets it from the replay after its next hello.
                    let _ = other.send_with_str(rec);
                }
            }
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
