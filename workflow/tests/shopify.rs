//! `workflow shopify` through the real binary, against a fake `shopify` on
//! PATH: it records each call's argv and the Shopify variables it was given,
//! and answers `theme list` and `theme push` with the JSON Shopify CLI 5.0.1
//! prints (its `--json-schema`; the rejected password is its real output
//! for a wrong Theme Access password, 2026-11-09).

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::Output;
use std::sync::RwLock;

use scratch::Scratch;

/// A fixture written while another test forks leaves "Text file busy" on
/// exec, so fixtures are written under the write lock and spawns wait.
static FORKS: RwLock<()> = RwLock::new(());

const REJECTED: &str = r#"{"error":{"type":"abort","message":"The Theme Access password was rejected for the store pawsome-theme.myshopify.com.","tryMessage":"Theme Access passwords only work on the store they were generated for, and only with its permanent .myshopify.com domain.","nextSteps":["Check that the password was created in the Theme Access app on this store and has not been deleted."]}}"#;

/// A themes repo with themes/pawsome, and a fake shopify whose `theme list`
/// prints `list.json` and exits `list.code`.
fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.mkdir("repo/themes/pawsome/layout");
    // Built: pawsome's assets come from its build and are not in git.
    work.put("repo/themes/pawsome/assets/base.css", "body {}\n");
    work.put("list.json", r#"{"themes":[]}"#);
    work.put("list.code", "0");
    let at = |rel: &str| work.at(rel).display().to_string();
    work.script(
        "bin/shopify",
        &format!(
            "echo \"$* | token=$SHOPIFY_CLI_THEME_TOKEN store=$SHOPIFY_FLAG_STORE password=$SHOPIFY_FLAG_STORE_PASSWORD\" >> {calls}\n\
             case \"$1 $2\" in\n\
             'theme list') cat {list}; echo '<claude-code-hint v=\"1\" type=\"plugin\" />' >&2; exit $(cat {code});;\n\
             'theme push') echo '{{\"status\":\"success\",\"changed\":true,\"theme\":{{\"id\":\"777\",\"name\":\"pawsome (cloud)\",\"role\":\"unpublished\",\"storeDomain\":\"pawsome-theme.myshopify.com\",\"previewUrl\":null,\"editorUrl\":null}},\"issues\":[]}}';;\n\
             'theme dev') echo serving;;\n\
             esac",
            calls = at("calls"),
            list = at("list.json"),
            code = at("list.code"),
        ),
    );
    work
}

fn shopify(work: &Scratch, cwd: &str, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"));
    cmd.arg("shopify")
        .args(args)
        .current_dir(work.at(cwd))
        .env(
            "PATH",
            format!(
                "{}:{}",
                work.at("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("HOME", work.at("home"));
    for (key, _) in std::env::vars() {
        if key.starts_with("SHOPIFY_")
            || key.contains("_SHOPIFY_")
            || key.ends_with("_PASSWORD")
            || key.starts_with("CLOUDFLARE_")
            || key == "NODE_USE_ENV_PROXY"
        {
            cmd.env_remove(key);
        }
    }
    for (key, value) in vars {
        cmd.env(key, value);
    }
    cmd.output().unwrap()
}

fn calls(work: &Scratch) -> Vec<String> {
    if !work.has("calls") {
        return vec![];
    }
    work.read("calls").lines().map(str::to_string).collect()
}

const PAWSOME: [(&str, &str); 3] = [
    ("PAWSOME_SHOPIFY_CLI_THEME_TOKEN", "shptka_abc"),
    ("PAWSOME_SHOPIFY_FLAG_STORE", "pawsome-theme.myshopify.com"),
    ("PAWSOME_STORE_PASSWORD", "hunter2"),
];

/// With the theme's token missing, nothing reaches Shopify, and the line
/// says which token, so the task parks on it.
#[test]
fn a_theme_with_no_token_says_so_and_starts_nothing() {
    let work = world("shopify-theme-no-token");
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME[1..]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("SHOPIFY_CLI_THEME_TOKEN is not set for pawsome")
            && said.contains("PAWSOME_SHOPIFY_CLI_THEME_TOKEN"),
        "{said}"
    );
    assert!(calls(&work).is_empty(), "{:?}", calls(&work));
}

#[test]
fn a_theme_with_no_store_says_so_and_starts_nothing() {
    let work = world("shopify-theme-no-store");
    let out = shopify(
        &work,
        "repo",
        &["theme-dev", "pawsome"],
        &[PAWSOME[0], PAWSOME[2]],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("SHOPIFY_FLAG_STORE is not set for pawsome")
            && said.contains("PAWSOME_SHOPIFY_FLAG_STORE"),
        "{said}"
    );
    assert!(calls(&work).is_empty());
}

/// Development stores keep a password page, so a walk without the password
/// would judge that page; only Saiful can give it.
#[test]
fn a_theme_with_no_store_password_says_so_and_starts_nothing() {
    let work = world("shopify-theme-no-password");
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME[..2]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("PAWSOME_STORE_PASSWORD"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// theme dev syncs the cloud theme whole, deletes included: started before
/// the build, it would strip the store's copy of every built asset.
#[test]
fn an_unbuilt_theme_is_refused_before_it_reaches_the_store() {
    let work = world("shopify-theme-unbuilt");
    std::fs::remove_dir_all(work.at("repo/themes/pawsome/assets")).unwrap();
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("build"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// A fresh VM has no development theme of its own, so each would make
/// another; theme dev instead runs on one unpublished theme the cloud keeps
/// by name, made once, which every start syncs whole from the checkout.
#[test]
fn a_first_theme_dev_makes_the_cloud_theme_then_serves_it_on_9292() {
    let work = world("shopify-theme-first");
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout)
            .trim_end()
            .lines()
            .last(),
        Some("serving")
    );
    let dir = work.at("repo/themes/pawsome").display().to_string();
    let env = "token=shptka_abc store=pawsome-theme.myshopify.com password=hunter2";
    assert_eq!(
        calls(&work),
        [
            format!("theme list --name pawsome (cloud) --json --no-input --path {dir} | {env}"),
            format!(
                "theme push --unpublished --theme pawsome (cloud) --json --no-input --path {dir} | {env}"
            ),
            format!(
                "theme dev --theme 777 --host 127.0.0.1 --port 9292 --no-input --path {dir} | {env}"
            ),
        ]
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("http://127.0.0.1:9292"),
        "{out:?}"
    );
}

/// A later VM finds the cloud theme by its exact name and makes none.
#[test]
fn a_later_theme_dev_reuses_the_cloud_theme_from_the_theme_folder() {
    let work = world("shopify-theme-again");
    work.put(
        "list.json",
        r#"{"themes":[{"id":"555","name":"pawsome (cloud) old","role":"unpublished","processing":false},{"id":"556","name":"pawsome (cloud)","role":"unpublished","processing":false}]}"#,
    );
    let out = shopify(
        &work,
        "repo/themes/pawsome/layout",
        &["theme-dev", "pawsome"],
        &PAWSOME,
    );
    assert!(out.status.success(), "{out:?}");
    let made = calls(&work);
    assert_eq!(made.len(), 2, "{made:?}");
    assert!(made[1].starts_with("theme dev --theme 556 "), "{made:?}");
}

/// A password Shopify refuses (deleted, or for another store) names the
/// theme's own variable and parks, rather than failing like a bug.
#[test]
fn a_rejected_theme_token_names_the_variable() {
    let work = world("shopify-theme-rejected");
    work.put("list.json", REJECTED);
    work.put("list.code", "1");
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("PAWSOME_SHOPIFY_CLI_THEME_TOKEN was rejected")
            && said.contains("(The Theme Access password was rejected for the store pawsome-theme.myshopify.com.)"),
        "{said}"
    );
    assert_eq!(calls(&work).len(), 1);
}

#[test]
fn a_theme_with_no_folder_is_refused() {
    let work = world("shopify-theme-no-folder");
    let out = shopify(&work, "repo", &["theme-dev", "tambour"], &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("themes/tambour"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// Shopify CLI 5.0.1's real output for a service account token it refuses
/// (a made-up token, 2026-11-09).
const SA_REJECTED: &str =
    "╭─ error ──────────────────────────────────────────────────────────────────────╮
│                                                                              │
│  The token in SHOPIFY_SERVICE_ACCOUNT_TOKEN was rejected: it has been        │
│  revoked, has expired, or isn't a valid token.                               │
│                                                                              │
│  Check the value, or generate a new token in the Shopify admin under         │
│  Settings > Users > Security.                                                │
│                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯";

const PRODUCTION: &str = "client_id = \"e3cc0000000000000000000000000000\"\nname = \"EventSmith\"\nhandle = \"eventsmith\"\napplication_url = \"https://eventsmith.lareys.dev\"\n";

const STAGING: &str = "client_id = \"57a90000000000000000000000000000\"\nname = \"EventSmith staging\"\nhandle = \"eventsmith-staging\"\napplication_url = \"https://eventsmith-staging.lareys.dev\"\nembedded = true\n\n[build]\nautomatically_update_urls_on_dev = false\ndev_store_url = \"lareys-development.myshopify.com\"\n\n[access_scopes]\nscopes = \"read_products\"\n";

/// EventSmith's wrangler.jsonc: production's Worker and D1 at the top, and
/// staging's own under `env.staging`, in JSONC as wrangler reads it.
const WRANGLER: &str = r#"{
  // Production: a GitHub Release deploys it.
  "name": "eventsmith",
  "d1_databases": [
    { "binding": "DB", "database_name": "eventsmith-na", "database_id": "aaaa0000-0000-0000-0000-000000000000" },
  ],
  /* The staging app's Worker, "its own". */
  "env": {
    "staging": {
      "name": "eventsmith-staging",
      "account_id": "7d330000000000000000000000000000",
      "vars": { "SHOPIFY_APP_URL": "https://eventsmith-staging.lareys.dev" },
      "d1_databases": [
        {
          "binding": "DB",
          "database_name": "eventsmith-staging", // its own
          "database_id": "bbbb0000-0000-0000-0000-000000000000",
          "migrations_dir": "./db/migrations",
        },
      ],
    },
  },
}
"#;

/// An apps repo with apps/eventsmith's production and staging configs and
/// its wrangler.jsonc, and fakes that each append their call to `calls`: a
/// shopify whose `app deploy` prints `deploy.out` and exits `deploy.code`;
/// the app's own wrangler, whose d1 call exits `migrate.code` and whose
/// deploy exits `worker.code`; and a pnpm whose `run build` writes the
/// Vite plugin's redirected config for the Worker named in `built.name`.
fn apps(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.put("repo/apps/eventsmith/shopify.app.toml", PRODUCTION);
    work.put("repo/apps/eventsmith/shopify.app.staging.toml", STAGING);
    work.put("repo/apps/eventsmith/wrangler.jsonc", WRANGLER);
    work.put(
        "repo/apps/eventsmith/package.json",
        r#"{"name":"eventsmith"}"#,
    );
    work.put("deploy.out", "New version released to users.");
    work.put("deploy.code", "0");
    work.put("migrate.code", "0");
    work.put("worker.code", "0");
    work.put("token.code", "0");
    work.put("built.name", "eventsmith-staging");
    let at = |rel: &str| work.at(rel).display().to_string();
    work.script(
        "repo/apps/eventsmith/node_modules/.bin/wrangler",
        &format!(
            "echo \"wrangler $* | token=$CLOUDFLARE_API_TOKEN proxy=$NODE_USE_ENV_PROXY\" >> {calls}\n\
             case \"$1\" in\n\
             d1) [ \"$2\" = list ] && exit $(cat {token}); echo 'Migrations to be applied: 0001_init.sql'; exit $(cat {migrate});;\n\
             deploy) echo 'Deployed eventsmith-staging triggers'; exit $(cat {worker});;\n\
             esac",
            calls = at("calls"),
            migrate = at("migrate.code"),
            worker = at("worker.code"),
            token = at("token.code"),
        ),
    );
    work.script(
        "bin/rustup",
        &format!("echo \"rustup $*\" >> {calls}", calls = at("calls")),
    );
    work.script(
        "bin/pnpm",
        &format!(
            "echo \"pnpm $* | env=$CLOUDFLARE_ENV proxy=$NODE_USE_ENV_PROXY in=$(basename $(pwd))\" >> {calls}\n\
             if [ \"$1 $2\" = 'run build' ]; then\n\
             mkdir -p build/server .wrangler/deploy\n\
             printf '{{\"name\":\"%s\",\"targetEnvironment\":\"staging\"}}' \"$(cat {built})\" > build/server/wrangler.json\n\
             echo '{{\"configPath\":\"../../build/server/wrangler.json\"}}' > .wrangler/deploy/config.json\n\
             fi",
            calls = at("calls"),
            built = at("built.name"),
        ),
    );
    work.script(
        "bin/shopify",
        &format!(
            "echo \"$* | token=$SHOPIFY_SERVICE_ACCOUNT_TOKEN automation=$SHOPIFY_APP_AUTOMATION_TOKEN partners=$SHOPIFY_CLI_PARTNERS_TOKEN\" >> {calls}\n\
             cat {out}\nexit $(cat {code})",
            calls = at("calls"),
            out = at("deploy.out"),
            code = at("deploy.code"),
        ),
    );
    work
}

const EVENTSMITH: (&str, &str) = ("EVENTSMITH_SHOPIFY_SERVICE_ACCOUNT_TOKEN", "shpsa_abc");

/// The staging deploy takes the app's own token, never production's
/// config, and says where in the admin to walk it.
#[test]
fn an_app_deploys_its_staging_config_with_its_own_token() {
    let work = apps("shopify-deploy");
    let out = shopify(
        &work,
        "repo",
        &["deploy", "eventsmith"],
        &[
            EVENTSMITH,
            // The CLI fails when either is set beside a service account token.
            ("SHOPIFY_APP_AUTOMATION_TOKEN", "old"),
            ("SHOPIFY_CLI_PARTNERS_TOKEN", "older"),
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let dir = work.at("repo/apps/eventsmith").display().to_string();
    assert_eq!(
        calls(&work).last().map(String::as_str),
        Some(format!(
            "app deploy --config staging --allow-updates --no-input --path {dir} | token=shpsa_abc automation= partners="
        ).as_str())
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("New version released to users."), "{said}");
    assert_eq!(
        said.trim_end().lines().last(),
        Some("walk: https://admin.shopify.com/store/lareys-development/apps/eventsmith-staging"),
        "{said}"
    );
}

#[test]
fn an_app_with_no_token_deploys_nothing() {
    let work = apps("shopify-deploy-no-token");
    let out = shopify(
        &work,
        "repo/apps/eventsmith",
        &["deploy", "eventsmith"],
        &[],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("EVENTSMITH_SHOPIFY_SERVICE_ACCOUNT_TOKEN is not set"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// No staging config yet: only Saiful can make the staging app.
#[test]
fn an_app_with_no_staging_config_deploys_nothing() {
    let work = apps("shopify-deploy-no-staging");
    std::fs::remove_file(work.at("repo/apps/eventsmith/shopify.app.staging.toml")).unwrap();
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("shopify.app.staging.toml"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// A staging config linked to the production app would deploy production.
#[test]
fn a_staging_config_of_the_production_app_is_refused() {
    let work = apps("shopify-deploy-production");
    work.put(
        "repo/apps/eventsmith/shopify.app.staging.toml",
        &STAGING.replace(
            "57a90000000000000000000000000000",
            "e3cc0000000000000000000000000000",
        ),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("production"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// A token past its 1, 3 or 6 months names the app's variable and parks,
/// rather than failing like a bug.
#[test]
fn a_rejected_service_account_token_names_the_variable() {
    let work = apps("shopify-deploy-rejected");
    work.put("deploy.out", SA_REJECTED);
    work.put("deploy.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("EVENTSMITH_SHOPIFY_SERVICE_ACCOUNT_TOKEN was rejected")
            && said.contains("expired"),
        "{said}"
    );
}

#[test]
fn a_deploy_that_fails_otherwise_exits_1() {
    let work = apps("shopify-deploy-fails");
    work.put("deploy.out", "Build failed: extensions/foo");
    work.put("deploy.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Build failed"),
        "{out:?}"
    );
}

/// With no dev store or handle there is no admin page to walk, so nothing
/// is deployed and the line says what to add.
#[test]
fn a_staging_config_with_no_dev_store_deploys_nothing() {
    let work = apps("shopify-deploy-no-store");
    work.put(
        "repo/apps/eventsmith/shopify.app.staging.toml",
        &STAGING.replace("dev_store_url = \"lareys-development.myshopify.com\"\n", ""),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("dev_store_url"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// The CLI wraps its error box to the terminal's width: narrower, the
/// variable and "was rejected" fall on different lines.
#[test]
fn a_rejected_token_in_a_narrow_box_still_names_the_variable() {
    let work = apps("shopify-deploy-rejected-narrow");
    work.put(
        "deploy.out",
        "╭─ error ──────────────────────────────╮\n│  The token in                        │\n│  SHOPIFY_SERVICE_ACCOUNT_TOKEN was   │\n│  rejected: it has been revoked, has  │\n│  expired, or isn't a valid token.    │\n╰──────────────────────────────────────╯",
    );
    work.put("deploy.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("EVENTSMITH_SHOPIFY_SERVICE_ACCOUNT_TOKEN was rejected"),
        "{out:?}"
    );
}

/// The release's order, for staging: the D1 migrations, then the build and
/// the Worker, then the Shopify app version, so the webhooks and extensions
/// it publishes point at a Worker already live. Wrangler reaches the proxy,
/// which adds the Cloudflare token, only with NODE_USE_ENV_PROXY and a token
/// variable; pnpm hangs with NODE_USE_ENV_PROXY, so it gets neither. The
/// build runs outside turbo, whose cache would hand back production's.
#[test]
fn an_app_deploys_its_staging_worker_before_its_shopify_config() {
    let work = apps("shopify-deploy-worker");
    let out = shopify(
        &work,
        "repo",
        &["deploy", "eventsmith"],
        &[EVENTSMITH, ("NODE_USE_ENV_PROXY", "1")],
    );
    assert!(out.status.success(), "{out:?}");
    let made = calls(&work);
    assert_eq!(
        made[..5],
        [
            "wrangler d1 list --json --env staging --config wrangler.jsonc | token=proxy-injected proxy=1",
            "wrangler d1 migrations apply eventsmith-staging --remote --env staging --config wrangler.jsonc | token=proxy-injected proxy=1",
            "pnpm turbo run build --filter=eventsmith^... | env= proxy= in=repo",
            "pnpm run build | env=staging proxy= in=eventsmith",
            "wrangler deploy --env staging | token=proxy-injected proxy=1",
        ],
        "{made:?}"
    );
    assert!(
        made[5].starts_with("app deploy --config staging "),
        "{made:?}"
    );
    assert_eq!(made.len(), 6, "{made:?}");
    assert!(
        made.iter().all(|call| !call.contains("eventsmith-na")),
        "{made:?}"
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("Migrations to be applied: 0001_init.sql"),
        "{said}"
    );
    assert_eq!(
        said.trim_end().lines().last(),
        Some("walk: https://admin.shopify.com/store/lareys-development/apps/eventsmith-staging"),
        "{said}"
    );
}

/// A failed migration leaves the Worker and the app as they were.
#[test]
fn a_failed_migration_deploys_nothing() {
    let work = apps("shopify-deploy-migration-fails");
    work.put("migrate.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("the staging D1 migrations of eventsmith-staging failed"),
        "{out:?}"
    );
    assert_eq!(calls(&work).len(), 2, "{:?}", calls(&work));
}

#[test]
fn a_failed_worker_deploy_leaves_the_shopify_config() {
    let work = apps("shopify-deploy-worker-fails");
    work.put("worker.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the staging Worker deploy failed"),
        "{out:?}"
    );
    assert!(
        calls(&work)
            .iter()
            .all(|call| !call.starts_with("app deploy")),
        "{:?}",
        calls(&work)
    );
}

/// A build that came out for another Worker, as production's would, is
/// never deployed: `wrangler deploy --env staging` refuses a mismatch only
/// when the build names an environment.
#[test]
fn a_build_for_another_worker_is_not_deployed() {
    let work = apps("shopify-deploy-wrong-build");
    work.put("built.name", "eventsmith");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("the build is for the Worker eventsmith, not eventsmith-staging"),
        "{out:?}"
    );
    assert!(
        calls(&work)
            .iter()
            .all(|call| !call.starts_with("wrangler deploy") && !call.starts_with("app deploy")),
        "{:?}",
        calls(&work)
    );
}

/// Calls past the read-only `wrangler d1 list`: what a refusal must not reach.
fn deploys(work: &Scratch) -> Vec<String> {
    calls(work)
        .into_iter()
        .filter(|call| !call.starts_with("wrangler d1 list"))
        .collect()
}

#[test]
fn an_app_with_no_staging_worker_deploys_nothing() {
    let work = apps("shopify-deploy-no-env");
    work.put(
        "repo/apps/eventsmith/wrangler.jsonc",
        r#"{ "name": "eventsmith", }"#,
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(
            "eventsmith has no env.staging in wrangler.jsonc: its staging Worker is not set up"
        ),
        "{out:?}"
    );
    assert!(calls(&work).is_empty(), "{:?}", calls(&work));
}

/// A staging app on production's URL sends its admin frame, webhooks and
/// pixel to production's Worker.
#[test]
fn a_staging_config_on_production_s_url_is_refused() {
    let work = apps("shopify-deploy-production-url");
    work.put(
        "repo/apps/eventsmith/shopify.app.staging.toml",
        &STAGING.replace("eventsmith-staging.lareys.dev", "eventsmith.lareys.dev"),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("shopify.app.staging.toml")
            && said.contains("shopify.app.toml")
            && said.contains("https://eventsmith.lareys.dev"),
        "{said}"
    );
    assert!(calls(&work).is_empty(), "{:?}", calls(&work));
}

#[test]
fn a_staging_worker_on_production_s_database_is_refused() {
    let work = apps("shopify-deploy-production-db");
    work.put(
        "repo/apps/eventsmith/wrangler.jsonc",
        &WRANGLER.replace(
            "bbbb0000-0000-0000-0000-000000000000",
            "aaaa0000-0000-0000-0000-000000000000",
        ),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("aaaa0000-0000-0000-0000-000000000000"),
        "{out:?}"
    );
    assert!(deploys(&work).is_empty(), "{:?}", calls(&work));
}

#[test]
fn a_staging_worker_named_as_production_s_is_refused() {
    let work = apps("shopify-deploy-production-worker");
    work.put(
        "repo/apps/eventsmith/wrangler.jsonc",
        &WRANGLER.replace(
            "\"name\": \"eventsmith-staging\"",
            "\"name\": \"eventsmith\"",
        ),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the Worker eventsmith"),
        "{out:?}"
    );
    assert!(deploys(&work).is_empty(), "{:?}", calls(&work));
}

/// With no network secret for api.cloudflare.com, the proxy adds no token
/// and wrangler is refused: only Saiful can add one.
#[test]
fn no_cloudflare_token_deploys_nothing() {
    let work = apps("shopify-deploy-no-cloudflare");
    work.put("token.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("add api.cloudflare.com as a network secret of the shopify_apps environment"),
        "{out:?}"
    );
    assert!(deploys(&work).is_empty(), "{:?}", calls(&work));
}

/// wrangler finds a D1 binding with no id by its name, so a staging
/// database named as production's is production's, id or none.
#[test]
fn a_staging_database_named_as_production_s_is_refused() {
    let work = apps("shopify-deploy-production-db-name");
    work.put(
        "repo/apps/eventsmith/wrangler.jsonc",
        &WRANGLER
            .replace(
                "\"database_name\": \"eventsmith-staging\"",
                "\"database_name\": \"eventsmith-na\"",
            )
            .replace(
                "\"database_id\": \"bbbb0000-0000-0000-0000-000000000000\",",
                "",
            ),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("eventsmith-na"),
        "{out:?}"
    );
    assert!(deploys(&work).is_empty(), "{:?}", calls(&work));
}

/// Two apps' threads may deploy at once: each touches only its own
/// `<app>-staging`, so an env.staging copied from another app is refused.
#[test]
fn a_staging_worker_named_for_another_app_is_refused() {
    let work = apps("shopify-deploy-other-app");
    work.put(
        "repo/apps/eventsmith/wrangler.jsonc",
        &WRANGLER.replace(
            "\"name\": \"eventsmith-staging\"",
            "\"name\": \"shortcart-staging\"",
        ),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("shortcart-staging") && said.contains("eventsmith-staging"),
        "{said}"
    );
    assert!(deploys(&work).is_empty(), "{:?}", calls(&work));
}

/// The staging app's redirect, proxy and pixel URLs must leave production's
/// host too, not only its application_url.
#[test]
fn a_staging_config_with_any_production_url_is_refused() {
    let work = apps("shopify-deploy-production-redirect");
    work.put(
        "repo/apps/eventsmith/shopify.app.staging.toml",
        &format!(
            "{STAGING}\n[auth]\nredirect_urls = [\"https://eventsmith.lareys.dev/auth/callback\"]\n"
        ),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("shopify.app.staging.toml") && said.contains("https://eventsmith.lareys.dev"),
        "{said}"
    );
    assert!(calls(&work).is_empty(), "{:?}", calls(&work));
}

/// Without an account, wrangler lists the token's accounts, and stops when
/// it reaches more than one; the thread's token reaches only the paths of
/// one account.
#[test]
fn a_staging_worker_with_no_account_is_refused() {
    let work = apps("shopify-deploy-no-account");
    work.put(
        "repo/apps/eventsmith/wrangler.jsonc",
        &WRANGLER.replace("\"account_id\": \"7d330000000000000000000000000000\",", ""),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("account_id"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty(), "{:?}", calls(&work));
}

/// shopify app deploy builds each Rust Function for wasm, and a fresh
/// thread's toolchain has no wasm target, as the release adds it too.
#[test]
fn an_app_with_rust_functions_gets_the_wasm_target_before_its_shopify_config() {
    let work = apps("shopify-deploy-wasm");
    work.put(
        "repo/apps/eventsmith/extensions/gate/Cargo.toml",
        "[package]\nname = \"gate\"\n",
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert!(out.status.success(), "{out:?}");
    let made = calls(&work);
    let wasm = made
        .iter()
        .position(|c| c == "rustup target add wasm32-unknown-unknown")
        .unwrap_or_else(|| panic!("{made:?}"));
    let shopify = made
        .iter()
        .position(|c| c.starts_with("app deploy"))
        .unwrap_or_else(|| panic!("{made:?}"));
    assert!(wasm < shopify, "{made:?}");
}
