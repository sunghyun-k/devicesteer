//! Drives live touches through the daemon as the viewer does (one step in flight, each move with
//! its time, the video subscribed) and prints how the device's frames came while the finger moved,
//! and the steps' round trips. Used for PITFALLS #18; together with `DEVICESTEER_INPUT_TRACE` (see
//! `devicesteer_core::trace`) it shows where steps get lost or bunched.
//!
//! Swipes go up and down 400 pt in the middle of the screen over 0.6 s, so open a long list first
//! (Settings). On a personal device, follow AGENTS.md: this puts input on the screen.
//!
//! ```sh
//! cargo run -p devicesteer-daemon --example touch_bench -- <udid> [hz=60] [swipes=4] [jitter_ms=0] [flick]
//! ```
//!
//! `jitter_ms` delays each step by up to that much, as the page and IPC do; `flick` lifts while
//! moving instead of resting first.

use std::time::{Duration, Instant};

use devicesteer_daemon::{Client, Source};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let udid = args.get(1).cloned();
    let hz: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let swipes: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    // Extra random delay per step (ms), like the page's event loop and IPC add.
    let jitter: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    // Lift while moving instead of resting first.
    let flick = args.get(5).is_some_and(|s| s == "flick");
    let epoch = Instant::now();
    let ms = move |i: Instant| (i - epoch).as_secs_f64() * 1000.0;
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut client = Client::connect(udid.as_deref(), Source::Human).await?;
    let mut events = client.take_events().unwrap();
    let log: std::sync::Arc<std::sync::Mutex<Vec<(Instant, u64)>>> = Default::default();
    let flog = log.clone();
    let frames = tokio::spawn(async move {
        while let Some(m) = events.recv().await {
            if let devicesteer_daemon::wire::Message::Frame { header, .. } = m {
                let pts = header.get("pts_us").and_then(|v| v.as_u64()).unwrap_or(0);
                flog.lock().unwrap().push((Instant::now(), pts));
            }
        }
    });
    println!(
        "subscribe: {}",
        client
            .request("subscribe", json!({ "frames": true }))
            .await?
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    let tick = Duration::from_secs_f64(1.0 / hz);
    let mut rtts = Vec::new();
    for i in 0..swipes {
        // Alternate up and down drags of 400 pt over 0.6 s in the middle of the screen.
        let (y0, y1) = if i % 2 == 0 {
            (700.0, 300.0)
        } else {
            (300.0, 700.0)
        };
        let x = 200.0;
        client
            .request("touch", json!({ "phase": "down", "point": [x, y0] }))
            .await?;
        let began = Instant::now();
        let mark = log.lock().unwrap().len();
        let total = Duration::from_millis(600);
        let mut next = began + tick;
        loop {
            tokio::time::sleep_until(next.into()).await;
            next += tick;
            let drawn = Instant::now();
            let t = (began.elapsed().as_secs_f64() / total.as_secs_f64()).min(1.0);
            // A flick lifts at 95 % of the way, still moving.
            let y = y0 + (y1 - y0) * if flick { t * 0.95 } else { t };
            if jitter > 0 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                tokio::time::sleep(Duration::from_micros(seed % (jitter * 1000))).await;
            }
            let s = Instant::now();
            client
                .request(
                    "touch",
                    json!({ "phase": "move", "point": [x, y], "t": ms(drawn) }),
                )
                .await?;
            rtts.push(s.elapsed().as_secs_f64() * 1000.0);
            if t >= 1.0 {
                break;
            }
        }
        // Device-side frame cadence while the finger moved (pts is the device's clock).
        let pts: Vec<u64> = log.lock().unwrap()[mark..]
            .iter()
            .filter(|(at, _)| *at > began + Duration::from_millis(100))
            .map(|f| f.1)
            .collect();
        let mut d: Vec<f64> = pts
            .windows(2)
            .map(|w| (w[1].saturating_sub(w[0])) as f64 / 1000.0)
            .collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if d.len() > 2 {
            let span = (pts[pts.len() - 1] - pts[0]) as f64 / 1e6;
            let over = d.iter().filter(|x| **x > 25.0).count();
            println!(
                "swipe {i}: frames {} ({:.1}/s) interval ms p50 {:.1} p95 {:.1} max {:.1}, gaps>25ms {over}",
                pts.len(),
                (pts.len() - 1) as f64 / span,
                d[d.len() / 2],
                d[d.len() * 95 / 100],
                d[d.len() - 1]
            );
        }
        // Rest, then lift: the list should stop.
        if !flick {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        client.request("touch", json!({ "phase": "up" })).await?;
        tokio::time::sleep(Duration::from_millis(if flick { 2500 } else { 800 })).await;
    }
    rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = rtts.len();
    println!(
        "moves {n}, rtt ms avg {:.2} p50 {:.2} p95 {:.2} max {:.2}",
        rtts.iter().sum::<f64>() / n as f64,
        rtts[n / 2],
        rtts[n * 95 / 100],
        rtts[n - 1]
    );
    drop(client);
    let _ = tokio::time::timeout(Duration::from_millis(300), frames).await;
    Ok(())
}
