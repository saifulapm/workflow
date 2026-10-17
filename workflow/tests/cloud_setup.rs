//! `workflow cloud-setup`: the cloud environment's setup script, run here by
//! bash with `$HOME` and the shim's folder pointed at a scratch directory.

#[path = "../src/scratch.rs"]
mod scratch;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use scratch::Scratch;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// A command that sees none of the cloud's variables from the shell running
/// the tests: a session started by the hub carries the mini's `HUB_*` keys.
fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        // A cloud VM sets no XDG dirs: the shim must find the scratch home's.
        if k.starts_with("HUB_") || k.starts_with("XDG_") || k == "TS_AUTHKEY" || k == "MEM_REMOTE"
        {
            cmd.env_remove(&*k);
        }
    }
    cmd
}

/// The printed script, its shim folder moved into the scratch directory.
fn script(home: &Scratch) -> String {
    let out = command(env!("CARGO_BIN_EXE_workflow"))
        .arg("cloud-setup")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("#!/bin/bash\n"), "{text}");
    let line = "bin=/usr/local/bin\n";
    assert!(text.contains(line), "the shim's folder is one line: {text}");
    text.replace(
        line,
        &format!("bin={}\n", home.mkdir("usr-local-bin").display()),
    )
}

#[test]
fn the_script_writes_every_skill_and_agent_byte_for_byte() {
    let home = Scratch::new("cloud-setup-files");
    home.put("setup.sh", &script(&home));
    let ran = command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");

    let mut seen = 0;
    for top in ["skills", "agents"] {
        let mut stack = vec![repo().join(top)];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path.strip_prefix(repo()).unwrap();
                let landed = home.at(&format!(".claude/{}", rel.display()));
                assert_eq!(
                    fs::read(&path).unwrap(),
                    fs::read(&landed).unwrap_or_default(),
                    "{}",
                    rel.display()
                );
                seen += 1;
            }
        }
    }
    assert!(seen > 5, "{seen} files");
    assert!(home.has(".claude/skills/orchestrate/SKILL.md"));
}

#[test]
fn the_mem_shim_fetches_the_binary_from_the_mem_server_once_then_runs_it() {
    let home = Scratch::new("cloud-setup-shim");
    home.put("setup.sh", &script(&home));
    let ran = command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");

    // A curl that logs its argv and writes a mem that echoes its own.
    let log = home.at("curl.log");
    home.script(
        "fake/curl",
        &format!(
            "#!/bin/sh\necho \"$@\" >> {}\nwhile [ \"$1\" != -o ]; do shift; done\n\
printf '#!/bin/sh\\necho real mem \"$@\"\\n' > \"$2\"\n",
            log.display()
        ),
    );
    let path = format!(
        "{}:{}",
        home.at("fake").display(),
        std::env::var("PATH").unwrap()
    );
    let shim = home.at("usr-local-bin/mem");
    for _ in 0..2 {
        let out = command(&shim)
            .args(["log", "landed t1"])
            .env("HOME", home.path())
            .env("PATH", &path)
            .env("MEM_REMOTE", "https://mem.example.test")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "real mem log landed t1\n"
        );
    }
    let fetched = fs::read_to_string(&log).unwrap();
    assert_eq!(fetched.lines().count(), 1, "fetched once: {fetched}");
    assert!(
        fetched.contains("https://mem.example.test/dl/mem"),
        "{fetched}"
    );
}

/// A cloud home where every binary is already fetched and each logs how it
/// was started. The fake hub "listens" by creating its socket file, and the
/// fake curl's liveness probe succeeds while that file is there. The fake
/// tailscale answers `status` only once `up` has run, naming the machine
/// `cloud-given-1`, as Tailscale does when the name asked for is taken.
fn cloud_home(tag: &str) -> (Scratch, PathBuf, String) {
    let home = Scratch::new(tag);
    home.put("setup.sh", &script(&home));
    let ran = command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");
    let log = home.at("started.log");
    let logs = |name: &str| {
        format!(
            "#!/bin/sh\necho \"{name} $* ts=${{TS_AUTHKEY:-}} socks=${{HUB_SOCKS5:-}} host=${{HUB_HOST:-}}\" >> {}\n",
            log.display()
        )
    };
    let state = home.at(".local/state/hub");
    home.script(".local/bin/mem-x86_64", "#!/bin/sh\necho real mem \"$@\"\n");
    home.script(
        ".local/bin/tailscaled",
        &format!(
            "{}for a in \"$@\"; do case $a in --socket=*) : > \"${{a#--socket=}}\";; esac; done\nsleep 5\n",
            logs("tailscaled")
        ),
    );
    home.script(
        ".local/bin/tailscale",
        &format!(
            "case \"$*\" in *' status'*) [ -e {joined} ] || exit 1\n\
             printf '{{\"BackendState\": \"Running\",\\n\"Self\": {{\"ID\": \"n1\", \"HostName\": \"cloud-given\", \"DNSName\": \"cloud-given-1.tail.ts.net.\"}}}}\\n'; exit 0;; esac\n\
             {}for a in \"$@\"; do case $a in --auth-key=file:*) cat \"${{a#--auth-key=file:}}\" >> {};; esac; done\n: > {joined}\n",
            logs("tailscale"),
            home.at("key-read").display(),
            joined = home.at("joined").display()
        ),
    );
    home.script(
        ".local/bin/hub-x86_64",
        &format!("{}: > {}/local.sock\n", logs("hub"), state.display()),
    );
    home.script(
        "fake/curl",
        &format!(
            "#!/bin/sh\ncase \"$*\" in *--unix-socket*) [ -e {}/local.sock ];; *) exit 1;; esac\n",
            state.display()
        ),
    );
    let path = format!(
        "{}:{}",
        home.at("fake").display(),
        std::env::var("PATH").unwrap()
    );
    (home, log, path)
}

fn run_shim(home: &Scratch, path: &str, args: &[&str]) {
    let out = command(home.at("usr-local-bin/mem"))
        .args(args)
        .env("HOME", home.path())
        .env("PATH", path)
        .env("MEM_REMOTE", "https://mem.example.test")
        .env("TS_AUTHKEY", "tskey-auth-secret")
        .env("HUB_PEER_TOKEN", "peer-secret")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("real mem {}\n", args.join(" "))
    );
}

fn lines_of(log: &Path, name: &str) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with(&format!("{name} ")))
        .map(str::to_string)
        .collect()
}

/// With the Tailscale key and the peer token in its environment, a cloud
/// session's mem joins the tailnet in userspace as tag:cloud and starts the
/// local hub under the name Tailscale gave it, with neither secret on a
/// command line; an ask waits for that hub.
#[test]
fn the_first_mem_with_the_keys_starts_tailscale_and_the_local_hub_once() {
    let (home, log, path) = cloud_home("cloud-setup-local-hub");
    // The ask itself waits for the hub it started.
    run_shim(&home, &path, &["ask", "probe?"]);
    assert_eq!(
        lines_of(&log, "hub").len(),
        1,
        "{:?}",
        fs::read_to_string(&log)
    );
    run_shim(&home, &path, &["context"]);

    let tailscaled = lines_of(&log, "tailscaled");
    assert_eq!(tailscaled.len(), 1, "{tailscaled:?}");
    assert!(
        tailscaled[0].contains("--tun=userspace-networking"),
        "{tailscaled:?}"
    );
    assert!(
        tailscaled[0].contains("--socks5-server=localhost:1055"),
        "{tailscaled:?}"
    );
    let up = lines_of(&log, "tailscale");
    assert_eq!(up.len(), 1, "{up:?}");
    assert!(up[0].contains(" up "), "{up:?}");
    assert!(up[0].contains("--hostname=cloud-"), "{up:?}");
    assert!(up[0].contains("--advertise-tags=tag:cloud"), "{up:?}");
    assert_eq!(home.read("key-read"), "tskey-auth-secret");
    let hub = lines_of(&log, "hub");
    assert_eq!(hub.len(), 1, "{hub:?}");
    assert!(hub[0].starts_with("hub local "), "{hub:?}");
    assert!(hub[0].contains("socks=127.0.0.1:1055"), "{hub:?}");
    assert!(
        hub[0].ends_with("host=cloud-given-1.tail.ts.net"),
        "{hub:?}"
    );
    // The key went in a file, read and removed; no process was handed it.
    let started = fs::read_to_string(&log).unwrap();
    assert!(!started.contains("tskey-auth-secret"), "{started}");
    assert!(!started.contains("peer-secret"), "{started}");
    assert!(!home.has(".local/state/hub/ts-authkey"));
}

/// A local hub that died is started again by the next mem, without joining
/// the tailnet a second time.
#[test]
fn a_dead_local_hub_is_started_again_by_the_next_mem() {
    let (home, log, path) = cloud_home("cloud-setup-restart");
    run_shim(&home, &path, &["ask", "probe?"]);
    fs::remove_file(home.at(".local/state/hub/local.sock")).unwrap();
    run_shim(&home, &path, &["ask", "again?"]);
    assert_eq!(
        lines_of(&log, "hub").len(),
        2,
        "{:?}",
        fs::read_to_string(&log)
    );
    assert_eq!(lines_of(&log, "tailscaled").len(), 1);
    assert_eq!(lines_of(&log, "tailscale").len(), 1);
}

/// A run that logged out of the tailnet (a landed or local-only run's last
/// step) and then asks again joins it again, though its hub never died:
/// otherwise no answer could reach it.
#[test]
fn a_mem_after_a_logout_joins_the_tailnet_again() {
    let (home, log, path) = cloud_home("cloud-setup-rejoin");
    run_shim(&home, &path, &["ask", "probe?"]);
    fs::remove_file(home.at("joined")).unwrap();
    run_shim(&home, &path, &["ask", "again?"]);
    let up: Vec<String> = lines_of(&log, "tailscale")
        .into_iter()
        .filter(|l| l.contains(" up "))
        .collect();
    assert_eq!(up.len(), 2, "{:?}", fs::read_to_string(&log));
    assert_eq!(lines_of(&log, "hub").len(), 1, "the live hub is kept");
}

#[test]
fn without_the_keys_the_shim_starts_nothing() {
    let home = Scratch::new("cloud-setup-no-keys");
    home.put("setup.sh", &script(&home));
    command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    home.script(".local/bin/mem-x86_64", "#!/bin/sh\necho real mem \"$@\"\n");
    let out = command(home.at("usr-local-bin/mem"))
        .args(["context"])
        .env("HOME", home.path())
        .env("MEM_REMOTE", "https://mem.example.test")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!home.has(".local/state/hub"));
}

/// The variables of an environment whose sessions keep a replica of the
/// store, the walk's `workflow-next`.
const STORE_ENV: [(&str, &str); 6] = [
    ("MEM_REMOTE", "https://mem.example.test"),
    ("HUB_STORE_URL", "wss://store.example.test/store/share"),
    ("HUB_STORE_TOKEN", "store-secret"),
    ("HUB_STORE_ACCESS_ID", "access-id.access"),
    ("HUB_STORE_ACCESS_SECRET", "access-secret"),
    ("HUB_CLOUD_TOKEN", "cloud-secret"),
];

/// Fakes for the setup script: a curl that logs its argv and the header
/// file it was given, then writes a binary that logs how it was run.
fn store_fakes(home: &Scratch, hub_sync_exit: u8) -> (PathBuf, String) {
    let log = home.at("started.log");
    // `share switch` leaves the flag the real mem leaves.
    let mem = format!(
        "#!/bin/sh\necho \"mem $* remote=${{MEM_REMOTE:-}}\" >> {log}\n\
         case \"$*\" in 'share switch') mkdir -p {share} && : > {share}/switched;; esac\n\
         echo real mem \"$@\"\n",
        log = log.display(),
        share = home.at(".local/state/mem/share").display(),
    );
    let hub = format!(
        "#!/bin/sh\necho \"hub $* url=${{HUB_STORE_URL:-}} remote=${{MEM_REMOTE:-}}\" >> {log}\n\
         case \"$*\" in 'share sync') exit {hub_sync_exit};; esac\n",
        log = log.display()
    );
    home.put("fake-mem", &mem);
    home.put("fake-hub", &hub);
    home.script(
        "fake/curl",
        &format!(
            "#!/bin/sh\nargs=\"$*\"\necho \"curl $args\" >> {log}\n\
             prev=; for a in \"$@\"; do [ \"$prev\" = -H ] && case $a in @*) cat \"${{a#@}}\" >> {log};; esac; prev=$a; done\n\
             case \"$args\" in *--unix-socket*) [ -e {state}/local.sock ]; exit;; esac\n\
             case \"$args\" in *.sha256*) for a; do u=$a; done; \
             f={sums}/$(basename \"${{u%.sha256}}\"); [ -f \"$f\" ] || exit 22; cat \"$f\"; exit;; esac\n\
             while [ \"$1\" != -o ]; do shift; done\n\
             case \"$args\" in */dl/hub*|*/hub-x86_64*) cp {hub} \"$2\";; *) cp {mem} \"$2\";; esac\n",
            log = log.display(),
            state = home.at(".local/state/hub").display(),
            sums = home.at("sums").display(),
            hub = home.at("fake-hub").display(),
            mem = home.at("fake-mem").display(),
        ),
    );
    let path = format!(
        "{}:{}",
        home.at("fake").display(),
        std::env::var("PATH").unwrap()
    );
    (log, path)
}

fn setup_with(home: &Scratch, path: &str, vars: &[(&str, &str)]) -> std::process::Output {
    home.put("setup.sh", &script(home));
    let mut setup = command("bash");
    setup
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .env("PATH", path);
    for (k, v) in vars {
        setup.env(k, v);
    }
    let ran = setup.output().unwrap();
    assert!(ran.status.success(), "{ran:?}");
    ran
}

/// claude.ai runs the setup script with none of the environment's
/// variables, so the setup bakes both binaries into the cached environment
/// from the public release the mini publishes, with no secret on the way.
#[test]
fn the_setup_bakes_both_binaries_from_the_release_with_no_secret() {
    let home = Scratch::new("cloud-setup-bake");
    let (log, path) = store_fakes(&home, 0);
    let ran = setup_with(&home, &path, &[]);
    let curls = lines_of(&log, "curl");
    for name in ["mem", "hub"] {
        let url = format!(
            "https://github.com/saifulapm/workflow/releases/download/cloud-bins/{name}-x86_64"
        );
        assert!(curls.iter().any(|c| c.contains(&url)), "{curls:?}");
        let baked = home.at(&format!(".local/bin/{name}-x86_64"));
        assert!(
            baked
                .metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0),
            "{name}"
        );
    }
    let started = fs::read_to_string(&log).unwrap();
    assert!(!started.contains("Authorization"), "{started}");
    assert!(lines_of(&log, "hub").is_empty(), "{started}");
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("baked"),
        "{ran:?}"
    );
}

/// A release that does not answer leaves nothing baked, and the setup still
/// succeeds: each session fetches its own.
#[test]
fn a_bake_that_fails_does_not_fail_the_setup() {
    let home = Scratch::new("cloud-setup-bake-fails");
    let (_, path) = store_fakes(&home, 0);
    home.script("fake/curl", "#!/bin/sh\nexit 22\n");
    let ran = setup_with(&home, &path, &[]);
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("no baked binaries"),
        "{ran:?}"
    );
    assert!(!home.at(".local/bin/mem-x86_64").exists());
}

/// In a session with the store variables, the first mem brings the replica
/// up to the store before it reads, then starts the local hub that holds the
/// socket; mem itself runs on the replica, not on the mem server.
#[test]
fn the_first_mem_of_a_store_session_syncs_before_it_reads() {
    let home = Scratch::new("cloud-shim-sync");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    fs::remove_file(&log).unwrap();
    let run = |args: &[&str]| {
        let mut shim = command(home.at("usr-local-bin/mem"));
        shim.args(args).env("HOME", home.path()).env("PATH", &path);
        for (k, v) in STORE_ENV {
            shim.env(k, v);
        }
        shim.env("HUB_PEER_TOKEN_SHA256", "ab12");
        let out = shim.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        out
    };
    run(&["context"]);
    let started = fs::read_to_string(&log).unwrap();
    let order: Vec<&str> = started
        .lines()
        .filter(|l| l.starts_with("hub share sync") || l.starts_with("mem context"))
        .collect();
    assert_eq!(
        order,
        [
            "hub share sync url=wss://store.example.test/store/share remote=",
            "mem context remote="
        ],
        "{started}"
    );
    // The hub starts in the background; wait for its line.
    for _ in 0..50 {
        if lines_of(&log, "hub")
            .iter()
            .any(|l| l.starts_with("hub local"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // The hub records answers in the replica only when mem is not remote.
    assert!(
        lines_of(&log, "hub")
            .iter()
            .any(|l| l.starts_with("hub local ") && l.ends_with(" remote=")),
        "{}",
        fs::read_to_string(&log).unwrap()
    );
    // The hub's own mem calls go straight to the replica: no sync, no hub.
    fs::remove_file(&log).unwrap();
    run(&["share", "apply", "--all"]);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "mem share apply --all remote=\n"
    );
}

/// Every machine shares through the store since the switch, and mem queues
/// a write only on a switched replica: a store session's first mem switches
/// it, once, before it writes anything.
#[test]
fn a_store_sessions_first_mem_switches_the_replica_once_before_it_writes() {
    let home = Scratch::new("cloud-shim-switch");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    fs::remove_file(&log).unwrap();
    for text in ["one", "two"] {
        let mut shim = command(home.at("usr-local-bin/mem"));
        shim.args(["log", text])
            .env("HOME", home.path())
            .env("PATH", &path);
        for (k, v) in STORE_ENV {
            shim.env(k, v);
        }
        let out = shim.output().unwrap();
        assert!(out.status.success(), "{out:?}");
    }
    let ran = lines_of(&log, "mem");
    let mems: Vec<&str> = ran
        .iter()
        .map(String::as_str)
        .filter(|l| l.starts_with("mem share switch") || l.starts_with("mem log"))
        .collect();
    assert_eq!(
        mems,
        [
            "mem share switch remote=",
            "mem log one remote=",
            "mem log two remote="
        ],
        "{ran:?}"
    );
}

/// Runs the store session's shim with `args` and returns its stdout.
fn run_store_shim(home: &Scratch, path: &str, args: &[&str]) -> String {
    let mut shim = command(home.at("usr-local-bin/mem"));
    shim.args(args).env("HOME", home.path()).env("PATH", path);
    for (k, v) in STORE_ENV {
        shim.env(k, v);
    }
    let out = shim.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The store's sum file for `name`, as `hub share put-bin` leaves it.
fn store_sum(home: &Scratch, name: &str, bytes_of: &str) {
    let out = command("sha256sum")
        .arg(home.at(bytes_of))
        .output()
        .unwrap();
    let sum = String::from_utf8_lossy(&out.stdout);
    let sum = sum.split_whitespace().next().unwrap();
    home.put(&format!("sums/{name}"), &format!("{sum}  {name}\n"));
}

/// Lines of the fakes' log that are curl calls whose argv contains `what`.
fn curls(log: &Path, what: &str) -> usize {
    lines_of(log, "curl")
        .iter()
        .filter(|l| l.contains(what))
        .count()
}

/// The setup baked one mem; the store has built a new one since. The first
/// mem of a boot sees the store's sum differ and fetches it again, and the
/// later ones that boot ask nothing more.
#[test]
fn a_binary_the_store_rebuilt_is_fetched_again_once_a_boot() {
    let home = Scratch::new("cloud-shim-rebuilt");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    home.put("fake-mem", "#!/bin/sh\necho new mem \"$@\"\n");
    store_sum(&home, "mem-x86_64", "fake-mem");
    fs::remove_file(&log).unwrap();
    assert_eq!(
        run_store_shim(&home, &path, &["context"]),
        "new mem context\n"
    );
    assert_eq!(
        run_store_shim(&home, &path, &["context"]),
        "new mem context\n"
    );
    assert_eq!(
        curls(&log, "mem-x86_64.sha256"),
        1,
        "{:?}",
        lines_of(&log, "curl")
    );
    assert_eq!(
        curls(&log, "mem-x86_64 -o"),
        1,
        "{:?}",
        lines_of(&log, "curl")
    );
}

#[test]
fn a_binary_the_store_holds_as_it_is_is_not_fetched_again() {
    let home = Scratch::new("cloud-shim-current");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    store_sum(&home, "mem-x86_64", ".local/bin/mem-x86_64");
    fs::remove_file(&log).unwrap();
    run_store_shim(&home, &path, &["context"]);
    assert_eq!(
        curls(&log, "mem-x86_64.sha256"),
        1,
        "{:?}",
        lines_of(&log, "curl")
    );
    assert_eq!(
        curls(&log, "mem-x86_64 -o"),
        0,
        "{:?}",
        lines_of(&log, "curl")
    );
}

/// A new build the store names but cannot hand over keeps the binary here.
#[test]
fn a_rebuilt_binary_that_does_not_come_keeps_the_one_here() {
    let home = Scratch::new("cloud-shim-rebuilt-lost");
    let (_log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    home.put(
        "sums/mem-x86_64",
        &format!("{}  mem-x86_64\n", "0".repeat(64)),
    );
    fs::remove_file(home.at("fake-mem")).unwrap();
    assert_eq!(
        run_store_shim(&home, &path, &["context"]),
        "real mem context\n"
    );
}

/// A store with no sum, or none it can give, keeps the binary here, and is
/// not asked again that boot.
#[test]
fn without_the_stores_sum_the_binary_here_stays() {
    let home = Scratch::new("cloud-shim-no-sum");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    fs::remove_file(&log).unwrap();
    assert_eq!(
        run_store_shim(&home, &path, &["context"]),
        "real mem context\n"
    );
    run_store_shim(&home, &path, &["context"]);
    assert_eq!(
        curls(&log, "mem-x86_64.sha256"),
        1,
        "{:?}",
        lines_of(&log, "curl")
    );
    assert_eq!(
        curls(&log, "mem-x86_64 -o"),
        0,
        "{:?}",
        lines_of(&log, "curl")
    );
}

/// A store the session cannot reach leaves the replica as it was baked, and
/// mem still reads it, never the mem server.
#[test]
fn a_failed_sync_reads_the_baked_replica() {
    let home = Scratch::new("cloud-shim-sync-fails");
    let (log, path) = store_fakes(&home, 1);
    setup_with(&home, &path, &STORE_ENV);
    fs::remove_file(&log).unwrap();
    let mut shim = command(home.at("usr-local-bin/mem"));
    shim.args(["context"])
        .env("HOME", home.path())
        .env("PATH", &path);
    for (k, v) in STORE_ENV {
        shim.env(k, v);
    }
    let out = shim.env("HUB_PEER_TOKEN_SHA256", "ab12").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the store did not answer"),
        "{out:?}"
    );
    let ran = fs::read_to_string(&log).unwrap();
    assert!(ran.lines().any(|l| l == "mem context remote="), "{ran}");
}

/// Runs the shim once in a home with no keys, a fake `hostname` printing
/// `host`, and the machine file holding `before` (none when `None`).
/// Returns the file after.
fn machine_file_after(tag: &str, host: &str, before: Option<&str>) -> Option<String> {
    let home = Scratch::new(tag);
    home.put("setup.sh", &script(&home));
    command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    home.script(".local/bin/mem-x86_64", "#!/bin/sh\necho real mem \"$@\"\n");
    home.script("fake/hostname", &format!("#!/bin/sh\necho {host}\n"));
    if let Some(text) = before {
        home.put(".config/qshell/machine", text);
    }
    let path = format!(
        "{}:{}",
        home.at("fake").display(),
        std::env::var("PATH").unwrap()
    );
    let out = command(home.at("usr-local-bin/mem"))
        .args(["context"])
        .env("HOME", home.path())
        .env("PATH", path)
        .env("MEM_REMOTE", "https://mem.example.test")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    fs::read_to_string(home.at(".config/qshell/machine")).ok()
}

#[test]
fn the_shim_names_a_cloud_machine_after_its_cleaned_host() {
    for before in [None, Some("")] {
        let after = machine_file_after("cloud-machine-new", "My_Host.example", before);
        assert_eq!(
            after.as_deref().map(str::trim),
            Some("cloud-myhostexample"),
            "{before:?}"
        );
    }
    // The store takes no write from a machine called just `cloud-`.
    let after = machine_file_after("cloud-machine-bare", "___", None);
    assert_eq!(after.as_deref().map(str::trim), Some("cloud-vm"));
}

#[test]
fn the_shim_leaves_a_machine_name_that_is_already_set() {
    let after = machine_file_after("cloud-machine-kept", "x", Some("mini\n"));
    assert_eq!(after.as_deref(), Some("mini\n"));
}
