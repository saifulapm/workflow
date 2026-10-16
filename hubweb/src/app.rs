//! The Worker: one entry point for every request, the walk store Durable
//! Object, and the home page built from it.

use serde_json::Value;
use worker::*;

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

/// Every request passes through here, so a check that guards every path
/// (the Access JWT) goes in this one place.
async fn handle(req: Request, env: Env) -> Result<Response> {
    match (req.method(), req.path().as_str()) {
        (Method::Get, "/") => home(&env).await,
        (Method::Put, "/store/records") => put_records(req, &env).await,
        _ => Response::error("Not Found", 404),
    }
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
            _ => Response::error("Not Found", 404),
        }
    }
}
