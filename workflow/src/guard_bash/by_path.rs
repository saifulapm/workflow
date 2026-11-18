//! The by-path form of a pnpm command: what it runs, its tool called as
//! node_modules/.bin/<tool>. 22 refused lareys-trendsi implementers had pnpm
//! in their VERIFY and were told only to call the tool by path, not which
//! tool.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::cd::resolve;
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
    // The folder each subshell depth is in, as the line's `cd`s move it.
    let mut dirs = vec![cwd.to_path_buf()];
    let mut out = Vec::new();
    for command in commands {
        dirs.truncate(command.depth + 1);
        while dirs.len() <= command.depth {
            dirs.push(dirs[dirs.len() - 1].clone());
        }
        let here = dirs[command.depth].clone();
        let argv = shell::argv(&command.words);
        if matches!(argv.first().map(|w| program_name(w)), Some("cd" | "pushd")) {
            if let Some(to) = argv
                .get(1)
                .filter(|t| !t.starts_with('-') && !t.contains('$'))
            {
                dirs[command.depth] = resolve(&here, to);
            }
        } else if command
            .words
            .iter()
            .any(|w| matches!(program_name(w), "pnpm" | "pnpx"))
        {
            out.extend(translate(&command.words, &here, cwd)?);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The lines for `words`, a pnpm command run in `here`, shown from `cwd`.
fn translate(words: &[String], here: &Path, cwd: &Path) -> Option<Vec<String>> {
    let p = words.iter().position(|w| program_name(w) == "pnpm")?;
    let mut dir = here.to_path_buf();
    let mut rest = &words[p + 1..];
    let sub = loop {
        let (word, after) = rest.split_first()?;
        rest = after;
        match word.as_str() {
            "-C" | "--dir" | "-F" | "--filter" => {
                let (value, after) = rest.split_first()?;
                rest = after;
                dir = folder(here, word, value)?;
            }
            "-s" | "--silent" => {}
            _ if word.starts_with("--dir=") || word.starts_with("--filter=") => {
                let (flag, value) = word.split_once('=')?;
                dir = folder(here, flag, value)?;
            }
            _ if word.starts_with('-') => return None,
            _ => break word.as_str(),
        }
    };
    let tool: Vec<String> = match sub {
        "exec" => rest.to_vec(),
        "run" | "run-script" => {
            let (name, args) = rest.split_first()?;
            let (package, text) = script(&dir, name)?;
            dir = package;
            with_args(text, args)?
        }
        name if BUILTINS.contains(&name) => return None,
        name => {
            let (package, text) = script(&dir, name)?;
            dir = package;
            with_args(text, rest)?
        }
    };
    let cd = if dir == cwd {
        String::new()
    } else {
        let shown = dir.strip_prefix(cwd).unwrap_or(&dir);
        format!("cd {} && ", quote(&shown.to_string_lossy()))
    };
    Some(vec![format!("{cd}{}", join(&by_path(tool)))])
}

/// The folder pnpm's `flag` (-C, --dir, -F or --filter) with `value` runs
/// in. A filter names one workspace package, found by its package.json's
/// name under the checkout's top.
fn folder(here: &Path, flag: &str, value: &str) -> Option<PathBuf> {
    if matches!(flag, "-C" | "--dir") {
        return Some(resolve(here, value));
    }
    let top = here.ancestors().find(|d| d.join(".git").exists())?;
    let mut found = Vec::new();
    packages_named(top, value, &mut found);
    (found.len() == 1).then(|| found.remove(0))
}

/// Every folder under `dir` whose package.json is named `name`, past
/// node_modules and hidden folders.
fn packages_named(dir: &Path, name: &str, found: &mut Vec<PathBuf>) {
    if package_json(dir).is_some_and(|p| p["name"] == name) {
        found.push(dir.to_path_buf());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file != "node_modules"
            && !file.starts_with('.')
            && entry.file_type().is_ok_and(|t| t.is_dir())
        {
            packages_named(&entry.path(), name, found);
        }
    }
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

/// The folder of the package.json nearest `dir`, as pnpm finds it, and its
/// script `name`.
fn script(dir: &Path, name: &str) -> Option<(PathBuf, String)> {
    let (package, json) = dir
        .ancestors()
        .find_map(|d| Some((d.to_path_buf(), package_json(d)?)))?;
    Some((package, json["scripts"][name].as_str()?.to_string()))
}

fn package_json(dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    serde_json::from_str(&text).ok()
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
