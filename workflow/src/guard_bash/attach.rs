//! A bare `playwright-cli attach` runs whichever playwright-cli comes first
//! on PATH. Where pnpm's comes before the dotfiles' wrapper, attaching to the
//! extension failed (shopify_themes m7, splitroute twice).

use std::path::Path;

use super::Call;
use crate::shell::{argv, subcommand};

/// Where the dotfiles keep the wrapper: v2's bin first, then v1's.
const WRAPPERS: [(&str, &str); 2] = [
    ("Codes/dotfiles/bin/playwright-cli", V2_REFUSAL),
    (".dotfiles/bin/playwright-cli", V1_REFUSAL),
];

const V2_REFUSAL: &str = "workflow guard refused this on purpose: a bare playwright-cli may be pnpm's, which cannot attach to the browser extension.\n\
Call the wrapper by path: ~/Codes/dotfiles/bin/playwright-cli attach --extension=chromium.";

const V1_REFUSAL: &str = "workflow guard refused this on purpose: a bare playwright-cli may be pnpm's, which cannot attach to the browser extension.\n\
Call the wrapper by path: ~/.dotfiles/bin/playwright-cli attach --extension=chromium.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    let bare_attach = call.commands.iter().any(|command| {
        let argv = argv(&command.words);
        argv.first().is_some_and(|w| w == "playwright-cli") && subcommand(argv) == Some("attach")
    });
    if !bare_attach {
        return None;
    }
    wrapper_refusal()
}

/// The refusal naming this machine's wrapper; none in a cloud session, which
/// has no dotfiles.
fn wrapper_refusal() -> Option<&'static str> {
    let home = std::env::var_os("HOME")?;
    WRAPPERS
        .iter()
        .find(|(path, _)| Path::new(&home).join(path).is_file())
        .map(|(_, refusal)| *refusal)
}
