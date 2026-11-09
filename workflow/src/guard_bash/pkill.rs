//! `pkill -f` and `pgrep -f` match the whole command line, and the shell
//! that runs them holds the pattern in its own: six runs killed their own
//! shell, the last eventsmith m32 (exit 144). `pkill -x` and `killall` stop
//! every process of a name, and a walker's `pkill -x chromium-browser` ran
//! on the host, where Saiful's own chromium has that name too.

use super::Call;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: pkill -f and pgrep -f match their own shell's command line and kill it.\n\
Find the process with ss -ltnp | grep :<port>, then kill -TERM -- -<pgid> in its own call.";

const BY_NAME: &str = "workflow guard refused this on purpose: pkill -x and killall stop every process of that name, Saiful's own among them.\n\
Stop a process by the pid you started it with, or find it with ss -ltnp | grep :<port>, then kill -TERM -- -<pgid> in its own call.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    let mut found = None;
    for command in call.commands {
        let argv = argv(&command.words);
        let Some(first) = argv.first() else { continue };
        match program_name(first) {
            "pkill" | "pgrep" if argv[1..].iter().any(|w| is_full(w)) => return Some(REFUSAL),
            "pkill" if argv[1..].iter().any(|w| is_flag(w, 'x', "--exact")) => {
                found = Some(BY_NAME)
            }
            "killall" => found = Some(BY_NAME),
            _ => {}
        }
    }
    found
}

/// `-f`, `--full`, or short flags run together with f among them (`-af`).
fn is_full(word: &str) -> bool {
    is_flag(word, 'f', "--full")
}

/// `long`, or short flags run together with `short` among them.
fn is_flag(word: &str, short: char, long: &str) -> bool {
    word == long
        || word
            .strip_prefix('-')
            .is_some_and(|flags| !flags.starts_with('-') && flags.contains(short))
}
