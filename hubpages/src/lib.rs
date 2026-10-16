//! hubpages — the hub's pages, drawn from what mem prints.
//!
//! A page reads mem only through a [`Source`] and asks everything else of a
//! [`Host`], so the same pages serve from the local hub, which runs the `mem`
//! binary, and from a Worker, which reads its own store.

pub mod form;
pub mod host;
pub mod html;
pub mod http;
pub mod live;
pub mod mem;
pub mod model;
pub mod multipart;
pub mod page_control;
pub mod page_decisions;
pub mod page_evidence;
pub mod page_home;
pub mod page_new;
pub mod page_project;
pub mod page_questions;
pub mod page_roadmap;
pub mod page_wiki;
pub mod pages;

pub use host::{Fleet, Host};
pub use mem::{Mem, Outcome, QUESTIONS_ARGV, Source, Written};
