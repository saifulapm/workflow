//! mem's MCP endpoint: Streamable HTTP, one JSON-RPC message per POST, a
//! JSON reply, no session. It runs on its own loopback port and never through
//! `App::handle`: the tunnel's Access token is its one guard, and the hub's
//! pairing and origin checks belong to the hub's pages.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::access::Access;
use crate::http::{Request, Response};
use crate::memcli::MemCli;
use crate::page_control::is_approval;

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

/// The reads, then an orchestrator's writes. Nothing here deletes, prunes,
/// sets a whole plan or roadmap, or sets a roadmap's status: those stay on
/// the machines that hold a checkout.
const TOOLS: &[Tool] = &[
    Tool {
        name: "context",
        description: "The session digest for a project; full adds facts, logs and the plan's next task.",
        schema: context_schema,
        call: context,
    },
    Tool {
        name: "search",
        description: "Full-text search over a project's memory.",
        schema: search_schema,
        call: search,
    },
    Tool {
        name: "show",
        description: "Print memory items by id.",
        schema: show_schema,
        call: show,
    },
    Tool {
        name: "wiki",
        description: "List a project's wiki pages, or print one page or one section of it.",
        schema: wiki_schema,
        call: wiki,
    },
    Tool {
        name: "questions",
        description: "List a project's questions, or only the pending ones.",
        schema: questions_schema,
        call: questions,
    },
    Tool {
        name: "plan",
        description: "Print the current plan, or a milestone's stored plan.",
        schema: plan_schema,
        call: plan,
    },
    Tool {
        name: "roadmap",
        description: "Print the roadmap, or only its status.",
        schema: roadmap_schema,
        call: roadmap,
    },
    Tool {
        name: "log",
        description: "List a project's recent log entries.",
        schema: log_schema,
        call: log,
    },
    Tool {
        name: "handoff",
        description: "The latest session handoff for a project: what was done and what comes next.",
        schema: project_schema,
        call: handoff,
    },
    Tool {
        name: "ask",
        description: "Writes a question for a person or the orchestrator to answer; it never waits.",
        schema: ask_schema,
        call: ask,
    },
    Tool {
        name: "answer",
        description: "Writes an answer to a question.",
        schema: answer_schema,
        call: answer,
    },
    Tool {
        name: "set_handoff",
        description: "Writes a new session handoff for a project.",
        schema: text_schema,
        call: set_handoff,
    },
    Tool {
        name: "add_log",
        description: "Writes one log entry.",
        schema: text_schema,
        call: add_log,
    },
    Tool {
        name: "decide",
        description: "Writes a decision and who made it.",
        schema: decide_schema,
        call: decide,
    },
    Tool {
        name: "tick_task",
        description: "Writes a check against one task of the current plan.",
        schema: tick_task_schema,
        call: tick_task,
    },
    Tool {
        name: "tick_milestone",
        description: "Writes a check against one milestone of the roadmap.",
        schema: tick_milestone_schema,
        call: tick_milestone,
    },
    Tool {
        name: "write_wiki",
        description: "Writes a wiki page, or one #section of it, in place of what was there, with a note of what changed.",
        schema: write_wiki_schema,
        call: write_wiki,
    },
    Tool {
        name: "add_evidence",
        description: "Writes text as a file of evidence for a task, with a note.",
        schema: evidence_schema,
        call: add_evidence,
    },
    Tool {
        name: "add_finding",
        description: "Writes a finding against a milestone's step; it starts open.",
        schema: finding_schema,
        call: add_finding,
    },
    Tool {
        name: "close_finding",
        description: "Writes a finding as fixed, by a commit or a reason.",
        schema: close_finding_schema,
        call: close_finding,
    },
];

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

/// An input schema: the project, then the tool's own properties.
fn schema(properties: Value, required: &[&str]) -> Value {
    let mut all = json!({"project": {"type": "string", "description": "The project's name, as mem knows it."}});
    if let (Some(all), Value::Object(own)) = (all.as_object_mut(), properties) {
        all.extend(own);
    }
    let mut names = vec!["project"];
    names.extend(required);
    json!({"type": "object", "properties": all, "required": names})
}

fn project_schema() -> Value {
    schema(json!({}), &[])
}

fn context_schema() -> Value {
    schema(
        json!({"full": {"type": "boolean", "description": "The whole digest."}}),
        &[],
    )
}

fn search_schema() -> Value {
    schema(
        json!({
            "query": {"type": "string", "description": "FTS5 query; title:, body: and tags: filter."},
            "limit": {"type": "integer", "description": "At most this many hits."},
        }),
        &["query"],
    )
}

fn show_schema() -> Value {
    schema(
        json!({"ids": {"type": "array", "items": {"type": "string"}, "description": "Full ids or their 8-character suffixes."}}),
        &["ids"],
    )
}

fn wiki_schema() -> Value {
    schema(
        json!({"slug": {"type": "string", "description": "A page slug, with #section for one section. Left out, the list of pages."}}),
        &[],
    )
}

fn questions_schema() -> Value {
    schema(
        json!({"pending": {"type": "boolean", "description": "Only the unanswered ones."}}),
        &[],
    )
}

fn plan_schema() -> Value {
    schema(
        json!({"slug": {"type": "string", "description": "A milestone's slug. Left out, the current plan."}}),
        &[],
    )
}

fn roadmap_schema() -> Value {
    schema(
        json!({"status": {"type": "boolean", "description": "Print only the roadmap's status."}}),
        &[],
    )
}

fn log_schema() -> Value {
    schema(
        json!({"limit": {"type": "integer", "description": "At most this many entries."}}),
        &[],
    )
}

fn ask_schema() -> Value {
    schema(
        json!({
            "question": {"type": "string"},
            "options": {"type": "string", "description": "The choices, comma-separated."},
            "recommend": {"type": "string", "description": "The answer the asker would pick."},
            "for": {"type": "string", "enum": AUDIENCES, "description": "Who answers. A person unless this says otherwise."},
        }),
        &["question"],
    )
}

fn answer_schema() -> Value {
    schema(
        json!({
            "id": {"type": "string", "description": "The question's id."},
            "text": {"type": "string"},
            "option": {"type": "string", "description": "One of the question's options."},
        }),
        &["id"],
    )
}

fn text_schema() -> Value {
    schema(json!({"text": {"type": "string"}}), &["text"])
}

fn decide_schema() -> Value {
    schema(
        json!({
            "text": {"type": "string"},
            "by": {"type": "string", "enum": DECIDERS},
        }),
        &["text", "by"],
    )
}

fn tick_task_schema() -> Value {
    schema(
        json!({"task": {"type": "string", "description": "The task's plan id, such as t3."}}),
        &["task"],
    )
}

fn tick_milestone_schema() -> Value {
    schema(
        json!({"milestone": {"type": "string", "description": "The milestone's slug."}}),
        &["milestone"],
    )
}

fn write_wiki_schema() -> Value {
    schema(
        json!({
            "slug": {"type": "string", "description": "The page slug, with #section for one section."},
            "note": {"type": "string", "description": "What changed and why."},
            "page": {"type": "string", "description": "The page's markdown."},
        }),
        &["slug", "note", "page"],
    )
}

fn evidence_schema() -> Value {
    schema(
        json!({
            "task": {"type": "string", "description": "The task's plan id."},
            "note": {"type": "string"},
            "content": {"type": "string", "description": "The evidence itself, as text."},
        }),
        &["task", "note", "content"],
    )
}

fn finding_schema() -> Value {
    schema(
        json!({
            "milestone": {"type": "string", "description": "The milestone's slug."},
            "step": {"type": "string"},
            "text": {"type": "string"},
        }),
        &["milestone", "step", "text"],
    )
}

fn close_finding_schema() -> Value {
    schema(
        json!({
            "id": {"type": "string", "description": "The finding's id."},
            "by": {"type": "string", "description": "The commit that fixed it, or the reason."},
        }),
        &["id", "by"],
    )
}

const AUDIENCES: [&str; 3] = ["orchestrator", "human", "saiful"];
const DECIDERS: [&str; 2] = ["agent", "saiful"];

/// The argv after `--project=<p>`, or why the arguments were refused.
type Argv = Result<Vec<String>, String>;

/// Runs `mem --project=<p> <argv>` and hands back what mem printed. Every
/// argument is checked before mem runs, so refused input runs nothing. A
/// write drops hub's cached reads, so its pages show the write at once.
fn run(mem: &MemCli, arguments: &Value, write: bool, argv: impl FnOnce(&Value) -> Argv) -> Value {
    run_fed(mem, arguments, write, None, argv)
}

/// `run`, with `input` on mem's stdin: always a write, such as a wiki page.
fn run_fed(
    mem: &MemCli,
    arguments: &Value,
    write: bool,
    input: Option<&str>,
    argv: impl FnOnce(&Value) -> Argv,
) -> Value {
    let project = match project_arg(arguments) {
        Ok(project) => project,
        Err(why) => return tool_text(why, true),
    };
    let rest = match argv(arguments) {
        Ok(rest) => rest,
        Err(why) => return tool_text(&why, true),
    };
    let project = format!("--project={project}");
    let mut args = vec![project.as_str()];
    args.extend(rest.iter().map(String::as_str));
    let run = match (input, write) {
        (Some(input), _) => mem.write_through_fed(&args, input.as_bytes()),
        (None, true) => mem.write_through(&args),
        (None, false) => mem.exec(&args),
    };
    if run.ok() {
        tool_text(&String::from_utf8_lossy(&run.stdout), false)
    } else {
        tool_text(&run.stderr, true)
    }
}

fn context(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let mut argv = vec!["context".to_string()];
        if flag(a, "full")? {
            argv.push("--full".into());
        }
        Ok(argv)
    })
}

fn search(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let query = required(a, "query")?;
        let mut argv = vec!["search".to_string()];
        argv.extend(limit(a)?);
        argv.extend(["--".into(), query.into()]);
        Ok(argv)
    })
}

fn show(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let ids = a
            .get("ids")
            .and_then(Value::as_array)
            .filter(|ids| !ids.is_empty())
            .ok_or("ids is required")?;
        let mut argv = vec!["show".to_string(), "--".into()];
        for id in ids {
            let id = id.as_str().ok_or("each id must be a string")?;
            argv.push(checked(id, "id", ID)?.into());
        }
        Ok(argv)
    })
}

fn wiki(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let mut argv = vec!["wiki".to_string()];
        if let Some(slug) = optional(a, "slug")? {
            argv.extend(["--".into(), checked(slug, "slug", WIKI_SLUG)?.into()]);
        }
        Ok(argv)
    })
}

fn questions(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let mut argv = vec!["questions".to_string()];
        if flag(a, "pending")? {
            argv.push("--pending".into());
        }
        Ok(argv)
    })
}

fn plan(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let mut argv = vec!["plan".to_string()];
        if let Some(slug) = optional(a, "slug")? {
            argv.extend(["--".into(), checked(slug, "slug", SLUG)?.into()]);
        }
        Ok(argv)
    })
}

/// `--status` goes last and bare: given a value it sets the status, which is
/// approving the roadmap, so nothing ever follows it.
fn roadmap(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let mut argv = vec!["roadmap".to_string()];
        if flag(a, "status")? {
            argv.push("--status".into());
        }
        Ok(argv)
    })
}

fn log(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |a| {
        let mut argv = vec!["log".to_string()];
        argv.extend(limit(a)?);
        Ok(argv)
    })
}

/// `mem handoff --project=<project>`, its text as mem prints it.
fn handoff(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, false, |_| Ok(vec!["handoff".into()]))
}

fn ask(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let question = required(a, "question")?;
        let mut argv = vec!["ask".to_string()];
        argv.extend(valued(a, "options")?);
        argv.extend(valued(a, "recommend")?);
        if let Some(audience) = optional(a, "for")? {
            argv.push(format!("--for={}", one_of(audience, "for", &AUDIENCES)?));
        }
        argv.extend(["--".into(), question.into()]);
        Ok(argv)
    })
}

/// Signed `connector`, whatever the caller says: an answer with no author
/// reads as Saiful's own tap, and one the model wrote is not.
fn answer(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let id = checked(required(a, "id")?, "id", ID)?;
        not_a_review(mem, project_arg(a)?, id)?;
        let mut argv = vec!["answer".to_string()];
        argv.extend(valued(a, "option")?);
        argv.extend(["--by=connector".into(), "--".into(), id.into()]);
        argv.extend(optional(a, "text")?.map(String::from));
        Ok(argv)
    })
}

/// Answering a roadmap's review approves it, which only Saiful does, on the
/// hub. The question has to be one of this project's, so naming another
/// project cannot step around the check, and a question mem cannot list is
/// refused rather than answered blind.
fn not_a_review(mem: &MemCli, project: &str, id: &str) -> Result<(), String> {
    let run = mem.exec(&[&format!("--project={project}"), "questions", "--json"]);
    if !run.ok() {
        return Err(format!("could not read the question: {}", run.stderr));
    }
    let listed: Value = serde_json::from_slice(&run.stdout)
        .map_err(|e| format!("could not read the question: {e}"))?;
    let question = listed["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["id"] == id || row["short_id"] == id)
        .ok_or_else(|| format!("{id} is not a question of {project}"))?;
    let body = question["body"].as_str().unwrap_or("").trim();
    let offers_approval = question["options"]
        .as_array()
        .is_some_and(|options| options.iter().any(|o| o == "approve"));
    if is_approval(body) || (offers_approval && body.to_lowercase().contains("roadmap")) {
        return Err("a roadmap review is Saiful's to answer, on the hub".into());
    }
    Ok(())
}

fn set_handoff(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let text = required(a, "text")?;
        Ok(vec!["handoff".into(), format!("--set={text}")])
    })
}

fn add_log(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let text = required(a, "text")?;
        Ok(vec!["log".into(), "--".into(), text.into()])
    })
}

fn decide(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let text = required(a, "text")?;
        let by = one_of(required(a, "by")?, "by", &DECIDERS)?;
        Ok(vec![
            "decide".into(),
            format!("--by={by}"),
            "--".into(),
            text.into(),
        ])
    })
}

fn tick_task(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let task = checked(required(a, "task")?, "task", SLUG)?;
        Ok(vec!["plan".into(), format!("--tick={task}")])
    })
}

fn tick_milestone(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let milestone = checked(required(a, "milestone")?, "milestone", SLUG)?;
        Ok(vec!["roadmap".into(), format!("--tick={milestone}")])
    })
}

/// The page goes on stdin, the one way mem takes a wiki page.
fn write_wiki(mem: &MemCli, arguments: &Value) -> Value {
    let page = match required(arguments, "page") {
        Ok(page) => page,
        Err(why) => return tool_text(&why, true),
    };
    run_fed(mem, arguments, true, Some(page), |a| {
        let slug = checked(required(a, "slug")?, "slug", WIKI_SLUG)?;
        let note = required(a, "note")?;
        Ok(vec![
            "wiki".into(),
            "--stdin".into(),
            format!("--note={note}"),
            "--".into(),
            slug.into(),
        ])
    })
}

/// mem files evidence from a file and keeps its base name, so the text goes
/// to a file named for this moment in a directory of its own, removed after.
fn add_evidence(mem: &MemCli, arguments: &Value) -> Value {
    let stamp = jiff::Timestamp::now().as_nanosecond();
    let dir = std::env::temp_dir().join(format!("hub-evidence-{}-{stamp}", std::process::id()));
    let file = dir.join(format!("connector-{stamp}.txt"));
    let mut made = false;
    let result = run(mem, arguments, true, |a| {
        let task = checked(required(a, "task")?, "task", SLUG)?;
        let note = required(a, "note")?;
        let content = required(a, "content")?;
        let path = file.to_str().ok_or("the temp directory is not UTF-8")?;
        std::fs::create_dir(&dir).map_err(|e| format!("could not stage the evidence: {e}"))?;
        made = true;
        std::fs::write(&file, content).map_err(|e| format!("could not stage the evidence: {e}"))?;
        Ok(vec![
            "evidence".into(),
            "add".into(),
            format!("--task={task}"),
            format!("--note={note}"),
            "--".into(),
            path.into(),
        ])
    });
    if made {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn add_finding(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let milestone = checked(required(a, "milestone")?, "milestone", SLUG)?;
        let step = required(a, "step")?;
        let text = required(a, "text")?;
        Ok(vec![
            "finding".into(),
            "add".into(),
            format!("--milestone={milestone}"),
            format!("--step={step}"),
            "--".into(),
            text.into(),
        ])
    })
}

fn close_finding(mem: &MemCli, arguments: &Value) -> Value {
    run(mem, arguments, true, |a| {
        let id = checked(required(a, "id")?, "id", ID)?;
        let by = required(a, "by")?;
        Ok(vec![
            "finding".into(),
            "close".into(),
            format!("--by={by}"),
            "--".into(),
            id.into(),
        ])
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

/// An optional text as one `--key=value` element, so the value can never
/// stand on its own as a flag or a positional.
fn valued(arguments: &Value, key: &str) -> Result<Option<String>, String> {
    Ok(optional(arguments, key)?.map(|value| format!("--{key}={value}")))
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
            .map(|n| Some(format!("--limit={n}")))
            .ok_or_else(|| "limit must be a whole number".into()),
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
