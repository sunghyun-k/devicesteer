//! The accessibility tree: what is on screen, with labels, values, roles and frames.

mod archive;
// Copied tables and payload shapes document more than `ui` uses yet.
#[allow(dead_code)]
mod attrs;
#[allow(dead_code)]
mod dump;
mod service;
mod tree;

pub use service::AxClient;
pub use tree::{AppInfo, BuildOptions, Element, Rect, Screen};
