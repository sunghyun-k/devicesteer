//! How a device's daemon tells the viewer about its agent session, and opens the viewer when an
//! agent starts one.
//!
//! The viewer listens on a socket of its own (a named pipe on Windows) next to the daemons'. Each
//! daemon connects to it when its session starts, changes owner or ends, and sends one message:
//! `{"event": "session", "udid", "session": <the control event's session or null>, "started"}`.
//! The viewer answers `{"ok": true}`. Nothing polls: a daemon with no session change sends nothing,
//! and without a viewer the connect fails at once. The viewer learns of sessions that started
//! before it ran by asking each listed device's running daemon once (`probe` in the shell).
//!
//! When a session starts and no viewer answers, the daemon launches the app for that device,
//! unless Settings › General › "Open the viewer when an agent starts a session" is off
//! (`Preferences::open_viewer_on_session`, read by the daemon, so the CLI stays thin) or
//! `DEVICESTEER_NO_VIEWER` is set (CI, scripts). The result says what happened, and never fails
//! the session: a dev build or a CI runner has no app to open.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    transport,
    wire::{self, Message},
};

/// The transport key of the viewer's socket. Pipe names are machine-wide on Windows, so it names
/// the user there: another user's viewer must not get (or block) this user's announcements.
pub fn key() -> String {
    if cfg!(windows) {
        let user: String = std::env::var("USERNAME")
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c))
            .collect();
        format!("viewer-{user}")
    } else {
        "viewer".into()
    }
}

/// What happened to the viewer when a session started, for the `session start` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A running viewer got the message: it shows the device, or a notice when it shows another.
    Notified,
    /// No viewer ran; the app was launched for the device.
    Launched,
    /// No viewer ran, and the setting is off (or `DEVICESTEER_NO_VIEWER` is set).
    Off,
    /// No viewer ran, and no app was found to launch (a dev build, CI), or it wouldn't start.
    NotInstalled(String),
}

impl Outcome {
    pub fn to_json(&self) -> Value {
        match self {
            Outcome::Notified => json!({ "viewer": "notified" }),
            Outcome::Launched => json!({ "viewer": "launched" }),
            Outcome::Off => json!({
                "viewer": "off",
                "note": "the viewer isn't running and opening it is turned off (Settings › General); the person can open DeviceSteer to watch"
            }),
            Outcome::NotInstalled(why) => json!({
                "viewer": "not_installed",
                "note": format!("the viewer isn't running and couldn't be opened ({why}); the session works without it")
            }),
        }
    }
}

/// Sends one session message to the viewer. Err: no viewer answers.
async fn send(msg: Value) -> Result<()> {
    let stream = transport::connect(&key()).await?;
    let (mut rd, mut wr) = tokio::io::split(stream);
    wire::write(&mut wr, &Message::Json(msg)).await?;
    // The answer says a viewer read it, not a socket file left by one that died.
    match tokio::time::timeout(Duration::from_secs(2), wire::read(&mut rd)).await {
        Ok(Ok(Some(_))) => Ok(()),
        _ => bail!("the viewer didn't answer"),
    }
}

/// Tells a running viewer that `udid`'s session changed (`session` null: none runs). Silent when
/// no viewer runs.
pub async fn tell(udid: &str, session: Option<Value>) {
    let _ = send(json!({ "event": "session", "udid": udid, "session": session, "started": false })).await;
}

/// A session started on `udid`: tells the viewer, or opens it for the device.
pub async fn session_started(udid: &str, session: Value) -> Outcome {
    if send(json!({ "event": "session", "udid": udid, "session": session, "started": true })).await.is_ok() {
        return Outcome::Notified;
    }
    if std::env::var_os("DEVICESTEER_NO_VIEWER").is_some_and(|v| !v.is_empty())
        || !devicesteer_core::preferences::Preferences::load().open_viewer_on_session
    {
        return Outcome::Off;
    }
    match launch(udid).await {
        Ok(()) => Outcome::Launched,
        Err(e) => Outcome::NotInstalled(format!("{e:#}")),
    }
}

/// Where the installed viewer is, in the order tried: the app this CLI is part of (the bundle it
/// sits in on macOS, the folder it sits in on Windows), then the one the viewer recorded on its
/// last launch (`app_path` in the state directory; the agent setup's links point elsewhere).
fn app_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        if cfg!(target_os = "macos") {
            if let Some(app) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
                out.push(app.to_path_buf());
            }
        } else if let Some(dir) = exe.parent() {
            out.extend(["DeviceSteer.exe", "devicesteer-app.exe"].map(|n| dir.join(n)));
        }
    }
    if let Ok(p) = std::fs::read_to_string(app_path_file()) {
        out.push(PathBuf::from(p.trim()));
    }
    if cfg!(windows) {
        // The installer's default places, per user and for all users.
        for base in ["LOCALAPPDATA", "ProgramFiles"] {
            if let Some(b) = std::env::var_os(base) {
                out.push(PathBuf::from(b).join("DeviceSteer").join("DeviceSteer.exe"));
            }
        }
    }
    out.retain(|p| p.exists());
    out
}

/// The file in which the viewer records where it runs from.
pub fn app_path_file() -> PathBuf {
    devicesteer_core::paths::state_dir().join("app_path")
}

/// The argument that makes a launched viewer open one device and show its screen.
pub const UDID_ARG: &str = "--udid";

/// Launches the viewer for `udid` without taking the keyboard from the terminal the agent runs
/// in: `open -g` on macOS (the app starts in the background, and its window is ordered to the
/// front without being activated); on Windows, the new window can't take the foreground from
/// another app anyway, so it shows behind it with a flashing taskbar button.
async fn launch(udid: &str) -> Result<()> {
    let apps = app_candidates();
    #[cfg(target_os = "macos")]
    {
        let mut cmd = tokio::process::Command::new("/usr/bin/open");
        cmd.arg("-g");
        match apps.first() {
            Some(app) => cmd.arg("-a").arg(app),
            // Wherever Launch Services knows the app from (an installed DeviceSteer).
            None => cmd.args(["-b", "com.sunghyunkim.devicesteer"]),
        };
        cmd.args(["--args", UDID_ARG, udid]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
        let out = cmd.output().await.context("could not run open")?;
        if !out.status.success() {
            bail!("no DeviceSteer app found");
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let app = apps.first().context("no DeviceSteer app found")?;
        let mut cmd = std::process::Command::new(app);
        cmd.args([UDID_ARG, udid])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        #[cfg(windows)]
        {
            // Not tied to the daemon's (absent) console or process group.
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            std::os::windows::process::CommandExt::creation_flags(&mut cmd, DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        cmd.spawn().with_context(|| format!("could not start {}", app.display()))?;
        Ok(())
    }
}

/// The viewer's end: listens for daemons' session messages.
pub struct Listener(transport::Listener);

impl Listener {
    /// Err when another viewer already listens (a second copy of the app) or the socket can't be
    /// made; the viewer then works as before, without announcements.
    pub async fn bind() -> Result<Self> {
        Ok(Self(transport::Listener::bind(&key()).await?))
    }

    /// The next message, answered. `on_message` gets `(udid, session, started)`.
    pub async fn serve(self, on_message: impl Fn(String, Option<Value>, bool) + Send + Sync + 'static) {
        let on_message = std::sync::Arc::new(on_message);
        loop {
            let Ok(stream) = self.0.accept().await else {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            };
            let on_message = on_message.clone();
            tokio::spawn(async move {
                let (mut rd, mut wr) = tokio::io::split(stream);
                let Ok(Ok(Some(Message::Json(v)))) = tokio::time::timeout(Duration::from_secs(2), wire::read(&mut rd)).await else {
                    return;
                };
                if let Some(udid) = v.get("udid").and_then(Value::as_str) {
                    let session = v.get("session").cloned().filter(|s| !s.is_null());
                    let started = v.get("started").and_then(Value::as_bool).unwrap_or(false);
                    on_message(udid.to_string(), session, started);
                }
                let _ = wire::write(&mut wr, &Message::Json(json!({ "ok": true }))).await;
            });
        }
    }
}

/// The session `udid`'s running daemon has, without starting a daemon: Ok(None) when none runs
/// or it has no session. For the viewer's first look at the devices it lists.
pub async fn probe(udid: &str) -> Option<Value> {
    let stream = tokio::time::timeout(Duration::from_millis(500), transport::connect(udid)).await.ok()?.ok()?;
    let (mut rd, mut wr) = tokio::io::split(stream);
    let req = json!({ "id": 1, "cmd": "session", "args": { "action": "status" }, "source": "human" });
    wire::write(&mut wr, &Message::Json(req)).await.ok()?;
    let reply = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match wire::read(&mut rd).await {
                Ok(Some(Message::Json(v))) if v.get("event").is_none() => return Some(v),
                Ok(Some(_)) => continue,
                _ => return None,
            }
        }
    })
    .await
    .ok()??;
    reply.get("result")?.get("session").cloned().filter(|s| !s.is_null() && s.get("owner").and_then(Value::as_str) != Some("finished"))
}
