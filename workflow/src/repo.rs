//! Standing where the hooks stand.

use std::path::PathBuf;

use crate::gitcmd::Git;
use crate::paths;

/// The toplevel, and a git that will keep answering correctly from there.
///
/// `GIT_INDEX_FILE` may be relative -- a partial commit's temporary index is --
/// so the git environment is made absolute before any move, and no move happens
/// at all when we are already at the toplevel (spec §7).
pub fn goto_toplevel() -> Option<(Git, PathBuf)> {
    let top = Git::here().toplevel()?;
    let mut git = Git::here();

    let here = paths::realpath(paths::cwd()).unwrap_or_default();
    let there = paths::realpath(&top).unwrap_or_default();
    if here != there {
        for key in ["GIT_INDEX_FILE", "GIT_DIR"] {
            if let Ok(v) = std::env::var(key)
                && !v.is_empty()
            {
                git = git.with_env(key, paths::realpath_m(&v).to_string_lossy().to_string());
            }
        }
        std::env::set_current_dir(&top).ok()?;
    }
    Some((git, top))
}
