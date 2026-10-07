//! The parts every page shares, escaped.
//!
//! Every value that reaches this page came from `mem`, and mem's questions are
//! written by an agent that has been reading repositories and web pages. A
//! title containing `<script>` would otherwise become script on the origin that
//! owns `POST /answer` — no external attacker required. So there
//! is exactly one way text gets into the page, `esc`, and no format string in
//! this file interpolates a value without it.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html as cmark};

use crate::config::Config;
use crate::form::encode_component;
use crate::model::{Activity, ItemDetail, Section, WikiProject, is_slug};

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
}

/// The one stylesheet. Colours, type and spacing are tokens on `:root`, so
/// the dark scheme is a second set of values rather than a second sheet, and
/// every page, form control and code block inherits Maple Mono from the hub's
/// own `/assets`.
const STYLE: &str = r#"
@font-face{font-family:"Maple Mono";src:url(/assets/maple-mono-400.woff2) format("woff2");font-weight:400;font-style:normal;font-display:swap}
@font-face{font-family:"Maple Mono";src:url(/assets/maple-mono-600.woff2) format("woff2");font-weight:600;font-style:normal;font-display:swap}
@font-face{font-family:"Maple Mono";src:url(/assets/maple-mono-700.woff2) format("woff2");font-weight:700;font-style:normal;font-display:swap}
@font-face{font-family:"Maple Mono";src:url(/assets/maple-mono-400-italic.woff2) format("woff2");font-weight:400;font-style:italic;font-display:swap}
:root{color-scheme:light dark;--bg:#f7f7f5;--surface:#fff;--line:#e9e9e5;--track:#ececea;--ink:#17181c;--mut:#6b6d75;
--accent:#2b59c3;--accent-soft:#e8eefb;--on-accent:#fff;--ok:#1f7a52;--ok-soft:#e3f4ec;--wait:#9a5b00;--wait-soft:#fdf1dc;--bad:#b42318;--bad-soft:#fdeceb;
--radius:12px;--space:12px;--font:"Maple Mono",ui-monospace,monospace}
@media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#0f1115;--surface:#171a21;--line:#262a33;--track:#262a33;--ink:#e7e9ee;--mut:#9aa0ab;
--accent:#7ea2f2;--accent-soft:#1c2742;--on-accent:#0f1115;--ok:#5cc996;--ok-soft:#15291f;--wait:#e7ad52;--wait-soft:#2e2413;--bad:#f2837a;--bad-soft:#341b1a}}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{margin:0;padding:0 16px 48px;background:var(--bg);color:var(--ink);font-size:14px;line-height:1.6}
body,button,input,textarea,select,code,pre,kbd{font-family:var(--font)}
body>*{max-width:60rem;margin-inline:auto}
a{color:var(--accent);text-decoration:none}
a:hover{text-decoration:underline}
:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
img{max-width:100%;height:auto}
header.top{max-width:none;display:flex;align-items:center;gap:10px;margin:0 -16px 14px;padding:12px max(16px,calc((100% - 60rem)/2 + 16px));
background:var(--surface);border-bottom:1px solid var(--line);position:sticky;top:0;z-index:2}
header.top .brand{display:inline-flex;align-items:center;gap:7px;font-weight:700;color:var(--ink)}
header.top .brand::before{content:"";width:14px;height:14px;border-radius:4px;background:var(--accent)}
header.top h1{margin:0;font-size:14px;font-weight:600;min-width:0;overflow-wrap:anywhere}
header.top h1::before{content:"/";margin-right:10px;color:var(--mut);font-weight:400}
header.top h1 a{color:inherit}
nav.pages{display:flex;flex-wrap:wrap;gap:6px;margin-block:0 18px}
nav.pages a{padding:4px 11px;border:1px solid var(--line);border-radius:99px;background:var(--surface);color:var(--ink);font-size:13px;white-space:nowrap}
nav.pages a:hover{text-decoration:none;border-color:var(--accent)}
nav.pages a[aria-current=page]{background:var(--accent);border-color:var(--accent);color:var(--on-accent)}
h2{font-size:12px;font-weight:600;text-transform:uppercase;letter-spacing:.08em;color:var(--mut);margin-block:28px 10px}
h3{font-size:15px;margin-block:18px 8px}
h2 .pill{text-transform:none;letter-spacing:0;margin-left:6px;vertical-align:1px}
p{margin-block:0 10px}
.meta{font-size:12.5px;color:var(--mut);overflow-wrap:anywhere}
.empty{color:var(--mut)}
.q{white-space:pre-wrap;word-break:break-word;margin-block:4px 8px;font:inherit}
.rec{font-size:12.5px;color:var(--mut)}
.sp{flex:1}
article,.card{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:14px 16px;margin-block:0 10px}
.cards{display:grid;gap:10px;grid-template-columns:repeat(auto-fill,minmax(17rem,1fr));margin-block:0 10px}
.cards>.card{margin:0}
.card h3{margin:0;font-size:15px}
.card>.meta{margin-top:4px}
.row{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
ul{list-style:none;padding:0;margin:0}
body>ul:not([class]){background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:0 16px;margin-block:0 10px}
body>ul:not([class])>li{padding:10px 0;border-bottom:1px solid var(--line)}
body>ul:not([class])>li:last-child{border-bottom:0}
.pill{display:inline-block;font-size:11.5px;font-weight:600;line-height:1.7;padding:0 9px;border-radius:99px;background:var(--accent-soft);color:var(--accent);white-space:nowrap}
.pill.ok{background:var(--ok-soft);color:var(--ok)}
.pill.wait{background:var(--wait-soft);color:var(--wait)}
.pill.bad{background:var(--bad-soft);color:var(--bad)}
.pill.mut{background:var(--track);color:var(--mut)}
.bar{height:6px;background:var(--track);border-radius:3px;overflow:hidden;margin-block:8px 4px}
.bar>i{display:block;height:100%;background:var(--accent);border-radius:3px}
.tl{margin:0 0 16px 6px;padding-left:20px;border-left:2px solid var(--line)}
.tl>li{position:relative;padding:2px 0 14px}
.tl>li::before{content:"";position:absolute;left:-29px;top:7px;width:12px;height:12px;border-radius:50%;background:var(--line);border:2px solid var(--bg)}
.tl>li.done::before{background:var(--ok)}
.tl>li.live::before{background:var(--accent);box-shadow:0 0 0 4px var(--accent-soft)}
.gallery{display:grid;gap:10px;grid-template-columns:repeat(auto-fill,minmax(10rem,1fr));margin-block:0 16px}
.gallery figure{margin:0;background:var(--surface);border:1px solid var(--line);border-radius:10px;overflow:hidden}
.gallery img{display:block;width:100%;aspect-ratio:4/3;object-fit:cover;background:var(--track)}
.gallery figcaption{padding:8px 10px;font-size:12px;color:var(--mut);overflow-wrap:anywhere}
.lightbox{display:none;position:fixed;inset:0;z-index:10;max-width:none;margin:0;padding:48px 16px 16px;overflow:auto;background:rgba(10,12,16,.94);color:#e7e9ee}
.lightbox:target{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:12px}
.lightbox img{max-width:100%;max-height:78vh;object-fit:contain;border-radius:8px;background:#fff}
.lightbox p{margin:0;max-width:40rem;text-align:center}
.lightbox a{color:#9fbcf7}
.lightbox a.close{position:absolute;top:14px;right:16px}
@media (max-width:959px){.docs>nav.contents{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:10px 16px}}
.docs{display:grid;grid-template-columns:minmax(0,1fr);gap:16px}
@media (min-width:960px){.docs{grid-template-columns:13rem minmax(0,1fr);gap:28px}.docs>nav.contents{position:sticky;top:72px;align-self:start;max-height:calc(100vh - 88px);overflow:auto}}
nav.contents{font-size:13px}
nav.contents a{display:block;padding:3px 0;color:var(--mut)}
nav.contents a:hover{color:var(--accent);text-decoration:none}
textarea,input[type=text],input[type=search],select{width:100%;font-size:16px;padding:9px 11px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--ink)}
select{-webkit-appearance:none;appearance:none;padding-right:36px;cursor:pointer;background-image:linear-gradient(45deg,transparent 50%,var(--mut) 50%),linear-gradient(135deg,var(--mut) 50%,transparent 50%);background-position:calc(100% - 19px) 52%,calc(100% - 14px) 52%;background-size:5px 5px;background-repeat:no-repeat}
select:hover{border-color:var(--accent)}
select option{background:var(--surface);color:var(--ink)}
button{margin-top:8px;font-size:14px;font-weight:600;padding:8px 16px;border-radius:8px;border:1px solid var(--accent);background:var(--accent);color:var(--on-accent);cursor:pointer}
form.search{display:flex;gap:8px;align-items:center;margin-block:0 16px}
button.alt{background:var(--surface);color:var(--accent);border-color:var(--line)}
button.alt:hover{border-color:var(--accent)}
form.opt{display:inline-block;margin:0 6px 8px 0}
input[type=number]{width:6rem;font-size:16px;padding:8px 10px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--ink)}
input[type=file]{font-size:13px;color:var(--mut)}
input[type=file]::file-selector-button{font:inherit;font-weight:600;margin-right:10px;padding:6px 12px;border-radius:8px;border:1px solid var(--line);background:var(--surface);color:var(--accent);cursor:pointer}
form>*+*{margin-top:8px}
form>input[type=hidden]+*{margin-top:0}
form.search button{margin:0}
.banner{padding:10px 14px;border-radius:10px;margin-block:0 16px;background:var(--accent-soft);color:var(--accent)}
.banner.ok{background:var(--ok-soft);color:var(--ok)}
.banner.warn{background:var(--wait-soft);color:var(--wait)}
.banner.degraded{background:var(--bad-soft);color:var(--bad)}
article.md{padding:20px 22px;line-height:1.7}
article.md h1{font-size:21px;line-height:1.3;margin:0 0 12px}
article.md h2{font-size:16px;text-transform:none;letter-spacing:normal;color:var(--ink);margin:28px 0 8px;padding-bottom:6px;border-bottom:1px solid var(--line)}
article.md h3{font-size:14.5px}
article.md :is(h1,h2,h3,h4){scroll-margin-top:64px}
article.md ul,article.md ol{list-style:revert;padding-left:1.4rem;margin-block:0 10px}
article.md li{padding:1px 0}
article.md code{font-size:.92em;background:var(--track);padding:1px 5px;border-radius:5px}
article.md pre{overflow-x:auto;background:var(--bg);border:1px solid var(--line);padding:10px 12px;border-radius:8px}
article.md pre code{background:none;padding:0}
article.md table{display:block;overflow-x:auto;border-collapse:collapse;margin-block:0 12px}
article.md th,article.md td{border:1px solid var(--line);padding:5px 9px;text-align:left}
article.md th{background:var(--bg)}
article.md blockquote{margin:0 0 10px;padding:2px 0 2px 12px;border-left:3px solid var(--accent);color:var(--mut)}
"#;

/// `GET /subscribe`: the topic, both links, and the sentence that
/// says accurately what subscribing exposes. No QR, and therefore no image
/// crate.
pub fn subscribe_page(config: &Config, machine: &str) -> String {
    let topic = esc(&config.topic);
    let subscribe = esc(&config.subscribe_url());
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
        top = top_bar("subscribe"),
    )
}

/// `GET /wiki`: every project that has pages, by name. The name opens the
/// project's own `index` page, which every wiki keeps by hand; listing each
/// project's pages here as well made the page grow with every project, and
/// the index already answers what a project's wiki holds.
pub fn wiki_index(section: &Section<WikiProject>) -> String {
    let mut out = head("wiki");
    out.push_str(&top_bar("wiki"));
    if let Some(why) = &section.degraded {
        out.push_str(&format!(
            "<p class=\"banner degraded\">mem is not answering, so this list is \
             out of date: {}</p>\n",
            esc(why)
        ));
    } else if section.rows.is_empty() {
        out.push_str(
            "<p class=\"empty\">No pages yet. A session writes one with \
             <code>mem wiki &lt;slug&gt; --stdin --note \"&lt;why&gt;\"</code>.</p>\n",
        );
    }
    if !section.rows.is_empty() {
        out.push_str("<ul>\n");
        for project in &section.rows {
            let pages = project.pages.len();
            // The first page: `index` whenever the wiki has one (the sort puts
            // it first), and a real page rather than a 404 when it does not.
            let Some(first) = project.pages.first() else {
                continue; // model drops empty projects; belt and braces
            };
            out.push_str(&format!(
                "<li><a href=\"{href}\">{name}</a>\
                 <div class=\"meta\">{pages} page{s}</div></li>\n",
                href = esc(&page_url(&project.name, &first.slug)),
                name = esc(&project.name),
                s = if pages == 1 { "" } else { "s" },
            ));
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

/// `GET /p/<project>/plan` and `GET /p/<project>/plan/<slug>`: a plan's whole
/// text, so its ticks show.
pub fn plan_page(
    project: &str,
    slug: Option<&str>,
    text: Option<&str>,
    degraded: Option<&str>,
) -> String {
    let label = slug.unwrap_or("plan");
    let mut out = detail_head(project, label);
    if let Some(why) = degraded {
        out.push_str(&degraded_banner(why));
    }
    out.push_str(&markdown_article(text, project, "No plan recorded."));
    out.push_str("</body>\n</html>\n");
    out
}

/// `GET /p/<project>/item/<id>`: one item, whole — its body pre-wrap, like a
/// question's text. `item` is `None` only when mem itself is broken; an id
/// mem simply does not have never reaches this page, since the route answers
/// 404 first.
pub fn item_page(project: &str, item: Option<&ItemDetail>, degraded: Option<&str>) -> String {
    let label = item.map(|item| item.title.as_str()).unwrap_or("item");
    let mut out = detail_head(project, label);
    if let Some(why) = degraded {
        out.push_str(&degraded_banner(why));
    }
    if let Some(item) = item {
        out.push_str(&format!("<div class=\"meta\">{}</div>\n", esc(&item.kind)));
        out.push_str(&body_block(Some(&item.body), ""));
    }
    out.push_str("</body>\n</html>\n");
    out
}

/// The head and header every detail page under `/p/<project>` shares: a title
/// naming the project and the page, and a nav back to the project and home.
pub fn detail_head(project: &str, label: &str) -> String {
    let mut out = head(&format!("{project} / {label}"));
    out.push_str(&top_bar(&format!(
        "{} / {}",
        project_link(project),
        esc(label)
    )));
    out
}

/// The bar across the top of every page: the hub's mark, which is the way
/// home, then the page's heading. `heading` is markup, escaped by the caller.
pub fn top_bar(heading: &str) -> String {
    format!(
        "<header class=\"top\"><a class=\"brand\" href=\"/\">hub</a>\
         <h1>{heading}</h1></header>\n"
    )
}

/// The project's name, linked to its overview.
pub fn project_link(project: &str) -> String {
    format!(
        "<a href=\"{}\">{}</a>",
        esc(&project_url(project)),
        esc(project)
    )
}

pub fn degraded_banner(why: &str) -> String {
    format!(
        "<p class=\"banner degraded\">mem is not answering, so this page is \
         out of date: {}</p>\n",
        esc(why)
    )
}

pub fn markdown_article(text: Option<&str>, project: &str, empty: &str) -> String {
    match text {
        None => format!("<p class=\"empty\">{empty}</p>\n"),
        Some(text) => format!(
            "<article class=\"md\">\n{}</article>\n",
            markdown(text, project)
        ),
    }
}

/// Markdown to HTML, with everything that could execute left out.
///
/// A page is written by a session that has been reading repositories and web
/// pages, so it is exactly as trusted as a question is — and this origin owns
/// `POST /answer`. `push_html` on its own copies raw HTML through verbatim,
/// which would make a page the easiest way there is to put script on it. So:
///
/// * raw HTML, block and inline, becomes text and is escaped like any other;
/// * a link points at another page, at http(s), or at nothing — its text stays
///   either way, so a refused link is visible rather than silently gone;
/// * an image is dropped whole and its alt text kept. `<img>` is the one tag
///   that fetches from a third party without being asked, and a page is read on
///   a phone over the tailnet.
pub fn markdown(text: &str, project: &str) -> String {
    markdown_ids(text, project, &[])
}

/// `markdown`, with ids on headings: a heading whose text is a pair's first
/// gets that pair's second as its id, so a contents list or a search hit can
/// link to it. The text is the heading's source line without its `#`s, which
/// is how mem names a section, so a heading with `code` in it still matches.
/// Each pair is used once and in order, so a repeated heading takes the ids
/// mem numbered for it one after the other.
pub fn markdown_ids(text: &str, project: &str, ids: &[(String, String)]) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut events = Vec::new();
    // Links do not nest in CommonMark, so one flag is exact: it says whether
    // the link now open is one whose `Start` was kept.
    let mut link_open = false;
    let mut used = vec![false; ids.len()];
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading {
                level,
                id: _,
                classes,
                attrs,
            }) => {
                let source = text[range].lines().next().unwrap_or_default();
                let heading = source.trim_start().trim_start_matches('#').trim();
                let id = ids.iter().enumerate().find_map(|(n, (name, id))| {
                    (!used[n] && name == heading).then(|| {
                        used[n] = true;
                        id.clone().into()
                    })
                });
                events.push(Event::Start(Tag::Heading {
                    level,
                    id,
                    classes,
                    attrs,
                }));
            }
            // A block of raw HTML keeps its place in the flow, as a paragraph
            // of text saying what was written.
            Event::Start(Tag::HtmlBlock) => events.push(Event::Start(Tag::Paragraph)),
            Event::End(TagEnd::HtmlBlock) => events.push(Event::End(TagEnd::Paragraph)),
            Event::Html(raw) | Event::InlineHtml(raw) => events.push(Event::Text(raw)),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                link_open = destination(&dest_url, project).is_some_and(|dest| {
                    events.push(Event::Start(Tag::Link {
                        link_type,
                        dest_url: dest.into(),
                        title,
                        id,
                    }));
                    true
                });
            }
            // Dropped with its `Start`, so `push_html` never closes a tag it
            // did not open.
            Event::End(TagEnd::Link) => {
                if link_open {
                    events.push(Event::End(TagEnd::Link));
                }
                link_open = false;
            }
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            other => events.push(other),
        }
    }

    let mut out = String::with_capacity(text.len() + text.len() / 2);
    cmark::push_html(&mut out, events.into_iter());
    out
}

/// An item body renders pre-wrap like a question's text — status
/// and handoff both render this way.
fn body_block(text: Option<&str>, empty: &str) -> String {
    match text {
        Some(text) => format!("<p class=\"q\">{}</p>\n", esc(text)),
        None => format!("<p class=\"empty\">{empty}</p>\n"),
    }
}

/// Rulings and log both list `Activity` rows, each linked to its own item.
pub fn item_list_section(title: &str, rows: &[Activity], project: &str) -> String {
    let mut out = format!("<h2>{}</h2>\n", esc(title));
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">Nothing yet.</p>\n");
    } else {
        out.push_str("<ul>\n");
        for row in rows {
            out.push_str(&format!(
                "<li><a href=\"{href}\">{title}</a><div class=\"meta\">{age}</div></li>\n",
                href = esc(&item_url(project, &row.id)),
                title = esc(&row.title),
                age = esc(&row.age),
            ));
        }
        out.push_str("</ul>\n");
    }
    out
}

pub fn project_url(project: &str) -> String {
    format!("/p/{}", encode_component(project))
}

pub fn item_url(project: &str, id: &str) -> String {
    format!("{}/item/{}", project_url(project), encode_component(id))
}

/// Where a link in a page may point: another page of the same wiki, or the web.
/// `None` means the link is dropped and only its text is kept.
fn destination(dest: &str, project: &str) -> Option<String> {
    let dest = dest.trim();
    // The plan's rule: pages link to each other as `[name](name.md)`, standard
    // markdown, so every renderer works. Here that becomes the route.
    if let Some(slug) = dest.strip_suffix(".md")
        && is_slug(slug)
    {
        return Some(page_url(project, slug));
    }
    let scheme = dest.to_ascii_lowercase();
    let allowed = ["http://", "https://", "mailto:"];
    allowed
        .iter()
        .any(|prefix| scheme.starts_with(prefix))
        .then(|| dest.to_string())
}

pub fn page_url(project: &str, slug: &str) -> String {
    format!(
        "/wiki/{}/{}",
        encode_component(project),
        encode_component(slug)
    )
}

/// The head every page but the dashboard shares. No reload script: there is
/// nothing on these pages that goes stale while it is being read.
pub fn head(title: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>hub — {}</title>\n\
         <style>{STYLE}</style>\n</head>\n<body>\n",
        esc(title),
    )
}

/// A sibling link, escaped, and only if it is http(s). A `javascript:` URL in
/// one's own config is self-inflicted, but the check costs one line.
pub fn safe_link(url: &str) -> Option<String> {
    (url.starts_with("http://") || url.starts_with("https://")).then(|| esc(url))
}

pub fn label(url: &str) -> String {
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
    fn a_pages_own_html_is_text_and_never_markup() {
        let out = markdown(
            "<script>alert(1)</script>\n\nand <b onmouseover=\"x\">inline</b>\n",
            "p",
        );
        assert!(!out.contains("<script"), "{out}");
        assert!(!out.contains("<b "), "{out}");
        assert!(out.contains("&lt;script&gt;"), "{out}");
        assert!(out.contains("inline"), "the text survives: {out}");
    }

    #[test]
    fn a_refused_link_keeps_its_text_and_closes_nothing() {
        let out = markdown("[tap](javascript:alert(1)) and [go](https://x.test/)", "p");
        assert!(!out.contains("javascript:"), "{out}");
        assert!(!out.contains("</a> and"), "no tag was closed twice: {out}");
        assert!(out.contains("tap and"), "{out}");
        assert!(out.contains("<a href=\"https://x.test/\">go</a>"), "{out}");
        // An empty link is the case a flag gets wrong: nothing between the
        // start and the end to notice the start was dropped.
        assert_eq!(markdown("[](javascript:alert(1))", "p").trim(), "<p></p>");
    }

    #[test]
    fn an_image_is_dropped_and_its_alt_text_kept() {
        let out = markdown("![beacon](https://tracker.test/p.png)", "p");
        assert!(!out.contains("<img"), "{out}");
        assert!(out.contains("beacon"), "{out}");
    }

    #[test]
    fn a_link_to_another_page_becomes_a_route_and_anything_odd_becomes_nothing() {
        assert_eq!(
            destination("storage.md", "proj-alpha").as_deref(),
            Some("/wiki/proj-alpha/storage")
        );
        assert_eq!(
            destination("https://example.com/x", "p").as_deref(),
            Some("https://example.com/x")
        );
        assert_eq!(
            destination("mailto:x@y.test", "p").as_deref(),
            Some("mailto:x@y.test")
        );
        for dest in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html;base64,PHNjcmlwdD4=",
            "../../etc/passwd.md",
            "Storage.md",
            "file:///etc/passwd",
            "",
        ] {
            assert_eq!(destination(dest, "p"), None, "{dest}");
        }
    }

    #[test]
    fn a_heading_named_by_a_pair_takes_its_id_once_and_in_order() {
        let ids = [("Notes", "notes"), ("Notes", "notes-2"), ("`x` y", "x-y")]
            .map(|(name, id)| (name.to_string(), id.to_string()));
        let out = markdown_ids("# Title\n\n## Notes\n\n## Notes\n\n## `x` y\n", "p", &ids);
        assert!(out.contains("<h1>Title</h1>"), "{out}");
        let first = out.find("<h2 id=\"notes\">Notes</h2>").expect(&out);
        let second = out.find("<h2 id=\"notes-2\">Notes</h2>").expect(&out);
        assert!(first < second, "{out}");
        assert!(
            out.contains("<h2 id=\"x-y\"><code>x</code> y</h2>"),
            "{out}"
        );
    }

    #[test]
    fn a_project_name_is_encoded_into_the_route_it_becomes() {
        assert_eq!(page_url("a b", "s"), "/wiki/a%20b/s");
    }

    #[test]
    fn only_http_siblings_become_links() {
        assert!(safe_link("http://nuc:8787").is_some());
        assert!(safe_link("https://nuc:8787").is_some());
        assert!(safe_link("javascript:alert(1)").is_none());
        assert_eq!(label("http://nuc:8787/"), "nuc:8787");
    }
}
