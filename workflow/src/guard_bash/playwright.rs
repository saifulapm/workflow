//! Rules for `playwright-cli` calls. `close-all` ends the browser of every
//! named session, other agents' included (a walker's close-all killed the
//! orchestrator's probe browser mid-fix).

use super::Call;
use crate::shell::{argv, subcommand};

const CLOSE_ALL: &str = "workflow guard refused this on purpose: playwright-cli close-all ends every session's browser, other agents' too.\n\
Close only the session you opened: playwright-cli -s=<name> close.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    close_all(call).then_some(CLOSE_ALL)
}

/// Whether any command is `playwright-cli close-all`, bare or by path.
fn close_all(call: &Call) -> bool {
    call.commands.iter().any(|command| {
        let argv = argv(&command.words);
        argv.first()
            .is_some_and(|w| w.rsplit('/').next() == Some("playwright-cli"))
            && subcommand(argv) == Some("close-all")
    })
}
