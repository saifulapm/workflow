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
    /// The strip fake_tabs_cli.sh keeps, one line per tab in strip order.
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
    std::fs::write(&tabs, "1\tsaiful\t0\thttps://his.test/\t0\n").unwrap();
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
            "S='{}' exec sh '{}/tests/fake_tabs_cli.sh' \"$@\"",
            state.display(),
            env!("CARGO_MANIFEST_DIR"),
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

    /// The tab strip, as (who opened it, URL) in order, without the tab
    /// the hub keeps for its own connection (hub_tabs).
    fn strip(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(&self.tabs)
            .unwrap()
            .lines()
            .map(|line| {
                let f: Vec<&str> = line.split('\t').collect();
                (f[1].to_string(), f[3].to_string())
            })
            .filter(|(who, _)| who != "hub.connection")
            .collect()
    }

    /// Adds a tab to the strip the way Saiful does, into `group`.
    fn his_tab(&self, group: &str, url: &str) {
        let mut text = std::fs::read_to_string(&self.tabs).unwrap();
        let id = 900 + text.lines().count();
        text.push_str(&format!("{id}\t{group}\t0\t{url}\t0\n"));
        std::fs::write(&self.tabs, text).unwrap();
    }

    /// Opens `url` from `session`'s current tab, as a link with
    /// target=_blank does; the extension adds it to the session's group.
    fn popup(&self, session: &str, url: &str) {
        let opener =
            std::fs::read_to_string(self.state.join(format!("current-{session}"))).unwrap();
        let mut text = std::fs::read_to_string(&self.tabs).unwrap();
        let id = 900 + text.lines().count();
        text.push_str(&format!("{id}\t{session}\t0\t{url}\t{}\n", opener.trim()));
        std::fs::write(&self.tabs, text).unwrap();
    }

    /// The extension's token cannot be read, so attach shows the connect
    /// page and Saiful picks one of his tabs.
    fn no_token(&self) {
        let helper = self.home.join("../tools/playwright-extension-token");
        std::fs::write(&helper, "#!/bin/sh\nexit 1\n").unwrap();
    }

    /// While `on`, the hub's run-code that closes tabs fails, as it does
    /// while a dialog is open.
    fn closing_fails(&self, on: bool) {
        let flag = self.state.join("close-fails");
        if on {
            std::fs::write(flag, "").unwrap();
        } else {
            std::fs::remove_file(flag).unwrap();
        }
    }

    /// Closes the strip's tab that shows `url`, as Saiful does by hand.
    fn close_by_hand(&self, url: &str) {
        let text = std::fs::read_to_string(&self.tabs).unwrap();
        let kept: String = text
            .lines()
            .filter(|line| line.split('\t').nth(3) != Some(url))
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
    assert_eq!(answer["stdout"], "Browser 's1' detached\n\n");
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

/// The hub's own tab at 0, s1's own tab at 1, a tab Saiful dragged into the
/// group at 2, and s1's second tab, current, at 3.
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
    let answer = rig.call("s1", &["tab-select", "2"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: tab 2 is not one this session opened\n"
    );
    assert!(!rig.calls().iter().any(|c| c == "-s=s1 tab-select 2"));
    rig.browser("s1", &["tab-select", "1"]);
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
    let answer = rig.call("s1", &["tab-close", "2"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    rig.browser("s1", &["tab-close"]);
    rig.browser("s1", &["tab-close", "1"]);
    // Its last tab gone, the session gets a blank one rather than work on
    // in the dragged tab.
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://dragged.test/"),
            tab("s1", "about:blank"),
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

#[test]
fn after_its_own_tab_close_the_session_works_in_its_other_tab() {
    let rig = with_a_dragged_tab();
    // b was current; playwright-core makes the dragged tab, now at b's
    // place, current.
    rig.browser("s1", &["tab-close"]);
    rig.browser("s1", &["goto", "https://c.test"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://c.test"),
            tab("s1", "https://dragged.test/"),
        ]
    );
}

#[test]
fn closing_its_last_tab_beside_a_dragged_one_gives_the_session_a_blank_tab() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.his_tab("s1", "https://dragged.test/");
    rig.browser("s1", &["tab-close"]);
    rig.browser("s1", &["goto", "https://c.test"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://dragged.test/"),
            tab("s1", "https://c.test"),
        ]
    );
}

#[test]
fn a_tab_the_session_s_click_opened_is_its_own() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.popup("s1", "https://pop.test/");
    rig.browser("s1", &["tab-select", "2"]);
    rig.browser("s1", &["detach"]);
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
}

#[test]
fn the_tab_saiful_picks_for_an_attach_with_no_token_is_never_closed() {
    let rig = rig();
    rig.no_token();
    rig.browser("s1", &["goto", "https://x.test"]);
    rig.browser("s1", &["tab-new", "https://b.test"]);
    rig.browser("s1", &["detach"]);
    assert_eq!(rig.strip(), vec![tab("s1", "https://x.test")]);
}

#[test]
fn a_tab_new_whose_page_fails_to_load_is_still_closed_on_detach() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    let answer = rig.call("s1", &["tab-new", "https://typo.invalid"]);
    assert_eq!(answer["exit"], 1, "{answer}");
    rig.browser("s1", &["detach"]);
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
}

#[test]
fn a_detach_whose_close_fails_keeps_the_session_for_the_idle_sweep() {
    let rig = rig_env(&[("HUB_DEVICE_IDLE_MS", "1000")]);
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.closing_fails(true);
    let answer = rig.call("s1", &["detach"]);
    assert_eq!(answer["exit"], 1, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: closing the session's tabs failed, so it stays attached: \
         Error: a modal dialog is open\n"
    );
    assert!(!rig.calls().iter().any(|c| c == "-s=s1 detach"));
    rig.closing_fails(false);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
}

/// How many times the hub attached through the extension: each one opens a
/// connect tab in Chromium.
fn connect_tabs(rig: &Rig) -> usize {
    rig.calls()
        .iter()
        .filter(|c| c.ends_with(" attach --extension=chromium"))
        .count()
}

/// The tabs of the hub's own connection session, which no walk opened.
fn hub_tabs(rig: &Rig) -> Vec<String> {
    std::fs::read_to_string(&rig.tabs)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            (f[1] == "hub.connection").then(|| f[3].to_string())
        })
        .collect()
}

#[test]
fn three_sessions_one_after_another_open_one_connect_tab() {
    let rig = rig();
    for (session, url) in [
        ("s1", "https://a.test"),
        ("s2", "https://b.test"),
        ("s3", "https://c.test"),
    ] {
        rig.browser(session, &["goto", url]);
        rig.browser(session, &["detach"]);
    }
    assert_eq!(connect_tabs(&rig), 1, "{:?}", rig.calls());
    assert_eq!(rig.strip(), vec![tab("saiful", "https://his.test/")]);
    assert_eq!(hub_tabs(&rig), ["about:blank"]);
}

#[test]
fn two_sessions_at_once_open_one_connect_tab() {
    let rig = rig();
    std::thread::scope(|scope| {
        scope.spawn(|| rig.browser("s1", &["goto", "https://a.test"]));
        scope.spawn(|| rig.browser("s2", &["goto", "https://b.test"]));
    });
    assert_eq!(connect_tabs(&rig), 1, "{:?}", rig.calls());
    let mut strip = rig.strip();
    strip.sort();
    assert_eq!(
        strip,
        vec![
            tab("s1", "https://a.test"),
            tab("s2", "https://b.test"),
            tab("saiful", "https://his.test/"),
        ]
    );
}

#[test]
fn a_session_that_closes_its_last_tab_leaves_the_connection_up() {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.browser("s1", &["tab-close"]);
    rig.browser("s1", &["detach"]);
    rig.browser("s2", &["goto", "https://b.test"]);
    assert_eq!(connect_tabs(&rig), 1, "{:?}", rig.calls());
    assert_eq!(hub_tabs(&rig), ["about:blank"]);
}

/// s1's tab a.test at 1, s2's b.test at 2 and s1's c.test, current, at 3,
/// after the hub's own tab at 0.
fn two_sessions() -> Rig {
    let rig = rig();
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.browser("s2", &["goto", "https://b.test"]);
    rig.browser("s1", &["tab-new", "https://c.test"]);
    rig
}

#[test]
fn a_session_s_tab_list_shows_its_own_tabs_only() {
    let rig = two_sessions();
    let answer = rig.browser("s1", &["tab-list"]);
    assert_eq!(
        answer["stdout"],
        "### Result\n- 1: [](https://a.test)\n- 3: (current) [](https://c.test)\n"
    );
}

#[test]
fn a_command_s_open_tabs_show_no_other_session_s_tab() {
    let rig = two_sessions();
    let answer = rig.browser("s2", &["goto", "https://b2.test"]);
    assert_eq!(
        answer["stdout"],
        "### Open tabs\n- 2: (current) [](https://b2.test)\n### Page\n- Page URL: https://b2.test\n"
    );
}

#[test]
fn tab_select_and_tab_close_of_another_session_s_tab_are_refused() {
    let rig = two_sessions();
    for command in ["tab-select", "tab-close"] {
        let answer = rig.call("s1", &[command, "2"]);
        assert_eq!(answer["exit"], 2, "{answer}");
        assert_eq!(
            answer["stderr"],
            "device: tab 2 is not one this session opened\n"
        );
    }
    rig.browser("s1", &["goto", "https://d.test"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://a.test"),
            tab("s2", "https://b.test"),
            tab("s1", "https://d.test"),
        ]
    );
}

#[test]
fn a_session_s_detach_leaves_the_other_working() {
    let rig = two_sessions();
    rig.browser("s1", &["detach"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s2", "https://b.test"),
        ]
    );
    rig.browser("s2", &["goto", "https://b2.test"]);
}

#[test]
fn an_idle_session_s_close_leaves_a_busy_one_s_tab() {
    let rig = rig_env(&[("HUB_DEVICE_IDLE_MS", "1500")]);
    rig.browser("s1", &["goto", "https://a.test"]);
    rig.browser("s2", &["goto", "https://b.test"]);
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(500));
        rig.browser("s2", &["snapshot"]);
    }
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s2", "https://b.test"),
        ]
    );
}

#[test]
fn a_cloud_run_s_close_leaves_a_local_walk_working() {
    let rig = rig_env(&SWEEP);
    rig.cloud_run("m2", None);
    rig.browser("cse_01Abc", &["goto", "https://a.test"]);
    rig.browser("local-mini", &["goto", "https://b.test"]);
    rig.cloud_run("m2", Some("parked: waiting on q1"));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("local-mini", "https://b.test"),
        ]
    );
    rig.browser("local-mini", &["goto", "https://b2.test"]);
}

#[test]
fn a_session_whose_tab_saiful_closed_works_on_in_its_own_not_the_neighbour() {
    let rig = two_sessions();
    rig.close_by_hand("https://c.test");
    // playwright-core makes the tab at the closed one's place current: s2's.
    let b = std::fs::read_to_string(&rig.tabs)
        .unwrap()
        .lines()
        .find(|line| line.ends_with("\thttps://b.test\t0"))
        .map(|line| line.split('\t').next().unwrap().to_string())
        .unwrap();
    std::fs::write(rig.state.join("current-s1"), b).unwrap();
    rig.browser("s1", &["goto", "https://d.test"]);
    assert_eq!(
        rig.strip(),
        vec![
            tab("saiful", "https://his.test/"),
            tab("s1", "https://d.test"),
            tab("s2", "https://b.test"),
        ]
    );
}

#[test]
fn a_session_with_no_token_works_on_in_the_tab_saiful_picked() {
    let rig = rig();
    rig.no_token();
    rig.browser("s1", &["goto", "https://x.test"]);
    rig.browser("s1", &["goto", "https://y.test"]);
    assert_eq!(rig.strip(), vec![tab("s1", "https://y.test")]);
}
