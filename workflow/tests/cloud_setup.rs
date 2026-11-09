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

/// A PATH whose curl fails, for a setup run that must bake nothing and stay
/// off the network: the setup bakes from the public cloud-bins release.
fn offline(home: &Scratch) -> String {
    home.script("offline/curl", "#!/bin/sh\nexit 22\n");
    format!(
        "{}:{}",
        home.at("offline").display(),
        std::env::var("PATH").unwrap()
    )
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

/// The release's bundle, packed from this repo's skills, agents and cloud
/// kit as CI packs it, with its sum.
fn repo_bundle(home: &Scratch) {
    let out = command("tar")
        .args(["-C", repo().to_str().unwrap(), "-czf"])
        .arg(home.at("fake-skills.tar.gz"))
        .args(["skills", "agents", "cloud"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    store_sum(home, "workflow-skills.tar.gz", "fake-skills.tar.gz");
}

/// The skills and agents are too big for the environment's setup field: the
/// setup takes them from the public release's bundle, as it takes the
/// binaries, with no secret.
#[test]
fn the_setup_takes_every_skill_and_agent_from_the_release_byte_for_byte() {
    let home = Scratch::new("cloud-setup-files");
    let (log, path) = store_fakes(&home, 0);
    repo_bundle(&home);
    let ran = setup_with(&home, &path, &[]);
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("took the skills and agents"),
        "{ran:?}"
    );
    let started = fs::read_to_string(&log).unwrap();
    assert!(
        started.contains("releases/download/cloud-bins/workflow-skills.tar.gz"),
        "{started}"
    );

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

/// The setup leaves `device` in the shim folder, running `workflow device`.
#[test]
fn the_setup_leaves_a_device_command_that_runs_workflow_device() {
    let home = Scratch::new("cloud-setup-device");
    setup_with(&home, &offline(&home), &[]);
    let device = home.at("usr-local-bin/device");
    assert_ne!(
        fs::metadata(&device).unwrap().permissions().mode() & 0o111,
        0
    );
    home.script("fake/workflow", "echo \"workflow $*\"");
    let path = format!(
        "{}:{}",
        home.at("fake").display(),
        std::env::var("PATH").unwrap()
    );
    let out = command(&device)
        .args(["run", "adb", "devices"])
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "workflow device run adb devices\n"
    );
}

/// claude.ai snapshots an environment's setup and runs it again only when the
/// script or its hosts change, or after about seven days, so a file only the
/// setup writes is missing from threads until then: m2-device's new `device`
/// shim was. The per-boot refresh lays down the binaries, the skills, the
/// agents, CLAUDE.md and settings.json; these are the files nothing but the
/// setup writes.
#[test]
fn only_the_known_shims_are_written_by_the_setup_alone() {
    let home = Scratch::new("cloud-setup-only");
    setup_with(&home, &offline(&home), &[]);
    let refreshed = [".claude/CLAUDE.md", ".claude/settings.json", "setup.sh"];
    let mut left = Vec::new();
    let mut stack = vec![home.path().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(home.path())
                .unwrap()
                .display()
                .to_string();
            if rel == "offline" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if !refreshed.contains(&rel.as_str()) {
                left.push(rel);
            }
        }
    }
    left.sort();
    assert_eq!(
        left,
        [
            "usr-local-bin/device",
            "usr-local-bin/mem",
            "usr-local-bin/mem-cloud",
            "usr-local-bin/workflow",
        ],
        "a file only the setup writes reaches a thread only once claude.ai runs the setup \
again: lay it down from the per-boot refresh (the mem shim's first run of a boot), or \
pin it here knowing a walk must change the setup to see it"
    );
}

/// The setup fits the field: claude.ai takes a setup file of 64 KB at most,
/// and an 86 KB paste did not reach the environment.
#[test]
fn the_setup_fits_the_environment_field() {
    let home = Scratch::new("cloud-setup-size");
    let size = script(&home).len();
    assert!(size < 64 * 1024, "{size} bytes");
}

/// A release that does not answer leaves the skills to the first mem of a
/// boot, and the setup says so.
#[test]
fn an_offline_setup_says_the_skills_come_later() {
    let home = Scratch::new("cloud-setup-no-bundle");
    let ran = setup_with(&home, &offline(&home), &[]);
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("no skills bundle"),
        "{ran:?}"
    );
}

#[test]
fn the_mem_shim_fetches_the_binary_from_the_mem_server_once_then_runs_it() {
    let home = Scratch::new("cloud-setup-shim");
    home.put("setup.sh", &script(&home));
    let ran = command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .env("PATH", offline(&home))
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
        .env("PATH", offline(&home))
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
        .env("PATH", offline(&home))
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
    // Nothing of the hub's own is in its state: no hub, no tailnet.
    let state: Vec<_> = fs::read_dir(home.at(".local/state/hub"))
        .map(|dir| {
            dir.map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        state.iter().all(|name| name.starts_with("skills")),
        "{state:?}"
    );
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
             case \"$args\" in */workflow-skills.tar.gz*) cp {skills} \"$2\";; \
             */dl/hub*|*/hub-x86_64*) cp {hub} \"$2\";; *) cp {mem} \"$2\";; esac\n",
            log = log.display(),
            state = home.at(".local/state/hub").display(),
            sums = home.at("sums").display(),
            skills = home.at("fake-skills.tar.gz").display(),
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
/// from the public release the workflow repo's CI publishes, with no secret
/// on the way.
#[test]
fn the_setup_bakes_both_binaries_from_the_release_with_no_secret() {
    let home = Scratch::new("cloud-setup-bake");
    let (log, path) = store_fakes(&home, 0);
    let ran = setup_with(&home, &path, &[]);
    let curls = lines_of(&log, "curl");
    for name in ["mem", "hub", "workflow"] {
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

/// The guard hook calls `workflow` by name: the setup puts one on PATH that
/// runs the baked binary, and fails quietly where none was baked, which the
/// hook takes as no guard.
#[test]
fn the_setup_puts_the_baked_workflow_on_path() {
    let home = Scratch::new("cloud-setup-workflow");
    let (_, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &[]);
    home.script(
        ".local/bin/workflow-x86_64",
        "#!/bin/sh\necho baked workflow \"$@\"\n",
    );
    let shim = home.at("usr-local-bin/workflow");
    let ran = command(&shim)
        .args(["guard", "--help"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "baked workflow guard --help\n",
        "{ran:?}"
    );
    fs::remove_file(home.at(".local/bin/workflow-x86_64")).unwrap();
    let ran = command(&shim)
        .args(["guard", "--help"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(!ran.status.success() && ran.stdout.is_empty(), "{ran:?}");
}

/// The release rebuilt workflow since the setup baked it: a store session's
/// first mem of a boot takes the new one, as it does mem.
#[test]
fn a_workflow_the_release_rebuilt_is_fetched_again_once_a_boot() {
    let home = Scratch::new("cloud-shim-workflow");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    home.put("fake-workflow", "#!/bin/sh\necho new workflow \"$@\"\n");
    store_sum(&home, "workflow-x86_64", "fake-workflow");
    fs::copy(home.at("fake-workflow"), home.at("fake-mem")).unwrap();
    fs::remove_file(&log).unwrap();
    run_store_shim(&home, &path, &["context"]);
    run_store_shim(&home, &path, &["context"]);
    assert_eq!(
        home.read(".local/bin/workflow-x86_64"),
        home.read("fake-workflow")
    );
    assert_eq!(
        curls(&log, "workflow-x86_64.sha256"),
        1,
        "{:?}",
        lines_of(&log, "curl")
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

/// The workflow repo's CI publishes each build, its sum and the skills on
/// the public cloud-bins release, so the once-a-boot checks ask the release:
/// through its redirect (`-L`), and with no header, since the store's bearer
/// and the Access token are no business of GitHub's.
#[test]
fn the_checks_of_a_boot_ask_the_release_with_no_secret() {
    let home = Scratch::new("cloud-shim-release");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    home.put("fake-mem", "#!/bin/sh\necho new mem \"$@\"\n");
    store_sum(&home, "mem-x86_64", "fake-mem");
    store_skills(&home, "# newer orchestrate\n", "# newer implementer\n");
    fs::remove_file(&log).unwrap();
    assert_eq!(
        run_store_shim(&home, &path, &["context"]),
        "new mem context\n"
    );
    let release = "https://github.com/saifulapm/workflow/releases/download/cloud-bins";
    let calls = lines_of(&log, "curl");
    for asked in [
        "mem-x86_64.sha256",
        "mem-x86_64 -o",
        "workflow-skills.tar.gz.sha256",
        "workflow-skills.tar.gz -o",
    ] {
        let call = calls
            .iter()
            .find(|c| c.contains(asked))
            .unwrap_or_else(|| panic!("no curl for {asked}: {calls:?}"));
        let file = asked.split(' ').next().unwrap();
        assert!(call.contains(&format!("{release}/{file}")), "{call}");
        assert!(call.contains("-fsSL"), "follows the redirect: {call}");
        assert!(!call.contains(" -H "), "no header to GitHub: {call}");
    }
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

/// A fresh VM replays the whole store, about 25 s on a cloud VM, and the
/// first mem waits for all of it, up to 120 s, so its first read is whole.
#[test]
fn the_first_mem_gives_the_catch_up_120_seconds() {
    let home = Scratch::new("cloud-shim-catch-up");
    let (log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    home.script(
        "fake/timeout",
        &format!(
            "#!/bin/sh\necho \"timeout $1\" >> {}\nshift\nexec \"$@\"\n",
            log.display()
        ),
    );
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
    assert_eq!(
        lines_of(&log, "timeout"),
        ["timeout 120"],
        "{}",
        fs::read_to_string(&log).unwrap()
    );
}

/// The release's bundle of the workflow skills and agents, as the workflow
/// repo's CI packs it: `skills/<name>/SKILL.md` and `agents/<name>.md`.
fn store_skills(home: &Scratch, skill: &str, agent: &str) {
    home.put("bundle/skills/orchestrate/SKILL.md", skill);
    home.put("bundle/agents/implementer.md", agent);
    let out = command("tar")
        .args(["-C", home.at("bundle").to_str().unwrap(), "-czf"])
        .arg(home.at("fake-skills.tar.gz"))
        .args(["skills", "agents"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    store_sum(home, "workflow-skills.tar.gz", "fake-skills.tar.gz");
}

/// The setup wrote the skills it carries; the store holds newer ones. The
/// first mem of a boot puts the store's in place, and later ones that boot
/// ask nothing more.
#[test]
fn the_first_mem_of_a_boot_takes_the_stores_skills() {
    let home = Scratch::new("cloud-shim-skills");
    let (log, path) = store_fakes(&home, 0);
    repo_bundle(&home);
    setup_with(&home, &path, &STORE_ENV);
    let skill = home.at(".claude/skills/orchestrate/SKILL.md");
    assert!(fs::read_to_string(&skill).unwrap() != "# newer orchestrate\n");
    home.put(
        ".claude/skills/someone-else/SKILL.md",
        "# not a workflow skill\n",
    );
    store_skills(&home, "# newer orchestrate\n", "# newer implementer\n");
    fs::remove_file(&log).unwrap();
    run_store_shim(&home, &path, &["context"]);
    run_store_shim(&home, &path, &["context"]);
    assert_eq!(fs::read_to_string(&skill).unwrap(), "# newer orchestrate\n");
    assert_eq!(
        fs::read_to_string(home.at(".claude/agents/implementer.md")).unwrap(),
        "# newer implementer\n"
    );
    // The bundle is the whole set: a workflow skill or agent it leaves out
    // was removed from the repo, and goes; a folder the setup did not write
    // stays.
    assert!(
        !home.at(".claude/skills/tdd").exists(),
        "a removed skill stayed"
    );
    assert!(
        !home.at(".claude/agents/scout.md").exists(),
        "a removed agent stayed"
    );
    assert!(
        home.at(".claude/skills/someone-else/SKILL.md").exists(),
        "a folder not ours went"
    );
    assert_eq!(
        curls(&log, "workflow-skills.tar.gz.sha256"),
        1,
        "{:?}",
        lines_of(&log, "curl")
    );
    assert_eq!(
        curls(&log, "workflow-skills.tar.gz -o"),
        1,
        "{:?}",
        lines_of(&log, "curl")
    );
}

/// A store with no bundle keeps the skills the setup took.
#[test]
fn without_the_stores_skills_the_setups_stay() {
    let home = Scratch::new("cloud-shim-no-skills");
    let (log, path) = store_fakes(&home, 0);
    repo_bundle(&home);
    setup_with(&home, &path, &STORE_ENV);
    fs::remove_file(home.at("sums/workflow-skills.tar.gz")).unwrap();
    let skill = home.at(".claude/skills/orchestrate/SKILL.md");
    let before = fs::read_to_string(&skill).unwrap();
    fs::remove_file(&log).unwrap();
    run_store_shim(&home, &path, &["context"]);
    assert_eq!(fs::read_to_string(&skill).unwrap(), before);
    assert_eq!(
        curls(&log, "workflow-skills.tar.gz -o"),
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
        .env("PATH", offline(&home))
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

#[test]
fn the_setup_gives_a_session_the_rules_and_the_mem_hooks() {
    let home = Scratch::new("cloud-setup-kit");
    // The image's own settings, which the merge keeps.
    home.put(
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Bash(ls)"]}}"#,
    );
    home.put("setup.sh", &script(&home));
    let out = command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .env("PATH", offline(&home))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let rules = home.read(".claude/CLAUDE.md");
    assert!(rules.contains("ctx7"), "{rules}");
    let settings: serde_json::Value =
        serde_json::from_str(&home.read(".claude/settings.json")).unwrap();
    assert_eq!(
        settings["permissions"]["allow"][0], "Bash(ls)",
        "{settings}"
    );
    for event in ["SessionStart", "PostToolBatch", "PreCompact", "Stop"] {
        let command = settings["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_default();
        assert!(command.starts_with("mem "), "{event}: {settings}");
    }
    // Every Bash and Monitor call passes workflow's guard first.
    let guard = &settings["hooks"]["PreToolUse"][0];
    assert_eq!(guard["matcher"], "Bash", "{settings}");
    assert!(
        guard["hooks"][0]["command"]
            .as_str()
            .is_some_and(|c| c.contains("workflow guard bash")),
        "{settings}"
    );
    // A cloud session waits in its turn for the answer to its own question.
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        "mem stop-hook --session-id \"$CLAUDE_CODE_SESSION_ID\" || true",
        "{settings}"
    );
}

/// The bundle's cloud kit: its skills join the others, its rules become the
/// user's CLAUDE.md and its hooks merge into settings.json, on the first mem
/// of a boot.
#[test]
fn the_first_mem_of_a_boot_takes_the_cloud_kit() {
    let home = Scratch::new("cloud-shim-kit");
    let (_log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    home.put("bundle/cloud/skills/vite/SKILL.md", "# vite\n");
    home.put("bundle/cloud/rules.md", "# newer rules\n");
    home.put(
        "bundle/cloud/settings-hooks.json",
        r#"{"PreCompact":[{"hooks":[{"type":"command","command":"mem precompact"}]}]}"#,
    );
    home.put("bundle/skills/orchestrate/SKILL.md", "# orchestrate\n");
    home.put("bundle/agents/implementer.md", "# implementer\n");
    let out = command("tar")
        .args(["-C", home.at("bundle").to_str().unwrap(), "-czf"])
        .arg(home.at("fake-skills.tar.gz"))
        .args(["skills", "agents", "cloud"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    store_sum(&home, "workflow-skills.tar.gz", "fake-skills.tar.gz");
    run_store_shim(&home, &path, &["context"]);
    assert_eq!(home.read(".claude/skills/vite/SKILL.md"), "# vite\n");
    assert_eq!(home.read(".claude/CLAUDE.md"), "# newer rules\n");
    let settings = home.read(".claude/settings.json");
    assert!(settings.contains("mem precompact"), "{settings}");
    assert!(
        !home.at(".claude/skills/cloud").exists(),
        "the kit's folder itself is no skill"
    );
}

/// The kit carries settings beside its hooks, the advisor first: they go
/// in at the top of settings.json, whatever else the image's settings hold
/// stays, and a second setup on the same disk, as a cached environment's
/// rebuild runs it, leaves one copy of each hook.
#[test]
fn the_setup_carries_the_advisor_and_runs_twice_without_doubling_a_hook() {
    let home = Scratch::new("cloud-setup-advisor");
    home.put(
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Bash(ls)"]},"model":"image-model","env":{"IMAGE_VAR":"1"},"hooks":{"Notification":[{"hooks":[{"type":"command","command":"image-note"}]}]}}"#,
    );
    home.put("setup.sh", &script(&home));
    for round in 0..2 {
        let out = command("bash")
            .arg(home.at("setup.sh"))
            .env("HOME", home.path())
            .env("PATH", offline(&home))
            .output()
            .unwrap();
        assert!(out.status.success(), "round {round}: {out:?}");
    }
    let settings: serde_json::Value =
        serde_json::from_str(&home.read(".claude/settings.json")).unwrap();
    assert_eq!(settings["advisorModel"], "fable", "{settings}");
    // The image sets no locale, so tmux clients draw UTF-8 as ASCII
    // (probe-amx3): the kit's own variables join the image's.
    assert_eq!(settings["env"]["LANG"], "C.UTF-8", "{settings}");
    assert_eq!(settings["env"]["LC_ALL"], "C.UTF-8", "{settings}");
    // The session runs as root, and Composer then loads no plugins, so
    // pest has no --parallel (probe-lathe-3).
    assert_eq!(
        settings["env"]["COMPOSER_ALLOW_SUPERUSER"], "1",
        "{settings}"
    );
    assert_eq!(settings["env"]["IMAGE_VAR"], "1", "{settings}");
    assert_eq!(
        settings["permissions"]["allow"][0], "Bash(ls)",
        "{settings}"
    );
    assert_eq!(settings["model"], "image-model", "{settings}");
    assert_eq!(
        settings["hooks"]["Notification"][0]["hooks"][0]["command"], "image-note",
        "{settings}"
    );
    for (event, entries) in settings["hooks"].as_object().unwrap() {
        assert_eq!(entries.as_array().unwrap().len(), 1, "{event}: {settings}");
    }
}

/// A kit that moves the advisor reaches a cached environment with the
/// skills refresh, as its hooks do.
#[test]
fn the_first_mem_of_a_boot_takes_the_kits_settings() {
    let home = Scratch::new("cloud-shim-kit-keys");
    let (_log, path) = store_fakes(&home, 0);
    setup_with(&home, &path, &STORE_ENV);
    home.put(
        "bundle/cloud/settings-keys.json",
        r#"{"advisorModel":"opus"}"#,
    );
    home.put("bundle/skills/orchestrate/SKILL.md", "# orchestrate\n");
    let out = command("tar")
        .args(["-C", home.at("bundle").to_str().unwrap(), "-czf"])
        .arg(home.at("fake-skills.tar.gz"))
        .args(["skills", "cloud"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    store_sum(&home, "workflow-skills.tar.gz", "fake-skills.tar.gz");
    run_store_shim(&home, &path, &["context"]);
    let settings: serde_json::Value =
        serde_json::from_str(&home.read(".claude/settings.json")).unwrap();
    assert_eq!(settings["advisorModel"], "opus", "{settings}");
    assert!(
        settings["hooks"]["SessionStart"].is_array(),
        "the setup's hooks stay: {settings}"
    );
}

/// A release with mem and hub but no workflow build, as before main was
/// pushed with it: the setup bakes the two it has and says which is missing,
/// and the guard hook passes every call until a boot fetches one.
#[test]
fn a_release_without_workflow_still_bakes_mem_and_hub() {
    let home = Scratch::new("cloud-setup-no-workflow");
    let (log, path) = store_fakes(&home, 0);
    let curl = home.read("fake/curl");
    home.script(
        "fake/curl",
        &curl.replacen(
            "args=\"$*\"\n",
            "args=\"$*\"\ncase \"$args\" in */workflow-x86_64*) exit 22;; esac\n",
            1,
        ),
    );
    let ran = setup_with(&home, &path, &[]);
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(said.contains("no workflow build on cloud-bins"), "{said}");
    assert!(!said.contains("no baked binaries"), "{said}");
    for name in ["mem", "hub"] {
        assert!(
            home.at(&format!(".local/bin/{name}-x86_64")).exists(),
            "{name}"
        );
    }
    assert!(!home.at(".local/bin/workflow-x86_64").exists());
    drop(log);
}

/// A scratch root with the image's Node 24, its pnpm 10 and no
/// playwright-cli, and a PATH whose npm, curl and apt-get log
/// what they are asked and leave what they install, as the real ones do.
fn runtime_fakes(home: &Scratch) -> (PathBuf, String) {
    let log = home.at("runtimes.log");
    let node = home.mkdir("root/opt/node24/bin");
    home.script(
        "root/opt/node24/bin/node",
        // The image sets NODE_OPTIONS=--use-openssl-ca, so node refuses a
        // second CA flag, as the image's does.
        "#!/bin/sh\ncase \"$1\" in --use-*) echo 'either --use-openssl-ca or --use-system-ca can be used, not both' >&2; exit 9;; esac\nexec /bin/sh \"$@\"\n",
    );
    home.script("root/opt/node24/bin/pnpm", "#!/bin/sh\necho 10.28.0\n");
    home.script("root/opt/node24/bin/npx", "#!/bin/sh\n");
    home.script(
        "root/opt/node24/bin/npm",
        &format!(
            "#!/bin/sh\necho \"npm $*\" >> {log}\ndir=$(dirname \"$0\")\n\
             case \"$*\" in\n\
             *pnpm@11*) printf '#!/bin/sh\\necho 11.21.0\\n' > \"$dir/pnpm\";;\n\
             *@playwright/cli@0.1.23*) printf '#!/bin/sh\\n[ \"$1\" = --version ] && exec echo 0.1.23\\necho real playwright-cli \"$PLAYWRIGHT_MCP_EXECUTABLE_PATH\" \"$PLAYWRIGHT_MCP_SANDBOX\" \"$@\"\\n' > \"$dir/playwright-cli\"; chmod +x \"$dir/playwright-cli\";;\n\
             *cf@1.0.0-beta.28*) printf '#!/bin/sh\\n[ \"$1\" = --version ] && exec echo 1.0.0-beta.28\\necho real cf \"$NODE_USE_ENV_PROXY\" \"$CLOUDFLARE_API_TOKEN\" \"$@\"\\n' > \"$dir/cf\"; chmod +x \"$dir/cf\";;\n\
             esac\n",
            log = log.display()
        ),
    );
    let fakes = home.mkdir("runtime-fakes");
    // The image's add-apt-repository crashes (its python3 has no apt_pkg).
    home.script(
        "runtime-fakes/add-apt-repository",
        "#!/bin/sh\necho 'ModuleNotFoundError: No module named apt_pkg' >&2\nexit 1\n",
    );
    home.put("root/etc/os-release", "ID=ubuntu\nVERSION_CODENAME=noble\n");
    // tmux's release source: a configure that keeps its prefix and a make
    // whose install leaves a tmux there, as the real build does.
    home.script(
        "tmux-src/tmux-3.7c/configure",
        "#!/bin/sh\nfor a; do case \"$a\" in --prefix=*) echo \"${a#--prefix=}\" > prefix;; esac; done\n",
    );
    let tarball = home.at("tmux-3.7c.tar.gz");
    assert!(
        Command::new("tar")
            .args(["-czf"])
            .arg(&tarball)
            .arg("-C")
            .arg(home.at("tmux-src"))
            .arg("tmux-3.7c")
            .status()
            .unwrap()
            .success()
    );
    home.script(
        "runtime-fakes/make",
        &format!(
            "#!/bin/sh\necho \"make $*\" >> {log}\n\
             [ \"$1\" = install ] || exit 0\n\
             p=$(cat prefix); mkdir -p \"$p/bin\"\n\
             printf '#!/bin/sh\\necho tmux 3.7c\\n' > \"$p/bin/tmux\"; chmod +x \"$p/bin/tmux\"\n",
            log = log.display()
        ),
    );
    home.script(
        "runtime-fakes/sha256sum",
        &format!(
            "#!/bin/sh\necho \"sha256sum $* $(cat)\" >> {log}\n",
            log = log.display()
        ),
    );
    home.script(
        "runtime-fakes/curl",
        &format!(
            "#!/bin/sh\necho \"curl $*\" >> {log}\n\
             case \"$*\" in *tmux-3.7c.tar.gz*) tmux=1;; esac\n\
             while [ \"$1\" != -o ]; do shift; done\n\
             case \"$tmux\" in 1) cp {tarball} \"$2\"; exit;; esac\n\
             echo '-----BEGIN PGP PUBLIC KEY BLOCK-----' > \"$2\"\n",
            log = log.display(),
            tarball = tarball.display()
        ),
    );
    home.script(
        "runtime-fakes/apt-get",
        &format!(
            "#!/bin/sh\necho \"apt-get $*\" >> {log}\n\
             case \"$*\" in *php8.4-cli*) printf '#!/bin/sh\\necho PHP 8.4.14\\n' > {fakes}/php8.4; chmod +x {fakes}/php8.4;; esac\n",
            log = log.display(),
            fakes = fakes.display()
        ),
    );
    let _ = node;
    let path = format!("{}:{}", fakes.display(), std::env::var("PATH").unwrap());
    (log, path)
}

fn runtimes(home: &Scratch, path: &str) -> std::process::Output {
    let out = command(home.at("usr-local-bin/mem-cloud"))
        .arg("runtimes")
        .env("HOME", home.path())
        .env("MEM_CLOUD_ROOT", home.at("root"))
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    out
}

/// The device's Node 24, pnpm 11, PHP 8.4, playwright-cli and tmux 3.7 go first on a
/// session's PATH, through ~/.local/bin; a second run, as a cached
/// environment's rebuild makes, installs nothing again.
#[test]
fn the_runtimes_are_the_devices_and_install_once() {
    let home = Scratch::new("cloud-runtimes");
    setup_with(&home, &offline(&home), &[]);
    let (log, path) = runtime_fakes(&home);
    runtimes(&home, &path);
    let first = fs::read_to_string(&log).unwrap();
    assert!(first.contains("npm install -g --force pnpm@11"), "{first}");
    assert!(
        first.contains("npm install -g @playwright/cli@0.1.23"),
        "{first}"
    );
    // Cloudflare's own CLI, which the network secret for api.cloudflare.com
    // serves (probe-cf, 2026-11-06).
    assert!(first.contains("npm install -g cf@1.0.0-beta.28"), "{first}");
    let sources =
        fs::read_to_string(home.at("root/etc/apt/sources.list.d/ondrej-php.sources")).unwrap();
    assert!(
        sources.contains("URIs: https://ppa.launchpadcontent.net/ondrej/php/ubuntu")
            && sources.contains("Suites: noble")
            && sources.contains("Signed-By: /etc/apt/keyrings/ondrej-php.asc"),
        "{sources}"
    );
    // The PPA's key, by the fingerprint that signs its InRelease.
    assert!(
        first.contains("search=0x14AA40EC0831756756D7F66C4F4EA0AAE5267A6C"),
        "{first}"
    );
    assert!(home.at("root/etc/apt/keyrings/ondrej-php.asc").exists());
    assert!(first.contains("php8.4-intl"), "lathe needs intl: {first}");
    // amx's tmux.conf sets extended-keys-format, which the image's tmux 3.4
    // lacks: tmux 3.7c is built from its release source, checked by its sum.
    assert!(
        first.contains("https://github.com/tmux/tmux/releases/download/3.7c/tmux-3.7c.tar.gz"),
        "{first}"
    );
    assert!(
        first.contains("7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf"),
        "{first}"
    );
    assert!(first.contains("libevent-dev"), "{first}");
    for (tool, says) in [
        ("node", None),
        ("pnpm", Some("11.21.0")),
        ("php", Some("PHP 8.4.14")),
        ("tmux", Some("tmux 3.7c")),
    ] {
        let link = home.at(&format!(".local/bin/{tool}"));
        assert!(link.exists(), "{tool}");
        if let Some(says) = says {
            let ran = command(&link).arg("-v").output().unwrap();
            assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), says, "{tool}");
        }
    }
    // playwright-cli runs headless on the image's Chrome, with no sandbox,
    // since the session is root.
    let ran = command(home.at(".local/bin/playwright-cli"))
        .args(["open", "https://example.com"])
        .env("CHROME_PATH", "/usr/bin/google-chrome")
        .env_remove("PLAYWRIGHT_MCP_EXECUTABLE_PATH")
        .env_remove("PLAYWRIGHT_MCP_SANDBOX")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "real playwright-cli /usr/bin/google-chrome false open https://example.com",
        "{ran:?}"
    );
    // cf reaches the proxy, which adds the network secret's token, only
    // with Node's proxy support on and a token variable to replace; a
    // global NODE_USE_ENV_PROXY hangs pnpm (probe-proxy, 2026-11-06).
    let ran = command(home.at(".local/bin/cf"))
        .args(["zones", "list"])
        .env_remove("CLOUDFLARE_API_TOKEN")
        .env_remove("NODE_USE_ENV_PROXY")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "real cf 1 proxy-injected zones list",
        "{ran:?}"
    );
    let ran = command(home.at(".local/bin/cf"))
        .arg("whoami")
        .env("CLOUDFLARE_API_TOKEN", "a-real-one")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "real cf 1 a-real-one whoami",
        "{ran:?}"
    );
    fs::remove_file(&log).unwrap();
    runtimes(&home, &path);
    assert!(
        !log.exists(),
        "{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
}

/// An image with no Node 24 keeps its own Node and says so; the rest goes
/// on.
#[test]
fn an_image_without_node_24_keeps_its_node() {
    let home = Scratch::new("cloud-runtimes-no-node");
    setup_with(&home, &offline(&home), &[]);
    let (log, path) = runtime_fakes(&home);
    fs::remove_dir_all(home.at("root/opt/node24")).unwrap();
    let out = runtimes(&home, &path);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no Node 24"),
        "{out:?}"
    );
    assert!(!home.at(".local/bin/node").exists());
    assert!(fs::read_to_string(&log).unwrap().contains("php8.4-cli"));
}

/// A runtime that does not install fails the step and names itself, so
/// the setup's output says which one stays the image's.
#[test]
fn a_runtime_that_does_not_install_is_named() {
    let home = Scratch::new("cloud-runtimes-fail");
    setup_with(&home, &offline(&home), &[]);
    let (log, path) = runtime_fakes(&home);
    home.script("root/opt/node24/bin/npm", "#!/bin/sh\nexit 1\n");
    home.script("runtime-fakes/curl", "#!/bin/sh\nexit 22\n");
    let out = command(home.at("usr-local-bin/mem-cloud"))
        .arg("runtimes")
        .env("HOME", home.path())
        .env("MEM_CLOUD_ROOT", home.at("root"))
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("did not install: pnpm playwright-cli cf php8.4 tmux"),
        "{said}"
    );
    // A key that did not come leaves no source for apt to fail on.
    assert!(
        !home
            .at("root/etc/apt/sources.list.d/ondrej-php.sources")
            .exists()
    );
    let _ = log;
}

/// A merge that fails leaves the settings as they were: never the bare
/// hooks map in their place.
#[test]
fn a_merge_that_fails_keeps_the_settings() {
    let home = Scratch::new("cloud-setup-merge-fails");
    home.put(".claude/settings.json", r#"{"model":"image-model"}"#);
    home.put("setup.sh", &script(&home));
    // A python3 that fails, as a missing one would.
    home.script("nopython/python3", "#!/bin/sh\nexit 1\n");
    let path = format!("{}:{}", home.at("nopython").display(), offline(&home));
    let out = command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        home.read(".claude/settings.json"),
        r#"{"model":"image-model"}"#
    );
}

/// A keys file that is no JSON object is passed over: the kit's hooks
/// still go in.
#[test]
fn a_keys_file_that_is_no_object_still_lets_the_hooks_in() {
    let home = Scratch::new("cloud-setup-bad-keys");
    home.put("setup.sh", &script(&home));
    let out = command("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .env("PATH", offline(&home))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    fs::remove_file(home.at(".claude/settings.json")).unwrap();
    home.put(
        "kit-hooks.json",
        r#"{"Stop":[{"hooks":[{"type":"command","command":"mem stop"}]}]}"#,
    );
    home.put("kit-keys.json", r#"["x"]"#);
    let out = command(home.at("usr-local-bin/mem-cloud"))
        .args([
            "hooks",
            home.at("kit-hooks.json").to_str().unwrap(),
            home.at("kit-keys.json").to_str().unwrap(),
        ])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let settings = home.read(".claude/settings.json");
    assert!(settings.contains("mem stop"), "{settings}");
}

/// The printed bootstrap: what a Project's environment holds in its setup
/// field.
fn bootstrap() -> String {
    let out = command(env!("CARGO_BIN_EXE_workflow"))
        .args(["cloud-setup", "--bootstrap"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// Each Project's environment holds a short bootstrap, pasted once, that
/// fetches the full setup CI publishes on the release: a kit change then
/// needs no paste in any environment.
#[test]
fn the_bootstrap_is_short_and_fetches_the_full_setup_from_the_release() {
    let text = bootstrap();
    assert!(text.starts_with("#!/bin/bash\n"), "{text}");
    assert!(text.len() < 2048, "{} bytes", text.len());
    assert!(
        text.contains("https://github.com/saifulapm/workflow/releases/download/cloud-bins"),
        "{text}"
    );
    assert!(text.contains("cloud-setup.sh"), "{text}");
}

/// A fake release serving `setup` as the full setup with `sum` as its sum,
/// and a PATH whose curl reads from it.
fn release_with(home: &Scratch, setup: &str, sum: &str) -> String {
    home.put("release/cloud-setup.sh", setup);
    home.put(
        "release/cloud-setup.sh.sha256",
        &format!("{sum}  cloud-setup.sh\n"),
    );
    home.script(
        "fake/curl",
        &format!(
            "#!/bin/sh\nout=\nurl=\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=$2; shift ;; http*) url=$1 ;; esac; shift; done\n\
             cp \"{}/${{url##*/}}\" \"${{out:-/dev/stdout}}\"\n",
            home.at("release").display()
        ),
    );
    format!(
        "{}:{}",
        home.at("fake").display(),
        std::env::var("PATH").unwrap()
    )
}

fn sha256(text: &str) -> String {
    let mut child = command("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

#[test]
fn the_bootstrap_runs_the_full_setup_it_fetched() {
    let home = Scratch::new("cloud-bootstrap");
    let full = format!("#!/bin/bash\ntouch {}\n", home.at("ran").display());
    let path = release_with(&home, &full, &sha256(&full));
    home.put("bootstrap.sh", &bootstrap());
    let ran = command("bash")
        .arg(home.at("bootstrap.sh"))
        .env("HOME", home.path())
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");
    assert!(home.has("ran"), "{ran:?}");
}

/// A setup cut short on the way, or one the release replaced between the
/// two fetches, is not run: the environment would hold half a kit.
#[test]
fn the_bootstrap_runs_no_setup_whose_sum_differs() {
    let home = Scratch::new("cloud-bootstrap-sum");
    let full = format!("#!/bin/bash\ntouch {}\n", home.at("ran").display());
    let path = release_with(&home, &full, &sha256("another setup"));
    home.put("bootstrap.sh", &bootstrap());
    let ran = command("bash")
        .arg(home.at("bootstrap.sh"))
        .env("HOME", home.path())
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(!ran.status.success(), "{ran:?}");
    assert!(!home.has("ran"), "{ran:?}");
}

/// CI publishes the full setup the bootstrap fetches, with its sum.
#[test]
fn ci_publishes_the_full_setup_on_the_release() {
    let ci = fs::read_to_string(repo().join(".github/workflows/cloud-bins.yml")).unwrap();
    assert!(ci.contains("cloud-setup >out/cloud-setup.sh"), "{ci}");
    assert!(ci.contains("out/cloud-setup.sh"), "{ci}");
    assert!(ci.contains("cloud-setup.sh.sha256"), "{ci}");
}

/// One 504 from GitHub's release downloads broke an environment's build
/// (cloud_walk w4): every fetch from the release retries.
#[test]
fn every_fetch_from_the_release_retries() {
    let home = Scratch::new("cloud-setup-retry");
    for text in [script(&home), bootstrap()] {
        let fetches: Vec<_> = text
            .lines()
            .filter(|l| l.contains("curl ") && (l.contains("$url") || l.contains("$release")))
            .collect();
        assert!(!fetches.is_empty(), "{text}");
        for line in fetches {
            assert!(line.contains("--retry"), "{line}");
        }
    }
}
