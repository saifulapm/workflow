//! The dogfood skill's cloud paragraph says what a cloud walk drives: web
//! pages through playwright-cli, headless, since the cloud setup installs it.
//! It said "no browser" while the linesofus cloud walks drove Chromium.

use std::fs;
use std::path::Path;

#[test]
fn the_dogfood_skill_walks_cloud_web_pages_with_playwright_cli() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let skill = fs::read_to_string(repo.join("skills/dogfood/SKILL.md")).unwrap();
    let at = skill
        .find("In a cloud session")
        .expect("the skill has a cloud paragraph");
    let paragraph = &skill[at..skill[at..].find("\n\n").map_or(skill.len(), |end| at + end)];
    // One line, so a phrase the wrapping split still matches.
    let paragraph = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(paragraph.contains("playwright-cli"), "{paragraph}");
    assert!(paragraph.contains("headless"), "{paragraph}");
    assert!(!paragraph.contains("no browser"), "{paragraph}");
    assert!(!paragraph.contains("no `playwright-cli`"), "{paragraph}");
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// m2-device: a cloud walk reaches Saiful's logged-in Chromium through
/// `device browser`, under the browser-operator rules, and a run that finds
/// no device asks Saiful which device to use.
#[test]
fn a_cloud_walk_reaches_the_logged_in_browser_through_device() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let skill = one_line(&fs::read_to_string(repo.join("skills/dogfood/SKILL.md")).unwrap());
    assert!(
        !skill.contains("no logged-in browser"),
        "the dogfood skill still says the cloud has no logged-in browser"
    );
    for said in [
        "`device browser <playwright-cli args>`",
        "`device browser detach`",
        "the brief names",
        "exits 3",
    ] {
        assert!(skill.contains(said), "the dogfood skill says {said}");
    }

    let cloud = one_line(&fs::read_to_string(repo.join("skills/orchestrate/cloud.md")).unwrap());
    for said in [
        "`device whoami`",
        "run `device ask`",
        "`I will turn it on` as options",
        "Never park for a device",
        "When `device ask` itself exits 3",
    ] {
        assert!(cloud.contains(said), "cloud.md says {said}");
    }
}

/// m3-ask-not-park: a task for Saiful's hands is a hub question with done
/// and hold, in the cloud page, the plan skill and the dogfood skill; no
/// run parks local-only for it.
#[test]
fn a_task_for_his_hands_is_a_question_never_a_park() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let read = |path: &str| one_line(&fs::read_to_string(repo.join(path)).unwrap());
    let cloud = read("skills/orchestrate/cloud.md");
    assert!(!cloud.contains("local-only"), "cloud.md says local-only");
    assert!(
        !cloud.contains("mem project set device"),
        "cloud.md hands a thread the command that moves the device"
    );
    assert!(!cloud.contains("[local]"), "cloud.md says [local]");
    for said in [
        "--option done --option hold",
        "it parks after 30 minutes like any other question",
        "a handoff starting `parked: Saiful holds <milestone> (#<id>)`",
    ] {
        assert!(cloud.contains(said), "cloud.md says {said}");
    }
    let plan = read("skills/plan/SKILL.md");
    assert!(
        !plan.contains("then device:"),
        "the plan skill says then device:"
    );
    assert!(
        plan.contains("a task for Saiful's hands: a question whose options are `done` and `hold`"),
        "the plan skill's rule 9"
    );
    let dogfood = read("skills/dogfood/SKILL.md");
    assert!(
        !dogfood.contains("local-only"),
        "the dogfood skill says local-only"
    );
    assert!(
        dogfood.contains("needs Saiful's hands: ask it with `mem ask --for saiful"),
        "the dogfood skill"
    );
}

/// m4-shopify: a theme walks headless on `workflow shopify theme-dev`, an app
/// on its staging deploy, typed into `device tmux` since m5-device-deploy, in
/// the admin through `device browser`, and a token
/// only Saiful can make is asked for, never filed as a finding.
#[test]
fn a_cloud_walk_says_how_shopify_themes_and_apps_are_walked() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let skill = one_line(&fs::read_to_string(repo.join("skills/dogfood/SKILL.md")).unwrap());
    for said in [
        "`workflow shopify theme-dev <theme>`",
        "http://127.0.0.1:9292",
        "`device tmux send 'workflow shopify deploy <app>'`",
        "`walk: <url>`",
        "`device browser goto <url>`",
        "never production",
        "`mem ask --for saiful`",
    ] {
        assert!(skill.contains(said), "the dogfood skill says {said}");
    }
}

/// c1-phone-by-agent: a check of a page on Android goes to a local agent
/// through `device amx new`, since the wall lets no port through but adb's;
/// the build stays behind the wall and the agent runs none of the branch's
/// programs. ebdfy-ui m29 sent its emulator check as `--no-wall` lines.
#[test]
fn an_android_page_check_goes_to_a_local_agent() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let cloud = one_line(&fs::read_to_string(repo.join("skills/orchestrate/cloud.md")).unwrap());
    let at = cloud
        .find("- A check of a page on Android")
        .expect("cloud.md has a paragraph on Android page checks");
    let check = &cloud[at..cloud[at + 2..]
        .find(" - ")
        .map_or(cloud.len(), |end| at + 2 + end)];
    for said in [
        "`device amx new",
        "never through `device tmux`",
        "the wall lets no port through but adb's",
        "`device tmux send`, behind the wall",
        "`~/.cache/workflow-cloud/<repo>`",
        "none of the branch's programs",
        "plain file server",
        "`dots-emulator <avd>`",
        "`dots-emulator stop <avd>`",
        "never the first serial `adb devices` lists",
        "one `device tmux send --no-wall` line",
        "`busy: <project>` (exit 5)",
        "never fall back to `device tmux`",
        // A device with no emulator is a question for Saiful.
        "`dots-emulator: this machine has no Android emulator`",
        "`workflow device use <name>`",
        "--option done --option hold",
        "Never send the check with `--no-wall` instead",
        // A device switch leaves the build and the agent on the old device.
        "stop the agent with `device amx stop <name>` before you ask",
        "build again with `device tmux send` on the new device",
        // An emulator the agent did not start is one it leaves up.
        "`adb -s <serial> emu avd name`",
        "send no other `device tmux` line until the agent's result is in",
        "stop the agent with `device amx stop <name>` once its result is in",
    ] {
        assert!(
            check.contains(said),
            "cloud.md's Android check says {said}: {check}"
        );
    }
    let tmux = &cloud[cloud.find("- Terminal work on the device").unwrap()..];
    let tmux = &tmux[..tmux[2..].find(" - ").unwrap() + 2];
    assert!(
        tmux.contains(
            "adb for what needs no page from the job (installing a build, reading a log)"
        ),
        "cloud.md's device tmux paragraph: {tmux}"
    );
}

/// c1-phone-by-agent: a cloud walk's Android page step walks in the device's
/// emulator through `device amx new`, and the plan skill counts that
/// emulator among what a cloud session reaches; a phone in hand stays a
/// question.
#[test]
fn a_cloud_walk_checks_an_android_page_in_the_devices_emulator() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let skill = fs::read_to_string(repo.join("skills/dogfood/SKILL.md")).unwrap();
    let at = skill
        .find("In a cloud session")
        .expect("the skill has a cloud paragraph");
    let paragraph =
        one_line(&skill[at..skill[at..].find("\n\n").map_or(skill.len(), |end| at + end)]);
    for said in [
        "`device amx new`",
        "emulator",
        "a phone in his hand",
        "--option done --option hold",
    ] {
        assert!(
            paragraph.contains(said),
            "the dogfood cloud paragraph says {said}: {paragraph}"
        );
    }
    let plan = one_line(&fs::read_to_string(repo.join("skills/plan/SKILL.md")).unwrap());
    for said in [
        "an Android emulator on the device, through `device amx`",
        "a phone or tablet in hand",
    ] {
        assert!(plan.contains(said), "the plan skill's rule 9 says {said}");
    }
}

/// A job whose line prints nothing gets a status line from `device tmux
/// read`; the cloud page says what it means and how to keep output coming.
#[test]
fn the_cloud_page_reads_a_silent_job_s_status_line() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let cloud = one_line(&fs::read_to_string(repo.join("skills/orchestrate/cloud.md")).unwrap());
    for said in [
        "device: no output for <N> min; the line still waits on: <names>",
        "the line still waits on",
        "`grep --line-buffered`",
        "`server >/tmp/server.log 2>&1 &`",
        "is run again, fixed, after `device tmux end`",
    ] {
        assert!(cloud.contains(said), "cloud.md says {said}");
    }
}
