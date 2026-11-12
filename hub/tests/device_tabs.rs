//! The tabs a `device browser` session opens, against a fake playwright-cli
//! that keeps Chromium's tab strip in a file. Like the Playwright extension,
//! the fake shows a session only the tabs in its own group: the one attach
//! opens, the ones tab-new opens, and any tab Saiful drags into the group.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

struct Rig {
    hub: Hub,
    home: PathBuf,
    state: PathBuf,
    /// One line per tab, in strip order: id, the session whose group holds
    /// it (`saiful` for none), 1 when the hub tagged it, its URL.
    tabs: PathBuf,
    /// The argv of every fake call, one line each.
    record: PathBuf,
    _dir: TempDir,
}

fn rig() -> Rig {
    rig_env(&[])
}

fn rig_env(env: &[(&str, &str)]) -> Rig {
    let dir = TempDir::new("device-tabs");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "nuc\n").unwrap();
    std::fs::create_dir_all(home.join("config/chromium")).unwrap();
    std::os::unix::fs::symlink(
        format!("testhost-{}", std::process::id()),
        home.join("config/chromium/SingletonLock"),
    )
    .unwrap();
    let tools = dir.join("tools");
    let bin = dir.join("bin");
    let state = dir.join("browser");
    std::fs::create_dir_all(&state).unwrap();
    let tabs = state.join("tabs");
    let record = state.join("record");
    std::fs::write(&tabs, "1\tsaiful\t0\thttps://his.test/\n").unwrap();
    std::fs::write(state.join("next"), "2\n").unwrap();
    fixture_bin(
        &bin,
        "systemctl",
        &format!("echo 'PATH={}:/usr/bin:/bin'", tools.display()),
    );
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "S='{state}'\n\
             case \"$1\" in\n\
             projects) cat \"$S/projects.json\" 2>/dev/null || echo '{{\"projects\":[]}}' ;;\n\
             handoff) cat \"$S/handoff.json\" 2>/dev/null || echo '{{}}' ;;\n\
             esac",
            state = state.display(),
        ),
    );
    fixture_bin(&tools, "playwright-extension-token", "echo fake-token");
    for starter in ["app-run", "systemd-run"] {
        fixture_bin(
            &tools,
            starter,
            &format!("echo {starter} >> '{}'", record.display()),
        );
    }
    fixture_bin(
        &tools,
        "playwright-cli",
        &format!(
            "S='{state}'\n\
             printf '%s\\n' \"$*\" >> \"$S/record\"\n\
             if [ \"$1\" = list ]; then\n\
               echo '### Browsers'\n\
               touch \"$S/sessions\"\n\
               while read -r s; do echo \"- $s:\"; done < \"$S/sessions\"\n\
               exit 0\n\
             fi\n\
             session=\"${{1#-s=}}\"; shift\n\
             edit() {{ awk -F'\\t' -v OFS='\\t' -v c=\"$(cat \"$S/current-$session\")\" \
                 -v s=\"$session\" -v u=\"$2\" \"$1\" \"$S/tabs\" > \"$S/t\" && mv \"$S/t\" \"$S/tabs\"; }}\n\
             new_tab() {{\n\
               id=$(cat \"$S/next\"); echo $((id + 1)) > \"$S/next\"\n\
               printf '%s\\t%s\\t0\\t%s\\n' \"$id\" \"$session\" \"$1\" >> \"$S/tabs\"\n\
               echo \"$id\" > \"$S/current-$session\"\n\
             }}\n\
             case \"$1\" in\n\
             attach) echo \"$session\" >> \"$S/sessions\"; new_tab about:blank; echo attached ;;\n\
             tab-new) new_tab \"${{2:-about:blank}}\"; echo opened ;;\n\
             goto) edit '$1 == c {{ $4 = u }} 1' \"$2\"; echo \"- Page URL: $2\" ;;\n\
             tab-select) awk -F'\\t' -v s=\"$session\" -v i=\"$2\" '$2 == s && n++ == i {{ print $1 }}' \"$S/tabs\" > \"$S/current-$session\"; echo selected ;;\n\
             tab-close)\n\
               if [ -n \"$2\" ]; then awk -F'\\t' -v s=\"$session\" -v i=\"$2\" '$2 == s && n++ == i {{ print $1 }}' \"$S/tabs\" > \"$S/current-$session\"; fi\n\
               edit '$1 != c'; echo closed ;;\n\
             run-code)\n\
               case \"$2\" in\n\
               *'pages()['*)\n\
                 i=$(printf '%s' \"$2\" | sed 's/.*pages()\\[\\([0-9]*\\)\\].*/\\1/')\n\
                 t=$(awk -F'\\t' -v s=\"$session\" -v i=\"$i\" '$2 == s && n++ == i {{ print $3 }}' \"$S/tabs\")\n\
                 echo '### Result'; [ \"$t\" = 1 ] && echo true || echo false ;;\n\
               *'!!page.__hubTab'*)\n\
                 t=$(awk -F'\\t' -v c=\"$(cat \"$S/current-$session\")\" '$1 == c {{ print $3 }}' \"$S/tabs\")\n\
                 echo '### Result'; [ \"$t\" = 1 ] && echo true || echo false ;;\n\
               *'__hubTab = true'*) edit '$1 == c {{ $3 = 1 }} 1' ;;\n\
               *'.close()'*)\n\
                 edit '!($2 == s && $3 == 1)'\n\
                 if ! awk -F'\\t' -v s=\"$session\" '$2 == s {{ f = 1 }} END {{ exit !f }}' \"$S/tabs\"; then\n\
                   grep -vx \"$session\" \"$S/sessions\" > \"$S/t\"; mv \"$S/t\" \"$S/sessions\"\n\
                   echo 'Session closed'\n\
                 fi ;;\n\
               *) echo \"fake: unknown code: $2\" >&2; exit 1 ;;\n\
               esac ;;\n\
             detach)\n\
               if grep -qx \"$session\" \"$S/sessions\"; then\n\
                 grep -vx \"$session\" \"$S/sessions\" > \"$S/t\"; mv \"$S/t\" \"$S/sessions\"\n\
                 echo detached\n\
               else echo \"Browser '$session' is not attached.\"; fi ;;\n\
             *) echo \"ran $*\" ;;\n\
             esac",
            state = state.display(),
        ),
    );
    let socket = dir.join("local.sock");
    let mut all = vec![
        ("HUB_PEER_TOKEN", TOKEN),
        ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
    ];
    all.extend_from_slice(env);
    let hub = Hub::spawn_env(&home, &[&bin], &["local", "--port", "0"], &all);
    Rig {
        hub,
        home,
        state,
        tabs,
        record,
        _dir: dir,
    }
}

impl Rig {
    fn browser(&self, session: &str, args: &[&str]) -> serde_json::Value {
        let answer = self.call(session, args);
        assert_eq!(answer["exit"], 0, "{args:?}: {answer}");
        answer
    }

    fn call(&self, session: &str, args: &[&str]) -> serde_json::Value {
        let body = serde_json::json!({
            "verb": "browser", "args": args, "project": "demo", "session": session,
        })
        .to_string();
        let response = self.hub.raw(&format!(
            "POST /device/here HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
             Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert_eq!(status_of(&response), 200, "{response}");
        serde_json::from_str(body_of(&response)).unwrap()
    }

    /// The tab strip, as (group, URL) in order.
    fn strip(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(&self.tabs)
            .unwrap()
            .lines()
            .map(|line| {
                let f: Vec<&str> = line.split('\t').collect();
                (f[1].to_string(), f[3].to_string())
            })
            .collect()
    }

    /// Adds a tab to the strip the way Saiful does, into `group`.
    fn his_tab(&self, group: &str, url: &str) {
        let mut text = std::fs::read_to_string(&self.tabs).unwrap();
        let id = 900 + text.lines().count();
        text.push_str(&format!("{id}\t{group}\t0\t{url}\n"));
        std::fs::write(&self.tabs, text).unwrap();
    }

    /// Closes the strip's tab that shows `url`, as Saiful does by hand.
    fn close_by_hand(&self, url: &str) {
        let text = std::fs::read_to_string(&self.tabs).unwrap();
        let kept: String = text
            .lines()
            .filter(|line| !line.ends_with(&format!("\t{url}")))
            .map(|line| format!("{line}\n"))
            .collect();
        std::fs::write(&self.tabs, kept).unwrap();
    }

    /// Points Chromium's lock at a pid that is not running, as after Saiful
    /// quit it.
    fn quit_chromium(&self) {
        let lock = self.home.join("config/chromium/SingletonLock");
        std::fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink("testhost-999999999", &lock).unwrap();
    }

    /// Project demo, building `milestone`, run in the cloud on m2 by
    /// session_01Abc since a minute ago; its latest handoff, `handoff`, was
    /// written now, or never when it is None.
    fn cloud_run(&self, milestone: &str, handoff: Option<&str>) {
        let now = jiff::Timestamp::now();
        let since = now - jiff::SignedDuration::from_secs(60);
        let handoff_at = handoff.map_or(serde_json::Value::Null, |_| now.to_string().into());
        let projects = serde_json::json!({"projects": [
            {"name": "demo", "roadmap_status": "approved", "checkouts": [],
             "milestone": milestone, "milestones_done": 1, "milestones_total": 3,
             "handoff_at": handoff_at,
             "runner": {"machine": "cloud", "id": "session_01Abc",
                        "since": since.to_string(), "milestone": "m2"}},
        ]});
        std::fs::write(self.state.join("projects.json"), projects.to_string()).unwrap();
        let body = serde_json::json!({ "body": handoff.unwrap_or_default() });
        std::fs::write(self.state.join("handoff.json"), body.to_string()).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn tab(group: &str, url: &str) -> (String, String) {
    (group.to_string(), url.to_string())
}

#[test]
fn a_session_s_first_goto_loads_in_a_tab_of_its_own() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://example.com"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://example.com")
        ]
    );
}

#[test]
fn detach_closes_the_two_tabs_the_session_opened() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.browser("s1", &["tab-new", "https://b.test"]);
    let answer = rig.browser("s1", &["detach"]);
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
    assert_eq!(answer["stdout"], "Browser 's1' is not attached.\n");
}

#[test]
fn detach_leaves_a_tab_dragged_into_the_session_s_group() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.his_tab("s1", "https://dragged.test/");
    rig.browser("s1", &["tab-new", "https://b.test"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://a.test"),
            tab("s1", "https://dragged.test/"),
            tab("s1", "https://b.test"),
        ]
    );
    rig.browser("s1", &["detach"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://dragged.test/"),
        ]
    );
}

#[test]
fn detach_after_saiful_closed_a_session_tab_closes_only_the_rest() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.browser("s1", &["tab-new", "https://b.test"]);
    rig.close_by_hand("https://b.test");
    rig.browser("s1", &["detach"]);
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
}

#[test]
fn detaching_one_session_leaves_the_other_s_tab() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.browser("s2", &["goto", "https://b.test"]);
    rig.browser("s1", &["detach"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s2", "https://b.test")
        ]
    );
}

#[test]
fn detach_of_a_session_that_never_attached_opens_nothing() {
    let rig = rig();
    rig.browser("s1", &["detach"]);
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
    assert_eq!(rig.calls(), vec!["list", "-s=s1 detach"]);
}

#[test]
fn a_session_idle_past_the_limit_loses_its_tabs() {
    let rig = rig_env(&[("HUB_DEVICE_IDLE_MS", "1000")]);
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.browser("s1", &["tab-new", "https://b.test"]);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.strip().len(), 3, "{:?}", rig.strip());
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
    assert_eq!(rig.calls().last().unwrap(), "-s=s1 detach");
}

#[test]
fn a_session_that_keeps_calling_keeps_its_tab() {
    let rig = rig_env(&[("HUB_DEVICE_IDLE_MS", "1500")]);
    rig.browser("s1", &["goto", "https://a.test"]);
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(500));
        rig.browser("s1", &["snapshot"]);
    }
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://a.test")
        ]
    );
}

#[test]
fn the_idle_sweep_starts_no_browser_when_chromium_is_down() {
    let rig = rig_env(&[("HUB_DEVICE_IDLE_MS", "1000")]);
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.quit_chromium();
    let before = rig.calls().len();
    std::thread::sleep(Duration::from_secs(3));
    let calls = rig.calls();
    assert_eq!(calls[before..], ["list", "-s=s1 detach"], "{calls:?}");
}

/// The sweep runs every 200 ms, and no session goes idle in these tests.
const SWEEP: [(&str, &str); 1] = [("HUB_DEVICE_SWEEP_MS", "200")];

#[test]
fn a_cloud_run_s_tabs_close_once_it_writes_a_handoff() {
    let rig = rig_env(&SWEEP);
    rig.cloud_run("m2", None);
    rig.browser("cse_01Abc", &["goto", "https://a.test"]);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(rig.strip().len(), 2, "{:?}", rig.strip());
    rig.cloud_run("m2", Some("parked: waiting on q1"));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
}

#[test]
fn a_cloud_run_s_tabs_close_once_its_milestone_is_ticked() {
    let rig = rig_env(&SWEEP);
    rig.cloud_run("m2", None);
    rig.browser("cse_01Abc", &["goto", "https://a.test"]);
    rig.cloud_run("m3", None);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
}

#[test]
fn a_local_walk_s_tabs_outlast_the_run_s_handoff() {
    let rig = rig_env(&SWEEP);
    rig.cloud_run("m2", None);
    rig.browser("local-mini", &["goto", "https://a.test"]);
    rig.cloud_run("m3", Some("parked: waiting on q1"));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("local-mini", "https://a.test")
        ]
    );
}

/// s1's own tab at 0, a tab Saiful dragged into its group at 1, and s1's
/// second tab, current, at 2.
fn with_a_dragged_tab() -> Rig {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.his_tab("s1", "https://dragged.test/");
    rig.browser("s1", &["tab-new", "https://b.test"]);
    rig
}

#[test]
fn tab_select_of_a_tab_the_session_did_not_open_is_refused() {
    let rig = with_a_dragged_tab();
    let answer = rig.call("s1", &["tab-select", "1"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: tab 1 is not one this session opened\n"
    );
    assert!(!rig.calls().iter().any(|c| c == "-s=s1 tab-select 1"));
    rig.browser("s1", &["tab-select", "0"]);
    rig.browser("s1", &["goto", "https://c.test"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://c.test"),
            tab("s1", "https://dragged.test/"),
            tab("s1", "https://b.test"),
        ]
    );
}

#[test]
fn tab_close_of_a_tab_the_session_did_not_open_is_refused() {
    let rig = with_a_dragged_tab();
    let answer = rig.call("s1", &["tab-close", "1"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    rig.browser("s1", &["tab-close"]);
    rig.browser("s1", &["tab-close", "0"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://dragged.test/"),
        ]
    );
}

#[test]
fn a_tab_number_that_is_not_one_is_refused() {
    let rig = with_a_dragged_tab();
    let answer = rig.call("s1", &["tab-select", "1; process.exit()"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: tab-select needs a tab number, not 1; process.exit()\n"
    );
}
