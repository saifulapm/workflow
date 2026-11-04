//! `pkill -f` and `pgrep -f` match the whole command line, and the shell
//! that runs them holds the pattern in its own: six runs killed their own
//! shell, the last eventsmith m32 (exit 144).

use super::Call;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard: pkill -f and pgrep -f match their own shell's command line and kill it.\n\
Find the process with ss -ltnp | grep :<port>, then kill -TERM -- -<pgid> in its own call.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    call.commands
        .iter()
        .any(|command| {
            let argv = argv(&command.words);
            argv.first()
                .is_some_and(|w| matches!(program_name(w), "pkill" | "pgrep"))
                && argv[1..].iter().any(|w| is_full(w))
        })
        .then_some(REFUSAL)
}

/// `-f`, `--full`, or short flags run together with f among them (`-af`).
fn is_full(word: &str) -> bool {
    word == "--full"
        || word
            .strip_prefix('-')
            .is_some_and(|flags| !flags.starts_with('-') && flags.contains('f'))
}
