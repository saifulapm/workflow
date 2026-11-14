//! The wall a thread's tmux session runs behind: pasta gives it a network
//! of its own, the internet and of the device only adb's port, and bwrap a
//! home that holds the toolchain, its caches, the deploy logins and the
//! session's worktree, and none of Saiful's secrets.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::device::find_in;

/// adb's server, the one port of the device the wall reaches.
const ADB: &str = "5037";

/// The resolver's address inside the wall; pasta sends its queries on to
/// the device's own resolver, which is on a loopback address the wall
/// cannot reach.
const DNS: &str = "169.254.1.1";

/// Not the internet: the LAN, the tailnet and link-local, where the
/// device's own addresses and the other devices are. Set inside pasta's
/// namespace before bwrap starts, where the walled shell cannot undo it.
const RULES: &str = "table inet wall {
  chain out {
    type filter hook output priority 0; policy accept;
    ip daddr != 169.254.1.1 ip daddr { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10, 169.254.0.0/16 } drop
    ip6 daddr { fc00::/7, fe80::/10 } drop
  }
}
";

/// The toolchain the PATH's shims and links reach, read-only: mise's
/// tools, its settings and the folders it trusts, rustup's toolchains,
/// Composer's global packages.
const TOOLS: [&str; 5] = [
    ".local/share/mise",
    ".config/mise",
    ".local/state/mise",
    ".rustup",
    ".config/composer/vendor",
];

/// Writable: the tool caches, and the Cloudflare and Shopify logins, whose
/// tokens a deploy may refresh.
const KEPT: [&str; 10] = [
    ".local/share/pnpm",
    ".cache/pnpm",
    ".npm",
    ".cache/composer",
    ".cargo/registry",
    ".cargo/git",
    ".cache/mise",
    ".config/.wrangler",
    ".config/cloudflare",
    ".config/shopify-cli-kit-nodejs",
];

/// The user environment's variables a walled shell keeps; the rest may
/// hold a key (TMDB_API_KEY) or name a socket the wall hides.
const ENV: [&str; 13] = [
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "SHELL",
    "EDITOR",
    "VISUAL",
    "ANDROID_HOME",
    "COMPOSER_HOME",
    "PNPM_HOME",
    "GOPATH",
    "GOBIN",
    "PATH",
];

/// The command that runs `shell` behind the wall in `folder`, with the
/// user's environment `env` cut to ENV, plus `extra`. Its two files,
/// the rules and the resolver, go in `state`. Err says why it cannot.
pub(crate) fn command(
    env: &[(String, String)],
    folder: &Path,
    extra: &[(&str, &str)],
    shell: &[String],
    state: &Path,
) -> Result<Vec<String>, String> {
    let var = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let path = var("PATH").unwrap_or_default();
    let find = |program: &str| {
        find_in(path, program)
            .or_else(|| find_in("/usr/bin:/usr/sbin", program))
            .ok_or_else(|| format!("{program} was not found"))
    };
    let (pasta, bwrap, bash, nft) = (find("pasta")?, find("bwrap")?, find("bash")?, find("nft")?);
    let home = PathBuf::from(var("HOME").ok_or("HOME is not set")?);
    let me = std::fs::metadata("/proc/self").map_err(|e| format!("cannot read /proc/self: {e}"))?;
    std::fs::create_dir_all(state).map_err(|e| format!("cannot make {}: {e}", state.display()))?;
    let rules = state.join("rules.nft");
    let resolver = state.join("resolv.conf");
    for (file, text) in [
        (&rules, RULES.to_string()),
        (&resolver, format!("nameserver {DNS}\n")),
    ] {
        std::fs::write(file, text).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    }

    let mut wall = Wall::default();
    let (uid, gid) = (me.uid().to_string(), me.gid().to_string());
    wall.add(&["--unshare-user", "--uid", &uid, "--gid", &gid]);
    wall.add(&[
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup-try",
        "--die-with-parent",
    ]);
    wall.add(&["--ro-bind", "/usr", "/usr"]);
    for top in ["/bin", "/sbin", "/lib", "/lib64"] {
        match std::fs::read_link(top) {
            Ok(to) => wall.add(&["--symlink", &to.to_string_lossy(), top]),
            Err(_) if Path::new(top).is_dir() => wall.add(&["--ro-bind", top, top]),
            Err(_) => {}
        }
    }
    wall.add(&["--ro-bind", "/etc", "/etc", "--ro-bind-try", "/opt", "/opt"]);
    wall.add(&["--proc", "/proc", "--dev", "/dev"]);
    for empty in ["/tmp", "/run", "/var"] {
        wall.add(&["--tmpfs", empty]);
    }
    wall.add(&["--dir", "/var/tmp"]);
    // /etc/resolv.conf is often a link into /run, which is empty here.
    let resolv = std::fs::canonicalize("/etc/resolv.conf")
        .unwrap_or_else(|_| PathBuf::from("/etc/resolv.conf"));
    wall.bind("--ro-bind", &resolver, &resolv);
    wall.add(&["--tmpfs", &home.to_string_lossy()]);
    let path_dirs = path
        .split(':')
        .map(PathBuf::from)
        .filter(|dir| dir.starts_with(&home) && dir.is_dir());
    for dir in path_dirs.chain(TOOLS.iter().map(|rel| home.join(rel))) {
        if dir.exists() {
            wall.bind("--ro-bind", &dir, &dir);
        }
    }
    for rel in KEPT {
        let dir = home.join(rel);
        if dir.exists() {
            wall.bind("--bind", &dir, &dir);
        }
    }
    // The worktree writable, and every git file read-only: the checkout's
    // .git, the worktree's own git folder in it, and the worktree's .git
    // file. The hub runs git here outside the wall, so no file the session
    // wrote may steer it.
    wall.bind("--bind", folder, folder);
    if let Some(common) = common_dir(folder) {
        wall.bind("--ro-bind", &common, &common);
        let link = folder.join(".git");
        wall.bind("--ro-bind", &link, &link);
    }
    wall.add(&["--chdir", &folder.to_string_lossy()]);
    for key in ENV {
        if let Some(value) = var(key) {
            wall.add(&["--setenv", key, value]);
        }
    }
    for (key, value) in extra {
        wall.add(&["--setenv", key, value]);
    }

    let mut argv: Vec<String> = [
        pasta.to_string_lossy().as_ref(),
        "--config-net",
        "--quiet",
        "--no-map-gw",
        "--dns-forward",
        DNS,
        "-t",
        "none",
        "-u",
        "none",
        "-T",
        ADB,
        "-U",
        "none",
        "--",
    ]
    .map(str::to_string)
    .to_vec();
    // pasta may not exec nft under SELinux, bash may. Then bwrap, with the
    // pane's TERM, which tmux sets: bwrap clears and sets in order.
    argv.extend([
        bash.to_string_lossy().into_owned(),
        "-c".to_string(),
        format!(
            r#"{} -f "$0" && exec "$1" --clearenv --setenv TERM "${{TERM:-tmux-256color}}" "${{@:2}}""#,
            shell_quote(&nft.to_string_lossy())
        ),
        rules.to_string_lossy().into_owned(),
        bwrap.to_string_lossy().into_owned(),
    ]);
    argv.extend(wall.args);
    argv.push("--".to_string());
    argv.extend(shell.iter().cloned());
    Ok(argv)
}

/// bwrap's arguments, in order: a later mount covers an earlier one.
#[derive(Default)]
struct Wall {
    args: Vec<String>,
}

impl Wall {
    fn add(&mut self, args: &[&str]) {
        self.args.extend(args.iter().map(|a| a.to_string()));
    }

    fn bind(&mut self, how: &str, from: &Path, to: &Path) {
        self.add(&[how, &from.to_string_lossy(), &to.to_string_lossy()]);
    }
}

/// The checkout's git folder, which holds the worktree's own one, from the
/// worktree's `.git` file.
fn common_dir(folder: &Path) -> Option<PathBuf> {
    let link = std::fs::read_to_string(folder.join(".git")).ok()?;
    let own = PathBuf::from(link.trim().strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(own.join("commondir")).ok()?;
    std::fs::canonicalize(own.join(common.trim())).ok()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
