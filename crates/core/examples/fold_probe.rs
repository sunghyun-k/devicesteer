//! Watches what a foldable reports when it is folded or unfolded: polls `getdisplayinfo` and prints
//! every panel's identity, size, orientation and flags whenever the list changes, and keeps idle
//! connections to the device-info and display services open to see whether either pushes
//! anything on its own. Starts no media stream and puts no input on the device; watch the
//! daemon's stream with `devicesteer events --frames` alongside. Used for PITFALLS #47.
//!
//! ```sh
//! cargo run -p devicesteer-core --example fold_probe -- <udid> <seconds>
//! ```

use std::time::{Duration, Instant};

use devicesteer_core::{DisplayPoller, Session};
use idevice::{IdeviceError, ReadWrite, RemoteXpcClient, RsdService};

/// A RemoteXPC connection to `NAME` that only listens.
struct Listener<const N: usize>(RemoteXpcClient<Box<dyn ReadWrite>>);

const NAMES: [&str; 2] = ["com.apple.coredevice.deviceinfo", "com.apple.coredevice.displayservice"];

impl<const N: usize> RsdService for Listener<N> {
    fn rsd_service_name() -> std::borrow::Cow<'static, str> {
        NAMES[N].into()
    }

    async fn from_stream(stream: Box<dyn ReadWrite>) -> Result<Self, IdeviceError> {
        let mut xpc = RemoteXpcClient::new(stream).await?;
        xpc.do_handshake().await?;
        Ok(Self(xpc))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // The tunnel futures aren't `Send` (PITFALLS #14).
    tokio::task::LocalSet::new().run_until(probe()).await
}

/// One line per sized panel: what tells the panels apart and which one is in use.
fn panels(v: &plist::Value) -> Vec<String> {
    let Some(list) = v.as_dictionary().and_then(|d| d.get("displays")).and_then(|d| d.as_array()) else {
        return vec![format!("no displays: {v:?}")];
    };
    list.iter()
        .filter_map(|d| d.as_dictionary())
        .map(|d| {
            let mut keys: Vec<String> = d
                .iter()
                .map(|(k, v)| match v {
                    plist::Value::Array(_) | plist::Value::Dictionary(_) => format!("{k}={}", compact(v)),
                    other => format!("{k}={}", compact(other)),
                })
                .collect();
            keys.sort();
            keys.join(" ")
        })
        .collect()
}

fn compact(v: &plist::Value) -> String {
    match v {
        plist::Value::Array(a) => format!("[{}]", a.iter().map(compact).collect::<Vec<_>>().join(",")),
        plist::Value::Dictionary(d) => {
            format!("{{{}}}", d.iter().map(|(k, v)| format!("{k}:{}", compact(v))).collect::<Vec<_>>().join(","))
        }
        plist::Value::String(s) => s.clone(),
        plist::Value::Boolean(b) => b.to_string(),
        plist::Value::Integer(i) => i.to_string(),
        plist::Value::Real(r) => r.to_string(),
        other => format!("{other:?}"),
    }
}

async fn probe() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let udid = args.get(1).cloned();
    let seconds: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120.0);
    let session = Session::open(udid.as_deref()).await?;
    let t0 = Instant::now();
    let ms = move || t0.elapsed().as_millis();

    let (mut adapter, mut rsd) = (session.adapter().clone(), session.rsd().clone());
    let mut info = Listener::<0>::connect_rsd(&mut adapter, &mut rsd).await?;
    let mut disp = Listener::<1>::connect_rsd(&mut adapter, &mut rsd).await?;
    tokio::task::spawn_local(async move {
        loop {
            match info.0.recv_any().await {
                Ok(v) => println!("{:>6} deviceinfo pushed: {}", ms(), compact(&v)),
                Err(e) => {
                    println!("{:>6} deviceinfo listener ended: {e}", ms());
                    break;
                }
            }
        }
    });
    tokio::task::spawn_local(async move {
        loop {
            match disp.0.recv_any().await {
                Ok(v) => println!("{:>6} displayservice pushed: {}", ms(), compact(&v)),
                Err(e) => {
                    println!("{:>6} displayservice listener ended: {e}", ms());
                    break;
                }
            }
        }
    });

    let mut poller = DisplayPoller::new(session.adapter().clone(), session.rsd().clone());
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last: Option<Vec<String>> = None;
    let mut last_active: Option<String> = None;
    while Instant::now() < end {
        let began = Instant::now();
        match poller.ask_raw().await {
            Ok(v) => {
                let p = panels(&v);
                if last.as_ref() != Some(&p) {
                    println!("{:>6} getdisplayinfo ({} ms):", ms(), began.elapsed().as_millis());
                    for line in &p {
                        println!("         {line}");
                    }
                    last = Some(p);
                }
                let active = poller.ask().await.map(|d| format!("{d:?}")).unwrap_or_else(|e| format!("parse: {e:#}"));
                if last_active.as_ref() != Some(&active) {
                    println!("{:>6} parsed: {active}", ms());
                    last_active = Some(active);
                }
            }
            Err(e) => println!("{:>6} getdisplayinfo error: {e:#}", ms()),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
