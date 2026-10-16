//! `GET /`, drawn by hubpages from this hub's own mem, agents, route and
//! Dulali.

pub use hubpages::page_home::*;

use crate::agents::Agents;
use crate::config::Config;
use crate::dulali::Held;
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
    dulali: &[(String, Held)],
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
        dulali,
        &jiff::tz::TimeZone::system(),
        &ROUTES,
        true,
    )
}

/// The routes this hub offers: Phone, Device and Dulali.
pub const ROUTES: [Kind; 3] = [Kind::Phone, Kind::Device, Kind::Dulali];

/// A stamp of what home draws, for `/api/stamp`.
pub fn stamp(mem: &MemCli, route: &Route, dulali: &[(String, Held)]) -> String {
    hubpages::page_home::stamp(mem, &route.get(), dulali)
}
