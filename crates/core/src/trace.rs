//! Input timing trace, a debug option: with `IPHONE_USE_INPUT_TRACE=<file>` in the daemon's (or
//! a `--no-daemon` command's) environment, every live touch step and HID touch report appends a
//! line `<monotonic µs> <stage> <fields>` to that file:
//!
//! - `recv <conn>`: the daemon read a `touch` request from client connection `conn`
//! - `req <conn> <phase>`: the request got the device and is handled
//! - `hid <state> <x> <y> <µs>`: a touchscreen report was written (state 194 contact, 2 release;
//!   x and y in report units 0..65535), and how long the write took
//!
//! PITFALLS #18 shows what it was used for. A daemon that clients start takes the variable from
//! the environment of the client that started it; stop a running one first.

use std::{
    io::Write,
    sync::{Mutex, OnceLock},
    time::Instant,
};

static FILE: OnceLock<Option<(Instant, Mutex<std::fs::File>)>> = OnceLock::new();

fn file() -> Option<&'static (Instant, Mutex<std::fs::File>)> {
    FILE.get_or_init(|| {
        let p = std::env::var_os("IPHONE_USE_INPUT_TRACE")?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()?;
        Some((Instant::now(), Mutex::new(f)))
    })
    .as_ref()
}

/// Appends one line when the trace is on; does nothing (and formats nothing) otherwise.
pub fn line(stage: &str, fields: std::fmt::Arguments<'_>) {
    if let Some((t0, f)) = file() {
        let us = t0.elapsed().as_micros();
        let _ = writeln!(f.lock().unwrap(), "{us} {stage} {fields}");
    }
}
