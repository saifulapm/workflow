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
const ACCESS_ID: &str = "access-id-SECRET-4";
const ACCESS_SECRET: &str = "access-secret-SECRET-5";

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
            "{log}\ncase \"$2\" in\nworkflow/hubweb-store-token) echo {ADMIN} ;;\nworkflow/hub-cloud-token) echo {CLOUD} ;;\nworkflow/cloud-next-access-id) echo {ACCESS_ID} ;;\nworkflow/cloud-next-access-secret) echo {ACCESS_SECRET} ;;\n*) exit 1 ;;\nesac"
        ),
    );
    work.script(
        "bin/hub",
        &format!(
            r#"{log}
echo "$2 url=$HUB_STORE_URL name=$HUB_STORE_NAME token=$HUB_STORE_TOKEN access=${{HUB_STORE_ACCESS_ID:+set}}${{HUB_STORE_ACCESS_SECRET:+set}}" >>"$dir/hub.env"
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
    // The name mem writes as, which the minted token carries.
    work.put("config/qshell/machine", "mac-mini-m2\n");
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
        .env("XDG_STATE_HOME", work.at("state"))
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
fn running_sessions_and_an_old_qshell_sync_do_not_stop_a_switch() {
    // chezmoi apply runs it beside the Remote Control servers, and the
    // store's second seed carries a write a session makes meanwhile.
    let w = world("switch-busy");
    w.put(
        "ls.json",
        r#"[{"id":"shopify_themes-m3","state":"working","ended":0}]"#,
    );
    w.put("pgrep.out", "PPID claude remote-control --spawn worktree");
    {
        let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
        w.script("bin/qshell-sync", LOG);
    }
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let log = calls(&w);
    assert!(log.starts_with("pass show"), "{log}");
    for never in ["amx", "pgrep", "qshell-sync"] {
        assert!(!log.contains(never), "{log}");
    }
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
        "pass show workflow/hubweb-store-token
pass show workflow/hub-cloud-token
pass show workflow/cloud-next-access-id
pass show workflow/cloud-next-access-secret
hub share mint mini mac-mini-m2
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
            "mint url={STORE} name=main token= access=setset\n\
             seed url={STORE} name=main token={ADMIN} access=setset\n\
             seed url={STORE} name=main token={ADMIN} access=setset\n"
        )
    );
    assert_eq!(
        w.read("config/hub/local.env"),
        format!(
            "HUB_PORT=7777\nHUB_STORE_URL={STORE}\nHUB_STORE_NAME=main\n\
             HUB_STORE_TOKEN={MINTED}\nHUB_CLOUD_TOKEN={CLOUD}\n\
             HUB_STORE_ACCESS_ID={ACCESS_ID}\nHUB_STORE_ACCESS_SECRET={ACCESS_SECRET}\n"
        )
    );
    let mode = std::fs::metadata(w.at("config/hub/local.env"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let said = text(&out);
    let log = calls(&w);
    for secret in [ADMIN, MINTED, CLOUD, ACCESS_ID, ACCESS_SECRET] {
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
fn a_rerun_on_a_switched_machine_moves_the_store_url_and_mints_its_token_again() {
    let w = world("switch-rerun");
    w.put("state/mem/share/switched", "");
    w.put(
        "config/hub/local.env",
        "HUB_PORT=7777\nHUB_STORE_URL=ws://old/store/share\nHUB_STORE_NAME=main\n\
         HUB_STORE_TOKEN=old-token\nHUB_CLOUD_TOKEN=old-cloud\n",
    );
    let new = "wss://new.test/store/share";
    let out = switch(&w, new);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    // A token minted before tokens named a machine writes nothing.
    assert_eq!(
        calls(&w),
        "pass show workflow/hubweb-store-token\n\
         pass show workflow/cloud-next-access-id\n\
         pass show workflow/cloud-next-access-secret\n\
         hub share mint mini mac-mini-m2\n\
         systemctl --user restart hub-local\n"
    );
    assert_eq!(w.read("mint.stdin").trim(), ADMIN);
    assert_eq!(
        w.read("hub.env"),
        format!("mint url={new} name=main token= access=setset\n")
    );
    assert_eq!(
        w.read("config/hub/local.env"),
        format!(
            "HUB_PORT=7777\nHUB_STORE_URL={new}\nHUB_STORE_NAME=main\n\
             HUB_STORE_TOKEN={MINTED}\nHUB_CLOUD_TOKEN=old-cloud\n\
             HUB_STORE_ACCESS_ID={ACCESS_ID}\nHUB_STORE_ACCESS_SECRET={ACCESS_SECRET}\n"
        )
    );
}

#[test]
fn a_rerun_whose_mint_fails_changes_nothing() {
    let w = world("switch-rerun-nomint");
    w.put("state/mem/share/switched", "");
    let before = "HUB_PORT=7777\nHUB_STORE_URL=ws://old/store/share\nHUB_STORE_TOKEN=old-token\n";
    w.put("config/hub/local.env", before);
    w.script("bin/hub", "exit 1");
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(w.read("config/hub/local.env"), before);
    assert!(!calls(&w).contains("systemctl"));
}

#[test]
fn a_failed_second_seed_warns_and_the_switch_still_lands() {
    let w = world("switch-second-seed");
    w.put("seed-exits", "0\n1\n");
    let out = switch(&w, STORE);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("second hub share seed exited 1"),
        "{stderr}"
    );
    assert!(calls(&w).ends_with("systemctl --user restart hub-local\n"));
    assert!(w.read("config/hub/local.env").contains(MINTED));
}
