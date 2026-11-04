//! A bare `playwright-cli attach` runs whichever playwright-cli comes first
//! on PATH. Where pnpm's comes before the dotfiles' wrapper, attaching to the
//! extension failed (shopify_themes m7, splitroute twice).

use std::path::Path;

use super::Call;
use crate::shell::{argv, subcommand};

const REFUSAL: &str = "workflow guard: a bare playwright-cli may be pnpm's, which cannot attach to the browser extension.\n\
Call the wrapper by path: ~/.dotfiles/bin/playwright-cli attach --extension=chromium.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    let bare_attach = call.commands.iter().any(|command| {
        let argv = argv(&command.words);
        argv.first().is_some_and(|w| w == "playwright-cli") && subcommand(argv) == Some("attach")
    });
    (bare_attach && wrapper_exists()).then_some(REFUSAL)
}

/// Whether this machine has the dotfiles' wrapper; a cloud session has not.
fn wrapper_exists() -> bool {
    std::env::var_os("HOME").is_some_and(|home| {
        Path::new(&home)
            .join(".dotfiles/bin/playwright-cli")
            .is_file()
    })
}
