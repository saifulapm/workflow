//! The dashboard (spec §3), escaped (spec §9.1).
//!
//! Every value that reaches this page came from `mem`, and mem's questions are
//! written by an agent that has been reading repositories and web pages. A
//! title containing `<script>` would otherwise become script on the origin that
//! owns `POST /answer` — no external attacker required (review B-4b). So there
//! is exactly one way text gets into the page, `esc`, and no format string in
//! this file interpolates a value without it.

use crate::config::Config;
use crate::model::{Activity, Project, Question, View};

/// The one escaping function. `'` and `"` are in here because values also land
/// in attributes (`value="…"`, `href="…"`).
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// What the last `POST /answer` did. A code, not the text itself: the banner is
/// chosen here rather than reflected out of the query string, so there is
/// nothing on this path to reflect.
#[derive(Debug, Clone, PartialEq)]
pub enum Banner {
    None,
    Answered(String),
    Unknown,
    Empty,
    Failed,
}

impl Banner {
    /// Reads the banner back off the redirect's query string.
    pub fn from_query(query: &crate::form::Form) -> Banner {
        if let Some(id) = query.get("answered") {
            // Only ever a mem id, and mem ids are base32; anything else is
            // dropped rather than shown.
            let clean: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            return Banner::Answered(clean);
        }
        if query.get("unknown").is_some() {
            return Banner::Unknown;
        }
        if query.get("empty").is_some() {
            return Banner::Empty;
        }
        if query.get("failed").is_some() {
            return Banner::Failed;
        }
        Banner::None
    }

    /// The query string this banner redirects with.
    pub fn query(&self) -> String {
        match self {
            Banner::None => String::new(),
            Banner::Answered(id) => format!("?answered={}", crate::form::encode_component(id)),
            Banner::Unknown => "?unknown=1".to_string(),
            Banner::Empty => "?empty=1".to_string(),
            Banner::Failed => "?failed=1".to_string(),
        }
    }

    fn render(&self) -> String {
        let (kind, text) = match self {
            Banner::None => return String::new(),
            Banner::Answered(id) => ("ok", format!("Answered #{}.", esc(id))),
            Banner::Unknown => (
                "warn",
                "That id is not in the pending queue — it may already be answered.".to_string(),
            ),
            Banner::Empty => ("warn", "Type an answer first.".to_string()),
            Banner::Failed => (
                "warn",
                "mem could not record that answer. Nothing was written.".to_string(),
            ),
        };
        format!("<p class=\"banner {kind}\">{text}</p>\n")
    }
}

const STYLE: &str = "\
:root{color-scheme:dark light}
*{box-sizing:border-box}
body{margin:0;padding:1rem;font:16px/1.5 system-ui,sans-serif;max-width:44rem}
header{display:flex;flex-wrap:wrap;gap:.5rem 1rem;align-items:baseline;\
border-bottom:1px solid #8884;padding-bottom:.5rem;margin-bottom:1rem}
h1{font-size:1.3rem;margin:0}
h2{font-size:1rem;text-transform:uppercase;letter-spacing:.05em;opacity:.7;\
margin:1.5rem 0 .5rem}
nav{margin-left:auto;font-size:.9rem}
nav a{margin-left:.75rem}
.meta{font-size:.8rem;opacity:.65}
.q{white-space:pre-wrap;word-break:break-word;margin:.25rem 0 .5rem;font:inherit}
article{border:1px solid #8884;border-radius:.5rem;padding:.75rem;margin-bottom:.75rem}
textarea{width:100%;font:inherit;padding:.5rem;border-radius:.4rem;\
border:1px solid #8886;background:transparent;color:inherit}
button{margin-top:.5rem;font:inherit;padding:.45rem 1rem;border-radius:.4rem;\
border:1px solid #8886;background:#8882;color:inherit}
ul{list-style:none;padding:0;margin:0}
li{padding:.35rem 0;border-bottom:1px solid #8882}
.banner{padding:.6rem .8rem;border-radius:.4rem;margin:0 0 1rem}
.banner.ok{background:#2a5}
.banner.warn{background:#a52}
.banner.degraded{background:#a33}
.empty{opacity:.6}
";

/// The whole page. `<meta http-equiv=refresh>` keeps it live without a line of
/// JavaScript, which is also what makes answering work with JS disabled (AC4).
pub fn page(view: &View, config: &Config, banner: &Banner) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str("<meta http-equiv=\"refresh\" content=\"15\">\n");
    out.push_str(&format!("<title>hub — {}</title>\n", esc(&view.machine)));
    out.push_str(&format!("<style>{STYLE}</style>\n"));
    out.push_str("</head>\n<body>\n");

    out.push_str("<header>\n");
    out.push_str(&format!("<h1>{}</h1>\n", esc(&view.machine)));
    out.push_str("<nav>");
    for sibling in &config.siblings {
        if let Some(link) = safe_link(sibling) {
            out.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                link,
                esc(&label(sibling))
            ));
        }
    }
    out.push_str("<a href=\"/subscribe\">subscribe</a>");
    out.push_str("</nav>\n</header>\n");

    out.push_str(&banner.render());
    if let Some(why) = view.degraded() {
        out.push_str(&format!(
            "<p class=\"banner degraded\">mem is not answering, so this page is \
             out of date: {}</p>\n",
            esc(why)
        ));
    }

    out.push_str("<h2>Pending questions</h2>\n");
    if view.questions.rows.is_empty() {
        out.push_str("<p class=\"empty\">Nothing waiting.</p>\n");
    }
    for question in &view.questions.rows {
        out.push_str(&question_block(question));
    }

    out.push_str("<h2>Recent activity</h2>\n");
    if view.activity.rows.is_empty() {
        out.push_str("<p class=\"empty\">Nothing recorded yet.</p>\n");
    } else {
        out.push_str("<ul>\n");
        for item in &view.activity.rows {
            out.push_str(&activity_row(item));
        }
        out.push_str("</ul>\n");
    }

    out.push_str("<h2>Projects</h2>\n");
    if view.projects.rows.is_empty() {
        out.push_str("<p class=\"empty\">No projects registered.</p>\n");
    } else {
        out.push_str("<ul>\n");
        for project in &view.projects.rows {
            out.push_str(&project_row(project));
        }
        out.push_str("</ul>\n");
    }

    out.push_str("</body>\n</html>\n");
    out
}

/// `GET /subscribe` (spec §3): the topic, both links, and the sentence that
/// says accurately what subscribing exposes. No QR, and therefore no image
/// crate — §3 offered three options and settled this one.
pub fn subscribe_page(config: &Config, machine: &str) -> String {
    let topic = esc(&config.topic);
    let subscribe = esc(&config.subscribe_url());
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>hub — subscribe</title>\n\
         <style>{STYLE}</style>\n</head>\n<body>\n\
         <header><h1>subscribe</h1><nav><a href=\"/\">back</a></nav></header>\n\
         <p>Topic, to type into the ntfy app:</p>\n\
         <p class=\"q\"><strong>{topic}</strong></p>\n\
         <ul>\n\
         <li><a href=\"ntfy://{topic}\">ntfy://{topic}</a></li>\n\
         <li><a href=\"{subscribe}\">{subscribe}</a></li>\n\
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
    )
}

fn question_block(question: &Question) -> String {
    let project = question.project.as_deref().unwrap_or("—");
    format!(
        "<article>\n\
         <div class=\"meta\">{project} · {age} · #{short}</div>\n\
         <p class=\"q\">{text}</p>\n\
         <form method=\"post\" action=\"/answer\">\n\
         <input type=\"hidden\" name=\"id\" value=\"{id}\">\n\
         <textarea name=\"text\" rows=\"3\" placeholder=\"answer\" \
         aria-label=\"answer\"></textarea>\n\
         <button type=\"submit\">Answer</button>\n\
         </form>\n\
         </article>\n",
        project = esc(project),
        age = esc(&question.age),
        short = esc(&question.short_id),
        // pre-wrap in the stylesheet, so a batched, numbered ask keeps its
        // lines instead of collapsing into one (review m-12).
        text = esc(&question.text),
        id = esc(&question.id),
    )
}

fn activity_row(item: &Activity) -> String {
    format!(
        "<li>{title}<div class=\"meta\">{project} · {age}</div></li>\n",
        title = esc(&item.title),
        project = esc(item.project.as_deref().unwrap_or("—")),
        age = esc(&item.age),
    )
}

fn project_row(project: &Project) -> String {
    format!(
        "<li><strong>{name}</strong> <span class=\"meta\">{age}</span>\
         <div>{status}</div></li>\n",
        name = esc(&project.name),
        age = esc(project.last_activity_age.as_deref().unwrap_or("—")),
        // AC7: a project with no status.md is an em dash, not an error.
        status = esc(project.status.as_deref().unwrap_or("—")),
    )
}

/// A sibling link, escaped, and only if it is http(s). A `javascript:` URL in
/// one's own config is self-inflicted, but the check costs one line.
fn safe_link(url: &str) -> Option<String> {
    (url.starts_with("http://") || url.starts_with("https://")).then(|| esc(url))
}

fn label(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_every_character_that_ends_a_context() {
        assert_eq!(
            esc("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
        assert_eq!(esc("\" onmouseover=\"x"), "&quot; onmouseover=&quot;x");
        assert_eq!(esc("' onfocus='x"), "&#39; onfocus=&#39;x");
        assert_eq!(esc("a & b"), "a &amp; b");
        assert_eq!(esc("🙂 — ünïcödé"), "🙂 — ünïcödé");
        // Escaping is not double-escaping.
        assert_eq!(esc("&amp;"), "&amp;amp;");
    }

    #[test]
    fn a_banner_survives_the_redirect_and_back() {
        for banner in [
            Banner::Answered("28J1TSD1".to_string()),
            Banner::Unknown,
            Banner::Empty,
            Banner::Failed,
        ] {
            let query = banner.query();
            let parsed = Banner::from_query(&crate::form::Form::parse(&query[1..]));
            assert_eq!(parsed, banner, "{query}");
        }
        assert_eq!(
            Banner::from_query(&crate::form::Form::parse("")),
            Banner::None
        );
    }

    #[test]
    fn a_crafted_banner_id_is_stripped_rather_than_shown() {
        // The banner is the one value that comes back off a URL, so it never
        // carries anything but base32.
        let form = crate::form::Form::parse("answered=%3Cscript%3E");
        assert_eq!(Banner::from_query(&form), Banner::Answered("script".into()));
    }

    #[test]
    fn only_http_siblings_become_links() {
        assert!(safe_link("http://nuc:8787").is_some());
        assert!(safe_link("https://nuc:8787").is_some());
        assert!(safe_link("javascript:alert(1)").is_none());
        assert_eq!(label("http://nuc:8787/"), "nuc:8787");
    }
}
