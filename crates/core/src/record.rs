//! The live screen as frames, and recordings of it to MP4.
//!
//! [`FrameStream`] turns a video [`MediaSession`]'s RTP packets into access units. The device sends
//! one keyframe when the stream starts and none after unless asked (PITFALLS #11), so a viewer
//! that joins late, or a lost packet, needs [`MediaSession::request_keyframe`].
//!
//! The track bookkeeping (first keyframe, durations from RTP timestamps) follows iphone-cli
//! `src/record/mod.rs`, MIT, see THIRD_PARTY_NOTICES.md.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::{
    Rotation,
    media::{MediaPacket, MediaSession, StreamKind},
    mp4::{Writer, seconds, ticks_between, ticks_from_micros},
    video::{Assembler, ParameterSets, RTP_CLOCK},
};

/// One decoded-order frame of the screen video.
#[derive(Debug, Clone)]
pub struct Frame {
    /// RTP timestamp (24 kHz).
    pub rtp_ts: u32,
    /// Microseconds since the stream's first frame, from the RTP clock.
    pub pts_us: u64,
    pub keyframe: bool,
    /// Annex-B (start codes), parameter sets included on keyframes.
    pub annexb: Arc<Vec<u8>>,
    /// Length-prefixed NAL units without parameter sets (MP4 / WebCodecs "hvcC" layout).
    pub sample: Arc<Vec<u8>>,
    /// The stream's parameter sets, once a keyframe brought them.
    pub params: Option<Arc<ParameterSets>>,
    /// When the frame's first packet reached the host.
    pub arrived: Instant,
    /// Packets lost in the stream so far.
    pub lost: u64,
}

/// Assembles frames from a media session's video packets and broadcasts them.
pub struct FrameStream {
    frames: broadcast::Sender<Frame>,
    /// SSRC of the video RTP packets (0 until the first one), for keyframe requests.
    ssrc: Arc<std::sync::atomic::AtomicU32>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FrameStream {
    pub fn start(media: &mut MediaSession) -> Self {
        let mut packets = media.subscribe();
        let (frames, _) = broadcast::channel(256);
        let tx = frames.clone();
        let ssrc = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let ssrc_w = ssrc.clone();
        let task = tokio::spawn(async move {
            let mut asm = Assembler::default();
            let mut params: Option<Arc<ParameterSets>> = None;
            let mut first_ts: Option<u32> = None;
            let mut arrival: Option<Instant> = None;
            loop {
                let p: MediaPacket = match packets.recv().await {
                    Ok(p) => p,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("frame assembler fell {n} packets behind");
                        continue;
                    }
                    Err(_) => break,
                };
                if p.kind != StreamKind::Video || idevice::core_device::is_rtcp(&p.data) {
                    continue;
                }
                if let Some(rtp) = idevice::core_device::RtpPacket::parse(&p.data) {
                    ssrc_w.store(rtp.ssrc, std::sync::atomic::Ordering::Relaxed);
                }
                arrival.get_or_insert(p.at);
                for au in asm.push(&p.data, 0) {
                    let arrived = arrival.take().unwrap_or(p.at);
                    if !au.has_picture() {
                        continue;
                    }
                    if au.is_keyframe()
                        && let Some(ps) = ParameterSets::from_keyframe(&au)
                        && params.as_deref() != Some(&ps)
                    {
                        params = Some(Arc::new(ps));
                    }
                    let first = *first_ts.get_or_insert(au.ts);
                    let ticks = au.ts.wrapping_sub(first);
                    let ticks = if ticks > u32::MAX / 2 { 0 } else { ticks };
                    let _ = tx.send(Frame {
                        rtp_ts: au.ts,
                        pts_us: u64::from(ticks) * 1_000_000 / u64::from(RTP_CLOCK),
                        keyframe: au.is_keyframe(),
                        annexb: Arc::new(au.annexb()),
                        sample: Arc::new(au.sample()),
                        params: params.clone(),
                        arrived,
                        lost: asm.lost,
                    });
                }
            }
        });
        Self { frames, ssrc, task }
    }

    /// The video's RTP SSRC, once a packet came: what [`MediaSession::request_keyframe`] names.
    pub fn ssrc(&self) -> Option<u32> {
        Some(self.ssrc.load(std::sync::atomic::Ordering::Relaxed)).filter(|s| *s != 0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.frames.subscribe()
    }
}

/// What a finished recording reports.
#[derive(Debug, Clone, Serialize)]
pub struct RecordSummary {
    pub path: PathBuf,
    pub bytes: u64,
    pub frames: u64,
    pub keyframes: u64,
    pub seconds: f64,
    /// Frames per second over the recording (the device sends frames only when the screen changes).
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub codec: String,
    pub lost_packets: u64,
}

/// Writes frames to an MP4 as they come.
pub struct Recorder {
    writer: Option<Writer>,
    params: Option<Arc<ParameterSets>>,
    last_ts: Option<u32>,
    first_ts: Option<u32>,
    frames: u64,
    keyframes: u64,
    lost_at_start: Option<u64>,
    lost: u64,
}

impl Recorder {
    pub fn create(path: &Path) -> Result<Self> {
        let writer = Writer::create(path).with_context(|| format!("could not create {}", path.display()))?;
        Ok(Self {
            writer: Some(writer),
            params: None,
            last_ts: None,
            first_ts: None,
            frames: 0,
            keyframes: 0,
            lost_at_start: None,
            lost: 0,
        })
    }

    /// Appends a frame. Frames before the first keyframe can't be decoded and are skipped.
    pub fn push(&mut self, f: &Frame) -> Result<()> {
        let Some(writer) = self.writer.as_mut() else { return Ok(()) };
        if self.params.is_none() {
            if !f.keyframe || f.params.is_none() {
                return Ok(());
            }
            self.params = f.params.clone();
            self.lost_at_start = Some(f.lost);
        }
        if let Some(prev) = self.last_ts {
            writer.set_last_duration(ticks_between(prev, f.rtp_ts));
        }
        writer.append(&f.sample, f.keyframe)?;
        self.last_ts = Some(f.rtp_ts);
        self.first_ts.get_or_insert(f.rtp_ts);
        self.frames += 1;
        self.keyframes += u64::from(f.keyframe);
        self.lost = f.lost - self.lost_at_start.unwrap_or(f.lost);
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Writes the index and moves the file into place. The last frame lasts `tail` (the time from
    /// its arrival to the stop).
    pub fn finish(mut self, tail: std::time::Duration) -> Result<RecordSummary> {
        let mut writer = self.writer.take().expect("not finished yet");
        let Some(params) = self.params.clone() else {
            let path = writer.path().to_path_buf();
            writer.discard();
            return Err(anyhow!(
                "no keyframe arrived, so nothing was recorded to {} (the screen video did not start)",
                path.display()
            ));
        };
        writer.set_last_duration_if_unset(ticks_from_micros(tail.as_micros() as u64));
        let path = writer.path().to_path_buf();
        let total = writer.total_ticks();
        let bytes = writer.finish(&params, Rotation::R0)?;
        let secs = seconds(total);
        Ok(RecordSummary {
            path,
            bytes,
            frames: self.frames,
            keyframes: self.keyframes,
            seconds: (secs * 1000.0).round() / 1000.0,
            fps: if secs > 0.0 { ((self.frames as f64 / secs) * 10.0).round() / 10.0 } else { 0.0 },
            width: params.width,
            height: params.height,
            codec: params.codec(),
            lost_packets: self.lost,
        })
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(w) = self.writer.take() {
            w.discard();
        }
    }
}
