//! The tabs a `device browser` session opens, against a fake playwright-cli
//! that keeps Chromium's tab strip in a file. Like the Playwright extension,
//! the fake shows a session only the tabs in its own group: the one attach
//! opens, the ones tab-new opens, and any tab Saiful drags into the group.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

struct Rig {
    hub: Hub,
    /// One line per tab, in strip order: id, the session whose group holds
    /// it (`saiful` for none), 1 when the hub tagged it, its URL.
    tabs: PathBuf,
    /// The argv of every fake call, one line each.
    record: PathBuf,
    _dir: TempDir,
}

fn rig() -> Rig {
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
    fixture_bin(&bin, "mem", "exit 0");
    fixture_bin(&tools, "playwright-extension-token", "echo fake-token");
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
             run-code)\n\
               case \"$2\" in\n\
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
    let hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
        ],
    );
    Rig {
        hub,
        tabs,
        record,
        _dir: dir,
    }
}

impl Rig {
    fn browser(&self, session: &str, args: &[&str]) -> serde_json::Value {
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
        let answer: serde_json::Value = serde_json::from_str(body_of(&response)).unwrap();
        assert_eq!(answer["exit"], 0, "{args:?}: {answer}");
        answer
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
