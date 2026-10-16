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

/// With the Tailscale key and the peer token in its environment, the first mem
/// of a cloud session joins the tailnet in userspace and starts the local hub,
/// once, with neither secret on a command line.
#[test]
fn the_first_mem_with_the_keys_starts_tailscale_and_the_local_hub_once() {
    let home = Scratch::new("cloud-setup-local-hub");
    home.put("setup.sh", &script(&home));
    let ran = Command::new("bash")
        .arg(home.at("setup.sh"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");

    // Every binary is already fetched; each logs how it was started.
    let log = home.at("started.log");
    let logs = |name: &str| {
        format!(
            "#!/bin/sh\necho \"{name} $* ts=${{TS_AUTHKEY:-}} socks=${{HUB_SOCKS5:-}} host=${{HUB_HOST:-}}\" >> {}\n",
            log.display()
        )
    };
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
            "{}for a in \"$@\"; do case $a in --auth-key=file:*) cat \"${{a#--auth-key=file:}}\" >> {};; esac; done\n",
            logs("tailscale"),
            home.at("key-read").display()
        ),
    );
    home.script(".local/bin/hub-x86_64", &logs("hub"));

    let shim = home.at("usr-local-bin/mem");
    for _ in 0..2 {
        let out = Command::new(&shim)
            .args(["context"])
            .env("HOME", home.path())
            .env("MEM_REMOTE", "https://mem.example.test")
            .env("TS_AUTHKEY", "tskey-auth-secret")
            .env("HUB_PEER_TOKEN", "peer-secret")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "real mem context\n");
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("hub local")
    {
        assert!(std::time::Instant::now() < deadline, "no local hub started");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let started = fs::read_to_string(&log).unwrap();
    let line = |name: &str| {
        started
            .lines()
            .filter(|l| l.starts_with(&format!("{name} ")))
            .collect::<Vec<_>>()
    };
    assert_eq!(line("tailscaled").len(), 1, "{started}");
    assert!(
        line("tailscaled")[0].contains("--tun=userspace-networking"),
        "{started}"
    );
    assert!(
        line("tailscaled")[0].contains("--socks5-server=localhost:1055"),
        "{started}"
    );
    assert_eq!(line("tailscale").len(), 1, "{started}");
    assert!(line("tailscale")[0].contains(" up "), "{started}");
    assert!(
        line("tailscale")[0].contains("--hostname=cloud-"),
        "{started}"
    );
    assert_eq!(home.read("key-read"), "tskey-auth-secret");
    assert_eq!(line("hub").len(), 1, "{started}");
    let hub = line("hub")[0];
    assert!(hub.starts_with("hub local "), "{hub}");
    assert!(hub.contains("socks=127.0.0.1:1055"), "{hub}");
    assert!(hub.contains("host=cloud-"), "{hub}");
    // The key went in a file, read and removed; no process was handed it.
    assert!(!started.contains("tskey-auth-secret"), "{started}");
    assert!(!started.contains("peer-secret"), "{started}");
    assert!(!home.has(".local/state/hub/ts-authkey"));
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
    assert!(!home.has(".local/state/hub/started"));
}
