//! `record`: the screen to an MP4 while other commands run.
//!
//! Until the per-device daemon owns recordings, `record start` runs `record run` as a background
//! process with a media session of its own (other commands' input sessions stop only their own
//! streams, PITFALLS #1), and `record stop` asks it to finish through a file in the state dir.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use iphone_use_core::{FrameStream, MediaMode, MediaSession, Recorder, Session, paths};
use serde_json::{Value, json};

/// The state files of a background recording: pid and path, stop request, the recorder's stdout
/// (its summary or nothing).
struct Files {
    running: PathBuf,
    stop: PathBuf,
    out: PathBuf,
}

fn files(udid: &str) -> Files {
    let dir = paths::state_dir();
    Files {
        running: dir.join(format!("{udid}.record.json")),
        stop: dir.join(format!("{udid}.record.stop")),
        out: dir.join(format!("{udid}.record.out")),
    }
}

async fn udid_of(udid: Option<&str>) -> Result<String> {
    match udid {
        Some(u) => Ok(u.to_string()),
        None => Ok(Session::open(None).await?.udid),
    }
}

/// Records until `seconds`, Ctrl-C, or the stop file. Returns the summary.
pub async fn run(udid: Option<&str>, output: &Path, seconds: Option<f64>) -> Result<Value> {
    let mut session = Session::open(udid).await?;
    let f = files(&session.udid);
    let _ = std::fs::remove_file(&f.stop);
    let display = session.display().await?;
    let began = Instant::now();
    let mut media = MediaSession::start(
        &session.udid,
        session.adapter(),
        session.rsd(),
        MediaMode::Video,
        display.display_id.unwrap_or(1),
    )
    .await?;
    let started_ms = began.elapsed().as_millis() as u64;
    let stream = FrameStream::start(&mut media);
    let mut frames = stream.subscribe();
    let mut recorder = Recorder::create(output)?;
    let mut first_frame_ms = None;
    let deadline = seconds.map(|s| Instant::now() + Duration::from_secs_f64(s));
    let mut last_frame_at = Instant::now();
    let mut skipped = 0u64;
    let mut check = tokio::time::interval(Duration::from_millis(200));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let reason = loop {
        tokio::select! {
            f = frames.recv() => match f {
                Ok(frame) => {
                    recorder.push(&frame)?;
                    if recorder.frames() > 0 {
                        first_frame_ms.get_or_insert(began.elapsed().as_millis() as u64);
                    }
                    last_frame_at = Instant::now();
                }
                // Frames dropped because the writer fell behind: the file has a gap but stays
                // decodable only until the next keyframe, so count them for the summary.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => skipped += n,
                Err(_) => break "the video stream ended",
            },
            _ = check.tick() => {
                if f.stop.exists() {
                    break "stopped";
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    break "duration reached";
                }
                // A still screen sends few frames; only a long silence means the stream died.
                if media.idle_ms().is_some_and(|ms| ms > 10_000) {
                    break "the device stopped sending media";
                }
            }
            _ = &mut ctrl_c => break "interrupted",
        }
    };
    drop(stream);
    let stopped = media.stop().await;
    // The last frame lasts until the stop.
    let summary = recorder.finish(last_frame_at.elapsed())?;
    let _ = std::fs::remove_file(&f.stop);
    let mut v = serde_json::to_value(&summary)?;
    v["udid"] = json!(session.udid);
    v["reason"] = json!(reason);
    v["stream_started_ms"] = json!(started_ms);
    v["first_frame_ms"] = json!(first_frame_ms);
    if skipped > 0 {
        v["skipped_frames"] = json!(skipped);
    }
    if let Err(e) = stopped {
        v["warning"] = json!(format!("{e:#}"));
    }
    Ok(v)
}

/// Starts `record run` in the background and waits until it records or fails.
pub async fn start(udid: Option<&str>, output: &Path) -> Result<Value> {
    let udid = udid_of(udid).await?;
    let f = files(&udid);
    if let Ok(text) = std::fs::read_to_string(&f.running) {
        bail!("a recording already runs for {udid} ({}); end it with `record stop` first", text.trim());
    }
    let _ = std::fs::remove_file(&f.out);
    let _ = std::fs::remove_file(&f.stop);
    let output = std::path::absolute(output)?;
    let log = paths::state_dir().join(format!("{udid}.record.log"));
    let child = std::process::Command::new(std::env::current_exe()?)
        .args(["record", "run", "--udid", &udid, "-o"])
        .arg(&output)
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(&f.out)?)
        .stderr(std::fs::File::create(&log)?)
        .spawn()
        .context("could not start the background recorder")?;
    let pid = child.id();
    std::fs::write(&f.running, json!({ "pid": pid, "path": output }).to_string())?;
    // It records once the stream's first keyframe is in the file; give it a few seconds.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let out = &f.out;
    if let Ok(text) = std::fs::read_to_string(out)
        && !text.trim().is_empty()
    {
        let _ = std::fs::remove_file(&f.running);
        bail!("the recorder ended at once: {}", text.trim());
    }
    if let Ok(err) = std::fs::read_to_string(&log)
        && err.contains("\"error\"")
    {
        let _ = std::fs::remove_file(&f.running);
        bail!("the recorder failed: {}", err.trim());
    }
    Ok(json!({ "udid": udid, "recording": output, "pid": pid }))
}

/// Asks the background recorder to finish and returns its summary.
pub async fn stop(udid: Option<&str>) -> Result<Value> {
    let udid = udid_of(udid).await?;
    let f = files(&udid);
    if !f.running.exists() {
        bail!("no background recording runs for {udid}; start one with `record start`");
    }
    std::fs::write(&f.stop, b"")?;
    let out = &f.out;
    let log = paths::state_dir().join(format!("{udid}.record.log"));
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(20) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let text = std::fs::read_to_string(out).unwrap_or_default();
        let err = std::fs::read_to_string(&log).unwrap_or_default();
        if !text.trim().is_empty() || err.contains("\"error\"") {
            let _ = std::fs::remove_file(&f.running);
            let _ = std::fs::remove_file(out);
            if text.trim().is_empty() {
                bail!("the recorder failed: {}", err.trim());
            }
            return Ok(serde_json::from_str(&text)?);
        }
    }
    bail!("the recorder did not finish within 20 s; its log is {}", log.display())
}
