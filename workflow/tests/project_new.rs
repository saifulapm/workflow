//! `workflow project-new <project>`: the steps that put a project into a
//! Claude Project on both accounts, and the checks a machine can make,
//! through the real binary with scripts in the place of mem and ssh.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::{Command, Output};

use scratch::Scratch;

/// This machine, as mem names it.
const HERE: &str = "mac-mini-m2";

/// A world whose mem lists `projects` (JSON rows, `{root}` standing for the
/// scratch folder) and logs every call it gets.
fn world(tag: &str, projects: &str) -> Scratch {
    let work = Scratch::new(tag);
    let rows = projects.replace("{root}", &work.path().display().to_string());
    work.put("projects.json", &format!(r#"{{"projects":[{rows}]}}"#));
    work.script(
        "mem",
        r#"
dir=$(dirname "$0")
printf '%s\n' "$*" >>"$dir/mem-calls"
case "$1" in
projects) cat "$dir/projects.json" ;;
*) exit 1 ;;
esac
"#,
    );
    work.put("config/qshell/machine", &format!("{HERE}\n"));
    work.mkdir("pass");
    work
}

fn run(work: &Scratch, project: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["project-new", project])
        .env("WORKFLOW_MEM", work.at("mem"))
        .env("XDG_CONFIG_HOME", work.at("config"))
        .env("PASSWORD_STORE_DIR", work.at("pass"))
        .env("HUB_LOCAL_SOCKET", work.at("no-hub.sock"))
        .env("WORKFLOW_SSH", work.at("ssh"))
        .current_dir(work.path())
        .output()
        .unwrap()
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The lines under `heading`, up to the next blank line.
fn section(text: &str, heading: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .skip_while(|l| l.trim_end_matches(':') != heading);
    assert!(lines.next().is_some(), "no heading {heading:?} in:\n{text}");
    lines
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .collect()
}

/// The numbers the lines start with, as `1.` gives 1.
fn numbers(lines: &[String]) -> Vec<u32> {
    lines
        .iter()
        .filter_map(|l| l.split_once(". ").and_then(|(n, _)| n.parse().ok()))
        .collect()
}

fn trendsi(tag: &str) -> Scratch {
    let work = world(
        tag,
        r#"{"name":"lareys-trendsi","remote":"github.com/saifulapm/lareys-trendsi","checkouts":["{root}/lareys-trendsi"],"subdir":null,"device":null,"device_by":null,"cloud_secrets":null}"#,
    );
    work.mkdir("lareys-trendsi");
    work
}

#[test]
fn it_prints_three_steps_for_each_account_naming_the_repo_and_the_text() {
    let work = trendsi("steps");
    let out = run(&work, "lareys-trendsi");
    let text = said(&out);

    let once = section(&text, "Once, on any device");
    assert!(
        once.iter()
            .any(|l| l.contains("`mem project set device <machine>`")),
        "{text}"
    );
    for account in ["On the work account", "On the main account"] {
        let steps = section(&text, account);
        assert_eq!(numbers(&steps), [1, 2, 3], "{account}:\n{text}");
        let all = steps.join("\n");
        assert!(all.contains("saifulapm/lareys-trendsi"), "{all}");
        assert!(all.contains("environment `workflow`"), "{all}");
        assert!(
            all.contains("`workflow project-text lareys-trendsi`"),
            "{all}"
        );
    }
}

#[test]
fn a_submodule_repo_is_added_to_the_project_in_step_two() {
    let work = world(
        "submodule",
        r#"{"name":"shopify_apps","remote":"github.com/lareysbd/shopify_apps","checkouts":["{root}/shopify_apps"],"subdir":null},{"name":"palmshop","remote":"github.com/lareysbd/shopify_apps","checkouts":["{root}/shopify_apps/apps/palmshop"],"subdir":"apps/palmshop"}"#,
    );
    work.put(
        "shopify_apps/.gitmodules",
        "[submodule \"apps/palmshop/engine\"]\n\tpath = apps/palmshop/engine\n\turl = ../dactyl.git\n[submodule \"vendor/kit\"]\n\tpath = vendor/kit\n\turl = https://github.com/saifulapm/kit.git\n",
    );
    let out = run(&work, "shopify_apps");
    let text = said(&out);
    for account in ["On the work account", "On the main account"] {
        let steps = section(&text, account);
        let two = steps
            .iter()
            .find(|l| l.starts_with("2. "))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(two.contains("lareysbd/shopify_apps"), "{two}");
        assert!(two.contains("lareysbd/dactyl"), "{two}");
        assert!(two.contains("saifulapm/kit"), "{two}");
    }
}

#[test]
fn it_writes_nothing_and_only_reads_mem() {
    let work = trendsi("writes");
    run(&work, "lareys-trendsi");
    let calls = work.read("mem-calls");
    assert!(!calls.trim().is_empty());
    for call in calls.lines() {
        assert_eq!(call, "projects --json", "mem was called with {call:?}");
    }
}

#[test]
fn an_unknown_project_is_refused() {
    let work = trendsi("unknown");
    let out = run(&work, "nosuch");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nosuch"));
}

fn code(out: &Output) -> Option<i32> {
    out.status.code()
}

/// The `ok:` and `missing:` lines, in order.
fn checks(out: &Output) -> Vec<String> {
    said(out)
        .lines()
        .filter(|l| l.starts_with("ok: ") || l.starts_with("missing: "))
        .map(str::to_string)
        .collect()
}

/// An ssh that logs its arguments and answers for `host` only: `mem
/// projects --json` with `projects`, and `test -f` for each file in `files`.
fn fake_ssh(work: &Scratch, host: &str, projects: &str, files: &[&str]) {
    let rows = projects.replace("{root}", &work.path().display().to_string());
    work.put(
        "remote-projects.json",
        &format!(r#"{{"projects":[{rows}]}}"#),
    );
    work.put("remote-files", &(files.join("\n") + "\n"));
    work.script(
        "ssh",
        &format!(
            r#"
dir=$(dirname "$0")
printf '%s\n' "$*" >>"$dir/ssh-calls"
while [ "${{1#-}}" != "$1" ]; do shift 2; done
[ "$1" = "{host}" ] || exit 255
shift
case "$1" in
mem) cat "$dir/remote-projects.json" ;;
test) grep -qxF "$3" "$dir/remote-files" ;;
*) exit 127 ;;
esac
"#
        ),
    );
}

/// A local hub that answers `GET /device/choice` with `device`, once.
fn local_hub(work: &Scratch, device: &str) -> std::thread::JoinHandle<()> {
    use std::io::{Read, Write};
    let listener = std::os::unix::net::UnixListener::bind(work.at("hub.sock")).unwrap();
    let body = format!(r#"{{"device":"{device}","at":1}}"#);
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(_) => return,
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    })
}

#[test]
fn a_project_with_a_remote_and_its_checkout_here_is_all_ok() {
    let work = world(
        "all-ok",
        r#"{"name":"lareys-trendsi","remote":"github.com/saifulapm/lareys-trendsi","checkouts":["{root}/lareys-trendsi"],"subdir":null,"device":"mac-mini-m2","device_by":"mac-mini-m2"}"#,
    );
    work.mkdir("lareys-trendsi");
    let out = run(&work, "lareys-trendsi");
    assert_eq!(
        checks(&out),
        [
            "ok: remote saifulapm/lareys-trendsi",
            "ok: checkout on mac-mini-m2"
        ],
        "{}",
        said(&out)
    );
    assert_eq!(code(&out), Some(0));
}

#[test]
fn no_remote_is_one_missing_line_no_account_steps_and_exit_one() {
    let work = world(
        "no-remote",
        r#"{"name":"eco","remote":null,"checkouts":["{root}/eco"],"subdir":null,"device":"mac-mini-m2","device_by":"mac-mini-m2"}"#,
    );
    work.mkdir("eco");
    let out = run(&work, "eco");
    let text = said(&out);
    assert_eq!(
        checks(&out)
            .iter()
            .filter(|l| l.contains("remote"))
            .collect::<Vec<_>>(),
        ["missing: remote"],
        "{text}"
    );
    assert!(!text.contains("On the work account"), "{text}");
    assert!(!text.contains("On the main account"), "{text}");
    assert_eq!(code(&out), Some(1));
}

#[test]
fn a_checkout_folder_gone_from_the_device_here_is_missing() {
    let work = world(
        "gone",
        r#"{"name":"lareys-trendsi","remote":"github.com/saifulapm/lareys-trendsi","checkouts":["{root}/was-here"],"subdir":null,"device":"mac-mini-m2","device_by":"mac-mini-m2"}"#,
    );
    let out = run(&work, "lareys-trendsi");
    assert!(
        checks(&out).contains(&"missing: checkout on mac-mini-m2".to_string()),
        "{}",
        said(&out)
    );
    assert_eq!(code(&out), Some(1));
}

#[test]
fn the_checkout_line_names_the_projects_own_device_and_asks_it() {
    let row = r#"{"name":"lareys-mobile","remote":"github.com/lareysbd/lareys-mobile","checkouts":["{root}/here"],"subdir":null,"device":"nuc","device_by":"mac-mini-m2"}"#;
    let work = world("own-device", row);
    work.mkdir("here");
    fake_ssh(
        &work,
        "nuc",
        r#"{"name":"lareys-mobile","checkouts":["/home/saiful/Sites/github/lareys-mobile"]}"#,
        &[],
    );
    let out = run(&work, "lareys-mobile");
    assert!(
        checks(&out).contains(&"ok: checkout on nuc".to_string()),
        "{}",
        said(&out)
    );
    assert!(work.read("ssh-calls").contains("nuc mem projects --json"));

    // nuc has none.
    fake_ssh(
        &work,
        "nuc",
        r#"{"name":"lareys-mobile","checkouts":[]}"#,
        &[],
    );
    let out = run(&work, "lareys-mobile");
    assert!(
        checks(&out).contains(&"missing: checkout on nuc".to_string()),
        "{}",
        said(&out)
    );
    assert_eq!(code(&out), Some(1));
}

#[test]
fn a_device_that_does_not_answer_is_never_ok() {
    let work = world(
        "silent",
        r#"{"name":"lareys-mobile","remote":"github.com/lareysbd/lareys-mobile","checkouts":[],"subdir":null,"device":"nuc","device_by":"nuc"}"#,
    );
    fake_ssh(&work, "macbook", "", &[]);
    let out = run(&work, "lareys-mobile");
    let line = checks(&out)
        .into_iter()
        .find(|l| l.contains("checkout"))
        .unwrap_or_else(|| panic!("{}", said(&out)));
    assert!(line.starts_with("missing: checkout on nuc"), "{line}");
    assert!(line.contains("did not answer"), "{line}");
    assert_eq!(code(&out), Some(1));
}

#[test]
fn a_project_with_no_device_of_its_own_uses_the_global_choice() {
    // A device key no device wrote is not followed, as the hub does not.
    let work = world(
        "global",
        r#"{"name":"lareys-trendsi","remote":"github.com/saifulapm/lareys-trendsi","checkouts":[],"subdir":null,"device":"mac-mini-m2","device_by":"cloud"}"#,
    );
    fake_ssh(
        &work,
        "macbook",
        r#"{"name":"lareys-trendsi","checkouts":["/x/lareys-trendsi"]}"#,
        &[],
    );
    let hub = local_hub(&work, "macbook-m2");
    let out = Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["project-new", "lareys-trendsi"])
        .env("WORKFLOW_MEM", work.at("mem"))
        .env("XDG_CONFIG_HOME", work.at("config"))
        .env("PASSWORD_STORE_DIR", work.at("pass"))
        .env("HUB_LOCAL_SOCKET", work.at("hub.sock"))
        .env("WORKFLOW_SSH", work.at("ssh"))
        .current_dir(work.path())
        .output()
        .unwrap();
    hub.join().unwrap();
    assert!(
        checks(&out).contains(&"ok: checkout on macbook-m2".to_string()),
        "{}",
        said(&out)
    );
}

#[test]
fn no_global_choice_says_so() {
    let work = world(
        "no-choice",
        r#"{"name":"lareys-trendsi","remote":"github.com/saifulapm/lareys-trendsi","checkouts":[],"subdir":null,"device":null,"device_by":null}"#,
    );
    let out = run(&work, "lareys-trendsi");
    let line = checks(&out)
        .into_iter()
        .find(|l| l.contains("checkout"))
        .unwrap_or_else(|| panic!("{}", said(&out)));
    assert!(line.starts_with("missing: checkout"), "{line}");
    assert_eq!(code(&out), Some(1));
}

const PAWSOME: &str = r#"{"name":"pawsome","remote":"github.com/lareysbd/shopify_themes","checkouts":["{root}/shopify_themes/themes/pawsome"],"subdir":"themes/pawsome","device":"mac-mini-m2","device_by":"mac-mini-m2","cloud_secrets":"SHOPIFY_CLI_THEME_TOKEN=Shopify/theme-access/pawsome-theme\nSTORE_PASSWORD=Shopify/pawsome-demo-password\nSHOPIFY_FLAG_STORE=pawsome-theme.myshopify.com","cloud_secrets_by":"mac-mini-m2"}"#;

#[test]
fn each_theme_secret_is_checked_in_pass_by_its_path_never_its_value() {
    let work = world("pass", PAWSOME);
    work.mkdir("shopify_themes/themes/pawsome");
    work.put(
        "pass/Shopify/theme-access/pawsome-theme.gpg",
        "sekrit-value",
    );
    let out = run(&work, "pawsome");
    let lines = checks(&out);
    assert!(
        lines.contains(&"ok: pass entry Shopify/theme-access/pawsome-theme".to_string()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"missing: pass entry Shopify/pawsome-demo-password".to_string()),
        "{lines:?}"
    );
    assert!(!said(&out).contains("sekrit"));
    assert!(
        !lines.iter().any(|l| l.contains("myshopify")),
        "a store is not a pass entry: {lines:?}"
    );
    assert_eq!(code(&out), Some(1));

    work.put("pass/Shopify/pawsome-demo-password.gpg", "x");
    assert_eq!(
        code(&run(&work, "pawsome")),
        Some(0),
        "{}",
        said(&run(&work, "pawsome"))
    );
}

#[test]
fn theme_secrets_on_another_device_are_checked_there() {
    let work = world(
        "pass-remote",
        &PAWSOME.replace(r#""device":"mac-mini-m2""#, r#""device":"nuc""#),
    );
    fake_ssh(
        &work,
        "nuc",
        r#"{"name":"pawsome","checkouts":["/t/pawsome"]}"#,
        &[
            "~/.password-store/Shopify/theme-access/pawsome-theme.gpg",
            "~/.password-store/Shopify/pawsome-demo-password.gpg",
        ],
    );
    let out = run(&work, "pawsome");
    assert_eq!(
        checks(&out),
        [
            "ok: remote lareysbd/shopify_themes",
            "ok: checkout on nuc",
            "ok: pass entry Shopify/theme-access/pawsome-theme",
            "ok: pass entry Shopify/pawsome-demo-password",
        ],
        "{}",
        said(&out)
    );
    assert_eq!(code(&out), Some(0));
}

#[test]
fn a_theme_with_no_cloud_secrets_key_says_so() {
    let work = world(
        "no-secrets",
        r#"{"name":"tambour","remote":"github.com/lareysbd/shopify_themes","checkouts":["{root}/shopify_themes/themes/tambour"],"subdir":"themes/tambour","device":"mac-mini-m2","device_by":"mac-mini-m2","cloud_secrets":null}"#,
    );
    work.mkdir("shopify_themes/themes/tambour");
    let out = run(&work, "tambour");
    assert!(
        checks(&out).contains(&"missing: cloud-secrets".to_string()),
        "{}",
        said(&out)
    );
    assert_eq!(code(&out), Some(1));
}

#[test]
fn a_checkout_of_the_repo_counts_for_an_app_whose_folder_is_on_its_own_branch() {
    let work = world(
        "repo-checkout",
        r#"{"name":"shopify_themes","remote":"github.com/lareysbd/shopify_themes","checkouts":["{root}/shopify_themes"],"subdir":null},{"name":"tambour","remote":"github.com/lareysbd/shopify_themes","checkouts":["{root}/shopify_themes/themes/tambour"],"subdir":"themes/tambour","device":"mac-mini-m2","device_by":"mac-mini-m2"}"#,
    );
    work.mkdir("shopify_themes/themes/pawsome");
    let out = run(&work, "tambour");
    assert!(
        checks(&out).contains(&"ok: checkout on mac-mini-m2".to_string()),
        "{}",
        said(&out)
    );

    // On another device, a checkout of any project of the repo counts too.
    let work = world(
        "repo-checkout-remote",
        r#"{"name":"tambour","remote":"github.com/lareysbd/shopify_themes","checkouts":[],"subdir":"themes/tambour","device":"nuc","device_by":"mac-mini-m2"}"#,
    );
    fake_ssh(
        &work,
        "nuc",
        r#"{"name":"tambour","remote":"github.com/lareysbd/shopify_themes","checkouts":[]},{"name":"shopify_themes","remote":"github.com/lareysbd/shopify_themes","checkouts":["/s/shopify_themes"]}"#,
        &[],
    );
    let out = run(&work, "tambour");
    assert!(
        checks(&out).contains(&"ok: checkout on nuc".to_string()),
        "{}",
        said(&out)
    );
}

#[test]
fn an_app_inside_a_repo_is_served_by_the_repos_project() {
    let work = world(
        "child",
        r#"{"name":"shopify_apps","remote":"github.com/lareysbd/shopify_apps","checkouts":["{root}/shopify_apps"],"subdir":null},{"name":"palmshop","remote":"github.com/lareysbd/shopify_apps","checkouts":["{root}/shopify_apps/apps/palmshop"],"subdir":"apps/palmshop","device":"mac-mini-m2","device_by":"mac-mini-m2"}"#,
    );
    work.mkdir("shopify_apps/apps/palmshop");
    let out = run(&work, "palmshop");
    let text = said(&out);
    assert!(
        text.lines().any(|l| l
            == r#"palmshop is a mem project of lareysbd/shopify_apps: its Project takes the start line "Run the next milestone of palmshop.""#),
        "{text}"
    );
    assert!(!text.contains("On the work account"), "{text}");
    assert!(!text.contains("On the main account"), "{text}");
    assert!(!text.contains("New Project"), "{text}");
    assert_eq!(
        checks(&out),
        [
            "ok: remote lareysbd/shopify_apps",
            "ok: checkout on mac-mini-m2"
        ],
        "{text}"
    );
    assert_eq!(code(&out), Some(0));
}
