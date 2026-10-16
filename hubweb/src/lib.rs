pub mod access;
pub mod mcp;
pub mod page;
pub mod share;

#[cfg(target_arch = "wasm32")]
mod app;
