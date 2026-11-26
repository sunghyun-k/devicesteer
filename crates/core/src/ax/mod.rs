//! The accessibility tree: what is on screen, with labels, values, roles and frames.

mod archive;
// Copied tables and payload shapes document more than `ui` uses yet.
#[allow(dead_code)]
mod attrs;
#[allow(dead_code)]
mod dump;
mod overlay;
mod service;
mod tree;
mod window;

pub(crate) use dump::Dump;
pub(crate) use overlay::{Cooldown, lone_empty_siri};
pub use service::{AxClient, AxUnresponsive};
pub use tree::{AppInfo, BuildOptions, Element, Geometry, Rect, Screen};
pub(crate) use window::{Image, find as find_window};


