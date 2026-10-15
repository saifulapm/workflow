//! mem's remote mode, the server half: `POST /cli` runs a client's argv on
//! this machine's store, and `GET /dl/mem` hands a cloud VM the mem binary.
//! Both sit on the MCP listener, behind its Access check.
//!
//! A verb runs only when it is on the allowlist below: the reads and the
//! writes an agent makes. Anything that deletes, prunes, syncs, repairs or
//! answers stays on the machines that hold the store.

use std::path::Path;

use serde_json::{Value, json};

use crate::http::Response;
use crate::memcli::MemCli;

/// mem's usage code, for a refusal and for a project the server cannot find.
const USAGE: i64 = 2;
/// mem's store code, for a mem that did not run or did not finish.
const STORE: i64 = 3;

const VERBS: &[&str] = &[
    "context",
    "search",
    "show",
    "projects",
    "save",
    "log",
    "handoff",
    "ask",
    "questions",
    "decide",
    "evidence",
    "finding",
    "brief",
    "idea",
    "wiki",
    "plan",
    "roadmap",
    "project",
];

/// The verbs that only read; every other verb runs as a write, which drops
/// the hub's cached reads.
const READS: &[&str] = &["context", "search", "show", "projects", "questions"];

/// The roadmap flags that replace, clear or approve it. A remote roadmap is
/// read or ticked, nothing more.
const ROADMAP_WRITES: &[&str] = &["--set", "--stdin", "--clear", "--status"];

/// Runs the argv in `body` through mem and replies `{code, stdout, stderr}`.
pub fn cli(mem: &MemCli, body: &[u8]) -> Response {
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return Response::new(400, "application/json", r#"{"error":"not json"}"#);
    };
    let argv: Option<Vec<String>> = request
        .get("argv")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect());
    let Some(argv) = argv else {
        return Response::new(400, "application/json", r#"{"error":"argv is not a list"}"#);
    };
    let stdin = request.get("stdin").and_then(Value::as_str);
    let remote = request.get("remote").and_then(Value::as_str);

    let words = positionals(&argv);
    if let Some(why) = refusal(&argv, &words) {
        return reply(USAGE, "", &format!("mem: {why}\n"));
    }
    let mut args: Vec<&str> = Vec::with_capacity(argv.len() + 1);
    let project;
    if !names_project(&argv) {
        let Some(name) = remote.and_then(|r| project_for(mem, r)) else {
            return reply(
                USAGE,
                "",
                &format!(
                    "mem: no project for {}\n",
                    remote.unwrap_or("this checkout")
                ),
            );
        };
        project = format!("--project={name}");
        args.push(&project);
    }
    args.extend(argv.iter().map(String::as_str));

    let run = match stdin {
        Some(input) => mem.write_through_fed(&args, input.as_bytes()),
        None if words.first().is_some_and(|v| READS.contains(v)) => mem.exec(&args),
        None => mem.write_through(&args),
    };
    // MemCli trims stderr; the client prints it as given, so the line ends.
    let stderr = if run.stderr.is_empty() {
        String::new()
    } else {
        format!("{}\n", run.stderr)
    };
    reply(
        run.code.map_or(STORE, i64::from),
        &String::from_utf8_lossy(&run.stdout),
        &stderr,
    )
}

/// The file `mem_binary` names, or a 404 when it names none or nothing is
/// there.
pub fn download(path: Option<&Path>) -> Response {
    match path.map(std::fs::read) {
        Some(Ok(bytes)) => Response::new(200, "application/octet-stream", bytes),
        _ => Response::not_found(),
    }
}

fn reply(code: i64, stdout: &str, stderr: &str) -> Response {
    Response::json(json!({"code": code, "stdout": stdout, "stderr": stderr}).to_string())
}

/// The flags before `--`, the part of the argv mem reads as options.
fn options(argv: &[String]) -> impl Iterator<Item = &String> {
    argv.iter().take_while(|a| *a != "--")
}

fn names_project(argv: &[String]) -> bool {
    options(argv).any(|a| a == "--project" || a.starts_with("--project="))
}

/// The words that are not flags: the verb first, then its subcommands. The
/// two global flags that take a value take the word after them with them.
fn positionals(argv: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => {
                words.extend(args.map(String::as_str));
                break;
            }
            "--project" | "--scope" => {
                args.next();
            }
            flag if flag.starts_with('-') => {}
            word => words.push(word),
        }
    }
    words
}

/// Why this argv does not run here, or `None` when it may.
fn refusal(argv: &[String], words: &[&str]) -> Option<String> {
    let Some(&verb) = words.first() else {
        return Some("no command given".to_string());
    };
    let refused = || Some(format!("{verb} is refused over the remote"));
    if !VERBS.contains(&verb) {
        return refused();
    }
    let flag = |names: &[&str]| options(argv).any(|a| names.iter().any(|n| a.starts_with(n)));
    match (verb, words.get(1).copied(), words.get(2).copied()) {
        ("project", Some("current"), _) | ("project", Some("set"), Some("runner")) => {}
        ("project", sub, _) => {
            return Some(format!(
                "project {} is refused over the remote",
                sub.unwrap_or("")
            ));
        }
        ("roadmap", _, _) if flag(ROADMAP_WRITES) => {
            return Some("roadmap is refused over the remote, except to read or --tick".into());
        }
        _ => {}
    }
    // A path names a file on this machine, not the client's: refused, so a
    // client can neither read the server's files nor file the wrong one.
    if flag(&["--set-file", "--evidence"]) || (verb == "evidence" && words.get(1) == Some(&"add")) {
        return Some(format!(
            "{verb} with a file is refused over the remote; send the text on stdin"
        ));
    }
    None
}

/// The project whose remote ends in the same owner/repo as `remote`.
fn project_for(mem: &MemCli, remote: &str) -> Option<String> {
    let wanted = owner_repo(remote)?;
    mem.projects().rows("projects").iter().find_map(|row| {
        let theirs = row.get("remote").and_then(Value::as_str)?;
        (owner_repo(theirs)? == wanted)
            .then(|| row.get("name").and_then(Value::as_str).map(str::to_string))
            .flatten()
    })
}

/// The last two segments of a remote URL, lowercased, without `.git`:
/// `git@github.com:o/r.git`, `https://github.com/o/r` and a proxy's
/// `http://proxy@127.0.0.1:1234/git/o/r` are all `o/r`.
fn owner_repo(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let mut parts = url.rsplit(['/', ':']).filter(|p| !p.is_empty());
    let repo = parts.next()?;
    let owner = parts.next()?;
    Some(format!("{owner}/{repo}").to_ascii_lowercase())
}
