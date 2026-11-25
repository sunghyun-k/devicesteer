//! The per-device daemon and its client.
//!
//! One daemon per device owns the tunnel, the media session, HID and the accessibility
//! subscription, so commands don't pay for a new tunnel each time, input and video share one
//! screen-sharing session, and the viewer sees what the agent does. The CLI and the viewer are
//! both clients.

mod client;
pub mod control;
mod server;
mod transport;
pub mod wire;

pub use client::Client;
pub use control::{ControlAction, Source};
pub use server::run_blocking;
#[cfg(windows)]
pub use transport::pipe_name;
#[cfg(unix)]
pub use transport::socket_path;
