//! A subagent's `rm` of a glob deleted folders it did not own: a walker's
//! `rm -rf tmp.??????????` took the orchestrator's scratch folders, and
//! `rm -f x/*` after a `cd` did the same. A subagent removes only the exact
//! path of its own folder.

use super::Call;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: a subagent removes only its own folder by its exact path, never a glob.\n\
Run rm -rf <its folder>, spelled out in full.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    call.agent?;
    call.commands
        .iter()
        .map(|command| argv(&command.words))
        .filter(|argv| argv.first().is_some_and(|w| program_name(w) == "rm"))
        .any(|argv| argv[1..].iter().any(|w| !w.starts_with('-') && has_glob(w)))
        .then_some(REFUSAL)
}

/// The lexer takes quotes off a word, so a quoted `'a*b'` reads as a glob
/// too. Refusing it is the safe side: the exact path needs no glob char.
fn has_glob(word: &str) -> bool {
    word.contains(['*', '?', '['])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::shell::commands;

    fn refused(agent: Option<&str>, line: &str) -> bool {
        let commands = commands(line).unwrap();
        let call = Call {
            commands: &commands,
            cwd: Path::new("/repo"),
            agent,
        };
        refusal(&call).is_some()
    }

    #[test]
    fn a_subagents_rm_of_a_glob_is_refused() {
        for line in [
            "rm -rf tmp.??????????",
            "cd d; rm -f x/*",
            "rm -rf /tmp/claude-1000/x/tmp.*",
            "rm -r build/[ab]",
            "/bin/rm -rf tmp.*",
            "\\rm -rf tmp.*",
        ] {
            assert!(refused(Some("dogfooder"), line), "{line}");
        }
    }

    #[test]
    fn the_sessions_own_agent_may_rm_a_glob() {
        for line in [
            "rm -rf tmp.??????????",
            "cd d; rm -f x/*",
            "rm -r build/[ab]",
        ] {
            assert!(!refused(None, line), "{line}");
        }
    }

    #[test]
    fn a_subagents_rm_of_an_exact_path_and_other_commands_run() {
        for line in [
            "rm -rf ~/.cache/workflow-m6/walk1",
            "echo 'rm -rf x*'",
            "git rm --cached x",
            "grep -r 'tmp.*' .",
        ] {
            assert!(!refused(Some("dogfooder"), line), "{line}");
        }
    }
}
