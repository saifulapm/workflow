//! The runner claim. In-place files (the plan, the roadmap, a wiki section)
//! are where sync leaves conflict copies when two machines edit the same file
//! between syncs, so while a project has a live run one machine owns them and
//! the others are refused. Reads work everywhere.

use anyhow::Result;
use jiff::{SignedDuration, Timestamp};

use crate::app::App;
use crate::exit;

/// How long a claim, or the run line that refreshes it, keeps another
/// machine's writes out.
const LIVE: SignedDuration = SignedDuration::from_hours(1);

/// Refuses an in-place write with exit 5 while another machine holds a live
/// claim on the project. A claim is live when it is under an hour old or the
/// project's newest `run` log line is; otherwise it is stale and any machine
/// may write.
pub fn runner_guard(app: &App, project_id: &str, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    let declared = |key| crate::project::declared(&app.store, project_id, key);
    let Some(runner) = declared("runner") else {
        return Ok(());
    };
    if runner == app.machine {
        return Ok(());
    }
    let since = declared("runner_since").unwrap_or_default();
    let floor = Timestamp::now() - LIVE;
    let fresh = since.parse::<Timestamp>().is_ok_and(|t| t > floor);
    if !fresh && !recent_run_line(app, project_id, floor)? {
        return Ok(());
    }
    let name = crate::project::Registry::load(&app.store)
        .by_id(project_id)
        .map_or_else(|| project_id.to_string(), |p| p.name.clone());
    Err(exit::coded(
        exit::CAS_CONFLICT,
        format!("{name} is run by {runner} since {since}; --force overrides"),
    ))
}

fn recent_run_line(app: &App, project_id: &str, floor: Timestamp) -> Result<bool> {
    let rows = app
        .read_index()?
        .recent_filtered(Some("log"), Some("run"), Some(project_id), 1)?;
    Ok(rows
        .first()
        .is_some_and(|r| r.modified_epoch > floor.as_second()))
}
