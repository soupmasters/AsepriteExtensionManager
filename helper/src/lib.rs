#![forbid(unsafe_code)]

pub mod github;
pub mod installation;
pub mod installed;
pub mod package;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod state;
pub mod tooling;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PROTOCOL_VERSION: u32 = 1;
