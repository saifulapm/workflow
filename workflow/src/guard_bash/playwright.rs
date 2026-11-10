//! Rules for `playwright-cli` calls. `close-all` ends the browser of every
//! named session, other agents' included (a walker's close-all killed the
//! orchestrator's probe browser mid-fix).

use std::path::Path;

use super::Call;
use crate::shell::{argv, subcommand};

const CLOSE_ALL: &str = "workflow guard refused this on purpose: playwright-cli close-all ends every session's browser, other agents' too.\n\
Close only the session you opened: playwright-cli -s=<name> close.";

const FILL_OUTSIDE_TRY: &str = "workflow guard refused this on purpose: a failed fill, type or insertText prints its text in the call log, a secret too.\n\
Wrap it in try { ... } catch (e) { return e.name } and read secrets back as names or lengths only.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    if close_all(call) {
        return Some(CLOSE_ALL);
    }
    fill_outside_try(call).then_some(FILL_OUTSIDE_TRY)
}

fn is_playwright_cli(argv: &[String]) -> bool {
    argv.first()
        .is_some_and(|w| w.rsplit('/').next() == Some("playwright-cli"))
}

/// Whether any `playwright-cli run-code` has a fill, type or insertText call
/// outside a try block. A failed call prints its argument in Playwright's
/// call log, so a secret typed there lands in the transcript.
fn fill_outside_try(call: &Call) -> bool {
    call.commands.iter().any(|command| {
        let argv = argv(&command.words);
        if !is_playwright_cli(argv) || subcommand(argv) != Some("run-code") {
            return false;
        }
        run_code_source(argv, call.cwd).is_some_and(|code| has_call_outside_try(&code))
    })
}

/// The code a run-code call runs: the first non-option word after `run-code`,
/// or the file `--filename` names. None when a file cannot be read.
fn run_code_source(argv: &[String], cwd: &Path) -> Option<String> {
    let mut words = argv.iter().skip_while(|w| *w != "run-code").skip(1);
    let mut code = None;
    let mut file = None;
    while let Some(word) = words.next() {
        if word == "--filename" {
            file = words.next().map(String::as_str);
        } else if let Some(name) = word.strip_prefix("--filename=") {
            file = Some(name);
        } else if !word.starts_with('-') && code.is_none() {
            code = Some(word.clone());
        }
    }
    match file {
        Some(name) => std::fs::read_to_string(cwd.join(name)).ok(),
        None => code,
    }
}

/// Scans for `.fill(`, `.type(` and `.insertText(` while no `try {` block is
/// open. Text in string literals counts as neither a call nor a brace.
fn has_call_outside_try(code: &str) -> bool {
    let chars: Vec<char> = code.chars().collect();
    let mut depth = 0usize;
    let mut tries: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' | '"' | '`' => {
                i += 1;
                while i < chars.len() && chars[i] != c {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            '{' => {
                depth += 1;
                let before: String = chars[..i].iter().collect();
                let before = before.trim_end();
                let is_word = before.ends_with("try")
                    && !before[..before.len() - 3]
                        .ends_with(|p: char| p.is_alphanumeric() || p == '_' || p == '$');
                if is_word {
                    tries.push(depth);
                }
            }
            '}' => {
                if tries.last() == Some(&depth) {
                    tries.pop();
                }
                depth = depth.saturating_sub(1);
            }
            '.' if tries.is_empty() => {
                let rest: String = chars[i + 1..].iter().collect();
                for name in ["fill", "type", "insertText"] {
                    if let Some(after) = rest.strip_prefix(name)
                        && after.trim_start().starts_with('(')
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Whether any command is `playwright-cli close-all`, bare or by path.
fn close_all(call: &Call) -> bool {
    call.commands.iter().any(|command| {
        let argv = argv(&command.words);
        is_playwright_cli(argv) && subcommand(argv) == Some("close-all")
    })
}
