//! mem's MCP endpoint: Streamable HTTP, one JSON-RPC message per POST, a
//! JSON reply, no session. It runs on its own loopback port and never through
//! `App::handle`: the tunnel's Access token is its one guard, and the hub's
//! pairing and origin checks belong to the hub's pages.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::access::Access;
use crate::http::{Request, Response};
use crate::memcli::MemCli;

/// The revisions whose `initialize` handshake this endpoint speaks, newest
/// first. A client asking for any other gets the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

pub struct Mcp {
    access: Access,
    mem: Arc<MemCli>,
}

/// One tool: its listing, and the call that runs it.
struct Tool {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    call: fn(&MemCli, &Value) -> Value,
}

const TOOLS: &[Tool] = &[Tool {
    name: "handoff",
    description: "The latest session handoff for a project: what was done and what comes next.",
    schema: project_schema,
    call: handoff,
}];

impl Mcp {
    pub fn new(access: Access, mem: Arc<MemCli>) -> Mcp {
        Mcp { access, mem }
    }

    pub fn handle(&self, request: &Request) -> Response {
        let now = jiff::Timestamp::now().as_second();
        if let Err(why) = self
            .access
            .check(request.header("cf-access-jwt-assertion"), now)
        {
            eprintln!("hub: mcp: refused {}: {why}", request.peer);
            return Response::new(401, "application/json", r#"{"error":"unauthorized"}"#);
        }
        if request.path != "/" && request.path != "/mcp" {
            return Response::not_found();
        }
        if request.method != "POST" {
            return Response::method_not_allowed("POST");
        }
        let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
            return bad_request(error(Value::Null, -32700, "parse error"));
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return bad_request(error(Value::Null, -32600, "invalid request"));
        };
        // A notification has no id and gets no answer.
        let Some(id) = message.get("id").cloned() else {
            return Response::new(202, "application/json", "");
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        reply(match method {
            "initialize" => result(id, initialize(&params)),
            "ping" => result(id, json!({})),
            "tools/list" => result(id, tools_list()),
            "tools/call" => self.tools_call(id, &params),
            _ => error(id, -32601, "method not found"),
        })
    }

    fn tools_call(&self, id: Value, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str);
        let Some(tool) = TOOLS.iter().find(|tool| Some(tool.name) == name) else {
            return error(id, -32602, "unknown tool");
        };
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        result(id, (tool.call)(&self.mem, &arguments))
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

fn tools_list() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.schema)(),
            })
        })
        .collect();
    json!({ "tools": tools })
}

fn project_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"project": {"type": "string", "description": "The project's name, as mem knows it."}},
        "required": ["project"],
    })
}

/// `mem handoff --project=<project>`, its text as mem prints it.
fn handoff(mem: &MemCli, arguments: &Value) -> Value {
    let project = match project_arg(arguments) {
        Ok(project) => project,
        Err(why) => return tool_text(why, true),
    };
    let run = mem.exec(&["handoff", &format!("--project={project}")]);
    if run.ok() {
        tool_text(&String::from_utf8_lossy(&run.stdout), false)
    } else {
        tool_text(&run.stderr, true)
    }
}

/// A project name is checked before anything runs, so it can only ever be
/// a name: no flag, no path, no space.
fn project_arg(arguments: &Value) -> Result<&str, &'static str> {
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
        Err("project must be letters, digits, '.', '_' or '-'")
    }
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn reply(message: Value) -> Response {
    Response::json(message.to_string())
}

/// A message hub cannot read at all is the HTTP 400 the transport asks for.
fn bad_request(message: Value) -> Response {
    Response::new(400, "application/json", message.to_string())
}
