//! `workflow switch` through the real binary, with scripts on PATH in the
//! place of amx, pgrep, qshell-sync, pass, hub, mem and systemctl. Every fake
//! appends `<name> <argv>` to one `calls.log`, so the order of calls across
//! them can be read back.

#[path = "../src/scratch.rs"]
mod scratch;

use std::os::unix::fs::PermissionsExt;
use std::process::Output;
use std::sync::RwLock;

use scratch::Scratch;

const STORE: &str = "wss://example.test/store/share";
const ADMIN: &str = "admin-SECRET-1";
const MINTED: &str = "minted-SECRET-2";
const CLOUD: &str = "cloud-SECRET-3";

/// A child forked while another test's fixture is still open for writing
/// holds that file open, and its exec then fails with "Text file busy". So
/// a fixture is written under the write lock and every spawn waits for it.
static FORKS: RwLock<()> = RwLock::new(());

const LOG: &str = r#"dir=$(dirname "$0")/..
echo "$(basename "$0") $*" >>"$dir/calls.log""#;

/// Every fake logs its argv; `hub share seed` exits with the next line of
/// seed-exits (0 once they run out) and records the store environment it saw.
fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.put("ls.json", "[]");
    work.put("pgrep.out", "");
    let log = LOG;
    work.script(
        "bin/amx",
        &format!("{log}\n[ \"$1\" = ls ] && cat \"$dir/ls.json\""),
    );
    // pgrep exits 1 when nothing matched, as the real one does.
    work.script(
        "bin/pgrep",
        &format!(
            "{log}\nout=$(cat \"$dir/pgrep.out\")\n[ -z \"$out\" ] && exit 1\nprintf '%s\\n' \"$out\" | sed \"s/PPID/$PPID/\""
        ),
    );
    // The marker the real qshell-sync tests for to skip its memory unit.
    work.script(
        "bin/qshell-sync",
        &format!("# [[ -e $XDG_STATE_HOME/mem/share/switched ]]\n{log}"),
    );
    work.script(
        "bin/pass",
        &format!(
            "{log}\ncase \"$2\" in\nworkflow/hubweb-store-token) echo {ADMIN} ;;\nworkflow/hub-cloud-token) echo {CLOUD} ;;\n*) exit 1 ;;\nesac"
        ),
    );
    work.script(
        "bin/hub",
        &format!(
            r#"{log}
echo "$2 url=$HUB_STORE_URL name=$HUB_STORE_NAME token=$HUB_STORE_TOKEN" >>"$dir/hub.env"
case "$2" in
mint) cat >"$dir/mint.stdin"; echo {MINTED} ;;
seed)
	code=$(head -n1 "$dir/seed-exits" 2>/dev/null)
	[ -f "$dir/seed-exits" ] && sed -i 1d "$dir/seed-exits"
	exit "${{code:-0}}"
	;;
esac"#
        ),
    );
    work.script("bin/mem", log);
    work.script("bin/systemctl", log);
    work.mkdir("home");
    work.mkdir("cwd");
    work
}

fn switch(work: &Scratch, store: &str) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["switch", "--store", store])
        .current_dir(work.at("cwd"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", work.at("bin").display()),
        )
        .env("HOME", work.at("home"))
        .env("XDG_CONFIG_HOME", work.at("config"))
        .env("WORKFLOW_MACHINE", "mini")
        .env_remove("WORKFLOW_AMX")
        .env_remove("WORKFLOW_MEM")
        .env_remove("HUB_STORE_URL")
        .env_remove("HUB_STORE_NAME")
        .env_remove("HUB_STORE_TOKEN")
        .output()
        .unwrap()
}

fn calls(work: &Scratch) -> String {
    if work.has("calls.log") {
        work.read("calls.log")
    } else {
        String::new()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_live_amx_agent_refuses_before_anything_runs() {
    let w = world("switch-amx");
    w.put(
        "ls.json",
        r#"[{"id":"shopify_themes-m3","state":"working","ended":0},
           {"id":"old-m1","state":"stopped","ended":0}]"#,
    );
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let said = text(&out);
    assert!(said.contains("shopify_themes-m3"), "{said}");
    assert!(!said.contains("old-m1"), "{said}");
    assert_eq!(calls(&w), "amx ls --json\npgrep -a -x claude\n");
    assert!(!w.has("config/hub/local.env"));
}

#[test]
fn a_parked_amx_agent_with_no_process_is_not_a_running_session() {
    let w = world("switch-parked");
    w.put(
        "ls.json",
        r#"[{"id":"dulali-m1-skeleton","state":"idle","ended":0,"evidence":"parked"}]"#,
    );
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(!text(&out).contains("dulali-m1-skeleton"), "{}", text(&out));
}

#[test]
fn a_running_claude_process_refuses_naming_its_pid_and_cwd() {
    let w = world("switch-claude");
    // PPID is the workflow process, so its cwd is the test's cwd dir.
    w.put("pgrep.out", "PPID claude remote-control --spawn worktree");
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let said = text(&out);
    let cwd = w.at("cwd").canonicalize().unwrap();
    let line = said
        .lines()
        .find(|l| l.contains("claude remote-control --spawn worktree"))
        .unwrap_or_else(|| panic!("{said}"));
    assert!(line.contains(&cwd.display().to_string()), "{line}");
    let pid = line
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .unwrap();
    assert!(pid.parse::<u32>().unwrap() > 1, "{line}");
    assert_eq!(calls(&w), "amx ls --json\npgrep -a -x claude\n");
}

#[test]
fn a_quiet_machine_switches_in_order_and_writes_local_env() {
    let w = world("switch-ok");
    w.put(
        "config/hub/local.env",
        "HUB_PORT=7777\nHUB_STORE_URL=ws://old/store/share\n",
    );
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        calls(&w),
        "amx ls --json
pgrep -a -x claude
qshell-sync --only memory
pass show workflow/hubweb-store-token
pass show workflow/hub-cloud-token
hub share mint mini
hub share seed
mem share switch
hub share seed
systemctl --user restart hub-local
"
    );
    assert_eq!(w.read("mint.stdin").trim(), ADMIN);
    assert_eq!(
        w.read("hub.env"),
        format!(
            "mint url={STORE} name=main token=\n\
             seed url={STORE} name=main token={MINTED}\n\
             seed url={STORE} name=main token={MINTED}\n"
        )
    );
    assert_eq!(
        w.read("config/hub/local.env"),
        format!(
            "HUB_PORT=7777\nHUB_STORE_URL={STORE}\nHUB_STORE_NAME=main\n\
             HUB_STORE_TOKEN={MINTED}\nHUB_CLOUD_TOKEN={CLOUD}\n"
        )
    );
    let mode = std::fs::metadata(w.at("config/hub/local.env"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let said = text(&out);
    let log = calls(&w);
    for secret in [ADMIN, MINTED, CLOUD] {
        assert!(!said.contains(secret), "{said}");
        assert!(!log.contains(secret), "{log}");
    }
    assert!(said.contains(STORE), "{said}");
}

#[test]
fn an_incomplete_first_seed_refuses_before_mem_switches() {
    let w = world("switch-budget");
    w.put("seed-exits", "3\n");
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let said = text(&out);
    assert!(
        said.contains("run workflow switch again after 06:00 Dhaka"),
        "{said}"
    );
    let log = calls(&w);
    assert!(log.ends_with("hub share seed\n"), "{log}");
    assert!(!log.contains("mem share switch"), "{log}");
    assert!(!w.has("config/hub/local.env"));
}

#[test]
fn a_store_url_not_ending_in_store_share_is_refused_first() {
    for bad in [
        "https://example.test/store/share",
        "wss://example.test/store",
        "example.test/store/share",
    ] {
        let w = world("switch-url");
        let out = switch(&w, bad);
        assert_eq!(out.status.code(), Some(2), "{bad}: {out:?}");
        assert_eq!(calls(&w), "", "{bad}");
    }
}

#[test]
fn a_qshell_sync_that_still_bisyncs_memory_refuses_before_anything_runs() {
    let w = world("switch-old-sync");
    {
        let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
        w.script("bin/qshell-sync", LOG);
    }
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        text(&out).contains(
            "qshell-sync here still bisyncs memory after a switch: update the dotfiles (chezmoi update) first"
        ),
        "{}",
        text(&out)
    );
    assert_eq!(calls(&w), "amx ls --json\npgrep -a -x claude\n");
    assert!(!w.has("config/hub/local.env"));
}
