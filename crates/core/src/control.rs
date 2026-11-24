//! One device under control: the tunnel, the media session that keeps HID input alive (and
//! carries the live screen), and the HID connection. The CLI uses it for one command; the daemon
//! keeps it open.

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::{
    Session,
    ax::{AxClient, BuildOptions, Screen},
    display::Display,
    input::{Button, Input, Motion, TAP_HOLD},
    media::{MediaMode, MediaSession, MediaStatus},
};

/// A media session that sent nothing for this long no longer keeps the HID gate open
/// (PITFALLS #7); it is restarted before the next input.
const MEDIA_STALE_MS: u64 = 2500;

/// What a gesture was sent with, for the caller's output.
#[derive(Debug, Clone, Serialize)]
pub struct Sent {
    /// The screen size the points were placed on.
    pub screen: (f64, f64),
    /// Milliseconds from the call until the last report was written.
    pub ms: u64,
}

pub struct Controller {
    pub session: Session,
    display: Option<Display>,
    media: Option<MediaSession>,
    input: Option<Input>,
    ax: Option<AxClient>,
    mode: MediaMode,
    /// The last reading, for `@eN` refs.
    pub last_screen: Option<Screen>,
    /// Input went out since the last reading.
    input_since_read: bool,
}

/// Longest wait for one accessibility reading.
const AX_TIMEOUT: Duration = Duration::from_secs(15);

impl Controller {
    /// Wraps an open session. `mode` decides whether the media session also carries video.
    pub fn new(session: Session, mode: MediaMode) -> Self {
        Self { session, display: None, media: None, input: None, ax: None, mode, last_screen: None, input_since_read: true }
    }

    /// The display in use, asked once and then reused (call [`Self::refresh_display`] after a turn).
    pub async fn display(&mut self) -> Result<Display> {
        if let Some(d) = &self.display {
            return Ok(d.clone());
        }
        let d = self.session.display().await?;
        self.display = Some(d.clone());
        Ok(d)
    }

    pub async fn refresh_display(&mut self) -> Result<Display> {
        self.display = None;
        self.display().await
    }

    /// Starts the media session if it isn't running or went quiet.
    pub async fn ensure_media(&mut self) -> Result<&mut MediaSession> {
        let stale = self
            .media
            .as_ref()
            .is_some_and(|m| m.idle_ms().is_none_or(|ms| ms > MEDIA_STALE_MS));
        if stale {
            tracing::debug!("media went quiet; restarting it and the HID connection");
            self.drop_input().await;
            if let Some(mut m) = self.media.take() {
                let _ = m.stop().await;
            }
        }
        if self.media.is_none() {
            let display = self.display().await?;
            let m = MediaSession::start(
                &self.session.udid,
                self.session.adapter(),
                self.session.rsd(),
                self.mode,
                display.display_id.unwrap_or(1),
            )
            .await?;
            self.media = Some(m);
        }
        Ok(self.media.as_mut().expect("started above"))
    }

    async fn ensure_input(&mut self) -> Result<(&mut Input, Display)> {
        self.ensure_media().await?;
        let display = self.display().await?;
        if self.input.is_none() {
            let input = Input::open(self.session.adapter().clone(), self.session.rsd().clone(), display.clone()).await?;
            self.input = Some(input);
        }
        Ok((self.input.as_mut().expect("opened above"), display))
    }

    async fn drop_input(&mut self) {
        if let Some(i) = self.input.take() {
            i.close().await;
        }
    }

    pub async fn tap(&mut self, at: (f64, f64)) -> Result<Sent> {
        let (_, display) = self.ensure_input().await?;
        let began = std::time::Instant::now();
        self.input_since_read = true;
        let input = self.input.as_mut().expect("opened above");
        input.tap(&display, at, TAP_HOLD).await?;
        Ok(Sent { screen: display.size(), ms: began.elapsed().as_millis() as u64 })
    }

    pub async fn drag(&mut self, from: (f64, f64), to: (f64, f64), duration: Duration, motion: Motion) -> Result<Sent> {
        let (_, display) = self.ensure_input().await?;
        let began = std::time::Instant::now();
        self.input_since_read = true;
        let input = self.input.as_mut().expect("opened above");
        input.drag(&display, from, to, duration, motion).await?;
        Ok(Sent { screen: display.size(), ms: began.elapsed().as_millis() as u64 })
    }

    pub async fn button(&mut self, button: Button) -> Result<()> {
        self.ensure_input().await?;
        self.input_since_read = true;
        self.input.as_mut().expect("opened above").button(button).await
    }

    /// Reads the screen's accessibility tree. `full` waits for the second pass (identifiers).
    pub async fn ui(&mut self, opts: BuildOptions, full: bool) -> Result<Screen> {
        if self.ax.is_none() {
            self.ax = Some(AxClient::open(self.session.adapter(), self.session.rsd()).await?);
        }
        let changed = std::mem::take(&mut self.input_since_read);
        let read = self.ax.as_mut().expect("opened above").read(AX_TIMEOUT, full, changed).await;
        let dump = match read {
            Ok(d) => d,
            Err(e) => {
                // A closed or stuck subscription is reopened on the next call.
                self.ax = None;
                return Err(e);
            }
        };
        let screen = tokio::task::spawn_blocking(move || Screen::build(&dump, opts)).await?;
        self.last_screen = Some(screen.clone());
        Ok(screen)
    }

    /// Launches an app by bundle id. Returns its pid.
    pub async fn launch(&mut self, bundle_id: &str, relaunch: bool) -> Result<u64> {
        self.input_since_read = true;
        self.session.launch(bundle_id, relaunch).await
    }

    pub fn media(&self) -> Option<&MediaSession> {
        self.media.as_ref()
    }

    pub fn media_mut(&mut self) -> Option<&mut MediaSession> {
        self.media.as_mut()
    }

    pub fn mode(&self) -> MediaMode {
        self.mode
    }

    /// Switches between input-only and video. A running media session (and the HID connection,
    /// which needs one) is stopped; the next input or `ensure_media` starts the new kind.
    pub async fn set_mode(&mut self, mode: MediaMode) -> Result<()> {
        if self.mode == mode {
            return Ok(());
        }
        self.mode = mode;
        self.drop_input().await;
        match self.media.take() {
            Some(mut m) => m.stop().await,
            None => Ok(()),
        }
    }

    /// Stops and restarts the media session (and HID). The device then sends a fresh keyframe.
    pub async fn restart_media(&mut self) -> Result<&mut MediaSession> {
        self.drop_input().await;
        if let Some(mut m) = self.media.take() {
            let _ = m.stop().await;
        }
        self.ensure_media().await
    }

    pub async fn media_status(&mut self) -> Result<Option<MediaStatus>> {
        match self.media.as_mut() {
            Some(m) => Ok(Some(m.status().await?)),
            None => Ok(None),
        }
    }

    /// Closes the HID connection and stops our streams (only ours).
    pub async fn close(&mut self) -> Result<()> {
        self.drop_input().await;
        match self.media.take() {
            Some(mut m) => m.stop().await,
            None => Ok(()),
        }
    }
}
