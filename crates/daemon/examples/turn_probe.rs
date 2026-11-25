//! Watches what happens when a device's screen turns or a foldable is folded or unfolded, through
//! the daemon: every panel's `getdisplayinfo` entry (polled every 0.1 s on a tunnel of its own),
//! the daemon's `config`, `orientation` and `panel` events, and its video. Each `config` starts a
//! new raw HEVC file (`<out>/segN.h265`, Annex-B) that ffmpeg decodes, so where the screen sits in
//! the picture can be looked at frame by frame; `<out>/frames.tsv` maps each frame to its time.
//! Puts no input on the device. Used for PITFALLS #48.
//!
//! ```sh
//! cargo run -p devicesteer-daemon --example turn_probe -- <udid> <seconds> <out dir>
//! ```

use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use base64::Engine;
use devicesteer_core::{DisplayPoller, Session};
use devicesteer_daemon::{Client, Source, wire::Message};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // The tunnel futures aren't `Send` (PITFALLS #14).
    tokio::task::LocalSet::new().run_until(probe()).await
}

/// The NAL units of an `hvcC` record's arrays, each with a start code.
fn hvcc_annexb(hvcc: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(&arrays) = hvcc.get(22) else { return out };
    let mut at = 23;
    for _ in 0..arrays {
        let Some(n) = hvcc.get(at + 1..at + 3) else { break };
        let count = u16::from_be_bytes([n[0], n[1]]);
        at += 3;
        for _ in 0..count {
            let Some(l) = hvcc.get(at..at + 2) else { return out };
            let len = usize::from(u16::from_be_bytes([l[0], l[1]]));
            at += 2;
            let Some(nal) = hvcc.get(at..at + len) else { return out };
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
            at += len;
        }
    }
    out
}

/// A sample's 4-byte length prefixes turned into start codes.
fn sample_annexb(sample: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sample.len() + 16);
    let mut at = 0;
    while let Some(l) = sample.get(at..at + 4) {
        let len = u32::from_be_bytes([l[0], l[1], l[2], l[3]]) as usize;
        at += 4;
        let Some(nal) = sample.get(at..at + len) else { break };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        at += len;
    }
    out
}

/// The device's orientation (top-level `orientation`) and one line per sized display, with the
/// keys that say which one it is, whether it is in use and how it is turned.
fn panels(v: &plist::Value) -> String {
    let text = |v: Option<&plist::Value>| match v {
        Some(plist::Value::String(s)) => s.clone(),
        Some(plist::Value::Boolean(b)) => b.to_string(),
        Some(plist::Value::Integer(i)) => i.to_string(),
        Some(plist::Value::Real(r)) => r.to_string(),
        Some(plist::Value::Array(a)) => format!("[{}]", a.iter().map(text_of).collect::<Vec<_>>().join(",")),
        Some(other) => format!("{other:?}"),
        None => "-".into(),
    };
    let Some(top) = v.as_dictionary() else { return format!("{v:?}") };
    let mut lines = Vec::new();
    if let Some(o) = top.get("orientation").and_then(|o| o.as_dictionary()) {
        let mut keys: Vec<String> = o.iter().map(|(k, v)| format!("{k}={}", text(Some(v)))).collect();
        keys.sort();
        lines.push(keys.join(" "));
    }
    for d in top.get("displays").and_then(|d| d.as_array()).into_iter().flatten().filter_map(|d| d.as_dictionary()) {
        let bounds = text(d.get("bounds"));
        if bounds.ends_with("[0,0]]") {
            continue;
        }
        lines.push(format!(
            "id={} type={} active={} primary={} orient={} native={} backlight={} bounds={} frame={}",
            text(d.get("displayId")),
            text(d.get("type")),
            text(d.get("active")),
            text(d.get("primary")),
            text(d.get("currentOrientation")),
            text(d.get("nativeOrientation")),
            text(d.get("backlightState")),
            bounds,
            text(d.get("frame")),
        ));
    }
    lines.join("\n         ")
}

fn text_of(v: &plist::Value) -> String {
    match v {
        plist::Value::Array(a) => format!("[{}]", a.iter().map(text_of).collect::<Vec<_>>().join(",")),
        plist::Value::Integer(i) => i.to_string(),
        plist::Value::Real(r) => r.to_string(),
        other => format!("{other:?}"),
    }
}

async fn probe() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let udid = args.get(1).cloned().context("usage: turn_probe <udid> <seconds> <out dir>")?;
    let seconds: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120.0);
    let out = PathBuf::from(args.get(3).cloned().unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&out)?;
    let t0 = Instant::now();
    let ms = move || t0.elapsed().as_millis();
    let end = t0 + Duration::from_secs_f64(seconds);

    let session = Session::open(Some(&udid)).await?;
    let mut poller = DisplayPoller::new(session.adapter().clone(), session.rsd().clone());
    tokio::task::spawn_local(async move {
        let mut last = String::new();
        loop {
            match poller.ask_raw().await {
                Ok(v) => {
                    let p = panels(&v);
                    if p != last {
                        println!("{:>6} getdisplayinfo:\n         {p}", ms());
                        if last.is_empty() {
                            std::fs::write(out_raw(), format!("{v:#?}")).ok();
                        }
                        last = p;
                    }
                }
                Err(e) => println!("{:>6} getdisplayinfo error: {e:#}", ms()),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let mut client = Client::connect(Some(&udid), Source::Agent).await?;
    let mut events = client.take_events().context("events already taken")?;
    client.request("subscribe", json!({ "frames": true })).await?;
    let mut tsv = std::fs::File::create(out.join("frames.tsv"))?;
    writeln!(tsv, "t_ms\tseg\tindex\tkeyframe\tbytes")?;
    let mut seg: Option<(usize, std::fs::File, u64)> = None;
    let mut segs = 0;
    while Instant::now() < end {
        let Ok(Some(m)) = tokio::time::timeout(end.saturating_duration_since(Instant::now()), events.recv()).await else {
            break;
        };
        match m {
            Message::Json(mut v) => {
                if v["event"] == "config" {
                    let hvcc = base64::engine::general_purpose::STANDARD.decode(v["description"].as_str().unwrap_or(""))?;
                    let path = out.join(format!("seg{segs}.h265"));
                    let mut f = std::fs::File::create(&path)?;
                    f.write_all(&hvcc_annexb(&hvcc))?;
                    seg = Some((segs, f, 0));
                    segs += 1;
                    v["description"] = json!(format!("<hvcC, {}>", path.display()));
                }
                if v["event"] != "frame" {
                    println!("{:>6} event {v}", ms());
                }
            }
            Message::Frame { header, data } => {
                let key = header["keyframe"].as_bool().unwrap_or(false);
                if let Some((n, f, i)) = seg.as_mut() {
                    f.write_all(&sample_annexb(&data))?;
                    writeln!(tsv, "{}\t{n}\t{i}\t{key}\t{}", ms(), data.len())?;
                    *i += 1;
                }
                if key {
                    println!("{:>6} keyframe ({} bytes)", ms(), data.len());
                }
            }
        }
    }
    Ok(())
}

fn out_raw() -> PathBuf {
    PathBuf::from(std::env::args().nth(3).unwrap_or_else(|| ".".into())).join("displayinfo.txt")
}
