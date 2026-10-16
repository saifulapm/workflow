//! The tools the connector lists: hub's table, name for name, so claude.ai
//! sees the same tools whichever end serves it.

use serde_json::{Value, json};

/// One tool's listing.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
}

/// The reads, then an orchestrator's writes. Nothing here deletes, prunes,
/// sets a whole plan or roadmap, or sets a roadmap's status: those stay on
/// the machines that hold a checkout.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "context",
        description: "The session digest for a project; full adds facts, logs and the plan's next task.",
        schema: context_schema,
    },
    Tool {
        name: "search",
        description: "Full-text search over a project's memory.",
        schema: search_schema,
    },
    Tool {
        name: "show",
        description: "Print memory items by id.",
        schema: show_schema,
    },
    Tool {
        name: "wiki",
        description: "List a project's wiki pages, or print one page or one section of it.",
        schema: wiki_schema,
    },
    Tool {
        name: "questions",
        description: "List a project's questions, or only the pending ones.",
        schema: questions_schema,
    },
    Tool {
        name: "plan",
        description: "Print the current plan, or a milestone's stored plan.",
        schema: plan_schema,
    },
    Tool {
        name: "roadmap",
        description: "Print the roadmap, or only its status.",
        schema: roadmap_schema,
    },
    Tool {
        name: "log",
        description: "List a project's recent log entries.",
        schema: log_schema,
    },
    Tool {
        name: "handoff",
        description: "The latest session handoff for a project: what was done and what comes next.",
        schema: project_schema,
    },
    Tool {
        name: "ask",
        description: "Writes a question for a person or the orchestrator to answer; it never waits.",
        schema: ask_schema,
    },
    Tool {
        name: "answer",
        description: "Writes an answer to a question.",
        schema: answer_schema,
    },
    Tool {
        name: "set_handoff",
        description: "Writes a new session handoff for a project.",
        schema: text_schema,
    },
    Tool {
        name: "add_log",
        description: "Writes one log entry.",
        schema: text_schema,
    },
    Tool {
        name: "decide",
        description: "Writes a decision and who made it.",
        schema: decide_schema,
    },
    Tool {
        name: "tick_task",
        description: "Writes a check against one task of the current plan.",
        schema: tick_task_schema,
    },
    Tool {
        name: "tick_milestone",
        description: "Writes a check against one milestone of the roadmap.",
        schema: tick_milestone_schema,
    },
    Tool {
        name: "write_wiki",
        description: "Writes a wiki page, or one #section of it, in place of what was there, with a note of what changed.",
        schema: write_wiki_schema,
    },
    Tool {
        name: "add_evidence",
        description: "Writes text as a file of evidence for a task, with a note.",
        schema: evidence_schema,
    },
    Tool {
        name: "add_finding",
        description: "Writes a finding against a milestone's step; it starts open.",
        schema: finding_schema,
    },
    Tool {
        name: "close_finding",
        description: "Writes a finding as fixed, by a commit or a reason.",
        schema: close_finding_schema,
    },
];

pub const AUDIENCES: [&str; 3] = ["orchestrator", "human", "saiful"];
pub const DECIDERS: [&str; 2] = ["agent", "saiful"];

pub fn tools_list() -> Value {
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
