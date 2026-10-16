//! mem's MCP endpoint, as hub serves it: Streamable HTTP, one JSON-RPC
//! message per POST, a JSON reply, no session. The Worker has one tool so
//! far, `handoff`, listed as hub lists it so claude.ai sees the same tool.
//! The store read the tool needs is the Worker's; this file decides what
//! each message asks for and shapes the reply.

use serde_json::{Value, json};

/// The revisions whose `initialize` handshake this endpoint speaks, newest
/// first. A client asking for any other gets the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

/// What the Worker does with one message.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Send this status and JSON body.
    Reply(u16, String),
    /// Read this project's handoff from the store, then answer with
    /// `handoff_reply`.
    Handoff { id: Value, project: String },
}

pub fn handle(body: &[u8]) -> Step {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return Step::Reply(400, error(Value::Null, -32700, "parse error"));
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Step::Reply(400, error(Value::Null, -32600, "invalid request"));
    };
    // A notification has no id and gets no answer.
    let Some(id) = message.get("id").cloned() else {
        return Step::Reply(202, String::new());
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let reply = match method {
        "initialize" => result(id, initialize(&params)),
        "ping" => result(id, json!({})),
        "tools/list" => result(id, tools_list()),
        "tools/call" => return tools_call(id, &params),
        _ => error(id, -32601, "method not found"),
    };
    Step::Reply(200, reply)
}

/// The answer to a `handoff` call, from the store's text for the project.
pub fn handoff_reply(id: Value, project: &str, text: Option<&str>) -> String {
    match text {
        Some(text) => result(id, tool_text(text, false)),
        None => result(id, tool_text(&format!("no handoff for {project}"), true)),
    }
}

fn tools_call(id: Value, params: &Value) -> Step {
    if params.get("name").and_then(Value::as_str) != Some("handoff") {
        return Step::Reply(200, error(id, -32602, "unknown tool"));
    }
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    match project_arg(&arguments) {
        Ok(project) => Step::Handoff {
            id,
            project: project.to_string(),
        },
        Err(why) => Step::Reply(200, result(id, tool_text(why, true))),
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
    json!({"tools": [{
        "name": "handoff",
        "description": "The latest session handoff for a project: what was done and what comes next.",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string", "description": "The project's name, as mem knows it."}},
            "required": ["project"],
        },
    }]})
}

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

fn result(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(body: &str) -> (u16, Value) {
        match handle(body.as_bytes()) {
            Step::Reply(status, body) if body.is_empty() => (status, Value::Null),
            Step::Reply(status, body) => (status, serde_json::from_str(&body).unwrap()),
            step => panic!("expected a reply, got {step:?}"),
        }
    }

    #[test]
    fn initialize_answers_the_version_asked_for_or_the_newest() {
        let (status, body) = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(body["result"]["serverInfo"]["name"], "mem");
        let (_, body) = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
    }

    #[test]
    fn a_notification_gets_202_and_no_body() {
        assert_eq!(
            reply(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            (202, Value::Null)
        );
    }

    #[test]
    fn unreadable_messages_get_400() {
        assert_eq!(reply("{").1["error"]["code"], -32700);
        assert_eq!(reply(r#"{"id":1}"#).1["error"]["code"], -32600);
    }

    #[test]
    fn tools_call_handoff_asks_for_the_project_s_handoff() {
        let step = handle(br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"handoff","arguments":{"project":"walkone"}}}"#);
        assert_eq!(
            step,
            Step::Handoff {
                id: json!(7),
                project: "walkone".to_string()
            }
        );
        let body: Value =
            serde_json::from_str(&handoff_reply(json!(7), "walkone", Some("Next: ship."))).unwrap();
        assert_eq!(
            body,
            json!({"jsonrpc": "2.0", "id": 7, "result": {"content": [{"type": "text", "text": "Next: ship."}], "isError": false}})
        );
    }

    #[test]
    fn tools_call_refuses_a_bad_project_and_an_unknown_tool() {
        let (_, body) = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"handoff","arguments":{"project":"../x"}}}"#,
        );
        assert_eq!(body["result"]["isError"], true);
        let (_, body) = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","arguments":{}}}"#,
        );
        assert_eq!(body["error"]["code"], -32602);
    }
}
