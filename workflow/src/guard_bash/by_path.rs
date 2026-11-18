//! The by-path form of a pnpm command: what it runs, its tool called as
//! node_modules/.bin/<tool>. 22 refused lareys-trendsi implementers had pnpm
//! in their VERIFY and were told only to call the tool by path, not which
//! tool.

use std::path::Path;

use serde_json::Value;

use crate::shell::{self, Simple, program_name};

/// pnpm's own commands that install, publish or fetch: a package script of
/// the same name (an `install` lifecycle script) never runs by them.
const BUILTINS: &[&str] = &[
    "install", "i", "add", "remove", "rm", "update", "up", "link", "rebuild", "prune", "dedupe",
    "deploy", "publish", "pack", "dlx", "create",
];

/// The by-path lines of every pnpm command in `commands`, run in `cwd`, or
/// None when one of them cannot be put that way.
pub fn lines(commands: &[Simple], cwd: &Path) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for command in commands {
        if command
            .words
            .iter()
            .any(|w| matches!(program_name(w), "pnpm" | "pnpx"))
        {
            out.extend(translate(&command.words, cwd)?);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The lines for `words`, a pnpm command run in `here`.
fn translate(words: &[String], here: &Path) -> Option<Vec<String>> {
    let p = words.iter().position(|w| program_name(w) == "pnpm")?;
    let (sub, rest) = words[p + 1..].split_first()?;
    let tool: Vec<String> = match sub.as_str() {
        "exec" => rest.to_vec(),
        "run" | "run-script" => {
            let (name, args) = rest.split_first()?;
            with_args(script(here, name)?, args)?
        }
        name if BUILTINS.contains(&name) || name.starts_with('-') => return None,
        name => with_args(script(here, name)?, rest)?,
    };
    Some(vec![join(&by_path(tool))])
}

/// The words of `text`, a script, with `args` after them.
fn with_args(text: String, args: &[String]) -> Option<Vec<String>> {
    let mut commands = shell::commands(&text)?;
    if commands.len() != 1 {
        return None;
    }
    let mut words = commands.remove(0).words;
    words.extend(args.iter().cloned());
    Some(words)
}

/// The script `name` of the package.json in `dir`.
fn script(dir: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let package: Value = serde_json::from_str(&text).ok()?;
    package["scripts"][name].as_str().map(String::from)
}

/// `words` with node_modules/.bin/ in front of the program.
fn by_path(mut words: Vec<String>) -> Vec<String> {
    if let Some(program) = words.first_mut() {
        *program = format!("node_modules/.bin/{program}");
    }
    words
}

/// `words` as one shell line, each quoted when the shell would split it.
fn join(words: &[String]) -> String {
    words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
}

fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,@%+".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}
