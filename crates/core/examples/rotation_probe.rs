//! Watches what the device does when the interface turns: starts a video media session of its own,
//! polls `getdisplayinfo`, and logs keyframes, parameter sets, RTP header extensions and each
//! orientation change with its time. The raw stream is written as Annex-B, so `ffmpeg` can pull
//! out the pictures around a turn. Used for PITFALLS #27.
//!
//! ```sh
//! cargo run -p iphone-use-core --example rotation_probe -- <udid> <seconds> <out.h265>
//! ```
//!
//! It puts no input on the device; turn it by hand while it runs.

use std::{io::Write, time::Instant};

use iphone_use_core::{DisplayPoller, FrameStream, MediaMode, MediaSession, Session};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // The tunnel futures aren't `Send` (PITFALLS #14): the display poll runs as a local task.
    tokio::task::LocalSet::new().run_until(probe()).await
}

async fn probe() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let udid = args.get(1).cloned();
    let seconds: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let out = args.get(3).cloned().unwrap_or_else(|| "rotation.h265".into());
    let session = Session::open(udid.as_deref()).await?;
    let t0 = Instant::now();
    let ms = move || t0.elapsed().as_millis();
    let mut poller = DisplayPoller::new(session.adapter().clone(), session.rsd().clone());
    let display = poller.ask().await?;
    println!("{:>6} display {:?} size {:?}", ms(), display.rotation, display.size());
    // Display polls on their own task, so a slow answer doesn't hold up the packets.
    let (tx, mut rotations) = tokio::sync::mpsc::unbounded_channel();
    tokio::task::spawn_local(async move {
        let mut last = display.rotation;
        let mut times = Vec::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let began = Instant::now();
            let r = poller.ask().await;
            let took = began.elapsed().as_millis();
            times.push(took);
            if took > 500 {
                let _ = tx.send(format!("display took {took} ms"));
            }
            match r {
                Ok(d) if d.rotation != last => {
                    last = d.rotation;
                    let _ = tx.send(format!("display {:?} size {:?} (asked {took} ms)", d.rotation, d.size()));
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.send(format!("display error {e:#}"));
                }
            }
            if times.len() % 40 == 0 {
                let mut t = times.clone();
                t.sort();
                let _ = tx.send(format!("display ms median {} max {}", t[t.len() / 2], t[t.len() - 1]));
            }
        }
    });
    let mut media = MediaSession::start(
        &session.udid,
        session.adapter(),
        session.rsd(),
        MediaMode::Video,
        display.display_id.unwrap_or(1),
    )
    .await?;
    let stream = FrameStream::start(&mut media);
    let mut frames = stream.subscribe();
    let mut packets = media.subscribe();
    let mut file = std::fs::File::create(&out)?;
    let mut last_params: Option<usize> = None;
    let mut extensions = std::collections::BTreeMap::<Vec<u8>, u64>::new();
    let (mut frame_count, mut sec_frames, mut sec_video, mut sec_audio, mut sec_bytes) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut second = tokio::time::interval(std::time::Duration::from_secs(1));
    let end = tokio::time::sleep(std::time::Duration::from_secs_f64(seconds));
    tokio::pin!(end);
    loop {
        tokio::select! {
            _ = &mut end => break,
            Some(line) = rotations.recv() => println!("{:>6} {line}", ms()),
            _ = second.tick() => {
                println!("{:>6} second: frames {sec_frames} video packets {sec_video} ({sec_bytes} B) audio packets {sec_audio}", ms());
                (sec_frames, sec_video, sec_audio, sec_bytes) = (0, 0, 0, 0);
            }
            f = frames.recv() => {
                let f = match f {
                    Ok(f) => f,
                    Err(e) => { println!("{:>6} frames: {e}", ms()); continue }
                };
                frame_count += 1;
                sec_frames += 1;
                file.write_all(&f.annexb)?;
                let types: Vec<u8> = nal_types(&f.annexb);
                let has_ps = types.iter().any(|t| (32..=34).contains(t));
                let params = f.params.as_ref().map(|p| std::sync::Arc::as_ptr(p) as usize);
                if f.keyframe || has_ps || params != last_params {
                    let size = f.params.as_ref().map(|p| (p.width, p.height));
                    println!("{:>6} frame {frame_count} keyframe {} nals {types:?} params {size:?} lost {}", ms(), f.keyframe, f.lost);
                    last_params = params;
                }
            }
            p = packets.recv() => {
                let p = match p {
                    Ok(p) => p,
                    Err(e) => { println!("{:>6} packets: {e}", ms()); continue }
                };
                if p.kind != iphone_use_core::StreamKind::Video {
                    sec_audio += 1;
                    continue;
                }
                sec_video += 1;
                sec_bytes += p.data.len() as u64;
                if let Some(ext) = extension(&p.data) {
                    // The last two bytes count up per frame; the rest is what may carry state.
                    let key = ext[..ext.len().saturating_sub(2)].to_vec();
                    *extensions.entry(key).or_default() += 1;
                }
            }
        }
    }
    for (k, n) in &extensions {
        println!("extension {k:02x?}: {n} packets");
    }
    println!("frames {frame_count}");
    media.stop().await?;
    Ok(())
}

/// NAL unit types in an Annex-B buffer.
fn nal_types(b: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 4 < b.len() {
        if b[i..i + 4] == [0, 0, 0, 1] {
            v.push((b[i + 4] >> 1) & 0x3f);
            i += 4;
        }
        i += 1;
    }
    v
}

/// The RTP header extension (profile and data), if the packet has one.
fn extension(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() < 12 || d[0] & 0x10 == 0 {
        return None;
    }
    let off = 12 + 4 * usize::from(d[0] & 0x0f);
    let words = usize::from(u16::from_be_bytes([*d.get(off + 2)?, *d.get(off + 3)?]));
    d.get(off..off + 4 + 4 * words).map(<[u8]>::to_vec)
}
