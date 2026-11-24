//! One device under control: the tunnel, the media session that keeps HID input alive (and
//! carries the live screen), and the HID connection. The CLI uses it for one command; the daemon
//! keeps it open.

use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use serde::Serialize;

use crate::{
    Session,
    apps::{AppDirectory, ProcName},
    ax::{AxClient, AxUnresponsive, BuildOptions, Dump, Screen},
    display::Display,
    input::{Button, Input, Motion, TAP_HOLD, TouchPhase},
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
    /// Executable path by pid, to name the processes `ui` shows.
    processes: HashMap<i64, String>,
    apps: AppDirectory,
    /// The app list was fetched again for a bundle it lacked; once per controller is enough.
    apps_refreshed: bool,
}

/// Longest wait for one accessibility reading.
const AX_TIMEOUT: Duration = Duration::from_secs(15);
/// The device's accessibility daemon, restarted when it stops answering (PITFALLS #25).
const AXAUDITD: &str = "/AccessibilityAudit.framework/Support/axauditd";
/// Restarted, it took a few seconds before a new connection was answered (PITFALLS #25).
const AXAUDITD_RESTART_TRIES: u32 = 4;

impl Controller {
    /// Wraps an open session. `mode` decides whether the media session also carries video.
    pub fn new(session: Session, mode: MediaMode) -> Self {
        Self {
            session,
            display: None,
            media: None,
            input: None,
            ax: None,
            mode,
            last_screen: None,
            input_since_read: true,
            processes: HashMap::new(),
            apps: AppDirectory::default(),
            apps_refreshed: false,
        }
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

    /// Takes a display read elsewhere (the daemon polls it to follow the interface orientation,
    /// PITFALLS #27), so the next gesture is placed on the screen as it is turned now.
    pub fn set_display(&mut self, d: Display) {
        self.display = Some(d);
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

    /// One step of a live touch (see [`Input::touch`]).
    pub async fn touch(&mut self, phase: TouchPhase, at: Option<(f64, f64)>) -> Result<Sent> {
        // Lifting a finger the HID connection no longer has (it was closed) needs no connection.
        if phase == TouchPhase::Up && !self.touching() {
            let screen = self.display().await?.size();
            return Ok(Sent { screen, ms: 0 });
        }
        let (_, display) = self.ensure_input().await?;
        let began = std::time::Instant::now();
        self.input_since_read = true;
        let input = self.input.as_mut().expect("opened above");
        input.touch(&display, phase, at).await?;
        Ok(Sent { screen: display.size(), ms: began.elapsed().as_millis() as u64 })
    }

    /// Whether a live touch has a finger down.
    pub fn touching(&self) -> bool {
        self.input.as_ref().is_some_and(Input::touching)
    }

    pub async fn button(&mut self, button: Button) -> Result<()> {
        self.ensure_input().await?;
        self.input_since_read = true;
        self.input.as_mut().expect("opened above").button(button).await
    }

    /// Reads the screen's accessibility tree. `full` waits for the second pass (identifiers).
    pub async fn ui(&mut self, opts: BuildOptions, full: bool) -> Result<Screen> {
        let changed = std::mem::take(&mut self.input_since_read);
        let mut dump = match self.read_ax(AX_TIMEOUT, full, changed).await {
            Err(e) if e.is::<AxUnresponsive>() => self.restart_axauditd(full).await?,
            r => r?,
        };
        let pids: Vec<i64> = dump.elements.keys().map(|k| k.pid).collect();
        self.learn_processes(&pids).await;
        let springboard_only = !dump.applications.is_empty()
            && dump.applications.iter().all(|k| self.describe(k.pid).bundle_id.as_deref() == Some(crate::apps::SPRINGBOARD));
        if !full && springboard_only {
            // SpringBoard's screens are told apart by identifiers, which only the second pass has
            // (PITFALLS #22).
            if let Some(d) = self.ax.as_mut().expect("opened above").complete().await {
                dump = d;
            }
        }
        let names: HashMap<i64, ProcName> = pids.iter().map(|&p| (p, self.describe(p))).collect();
        let screen = tokio::task::spawn_blocking(move || {
            let mut screen = Screen::build(&dump, opts);
            screen.name_processes(|pid| names.get(&pid).cloned().unwrap_or_default());
            screen
        })
        .await?;
        self.last_screen = Some(screen.clone());
        Ok(screen)
    }

    async fn read_ax(&mut self, timeout: Duration, full: bool, changed: bool) -> Result<std::sync::Arc<Dump>> {
        if self.ax.is_none() {
            self.ax = Some(AxClient::open(self.session.adapter(), self.session.rsd()).await?);
        }
        let read = self.ax.as_mut().expect("opened above").read(timeout, full, changed).await;
        if read.is_err() {
            // A closed or stuck subscription is reopened on the next call.
            self.ax = None;
        }
        read
    }

    /// The accessibility daemon took connections and answered none, also on a fresh tunnel, until
    /// it was killed; launchd starts it again on the next connection (PITFALLS #25).
    async fn restart_axauditd(&mut self, full: bool) -> Result<std::sync::Arc<Dump>> {
        let unresponsive = || anyhow::Error::new(AxUnresponsive);
        let pid = self
            .session
            .processes()
            .await?
            .into_iter()
            .find(|(_, exe)| exe.ends_with(AXAUDITD))
            .map(|(pid, _)| pid)
            .ok_or_else(unresponsive)?;
        tracing::warn!("accessibility: axauditd (pid {pid}) does not answer; restarting it");
        self.session.kill(pid).await?;
        let mut last = unresponsive();
        for _ in 0..AXAUDITD_RESTART_TRIES {
            match self.read_ax(AX_TIMEOUT, full, false).await {
                Err(e) if e.is::<AxUnresponsive>() => last = e,
                r => return r,
            }
        }
        Err(last.context("axauditd was restarted and still answers nothing; restart the device and retry"))
    }

    /// Drops the accessibility subscription: switching VoiceOver silences it for good while its
    /// connection stays open (PITFALLS #25).
    pub fn forget_accessibility(&mut self) {
        self.ax = None;
    }

    fn describe(&self, pid: i64) -> ProcName {
        self.processes.get(&pid).map(|exe| self.apps.describe(exe)).unwrap_or_default()
    }

    /// Fetches the process list when a pid is new, and the app list when a bundle is. Failures
    /// only leave names out.
    async fn learn_processes(&mut self, pids: &[i64]) {
        if pids.iter().all(|p| self.processes.contains_key(p)) {
            return;
        }
        match self.session.processes().await {
            Ok(list) => self.processes = list.into_iter().collect(),
            Err(e) => {
                tracing::debug!("process list: {e:#}");
                return;
            }
        }
        let unknown = pids
            .iter()
            .filter_map(|p| self.processes.get(p))
            .any(|exe| !self.apps.knows(exe));
        if self.apps.is_empty() || (unknown && !self.apps_refreshed) {
            self.apps_refreshed |= !self.apps.is_empty();
            match self.session.installed_apps().await {
                Ok(list) => self.apps = AppDirectory::new(list),
                Err(e) => tracing::debug!("app list: {e:#}"),
            }
        }
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
