//! Home's reload script, run in Node against a fake page: it reloads when
//! the stamp moved, and never while a field has focus or text, or text is
//! selected, so a reload never lands under an answer being written.

mod common;

use std::process::Command;

use common::TempDir;
use hub::agents::{Agents, DEFAULT_WINDOW};
use hub::config::Config;
use hub::html::Banner;
use hub::memcli::MemCli;
use hub::page_home;
use hub::route::Route;

/// The script home draws, taken from the page.
fn script(dir: &TempDir) -> String {
    let mem = MemCli::with_path(dir.join("no-mem-here"));
    let agents = Agents::open(dir.join("agents.json"), DEFAULT_WINDOW);
    let route = Route::open(dir.join("route.json"));
    let page = page_home::render(
        &mem,
        &agents,
        &route,
        &Config::default(),
        "here",
        0,
        &Banner::None,
        None,
        &[],
    );
    let at = page.find("/api/stamp").expect("home polls the stamp");
    let start = page[..at].rfind("<script>").unwrap() + "<script>".len();
    let end = at + page[at..].find("</script>").unwrap();
    page[start..end].to_string()
}

/// Each case's page state, and whether one tick of the script reloads it.
/// `same` answers the stamp the page was drawn from; `moved` another one.
const HARNESS: &str = r#"
const vm = require("vm");
const src = require("fs").readFileSync(process.argv[2], "utf8");
const stampOf = src.match(/var s="([^"]*)"/)[1];
function tick(c) {
  return new Promise((done) => {
    let reloaded = false, ms = 0, f;
    const fields = c.fields || [];
    const ctx = {
      String,
      document: {
        activeElement: { tagName: c.focus || "BODY" },
        querySelectorAll: (sel) =>
          fields.filter((x) => !(sel.includes(":not([type=hidden])") && x.type === "hidden")),
      },
      window: { getSelection: () => c.selection || "" },
      fetch: () => Promise.resolve({ json: () => Promise.resolve({ stamp: c.stamp === "same" ? stampOf : "moved" }) }),
      setInterval: (fn, every) => { f = fn; ms = every; },
      location: { reload: () => { reloaded = true; } },
    };
    vm.runInNewContext(src, ctx);
    f();
    setTimeout(() => done({ name: c.name, reloaded, ms }), 20);
  });
}
const cases = [
  { name: "nothing changed", stamp: "same" },
  { name: "a change, idle", stamp: "moved" },
  { name: "a change, only hidden fields hold values", stamp: "moved", fields: [{ type: "hidden", value: "01K0Q" }] },
  { name: "a change, focus in the answer box", stamp: "moved", focus: "TEXTAREA" },
  { name: "a change, a word selected", stamp: "moved", selection: "Which" },
  { name: "a change, an answer typed and left", stamp: "moved", fields: [{ type: "textarea", value: "nine" }] },
];
Promise.all(cases.map(tick)).then((r) => console.log(JSON.stringify(r)));
"#;

#[test]
fn home_reloads_on_a_change_and_never_under_the_cursor() {
    let dir = TempDir::new("home-reload");
    std::fs::write(dir.join("reload.js"), script(&dir)).unwrap();
    std::fs::write(dir.join("harness.js"), HARNESS).unwrap();
    let out = Command::new("node")
        .arg(dir.join("harness.js"))
        .arg(dir.join("reload.js"))
        .output()
        .expect("node runs the page's script; put it on PATH");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let results: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let got: Vec<(String, bool, u64)> = results
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_string(),
                r["reloaded"].as_bool().unwrap(),
                r["ms"].as_u64().unwrap(),
            )
        })
        .collect();
    let want = [
        ("nothing changed", false),
        ("a change, idle", true),
        ("a change, only hidden fields hold values", true),
        ("a change, focus in the answer box", false),
        ("a change, a word selected", false),
        ("a change, an answer typed and left", false),
    ];
    for ((name, reloaded, ms), (want_name, want_reload)) in got.iter().zip(want) {
        assert_eq!((name.as_str(), *reloaded), (want_name, want_reload));
        assert_eq!(*ms, 5_000, "every 5 s");
    }
    assert_eq!(got.len(), want.len());
}
