//! `iphone-use`: command-line access to a connected iPhone or iPad.
//!
//! Output is JSON on stdout. Errors go to stderr as `{"error": "..."}` with exit code 1.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iphone_use_core::{Session, list_devices};
use serde_json::json;

#[derive(Parser)]
#[command(name = "iphone-use", version, about = "Operate a real iPhone or iPad over USB or Wi-Fi")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct Target {
    /// Device to use. Required when more than one device is connected.
    #[arg(long, global = true)]
    udid: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// List connected devices (usbmuxd, USB and Wi-Fi).
    Devices,
    /// Open a tunnel and list the services the device advertises over RSD.
    Services {
        #[command(flatten)]
        target: Target,
    },
    /// Capture the screen as PNG.
    Screenshot {
        #[command(flatten)]
        target: Target,
        /// Where to write the PNG.
        #[arg(short, long, default_value = "screenshot.png")]
        output: PathBuf,
        /// Panel to capture (a display `uniqueId`); defaults to the device's primary panel.
        #[arg(long)]
        display: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match run(cli.command).await {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(e) => {
            eprintln!("{}", json!({ "error": format!("{e:#}") }));
            std::process::exit(1);
        }
    }
}

async fn run(command: Command) -> Result<serde_json::Value> {
    Ok(match command {
        Command::Devices => json!({ "devices": list_devices().await? }),
        Command::Services { target } => {
            let session = Session::open(target.udid.as_deref()).await?;
            json!({
                "udid": session.udid,
                "transport": session.transport,
                "services": session.services(),
            })
        }
        Command::Screenshot { target, output, display } => {
            let mut session = Session::open(target.udid.as_deref()).await?;
            let png = session.screenshot(display.as_deref()).await?;
            std::fs::write(&output, &png)
                .with_context(|| format!("could not write {}", output.display()))?;
            let (width, height) = png_size(&png).unzip();
            json!({
                "udid": session.udid,
                "transport": session.transport,
                "path": output,
                "bytes": png.len(),
                "width": width,
                "height": height,
            })
        }
    })
}

/// Width and height from a PNG's IHDR chunk.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    Some((be(&png[16..20]), be(&png[20..24])))
}
