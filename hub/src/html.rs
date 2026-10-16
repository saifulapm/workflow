//! The page markup lives in hubpages; the subscribe page reads the hub's
//! own config, so it stays here.

pub use hubpages::html::*;

use crate::config::Config;

/// `GET /subscribe`: the topic, both links, and the sentence that
/// says accurately what subscribing exposes. No QR, and therefore no image
/// crate.
pub fn subscribe_page(config: &Config, machine: &str) -> String {
    let topic = esc(&config.topic);
    let url = config.subscribe_url();
    let subscribe = outside_link(&url, &esc(&url));
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>hub — subscribe</title>\n\
         <style>{STYLE}</style>\n</head>\n<body>\n\
         {top}\
         <p>Topic, to type into the ntfy app:</p>\n\
         <p class=\"q\"><strong>{topic}</strong></p>\n\
         <ul>\n\
         <li><a href=\"ntfy://{topic}\" target=\"_blank\" rel=\"noopener\">ntfy://{topic}</a></li>\n\
         <li>{subscribe}</li>\n\
         </ul>\n\
         <h2>What this publishes</h2>\n\
         <p>The question text never leaves the tailnet. The doorbell publishes \
         the machine name, the project name, this hub&#39;s URL and the timing \
         of every question to a public third-party service, protected only by a \
         secret topic name that anyone holding it can also publish to. The \
         topic above is that secret: it is 128 random bits, it lives 0600 in \
         <code>~/.config/hub/config.toml</code>, and this page is reachable \
         only from {machine} and the tailnet.</p>\n\
         </body>\n</html>\n",
        machine = esc(machine),
        top = top_bar("subscribe"),
    )
}
