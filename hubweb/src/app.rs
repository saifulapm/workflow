//! The Worker: one entry point for every request, the walk store Durable
//! Object, and the home page built from it.

use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::{Value, json};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::wasm_bindgen_futures::JsFuture;
use worker::*;

use crate::access::{self, Access, Jwt};
use crate::mcp::{self, Step};
use crate::page;

/// The store's binding, and the one instance every request reaches.
const STORE: &str = "STORE";
const STORE_NAME: &str = "walk";
/// Saiful reads the hub from Dhaka, so the store is created in Asia-Pacific.
const STORE_LOCATION: &str = "apac";

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
    let mut answer = store(env)?.fetch_with_str("https://store/projects").await?;
    let bodies: Vec<String> = answer.json().await?;
    let projects: Vec<Value> = bodies
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .collect();
    let html = page::render_home(&projects);
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
    let token = env.secret("STORE_TOKEN").map(|t| t.to_string()).ok();
    let given = req.headers().get("Authorization")?;
    let allowed = match (token, given) {
        (Some(token), Some(given)) => {
            !token.is_empty() && same(given.as_bytes(), format!("Bearer {token}").as_bytes())
        }
        _ => false,
    };
    if !allowed {
        return Response::error("Unauthorized", 401);
    }
    let body = req.text().await?;
    let mut init = RequestInit::new();
    init.with_method(Method::Put).with_body(Some(body.into()));
    store(env)?
        .fetch_with_request(Request::new_with_init("https://store/records", &init)?)
        .await
}

/// Compares two byte strings in time that depends on their length only, so
/// a wrong token does not tell how much of it matched.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The walk store: records keyed by project, kind and key, in the Durable
/// Object's SQLite.
#[durable_object]
pub struct Store {
    sql: SqlStorage,
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
        Self { sql }
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
                Response::from_json(&bodies)
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
            _ => Response::error("Not Found", 404),
        }
    }
}
