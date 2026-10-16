//! The page routes. hubpages draws the pages; this hub answers the two
//! families that need its own files, the fonts and pairing, and hands every
//! other page what it needs from this machine.

pub use hubpages::pages::{PAGES, page_shell, page_shell_in};

use crate::app::App;
use crate::http::{Request, Response};
use crate::{assets, pair};

/// Answers a page route, or `None` for a path no page owns. Pairing runs
/// the method and origin checks every page runs, and is the one write that
/// cannot ask for a paired device.
pub fn route_page(app: &App, request: &Request) -> Option<Response> {
    let path = request.path.as_str();
    if path.starts_with("/assets/") {
        if request.method != "GET" {
            return Some(Response::method_not_allowed("GET"));
        }
        return Some(assets::get(request));
    }
    if let Some(code) = path.strip_prefix("/pair/") {
        return Some(match request.method.as_str() {
            "GET" => pair::get(app, code),
            "POST" if !app.guard.may_write(request) => {
                Response::text(403, "cross-origin writes are refused")
            }
            "POST" => pair::post(app, request, code),
            _ => Response::method_not_allowed("GET"),
        });
    }
    if !hubpages::pages::owns(path) {
        return None;
    }
    let site = hubpages::pages::Site {
        mem: &*app.mem,
        host: app,
        machine: &app.machine,
        siblings: &app.config.siblings,
        by: "saiful",
        paired: pair::paired(request),
        may_write: app.guard.may_write(request),
        now: jiff::Timestamp::now(),
        tz: jiff::tz::TimeZone::system(),
        routes: &crate::page_home::ROUTES,
        poll: true,
    };
    hubpages::pages::route_page(&site, request)
}
