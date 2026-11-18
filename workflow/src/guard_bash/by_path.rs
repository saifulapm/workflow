//! The by-path form of a pnpm command: what it runs, its tool called as
//! node_modules/.bin/<tool>. 22 refused lareys-trendsi implementers had pnpm
//! in their VERIFY and were told only to call the tool by path, not which
//! tool.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::cd::resolve;
use super::pnpm::runs_pnpm;
use crate::shell::{self, Op, Simple, program_name};

/// pnpm's own commands that install, publish or fetch: a package script of
/// the same name (an `install` lifecycle script) never runs by them.
const BUILTINS: &[&str] = &[
    "install", "i", "add", "remove", "rm", "update", "up", "link", "rebuild", "prune", "dedupe",
    "deploy", "publish", "pack", "dlx", "create",
];

/// The programs a script line cannot be split past: those that run another
/// script or install (turbo runs pnpm install, palmshop m20), and cd, whose
/// move the next call would not keep.
const RUNNERS: &[&str] = &[
    "pnpm", "pnpx", "npm", "npx", "yarn", "bun", "bunx", "turbo", "cd", "pushd",
];

/// The by-path lines of every pnpm command in `commands`, run in `cwd`, or
/// None when one of them cannot be put that way.
pub fn lines(commands: &[Simple], cwd: &Path) -> Option<Vec<String>> {
    // The folder each subshell depth is in, as the line's `cd`s move it.
    let mut dirs = vec![cwd.to_path_buf()];
    let mut out = Vec::new();
    for (at, command) in commands.iter().enumerate() {
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
        } else if runs_pnpm(&command.words) {
            out.extend(translate(commands, at, &here, cwd)?);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The lines for `commands[at]`, a pnpm command run in `here`, shown from
/// `cwd`.
fn translate(commands: &[Simple], at: usize, here: &Path, cwd: &Path) -> Option<Vec<String>> {
    let words = &commands[at].words;
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
    let (mut tools, args) = match sub {
        "exec" => (vec![vec![by_path(rest.to_vec())]], &[][..]),
        name if BUILTINS.contains(&name) => return None,
        name => {
            let (name, args): (&str, &[String]) = match name {
                "run" | "run-script" => {
                    let (name, args) = rest.split_first()?;
                    (name, args)
                }
                _ => (name, rest),
            };
            let (package, text) = script(&dir, name)?;
            dir = package;
            (script_lines(&text)?, args)
        }
    };
    // pnpm hands its arguments to the script's last command; its
    // redirections and the pipe it feeds go with every line.
    let mut redirects = Vec::new();
    let mut extra = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if is_redirect(arg) {
            redirects.push(arg.clone());
            // `>` waits for its target in the next word; `2>&1` has its own.
            if matches!(
                arg.trim_start_matches(|c: char| c.is_ascii_digit()),
                ">" | ">>" | "<" | "&>" | "&>>" | ">|"
            ) {
                redirects.extend(args.next().cloned());
            }
        } else {
            extra.push(arg.clone());
        }
    }
    tools.last_mut()?.last_mut()?.extend(extra);
    let mut tail = String::new();
    let mut i = at;
    while commands[i].then == Op::Pipe && i + 1 < commands.len() {
        i += 1;
        tail.push_str(" | ");
        tail.push_str(&join(&commands[i].words));
    }
    let cd = if dir == cwd {
        String::new()
    } else {
        let shown = dir.strip_prefix(cwd).unwrap_or(&dir);
        format!("cd {} && ", quote(&shown.to_string_lossy()))
    };
    let mut lead = join(&words[..p]);
    if !lead.is_empty() {
        lead.push(' ');
    }
    Some(
        tools
            .iter()
            .map(|pipeline| {
                let mut line = pipeline
                    .iter()
                    .map(|command| join(command))
                    .collect::<Vec<_>>()
                    .join(" | ");
                if !redirects.is_empty() {
                    line = format!("{line} {}", join(&redirects));
                }
                format!("{cd}{lead}{line}{tail}")
            })
            .collect(),
    )
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

/// The lines of `text`, a script, one for each command or pipeline it runs
/// in turn, each command's tool by path. None for a script it cannot split
/// that way: one that runs another script or package manager, moves with
/// cd, branches with `||`, or holds a subshell or a variable.
fn script_lines(text: &str) -> Option<Vec<Vec<Vec<String>>>> {
    if text.contains(['$', '`']) {
        return None;
    }
    let mut lines = vec![Vec::new()];
    for command in shell::commands(text)? {
        let argv = shell::argv(&command.words);
        // The program and the wrappers in front of it, `npx` among them.
        let leading = &command.words[..command.words.len() - argv.len()];
        if command.depth > 0
            || leading
                .iter()
                .chain(argv.first())
                .any(|w| RUNNERS.contains(&program_name(w)))
        {
            return None;
        }
        lines.last_mut()?.push(by_path(command.words));
        match command.then {
            Op::Pipe => {}
            Op::Seq | Op::And => lines.push(Vec::new()),
            Op::Or | Op::Background => return None,
        }
    }
    lines.retain(|line| !line.is_empty());
    (!lines.is_empty()).then_some(lines)
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

/// `words` with node_modules/.bin/ in front of the program, unless node
/// or the system has it, or it is a path.
fn by_path(mut words: Vec<String>) -> Vec<String> {
    let at = words.len() - shell::argv(&words).len();
    if let Some(program) = words.get_mut(at) {
        let system = ["/usr/bin", "/bin"]
            .iter()
            .any(|d| Path::new(d).join(&*program).exists());
        if !program.contains('/') && program != "node" && !system {
            *program = format!("node_modules/.bin/{program}");
        }
    }
    words
}

/// A redirection, as in `2>&1`, `>out.txt` or `>`.
fn is_redirect(word: &str) -> bool {
    word.trim_start_matches(|c: char| c.is_ascii_digit())
        .starts_with(['<', '>'])
        || word.starts_with("&>")
}

/// `words` as one shell line, each quoted when the shell would split it.
fn join(words: &[String]) -> String {
    words
        .iter()
        .map(|w| if is_redirect(w) { w.clone() } else { quote(w) })
        .collect::<Vec<_>>()
        .join(" ")
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
