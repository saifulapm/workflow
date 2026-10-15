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
