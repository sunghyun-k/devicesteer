//! "Use Shared Clipboard": keeps the Mac clipboard and the device clipboard in step, text only,
//! while the page says the screen is live and the human has the device.
//!
//! Both ways are polled, since neither side tells us when it changes:
//!
//! - **Mac to device**: the Mac pasteboard's `changeCount` every [`MAC_EVERY`] (a cheap integer
//!   read); when it moves, its text goes to the device clipboard (`clipboard_set`), never pasted.
//! - **Device to Mac**: the device clipboard every [`DEVICE_EVERY`], and once when the viewer window
//!   gains focus (the moment someone comes back to the Mac to paste). A read costs one pasteboard
//!   service connection through the daemon's tunnel (PITFALLS #44).
//!
//! Echoes are cut with a hash of the text last synced either way, plus the `changeCount` after our
//! own Mac writes. Both live here, in the shell, like the text itself: the page only says on or
//! off, and nothing here logs or returns the contents.
//!
//! Pasteboard items a password manager marks `org.nspasteboard.ConcealedType` or
//! `org.nspasteboard.TransientType` (see nspasteboard.org) are never sent: a copied password must
//! not land on a phone whose clipboard other apps read. The device side has no such marker that
//! the pasteboard service shows.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
    time::Duration,
};

use devicesteer_daemon::Client;
use serde_json::json;
use tokio::sync::{Mutex, Notify};

const MAC_EVERY: Duration = Duration::from_millis(500);
const DEVICE_EVERY: Duration = Duration::from_millis(1500);

/// What both sides agreed on last, kept across pauses so a resume syncs only what changed.
#[derive(Default)]
struct Synced {
    /// The device the hashes below belong to.
    udid: Option<String>,
    /// Hash of the text last synced either way.
    text: Option<u64>,
    /// The Mac pasteboard's `changeCount` last seen (ours or anyone's). None: not seen yet, so the
    /// first look sends the Mac's text.
    mac_count: Option<isize>,
    /// Hash of what the device clipboard held at the last look ([`NOT_TEXT`] for anything but
    /// text). None: not looked yet. Only a change goes to the Mac: what the device held before the
    /// viewer looked is older than the Mac's clipboard, which the first look sends instead.
    device: Option<u64>,
}

const NOT_TEXT: u64 = 0;

#[derive(Default)]
pub struct SharedClipboard {
    synced: Mutex<Synced>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Wakes the loop for a device read now (the window gained focus).
    poke: Notify,
}

fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl SharedClipboard {
    /// Starts syncing with `client` (`udid`'s daemon), or stops when `client` is None. Starting
    /// syncs once right away: the Mac's text if it changed while paused, then the device's.
    pub async fn set(self: &Arc<Self>, target: Option<(String, Arc<Client>)>) {
        if let Some(t) = self.task.lock().await.take() {
            t.abort();
        }
        let Some((udid, client)) = target else { return };
        {
            let mut s = self.synced.lock().await;
            if s.udid.as_deref() != Some(udid.as_str()) {
                // Another device's clipboard: what was agreed with the last one says nothing.
                *s = Synced { udid: Some(udid), ..Default::default() };
            }
        }
        let me = self.clone();
        *self.task.lock().await = Some(tokio::spawn(async move { me.run(client).await }));
    }

    /// Reads the device clipboard now, if syncing.
    pub fn poke(&self) {
        self.poke.notify_one();
    }

    async fn run(self: Arc<Self>, client: Arc<Client>) {
        let mut device_due = tokio::time::Instant::now();
        loop {
            self.mac_to_device(&client).await;
            if tokio::time::Instant::now() >= device_due {
                self.device_to_mac(&client).await;
                device_due = tokio::time::Instant::now() + DEVICE_EVERY;
            }
            tokio::select! {
                _ = tokio::time::sleep(MAC_EVERY) => {}
                _ = self.poke.notified() => device_due = tokio::time::Instant::now(),
            }
        }
    }

    async fn mac_to_device(&self, client: &Client) {
        let mut s = self.synced.lock().await;
        let count = mac::change_count();
        if s.mac_count == Some(count) {
            return;
        }
        let Some(text) = mac::shareable_text() else {
            s.mac_count = Some(count);
            return;
        };
        let h = hash(&text);
        if s.text == Some(h) {
            s.mac_count = Some(count);
            return;
        }
        // On failure (connection lost, the agent took the device) the count stays unseen, so the
        // next look tries again.
        if client.request("clipboard_set", json!({ "text": text })).await.is_ok() {
            s.mac_count = Some(count);
            s.text = Some(h);
            s.device = Some(h);
        }
    }

    async fn device_to_mac(&self, client: &Client) {
        let Ok(mut r) = client.request("clipboard_get", json!({})).await else { return };
        // An agent's paste is waiting for `paste --restore`: the clipboard holds the agent's text,
        // not the owner's copy.
        if r.get("paste_pending").and_then(|v| v.as_bool()) == Some(true) {
            return;
        }
        let text = match r.as_object_mut().and_then(|o| o.remove("text")) {
            Some(serde_json::Value::String(t)) => Some(t),
            _ => None,
        };
        let h = text.as_deref().map_or(NOT_TEXT, hash);
        let mut s = self.synced.lock().await;
        let before = s.device.replace(h);
        let Some(text) = text else { return };
        if before.is_none() || before == Some(h) || s.text == Some(h) {
            return;
        }
        // The Mac changed since the last look: that copy is newer and goes the other way next.
        if s.mac_count != Some(mac::change_count()) {
            return;
        }
        if let Some(count) = mac::set_text(&text) {
            s.mac_count = Some(count);
            s.text = Some(h);
        }
    }

    /// The viewer's own "Send Clipboard" or "Get Clipboard" moved `text`: both sides now hold it,
    /// so the loop must not send it back.
    pub async fn note_synced(&self, text: &str) {
        let mut s = self.synced.lock().await;
        s.text = Some(hash(text));
        s.device = s.text;
        s.mac_count = Some(mac::change_count());
    }
}

/// Whether this platform can watch its clipboard for the shared clipboard.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    /// Types password managers put on items that must not be synced or kept in history.
    const PRIVATE_TYPES: [&str; 2] = ["org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType"];

    pub fn change_count() -> isize {
        autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount())
    }

    /// The pasteboard's plain text, unless it is empty or marked private.
    pub fn shareable_text() -> Option<String> {
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            let types = pb.types()?;
            if types.iter().any(|t| PRIVATE_TYPES.iter().any(|p| t.to_string() == *p)) {
                return None;
            }
            // SAFETY: an immutable framework constant.
            let text = pb.stringForType(unsafe { NSPasteboardTypeString })?.to_string();
            (!text.is_empty()).then_some(text)
        })
    }

    /// Replaces the pasteboard with `text`; returns the `changeCount` it left.
    pub fn set_text(text: &str) -> Option<isize> {
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            // SAFETY: an immutable framework constant.
            pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
                .then(|| pb.changeCount())
        })
    }
}

/// Elsewhere the page doesn't offer the shared clipboard ([`SUPPORTED`]): without a way to see
/// password managers' private items it would send them.
#[cfg(not(target_os = "macos"))]
mod mac {
    pub fn change_count() -> isize {
        0
    }
    pub fn shareable_text() -> Option<String> {
        None
    }
    pub fn set_text(_: &str) -> Option<isize> {
        None
    }
}
