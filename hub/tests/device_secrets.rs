//! `device with-secrets`: the hub hands a cloud command the pass entries its
//! project's `cloud-secrets` names, only when a device wrote that key, and
//! never prints or logs a value. pass is a fake that replays what the real
//! one printed on the mini (2026-11-13) for a cached key, a locked one and a
//! missing entry.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

struct Rig {
    hub: Hub,
    home: PathBuf,
    /// What fake mem keeps: projects.json, read back, and log, the lines
    /// `mem log` was given.
    state: PathBuf,
    _dir: TempDir,
}

/// The test's theme token: 38 characters in a Shopify token's shape, built
/// here so no literal of that shape is committed.
fn theme_token() -> String {
    format!("shpat_{}", "0123456789abcdef".repeat(2))
}

fn rig() -> Rig {
    let dir = TempDir::new("device-secrets");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mac-mini-m2\n").unwrap();
    let bin = dir.join("bin");
    let state = dir.join("mem");
    std::fs::create_dir_all(&state).unwrap();
    fixture_bin(
        &bin,
        "systemctl",
        &format!("echo 'PATH={}:/usr/bin:/bin'", bin.display()),
    );
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "S='{}'\n\
             for a in \"$@\"; do last=\"$a\"; done\n\
             case \"$*\" in\n\
             *' log -- '*) printf '%s %s\\n' \"$1\" \"$last\" >> \"$S/log\" ;;\n\
             projects*) cat \"$S/projects.json\" 2>/dev/null || echo '{{\"projects\":[]}}' ;;\n\
             esac",
            state.display()
        ),
    );
    // The real pass's answers: the value for a cached key; `No pinentry`
    // and exit 2 for a locked one under --pinentry-mode=error.
    fixture_bin(
        &bin,
        "pass",
        &format!(
            "S='{}'\n\
             echo \"$PASSWORD_STORE_GPG_OPTS\" > \"$S/pass-opts\"\n\
             if [ -e \"$S/locked\" ]; then\n\
             echo 'gpg: public key decryption failed: No pinentry' >&2\n\
             echo 'gpg: decryption failed: No pinentry' >&2\n\
             exit 2\n\
             fi\n\
             cat \"$S/value-$(echo \"$2\" | tr / _)\"",
            state.display()
        ),
    );
    let hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", dir.join("local.sock").to_str().unwrap()),
        ],
    );
    let rig = Rig {
        hub,
        home,
        state,
        _dir: dir,
    };
    rig.entry("Shopify/theme-access/pawsome-theme", &theme_token());
    rig.secrets(
        "SHOPIFY_CLI_THEME_TOKEN=Shopify/theme-access/pawsome-theme",
        "mac-mini-m2",
    );
    rig
}

impl Rig {
    /// pawsome's `cloud-secrets`, written by `by`.
    fn secrets(&self, lines: &str, by: &str) {
        let projects = serde_json::json!({"projects": [
            {"name": "pawsome", "cloud_secrets": lines, "cloud_secrets_by": by},
        ]});
        std::fs::write(self.state.join("projects.json"), projects.to_string()).unwrap();
    }

    /// A pass entry: its file in the store, and the value fake pass prints.
    fn entry(&self, entry: &str, value: &str) {
        let file = self
            .home
            .join(".password-store")
            .join(format!("{entry}.gpg"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "encrypted").unwrap();
        std::fs::write(
            self.state
                .join(format!("value-{}", entry.replace('/', "_"))),
            format!("{value}\n"),
        )
        .unwrap();
    }

    fn call(&self) -> (String, serde_json::Value) {
        let body = serde_json::json!({
            "verb": "with-secrets",
            "args": ["--", "sh", "-c", "echo ${#SHOPIFY_CLI_THEME_TOKEN}"],
            "project": "pawsome",
            "session": "cse_01Theme",
        })
        .to_string();
        let response = self.hub.raw(&format!(
            "POST /device/here HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
             Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert_eq!(status_of(&response), 200, "{response}");
        (
            response.clone(),
            serde_json::from_str(body_of(&response)).unwrap(),
        )
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.state.join("log")).unwrap_or_default()
    }
}

#[test]
fn a_device_written_entry_reaches_the_command_and_no_output_or_log() {
    let rig = rig();
    // A store password has no token's shape, so no mask would cut it.
    rig.entry("Shopify/pawsome-demo-password", "plain-words-7");
    rig.secrets(
        "SHOPIFY_CLI_THEME_TOKEN=Shopify/theme-access/pawsome-theme\n\
         STORE_PASSWORD=Shopify/pawsome-demo-password",
        "mac-mini-m2",
    );
    let (_, answer) = rig.call();
    assert_eq!(answer["exit"], 0, "{answer}");
    assert_eq!(
        answer["secrets"]["SHOPIFY_CLI_THEME_TOKEN"],
        serde_json::json!(theme_token()),
        "{answer}"
    );
    assert_eq!(answer["stdout"], "", "{answer}");
    assert_eq!(answer["stderr"], "", "{answer}");
    let log = rig.log();
    assert!(log.contains("device mac-mini-m2 with-secrets"), "{log}");
    assert!(!log.contains(&theme_token()), "{log}");
    assert_eq!(
        answer["secrets"]["STORE_PASSWORD"], "plain-words-7",
        "{answer}"
    );
    assert!(!log.contains("plain-words-7"), "{log}");
    // A locked key fails at once instead of opening a pinentry on the desktop.
    let opts = std::fs::read_to_string(rig.state.join("pass-opts")).unwrap();
    assert!(opts.contains("--pinentry-mode=error"), "{opts}");
}

#[test]
fn cloud_secrets_a_cloud_machine_wrote_are_refused() {
    let rig = rig();
    rig.secrets(
        "SHOPIFY_CLI_THEME_TOKEN=Shopify/theme-access/pawsome-theme",
        "cloud-abc",
    );
    let (response, answer) = rig.call();
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: cloud-secrets of pawsome was not set from a device\n"
    );
    assert!(!response.contains(&theme_token()), "{response}");
}

#[test]
fn a_locked_pass_says_so_and_exits_7() {
    let rig = rig();
    std::fs::write(rig.state.join("locked"), "").unwrap();
    let (_, answer) = rig.call();
    assert_eq!(answer["exit"], 7, "{answer}");
    assert_eq!(answer["stderr"], "device: pass is locked on mac-mini-m2\n");
}

#[test]
fn an_entry_missing_from_the_store_is_named() {
    let rig = rig();
    rig.secrets("STORE_PASSWORD=Shopify/pawsome-demo-password", "nuc");
    let (_, answer) = rig.call();
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: no pass entry Shopify/pawsome-demo-password on mac-mini-m2\n"
    );
}

#[test]
fn a_project_with_no_cloud_secrets_is_refused() {
    let rig = rig();
    std::fs::write(
        rig.state.join("projects.json"),
        r#"{"projects":[{"name":"pawsome"}]}"#,
    )
    .unwrap();
    let (_, answer) = rig.call();
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(answer["stderr"], "device: pawsome has no cloud-secrets\n");
}

#[test]
fn an_entry_that_is_no_theme_token_or_store_password_is_refused() {
    let rig = rig();
    rig.entry("Shopify/ShortCart/token", "app-token");
    for entry in [
        "Shopify/ShortCart/token",
        "cloudflare/api-token",
        "Shopify/theme-access/../ShortCart/token",
        "Shopify/x/y-password",
    ] {
        rig.secrets(&format!("X={entry}"), "mac-mini-m2");
        let (response, answer) = rig.call();
        assert_eq!(answer["exit"], 2, "{entry}: {answer}");
        assert_eq!(
            answer["stderr"],
            format!("device: {entry} is not a theme token or a store password\n")
        );
        assert!(!response.contains("app-token"), "{response}");
    }
}
