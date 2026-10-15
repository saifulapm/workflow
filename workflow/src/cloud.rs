//! What a cloud run needs from this repository: the setup script of the
//! claude.ai cloud environment `workflow`.

use crate::embedded;
use crate::exit;

/// The heredoc delimiter; no embedded file may hold it as a line of its own.
const END: &str = "WORKFLOW_EOF";

/// The setup script: every file of `skills/` and `agents/` under
/// `~/.claude`, and a `mem` that fetches the real binary from the mem server
/// the first time it runs. The script runs before the session, where the
/// proxy adds no Access token, so the binary cannot come with it.
pub fn setup_script(files: &[(&str, &[u8])]) -> String {
    let mut out = String::from(
        "#!/bin/bash\n\
# The cloud environment `workflow`'s setup script, printed by `workflow cloud-setup`.\n\
set -euo pipefail\n",
    );
    for (rel, bytes) in files {
        if !(rel.starts_with("skills/") || rel.starts_with("agents/")) {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        let dest = format!("\"$HOME/.claude/{rel}\"");
        out.push_str(&format!("mkdir -p \"$(dirname {dest})\"\n"));
        out.push_str(&format!("cat > {dest} <<'{END}'\n{text}{END}\n"));
    }
    out.push_str(&format!(
        "bin=/usr/local/bin\n\
mkdir -p \"$bin\"\n\
cat > \"$bin/mem\" <<'{END}'\n{SHIM}{END}\n\
chmod +x \"$bin/mem\"\n"
    ));
    out
}

/// The `mem` a cloud session runs: the x86_64 binary from `$MEM_REMOTE/dl/mem`,
/// fetched inside the session, where the proxy adds the Access token.
const SHIM: &str = r#"#!/bin/sh
real="$HOME/.local/bin/mem-x86_64"
if [ ! -x "$real" ]; then
  mkdir -p "$HOME/.local/bin"
  if ! curl -fsSL --max-time 120 "${MEM_REMOTE:?MEM_REMOTE is not set}/dl/mem" -o "$real.part"; then
    echo "mem: cannot fetch the mem binary from $MEM_REMOTE/dl/mem" >&2
    exit 1
  fi
  chmod +x "$real.part"
  mv "$real.part" "$real"
fi
exec "$real" "$@"
"#;

pub fn cmd_cloud_setup() -> i32 {
    print!("{}", setup_script(embedded::FILES));
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_embedded_file_ends_its_heredoc_early_or_lacks_its_last_newline() {
        for (rel, bytes) in embedded::FILES {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.lines().any(|line| line == END), "{rel}");
            assert!(text.ends_with('\n'), "{rel} must end with a newline");
        }
    }

    #[test]
    fn hooks_stay_out_of_the_cloud() {
        let script = setup_script(&[("hooks/pre-commit", b"x\n"), ("skills/a/SKILL.md", b"a\n")]);
        assert!(!script.contains("pre-commit"), "{script}");
        assert!(script.contains(".claude/skills/a/SKILL.md"), "{script}");
    }
}
