//! `GET /`, drawn by hubpages from this hub's own mem, agents and route.

pub use hubpages::page_home::*;

use crate::agents::Agents;
use crate::config::Config;
use crate::html::Banner;
use crate::memcli::MemCli;
use crate::route::{Kind, Route};

/// The whole page, its times of day in this machine's zone.
// Each argument is one thing the page draws; a struct around them would only
// move the list.
#[allow(clippy::too_many_arguments)]
pub fn render(
    mem: &MemCli,
    agents: &Agents,
    route: &Route,
    config: &Config,
    machine: &str,
    now_ms: i64,
    banner: &Banner,
    pair_notice: Option<&str>,
) -> String {
    hubpages::page_home::render(
        mem,
        agents,
        &route.get(),
        &config.siblings,
        machine,
        now_ms,
        banner,
        pair_notice,
        &jiff::tz::TimeZone::system(),
        &ROUTES,
        true,
    )
}

/// The routes this hub offers: Phone and Device.
pub const ROUTES: [Kind; 2] = [Kind::Phone, Kind::Device];

/// A stamp of what home draws, for `/api/stamp`.
pub fn stamp(mem: &MemCli, route: &Route) -> String {
    hubpages::page_home::stamp(mem, &route.get())
}
