//! mem's MCP endpoint, as hub serves it: Streamable HTTP, one JSON-RPC
//! message per POST, a JSON reply, no session. It lists hub's tools and
//! answers each from the store's files: a read goes through memview, and a
//! write comes back as the store paths to write, since only the Worker can
//! write the store. Nothing here runs mem.

mod reads;
mod tools;
mod writes;

use serde_json::{Value, json};

use crate::memview;

/// The revisions whose `initialize` handshake this endpoint speaks, newest
/// first. A client asking for any other gets the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

/// The machine every item the connector writes names.
pub const MACHINE: &str = "connector";

/// What a call reads, when it runs, and the randomness each new item id takes.
pub struct Store<'a, F: memview::Files> {
    pub files: &'a F,
    pub now: jiff::Timestamp,
    pub random: &'a mut dyn FnMut() -> [u8; 10],
}

/// One message's answer.
pub struct Served {
    pub status: u16,
    pub body: String,
    /// Store paths to write, in order; None removes the path.
    pub writes: Vec<(String, Option<String>)>,
    /// Evidence bytes, by store path, for the files store.
    pub files: Vec<(String, Vec<u8>)>,
}

/// One JSON-RPC message in; the reply, the store paths to write (body None =
/// remove) in order, and any evidence bytes to store.
pub fn serve<F: memview::Files>(body: &[u8], store: Store<F>) -> Served {
    let mut call = Call {
        store,
        writes: Vec::new(),
        files: Vec::new(),
    };
    let (status, body) = call.message(body);
    Served {
        status,
        body,
        writes: call.writes,
        files: call.files,
    }
}

/// One message being served, and the writes it has made so far.
struct Call<'a, F: memview::Files> {
    store: Store<'a, F>,
    writes: Vec<(String, Option<String>)>,
    files: Vec<(String, Vec<u8>)>,
}

/// A tool's text, or the refusal it ends on.
type Text = Result<String, String>;

/// A project as the store names it: its directory id and its name.
struct Project {
    id: String,
    name: String,
}

impl<F: memview::Files> Call<'_, F> {
    fn message(&mut self, body: &[u8]) -> (u16, String) {
        let Ok(message) = serde_json::from_slice::<Value>(body) else {
            return (400, error(Value::Null, -32700, "parse error"));
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return (400, error(Value::Null, -32600, "invalid request"));
        };
        // A notification has no id and gets no answer.
        let Some(id) = message.get("id").cloned() else {
            return (202, String::new());
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let reply = match method {
            "initialize" => result(id, initialize(&params)),
            "ping" => result(id, json!({})),
            "tools/list" => result(id, tools::tools_list()),
            "tools/call" => self.tools_call(id, &params),
            _ => error(id, -32601, "method not found"),
        };
        (200, reply)
    }

    fn tools_call(&mut self, id: Value, params: &Value) -> String {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        if !tools::TOOLS.iter().any(|tool| tool.name == name) {
            return error(id, -32602, "unknown tool");
        }
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        let text = self.project(&arguments).and_then(|project| {
            let a = &arguments;
            let p = &project;
            match name {
                "context" => self.context(p, a),
                "search" => self.search(p, a),
                "show" => self.show(a),
                "wiki" => self.wiki(p, a),
                "questions" => self.questions(p, a),
                "plan" => self.plan(p, a),
                "roadmap" => self.roadmap(p, a),
                "log" => self.log(p, a),
                "handoff" => self.handoff(p),
                "ask" => self.ask(p, a),
                "answer" => self.answer(p, a),
                "set_handoff" => self.save(p, "handoff", required(a, "text")?, vec![]),
                "add_log" => self.save(p, "log", required(a, "text")?, vec![]),
                "decide" => self.decide(p, a),
                "tick_task" => self.tick(p, a, "task", "plan"),
                "tick_milestone" => self.tick(p, a, "milestone", "roadmap"),
                "write_wiki" => self.write_wiki(p, a),
                "add_evidence" => self.add_evidence(p, a),
                "add_finding" => self.add_finding(p, a),
                _ => self.close_finding(a),
            }
        });
        match text {
            Ok(text) => result(id, tool_text(&text, false)),
            Err(why) => result(id, tool_text(&why, true)),
        }
    }

    /// The project the arguments name: checked to be only a name before the
    /// store is read, then found as mem finds it, by name or alias.
    fn project(&self, arguments: &Value) -> Result<Project, String> {
        let name = project_arg(arguments)?;
        let (id, name) = memview::project(self.store.files, name).ok_or_else(|| {
            format!("mem: no project named '{name}' \u{2014} `mem projects` lists them")
        })?;
        Ok(Project { id, name })
    }

    /// What `mem <argv> --json` prints, read from the store; None where mem
    /// prints nothing.
    fn view(&self, argv: &[&str]) -> Result<Option<Value>, String> {
        // First, since everything after a `--` is a word.
        let mut argv = argv.to_vec();
        argv.insert(0, "--json");
        match memview::read(self.store.files, &argv, self.store.now) {
            memview::Reply::Json(doc) => Ok(Some(doc)),
            memview::Reply::Absent => Ok(None),
            memview::Reply::Unknown => Err(format!("cannot read {argv:?} from the store")),
        }
    }
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS
        .into_iter()
        .find(|v| Some(*v) == asked)
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "mem", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// The punctuation each kind of name may carry after its first character,
/// which is always a letter or a digit, so no name can start like a flag.
const ID: &[u8] = b"";
const SLUG: &[u8] = b"._-";
const WIKI_SLUG: &[u8] = b"._#-";

fn checked<'a>(value: &'a str, key: &str, extra: &[u8]) -> Result<&'a str, String> {
    let valid = value
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b));
    if valid {
        Ok(value)
    } else {
        Err(format!("{key} {value:?} is not a valid name"))
    }
}

fn one_of<'a>(value: &'a str, key: &str, allowed: &[&str]) -> Result<&'a str, String> {
    if allowed.contains(&value) {
        Ok(value)
    } else {
        Err(format!("{key} must be one of {}", allowed.join(", ")))
    }
}

fn optional<'a>(arguments: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn required<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
    optional(arguments, key)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn flag(arguments: &Value, key: &str) -> Result<bool, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(on)) => Ok(*on),
        Some(_) => Err(format!("{key} must be true or false")),
    }
}

fn limit(arguments: &Value) -> Result<Option<String>, String> {
    match arguments.get("limit") {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n
            .as_u64()
            .map(|n| Some(n.to_string()))
            .ok_or_else(|| "limit must be a whole number".into()),
    }
}

/// A project name is checked before anything is read, so it can only ever
/// be a name: no flag, no path, no space.
fn project_arg(arguments: &Value) -> Result<&str, String> {
    let project = arguments
        .get("project")
        .and_then(Value::as_str)
        .ok_or("project is required")?;
    let valid = !project.is_empty()
        && project
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if valid {
        Ok(project)
    } else {
        Err("project must be letters, digits, '.', '_' or '-'".into())
    }
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn result(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

#[cfg(test)]
mod tests;
