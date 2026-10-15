//! The one global route: where questions and rings reach the owner, Phone or
//! Device with its machine.
//!
//! It lives in `route.json` beside `agents.json`, written whole and renamed.
//! The file keeps the machine that last took Device even while the route is
//! Phone, so Device chosen on home's header goes back to that machine.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents;
use crate::http::{Request, Response};
use crate::memcli::MemCli;
use crate::pair;

/// What `route.json` holds. A missing file is Phone with no machine.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Saved {
    pub device: bool,
    /// The machine that last took Device.
    pub machine: Option<String>,
}

/// Device was asked for with no machine named and none ever taken.
pub struct NoMachine;

pub struct Route {
    path: PathBuf,
    saved: Mutex<Saved>,
}

impl Route {
    /// The route file at `path`, or Phone when it is missing or will not
    /// parse. A bad file is logged, as `Agents::open` does.
    pub fn open(path: PathBuf) -> Route {
        let saved = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("hub: ignoring {}: {e}", path.display());
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        Route {
            path,
            saved: Mutex::new(saved),
        }
    }

    pub fn get(&self) -> Saved {
        self.lock().clone()
    }

    /// Phone, or Device on `machine`, or with `None` on the machine that last
    /// took it. Changes run one at a time, and the last one wins.
    pub fn set(&self, device: bool, machine: Option<&str>) -> Result<(), NoMachine> {
        let mut saved = self.lock();
        let mut next = saved.clone();
        next.device = device;
        if let Some(machine) = machine {
            next.machine = Some(machine.to_string());
        }
        if device && next.machine.is_none() {
            return Err(NoMachine);
        }
        *saved = next;
        let result = serde_json::to_string(&*saved)
            .map_err(anyhow::Error::from)
            .and_then(|text| pair::rewrite_private(&self.path, &text));
        // The route in memory is still right until the hub restarts.
        if let Err(e) = result {
            eprintln!("hub: saving {}: {e:#}", self.path.display());
        }
        Ok(())
    }

    /// The route as agents read it, with the count of person questions
    /// pending.
    pub fn object(&self, mem: &MemCli) -> Value {
        let saved = self.get();
        let waiting = mem.questions().rows("questions").len();
        if saved.device {
            json!({"route": "device", "machine": saved.machine, "waiting": waiting})
        } else {
            json!({"route": "phone", "machine": null, "waiting": waiting})
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Saved> {
        self.saved.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The body of `POST /api/route`.
#[derive(Deserialize)]
struct Choice {
    route: String,
    machine: Option<String>,
}

/// `GET /api/route`, for a paired agent.
pub fn get(route: &Route, mem: &MemCli, request: &Request) -> Response {
    if !agents::authorized(request) {
        return agents::unauthorized();
    }
    agents::json(200, route.object(mem))
}

/// `POST /api/route`, for a paired agent: the token first, so a refused
/// change touches nothing.
pub fn post(route: &Route, mem: &MemCli, request: &Request) -> Response {
    if !agents::authorized(request) {
        return agents::unauthorized();
    }
    let choice: Choice = match serde_json::from_slice(&request.body) {
        Ok(choice) => choice,
        Err(e) => return agents::json(400, json!({"error": e.to_string()})),
    };
    let device = match choice.route.as_str() {
        "phone" => false,
        "device" => true,
        _ => return agents::json(400, json!({"error": "route is phone or device"})),
    };
    if choice
        .machine
        .as_deref()
        .is_some_and(|m| m.trim().is_empty())
    {
        return agents::json(400, json!({"error": "machine is empty"}));
    }
    // A machine is its tailnet label, which DNS caps at 63 characters, and
    // home draws it in the header.
    if choice.machine.as_deref().is_some_and(|m| m.len() > 63) {
        return agents::json(
            400,
            json!({"error": "machine is longer than 63 characters"}),
        );
    }
    match route.set(device, choice.machine.as_deref()) {
        Ok(()) => agents::json(200, route.object(mem)),
        Err(NoMachine) => agents::json(
            409,
            json!({"error": "No machine has taken Device yet, so name one."}),
        ),
    }
}
