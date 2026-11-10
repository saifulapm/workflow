//! Rule for reading a playwright-cli page snapshot into the transcript. A
//! snapshot lists every textbox value, so a settings page holds its client
//! secret in it (one grep on a saved snapshot printed a secret that then had
//! to be rotated). The read is safe only through `workflow mask`.

use super::Call;
use crate::shell::{Simple, argv, program_name, subcommand};

const SNAPSHOT_READ: &str = "workflow guard refused this on purpose: a page snapshot holds textbox values, a secret too, and this prints it.\n\
Pipe it through workflow mask, or read the page with run-code returning labels or lengths only.";

const READERS: [&str; 9] = [
    "cat", "grep", "rg", "sed", "head", "tail", "less", "bat", "awk",
];

pub fn refusal(call: &Call) -> Option<&'static str> {
    let unmasked = call.commands.iter().enumerate().any(|(i, command)| {
        prints_snapshot(command) && !piped_to_mask(call.commands.get(i + 1), command)
    });
    unmasked.then_some(SNAPSHOT_READ)
}

/// A `playwright-cli snapshot` with no `--filename`, or a reader given a
/// snapshot file.
fn prints_snapshot(command: &Simple) -> bool {
    let argv = argv(&command.words);
    let Some(program) = argv.first().map(|w| program_name(w)) else {
        return false;
    };
    if program == "playwright-cli" {
        return subcommand(argv) == Some("snapshot")
            && !argv
                .iter()
                .any(|w| w == "--filename" || w.starts_with("--filename="));
    }
    READERS.contains(&program) && argv.iter().skip(1).any(|w| is_snapshot_path(w))
}

/// A yml or yaml path inside a `.playwright-cli` folder, or one whose file
/// name says snapshot.
fn is_snapshot_path(word: &str) -> bool {
    if word.starts_with('-') || !(word.ends_with(".yml") || word.ends_with(".yaml")) {
        return false;
    }
    let name = word.rsplit('/').next().unwrap_or(word);
    name.contains("snapshot")
        || word
            .split('/')
            .rev()
            .skip(1)
            .any(|d| d == ".playwright-cli")
}

/// Whether `command`'s stdout goes straight into `workflow mask`.
fn piped_to_mask(next: Option<&Simple>, command: &Simple) -> bool {
    command.then == crate::shell::Op::Pipe
        && next.is_some_and(|next| {
            let argv = argv(&next.words);
            argv.first().map(|w| program_name(w)) == Some("workflow")
                && subcommand(argv) == Some("mask")
        })
}
