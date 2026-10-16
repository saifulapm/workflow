//! `workflow cloud-setup`: the cloud environment's setup script, run here by
//! bash with `$HOME` and the shim's folder pointed at a scratch directory.

#[path = "../src/scratch.rs"]
mod scratch;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use scratch::Scratch;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The printed script, its shim folder moved into the scratch directory.
fn script(home: &Scratch) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_workflow"))
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
    let ran = Command::new("bash")
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
    let ran = Command::new("bash")
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
        let out = Command::new(&shim)
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
    let ran = Command::new("bash")
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
    let out = Command::new(home.at("usr-local-bin/mem"))
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

#[test]
fn without_the_keys_the_shim_starts_nothing() {
    let home = Scratch::new("cloud-setup-no-keys");
    home.put("setup.sh", &script(&home));
    Command::new("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    home.script(".local/bin/mem-x86_64", "#!/bin/sh\necho real mem \"$@\"\n");
    let out = Command::new(home.at("usr-local-bin/mem"))
        .args(["context"])
        .env("HOME", home.path())
        .env("MEM_REMOTE", "https://mem.example.test")
        .env_remove("TS_AUTHKEY")
        .env_remove("HUB_PEER_TOKEN")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!home.has(".local/state/hub"));
}
