//! Device layer for iphone-use.
//!
//! Everything goes through [idevice](https://github.com/jkcoxson/idevice), with no dependency on
//! Xcode or CoreDevice on the host:
//!
//! ```text
//! usbmuxd (USB or Wi-Fi) -> lockdown -> CoreDeviceProxy -> CDTunnel
//!   -> userspace TCP stack -> RSD handshake -> RemoteXPC services
//! ```
//!
//! The same path works on macOS and Windows (Apple's usbmuxd ships with iTunes / Apple Devices
//! there). Nothing needs root or a TUN interface.

pub mod ax;
mod control;
mod coredevice;
mod devices;
mod display;
mod input;
mod media;
mod mp4;
pub mod paths;
mod record;
mod video;
mod session;

pub use control::{Controller, Sent};
pub use devices::{DeviceInfo, Transport, list_devices};
pub use display::{Display, Rotation};
pub use input::{Button, Input, Motion, TAP_HOLD};
pub use media::{MediaMode, MediaPacket, MediaSession, MediaStatus, StreamKind, StreamStatus, server_status};
pub use record::{Frame, FrameStream, RecordSummary, Recorder};
pub use session::{ServiceInfo, Session};
pub use video::ParameterSets;

/// How long one connection step (usbmuxd, lockdown, proxy, RSD) may take before we give up.
pub(crate) const STEP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

/// Runs one connection step with [`STEP_TIMEOUT`] and names the step in the error.
///
/// Connection-failure messages must say "connection" or "tunnel": a resident process that caches
/// sessions (the future per-device daemon) drops its cached session when it sees those words.
pub(crate) async fn step<T, E, F>(what: &str, fut: F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    match tokio::time::timeout(STEP_TIMEOUT, fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => anyhow::bail!("{what} failed (connection error: {e:?})"),
        Err(_) => anyhow::bail!(
            "{what} timed out after {}s (connection stalled)",
            STEP_TIMEOUT.as_secs()
        ),
    }
}
