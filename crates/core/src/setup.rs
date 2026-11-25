//! Making a freshly connected device ready, without Xcode: trust (pairing), Developer Mode and
//! the Developer Disk Image.
//!
//! Every step works over lockdown, not the tunnel: the tunnel's developer services exist only
//! once all three steps are done.
//!
//! ```text
//! trust             usbmuxd pair record + lockdown Pair (the device asks "Trust This Computer?")
//! developer_mode    AMFI reveals the switch; the user turns it on, the device restarts, the user
//!                   confirms after the restart
//! disk_image        mobile_image_mounter: a personalized DDI, signed for this device by Apple's
//!                   TSS server, uploaded and mounted
//! ```

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use idevice::{
    IdeviceError, IdeviceService,
    amfi::AmfiClient,
    lockdown::LockdownClient,
    mobile_image_mounter::ImageMounter,
    provider::{IdeviceProvider, UsbmuxdProvider},
    usbmuxd::UsbmuxdDevice,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    devices::{Transport, mux_addr, mux_devices, transport_of, unsupported_in},
    step,
};

/// One step of making a device ready, in the order they must happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStep {
    /// The device trusts this computer (a pair record both sides accept).
    Trust,
    /// Developer Mode is on.
    DeveloperMode,
    /// The personalized Developer Disk Image is mounted ("developer tools" in the viewer).
    DiskImage,
}

impl SetupStep {
    pub const ALL: [SetupStep; 3] = [SetupStep::Trust, SetupStep::DeveloperMode, SetupStep::DiskImage];

    /// The CLI's name for the step's command.
    pub fn command(self) -> &'static str {
        match self {
            SetupStep::Trust => "trust",
            SetupStep::DeveloperMode => "developer-mode",
            SetupStep::DiskImage => "disk-image",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.replace('_', "-").as_str() {
            "trust" | "pair" => SetupStep::Trust,
            "developer-mode" => SetupStep::DeveloperMode,
            "disk-image" | "ddi" => SetupStep::DiskImage,
            _ => bail!("unknown setup step `{s}`; use trust, developer-mode or disk-image"),
        })
    }
}

/// Where one step stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Done,
    /// This step is the next one to do.
    Needed,
    /// An earlier step has to be done before this one can be checked.
    Blocked,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepStatus {
    pub step: SetupStep,
    pub state: StepState,
    /// What is wrong, when the step isn't done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// How ready a device is, and what to do next.
#[derive(Debug, Clone, Serialize)]
pub struct SetupStatus {
    pub udid: String,
    pub transports: Vec<Transport>,
    /// All three steps are done: every other command works.
    pub ready: bool,
    /// Why DeviceSteer can't work with this device at all (older than iOS 17). Then `ready` is
    /// false, `step` is absent and every step is `blocked`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<String>,
    /// The first step that isn't done.
    pub step: Option<SetupStep>,
    pub steps: Vec<StepStatus>,
    /// What to do next, for an agent to follow or tell the user.
    pub next: String,
}

impl SetupStatus {
    /// Errors with the reason when the device is too old for DeviceSteer.
    fn ensure_supported(self) -> Result<Self> {
        match &self.unsupported {
            Some(r) => bail!("{r}"),
            None => Ok(self),
        }
    }

    fn state(&self, step: SetupStep) -> StepState {
        self.steps.iter().find(|s| s.step == step).map_or(StepState::Blocked, |s| s.state)
    }
}

/// What a long step reports while it runs.
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub step: SetupStep,
    /// The device waits for the person holding it (tap Trust, unlock, restart, confirm).
    pub waiting_for_user: bool,
    pub message: String,
    /// 0..1, for downloads and uploads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
}

/// Receives [`Progress`] reports. Cheap to clone.
pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// A progress sink that drops everything.
pub fn no_progress() -> ProgressFn {
    Arc::new(|_| {})
}

fn report(progress: &ProgressFn, step: SetupStep, waiting_for_user: bool, message: impl Into<String>) {
    progress(Progress { step, waiting_for_user, message: message.into(), fraction: None });
}

// ---------- reading the state ----------

/// What lockdown says about the steps, read with one lockdown connection (and one image mounter
/// connection once Developer Mode is on).
#[derive(Debug, Default)]
pub(crate) struct Probe {
    /// `None`: trusted. Otherwise why not.
    pub untrusted: Option<String>,
    /// `None` when it could not be read (not trusted yet).
    pub developer_mode: Option<bool>,
    /// `None` when it could not be read (not trusted, or Developer Mode off).
    pub disk_image: Option<bool>,
    /// Set when the device's OS is too old for DeviceSteer; nothing else is read then.
    pub unsupported: Option<String>,
    /// Every lockdown value, for the device list.
    pub values: plist::Dictionary,
}

impl Probe {
    /// The first step that isn't done.
    /// `None` for an unsupported device too: no step would make it usable.
    pub fn step(&self) -> Option<SetupStep> {
        if self.unsupported.is_some() {
            None
        } else if self.untrusted.is_some() {
            Some(SetupStep::Trust)
        } else if self.developer_mode == Some(false) {
            Some(SetupStep::DeveloperMode)
        } else if self.disk_image == Some(false) {
            Some(SetupStep::DiskImage)
        } else {
            None
        }
    }
}

/// Whether an error means the device doesn't trust this computer (rather than a dropped
/// connection, which must stay an error).
fn is_trust_error(e: &IdeviceError) -> bool {
    matches!(
        e,
        IdeviceError::InvalidHostID
            | IdeviceError::PasswordProtected
            | IdeviceError::UserDeniedPairing
            | IdeviceError::PairingDialogResponsePending
    )
}

/// Reads the setup state of one device. Errors only for connection failures; a missing or
/// rejected pair record is a result.
pub(crate) async fn probe(provider: &UsbmuxdProvider) -> Result<Probe> {
    let mut p = Probe::default();
    let mut mux = step("usbmuxd connection", mux_addr().connect(0)).await?;
    // usbmuxd answers without PairRecordData when it has no record for the device.
    let pairing = match step("pair record lookup", mux.get_pair_record(&provider.udid)).await {
        Ok(r) => r,
        Err(_) => {
            p.untrusted = Some("this computer has no pair record for the device".into());
            p.values = public_values(provider).await;
            p.unsupported = unsupported_in(&p.values);
            return Ok(p);
        }
    };
    let mut lockdown = step("lockdown connection", LockdownClient::connect(provider)).await?;
    match tokio::time::timeout(crate::STEP_TIMEOUT, lockdown.start_session(&pairing)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) if is_trust_error(&e) => {
            // The record is here but the device forgot it (Trust reset, "Don't Trust", erase).
            p.untrusted = Some(format!("the device no longer trusts this computer ({e:?})"));
            p.values = public_values(provider).await;
            p.unsupported = unsupported_in(&p.values);
            return Ok(p);
        }
        Ok(Err(e)) => bail!("lockdown session failed (connection error: {e:?})"),
        Err(_) => bail!("lockdown session timed out (connection stalled)"),
    }
    let all = step("lockdown read", lockdown.get_value(None, None)).await?;
    p.values = all.into_dictionary().unwrap_or_default();
    // A device older than iOS 17 has none of the services the steps lead to; checking them would
    // only make it look ready (it has no Developer Mode before 16, and its own kind of disk image).
    p.unsupported = unsupported_in(&p.values);
    if p.unsupported.is_some() {
        return Ok(p);
    }
    p.developer_mode = Some(
        match lockdown.get_value(Some("DeveloperModeStatus"), Some("com.apple.security.mac.amfi")).await {
            Ok(v) => v.as_boolean().unwrap_or(false),
            // Some releases leave the key out while the mode is off.
            Err(IdeviceError::UnexpectedResponse(_)) => false,
            Err(e) => bail!("Developer Mode read failed (connection error: {e:?})"),
        },
    );
    // The image mounter refuses personalized images while Developer Mode is off, so there is
    // nothing to check before it.
    if p.developer_mode == Some(true) {
        p.disk_image = Some(disk_image_mounted(provider).await?);
    }
    Ok(p)
}

/// What lockdown tells anyone without a session (model, OS version, class; no name), so an
/// untrusted device can still be shown for what it is. Best effort.
pub(crate) async fn public_values(provider: &UsbmuxdProvider) -> plist::Dictionary {
    let read = async {
        let mut lockdown = LockdownClient::connect(provider).await.ok()?;
        lockdown.get_value(None, None).await.ok()?.into_dictionary()
    };
    tokio::time::timeout(crate::STEP_TIMEOUT, read).await.ok().flatten().unwrap_or_default()
}

/// Where the personalized developer disk image is mounted (iOS 17 and later, the only ones probed).
const MOUNT_PATH: &str = "/System/Developer";

/// Whether the developer disk image is mounted, from the mounter's list of mounted images.
///
/// Not `LookupImage`: when Xcode 27 mounted the image (through cryptexd, from a MobileAsset) it
/// answers with no signature although the image is mounted (PITFALLS #31).
async fn disk_image_mounted(provider: &UsbmuxdProvider) -> Result<bool> {
    let mut mounter = step("image mounter connection", ImageMounter::connect(provider)).await?;
    let list = step("mounted image list", mounter.copy_devices()).await?;
    Ok(is_mounted(&list, MOUNT_PATH))
}

fn is_mounted(list: &[plist::Value], path: &str) -> bool {
    list.iter().filter_map(|e| e.as_dictionary()).any(|e| {
        e.get("MountPath").and_then(|v| v.as_string()) == Some(path)
            && e.get("IsMounted").and_then(|v| v.as_boolean()) != Some(false)
    })
}

/// The usbmuxd entry for `udid` (or the only device), USB first.
async fn pick(udid: Option<&str>) -> Result<(UsbmuxdDevice, Vec<Transport>)> {
    let all = mux_devices().await?;
    let mut matching: Vec<UsbmuxdDevice> = all
        .into_iter()
        .filter(|d| transport_of(&d.connection_type).is_some() && udid.is_none_or(|u| d.udid == u))
        .collect();
    matching.sort_by_key(|d| transport_of(&d.connection_type) != Some(Transport::Usb));
    let mut udids: Vec<&str> = matching.iter().map(|d| d.udid.as_str()).collect();
    udids.dedup();
    match (udid, udids.len()) {
        (Some(u), 0) => bail!("device {u} is not connected (no usbmuxd connection). Run `devices` to see what is"),
        (None, 0) => bail!("no device is connected (usbmuxd lists none). Connect one with a USB cable"),
        (None, n) if n > 1 => bail!("{n} devices are connected; pass --udid with one of: {}", udids.join(", ")),
        _ => {}
    }
    let transports: Vec<Transport> = matching.iter().filter_map(|d| transport_of(&d.connection_type)).collect();
    Ok((matching.swap_remove(0), transports))
}

/// The setup state of `udid` (or the only connected device).
pub async fn status(udid: Option<&str>) -> Result<SetupStatus> {
    let (device, transports) = pick(udid).await?;
    let provider = device.to_provider(mux_addr(), "devicesteer");
    let p = probe(&provider).await?;
    Ok(describe(&device.udid, transports, &p))
}

fn describe(udid: &str, transports: Vec<Transport>, p: &Probe) -> SetupStatus {
    let usb = transports.contains(&Transport::Usb);
    let mut steps = Vec::new();
    let mut earlier_open = false;
    for s in SetupStep::ALL {
        let (done, detail) = match s {
            SetupStep::Trust => (p.untrusted.is_none(), p.untrusted.clone()),
            SetupStep::DeveloperMode => (p.developer_mode == Some(true), Some("Developer Mode is off".into())),
            SetupStep::DiskImage => (
                p.disk_image == Some(true),
                Some("the Developer Disk Image is not mounted (a restart can unmount it)".into()),
            ),
        };
        let state = if earlier_open || p.unsupported.is_some() {
            StepState::Blocked
        } else if done {
            StepState::Done
        } else {
            earlier_open = true;
            StepState::Needed
        };
        steps.push(StepStatus { step: s, state, detail: (state == StepState::Needed).then_some(detail).flatten() });
    }
    let step = p.step();
    let u = format!("--udid {udid}");
    let next = match step {
        None if p.unsupported.is_some() => {
            format!("{}. It can't be set up or used; tell the user", p.unsupported.as_deref().unwrap_or_default())
        }
        None => "the device is ready".to_string(),
        Some(SetupStep::Trust) if !usb => {
            "connect the device with a USB cable: it can only be paired over USB. Then run `devicesteer setup trust`".into()
        }
        Some(SetupStep::Trust) => format!(
            "run `devicesteer setup trust {u}` and ask the user to unlock the device, tap Trust and enter its passcode"
        ),
        Some(SetupStep::DeveloperMode) => format!(
            "run `devicesteer setup developer-mode {u}`, then ask the user to turn on Settings > Privacy & Security > \
             Developer Mode and tap Restart; after the restart they unlock the device, tap Turn On and enter the passcode"
        ),
        Some(SetupStep::DiskImage) => format!("run `devicesteer setup disk-image {u}` (no action on the device is needed)"),
    };
    SetupStatus {
        udid: udid.to_string(),
        transports,
        ready: step.is_none() && p.unsupported.is_none(),
        unsupported: p.unsupported.clone(),
        step,
        steps,
        next,
    }
}

// ---------- doing the steps ----------

/// Runs every step that isn't done, in order, waiting for the user up to `wait` at each step
/// that needs them. Returns the state afterwards (which says what is left on a timeout).
pub async fn run(udid: Option<&str>, wait: Duration, ddi: Option<&Path>, progress: ProgressFn) -> Result<SetupStatus> {
    let mut s = status(udid).await?.ensure_supported()?;
    let udid = s.udid.clone();
    if s.state(SetupStep::Trust) == StepState::Needed {
        s = trust(Some(&udid), wait, progress.clone()).await?;
    }
    if s.state(SetupStep::DeveloperMode) == StepState::Needed {
        s = developer_mode(Some(&udid), Some(wait), progress.clone()).await?;
    }
    if s.state(SetupStep::DiskImage) == StepState::Needed {
        s = disk_image(Some(&udid), ddi, progress).await?;
    }
    Ok(s)
}

/// Pairs with the device: it shows "Trust This Computer?", the user taps Trust and enters the
/// passcode, and the new pair record goes to usbmuxd (which keeps it where the OS expects:
/// `/var/db/lockdown` on macOS, `%ProgramData%\Apple\Lockdown` with Apple's usbmuxd on Windows).
///
/// Waits up to `wait` for the user. Errors if they tap Don't Trust.
pub async fn trust(udid: Option<&str>, wait: Duration, progress: ProgressFn) -> Result<SetupStatus> {
    let (device, transports) = pick(udid).await?;
    let provider = device.to_provider(mux_addr(), "devicesteer");
    let before = probe(&provider).await?;
    if let Some(r) = before.unsupported {
        bail!("{r}");
    }
    if before.untrusted.is_none() {
        return status(Some(&device.udid)).await;
    }
    if !transports.contains(&Transport::Usb) {
        bail!("pairing works over USB only: connect the device with a cable, then run `devicesteer setup trust` again");
    }
    let s = SetupStep::Trust;
    let mut mux = step("usbmuxd connection", mux_addr().connect(0)).await?;
    let buid = step("usbmuxd system BUID", mux.get_buid()).await?;
    let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let deadline = Instant::now() + wait;

    report(&progress, s, true, "Unlock the device, then tap Trust and enter its passcode");
    let mut record = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!(
                "nobody tapped Trust on the device within {}s. Unlock it, then run `devicesteer setup trust` again \
                 and tap Trust when it asks",
                wait.as_secs()
            );
        }
        let mut lockdown = step("lockdown connection", LockdownClient::connect(&provider)).await?;
        // `pair` resends the request every second while the dialog is up (PITFALLS #30).
        match tokio::time::timeout(left, lockdown.pair(host_id.clone(), buid.clone(), None)).await {
            Err(_) => continue,
            Ok(Ok(r)) => break r,
            // Locked: the device shows the dialog only once it is unlocked.
            Ok(Err(IdeviceError::PasswordProtected)) => {
                report(&progress, s, true, "Unlock the device to see the Trust prompt");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(Err(IdeviceError::UserDeniedPairing)) => bail!(
                "the user tapped Don't Trust. Unplug the cable and plug it back in so the device asks again, then \
                 run `devicesteer setup trust` again"
            ),
            Ok(Err(e)) => bail!("pairing failed (connection error: {e:?})"),
        }
    };
    report(&progress, s, false, "Trusted. Saving the pair record");
    record.udid = Some(device.udid.clone());
    // Check the record before handing it to usbmuxd: a bad one would break every later command.
    let mut lockdown = step("lockdown connection", LockdownClient::connect(&provider)).await?;
    step("lockdown session with the new pair record", lockdown.start_session(&record)).await?;
    let bytes = record.serialize().map_err(|e| anyhow::anyhow!("could not encode the pair record: {e:?}"))?;
    step("saving the pair record with usbmuxd", mux.save_pair_record(&device.udid, bytes)).await?;
    status(Some(&device.udid)).await
}

/// Reveals the Developer Mode switch in Settings and, with `wait`, waits for the user to turn it
/// on (the device restarts in between).
pub async fn developer_mode(udid: Option<&str>, wait: Option<Duration>, progress: ProgressFn) -> Result<SetupStatus> {
    let s = SetupStep::DeveloperMode;
    let before = status(udid).await?.ensure_supported()?;
    match before.state(s) {
        StepState::Done => return Ok(before),
        StepState::Blocked => bail!("the device doesn't trust this computer yet: run `devicesteer setup trust` first"),
        StepState::Needed => {}
    }
    let udid = before.udid.clone();
    let (device, _) = pick(Some(&udid)).await?;
    let provider = device.to_provider(mux_addr(), "devicesteer");
    let mut amfi = step("AMFI connection", AmfiClient::connect(&provider)).await?;
    step("revealing the Developer Mode switch", amfi.reveal_developer_mode_option_in_ui()).await?;
    report(
        &progress,
        s,
        true,
        "On the device: Settings > Privacy & Security > Developer Mode, turn it on and tap Restart. After the \
         restart, unlock it, tap Turn On and enter the passcode",
    );
    let Some(wait) = wait else { return Ok(before) };
    let deadline = Instant::now() + wait;
    let mut gone = false;
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        // While it restarts the device leaves usbmuxd, and right after it lockdown can fail.
        match status(Some(&udid)).await {
            Ok(now) if now.state(s) == StepState::Done => return Ok(now),
            Ok(_) if gone => {
                report(&progress, s, true, "Unlock the device, tap Turn On and enter the passcode");
            }
            Ok(_) => {}
            Err(_) if !gone => {
                gone = true;
                report(&progress, s, true, "The device is restarting");
            }
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return status(Some(&udid)).await;
        }
    }
}

/// Mounts the personalized Developer Disk Image, finding or downloading it first (see
/// [`find_disk_image`]). `source` names a folder with `BuildManifest.plist` to use instead.
pub async fn disk_image(udid: Option<&str>, source: Option<&Path>, progress: ProgressFn) -> Result<SetupStatus> {
    let s = SetupStep::DiskImage;
    let before = status(udid).await?.ensure_supported()?;
    match before.state(s) {
        StepState::Done => return Ok(before),
        StepState::Blocked if before.step == Some(SetupStep::Trust) => {
            bail!("the device doesn't trust this computer yet: run `devicesteer setup trust` first")
        }
        StepState::Blocked => bail!("Developer Mode is off: run `devicesteer setup developer-mode` first"),
        StepState::Needed => {}
    }
    let udid = before.udid.clone();
    let (device, _) = pick(Some(&udid)).await?;
    let provider = device.to_provider(mux_addr(), "devicesteer");

    let mut lockdown = step("lockdown connection", LockdownClient::connect(&provider)).await?;
    let pairing = step("pair record lookup", provider.get_pairing_file()).await?;
    step("lockdown session", lockdown.start_session(&pairing)).await?;
    let ecid = step("lockdown read", lockdown.get_value(Some("UniqueChipID"), None))
        .await?
        .as_unsigned_integer()
        .context("the device did not report its UniqueChipID")?;

    let mut mounter = step("image mounter connection", ImageMounter::connect(&provider)).await?;
    let ids = step("personalization identifiers", mounter.query_personalization_identifiers(None)).await?;
    let id = |k: &str| ids.get(k).and_then(|v| v.as_unsigned_integer());
    let (board, chip) = (id("BoardId").context("no BoardId")?, id("ChipID").context("no ChipID")?);

    let image = find_disk_image(source, board, chip, &progress).await?;
    report(&progress, s, false, format!("Using the developer disk image from {}", image.origin));
    let manifest = std::fs::read(&image.manifest)?;
    let dmg = std::fs::read(&image.image).with_context(|| format!("could not read {}", image.image.display()))?;
    let trust_cache = std::fs::read(&image.trust_cache)?;

    report(&progress, s, false, "Asking Apple to sign the disk image for this device");
    let p = progress.clone();
    mounter
        .mount_personalized_with_callback(
            &provider,
            dmg,
            trust_cache,
            &manifest,
            None,
            ecid,
            move |((sent, total), ())| {
                let p = p.clone();
                async move {
                    p(Progress {
                        step: SetupStep::DiskImage,
                        waiting_for_user: false,
                        message: "Uploading the disk image".into(),
                        fraction: Some(if total == 0 { 1.0 } else { sent as f64 / total as f64 }),
                    });
                }
            },
            (),
        )
        .await
        .map_err(|e| anyhow::anyhow!("mounting the developer disk image failed: {e:?}"))?;
    report(&progress, s, false, "Mounted");
    // Close the mount's connections before checking: with the mounter connection still open, the
    // check's own mounter connection stalled on Windows although the image was mounted
    // (PITFALLS #52). And the mount did succeed, so a failing check is retried once, not trusted.
    drop(mounter);
    drop(lockdown);
    match status(Some(&udid)).await {
        Ok(st) => Ok(st),
        Err(first) => {
            tokio::time::sleep(Duration::from_secs(1)).await;
            status(Some(&udid)).await.with_context(|| {
                format!("the disk image was mounted, but checking the device afterwards failed ({first:#}); run `devicesteer setup status`")
            })
        }
    }
}

// ---------- finding the disk image ----------

/// A personalized DDI on disk: the build manifest and the two files its identity names.
#[derive(Debug, Clone)]
pub struct DiskImageFiles {
    pub manifest: PathBuf,
    pub image: PathBuf,
    pub trust_cache: PathBuf,
    /// Where it came from, for messages.
    pub origin: String,
}

/// Where Xcode (and the CoreDevice support it installs) keeps the iOS DDI on macOS.
const SYSTEM_DDI: &str = "/Library/Developer/DeveloperDiskImages/iOS_DDI/Restore";

/// The download: Apple's personalized iOS DDI as mirrored by the DeveloperDiskImage project
/// (pymobiledevice3 uses the same files). Apple publishes these only inside Xcode. Pinned to a
/// commit and checked by SHA-256; the device also verifies Apple's signature over the image, so a
/// tampered file can't be mounted. See docs/SETUP.md.
const DDI_REPO_COMMIT: &str = "a9ae4c1c47105d195d860badf04c0e2a835b99d8";
const DDI_REPO_DIR: &str = "PersonalizedImages/Xcode_iOS_DDI_Personalized";
const DDI_FILES: [(&str, &str); 3] = [
    ("BuildManifest.plist", "3c4eee2be4a0a853044dc32dbbd62699302b1dfd317aaf4f1cf36368b7639bb0"),
    ("Image.dmg", "0de55b0ccde9d5790c9c9f4e1a413225443fce7ea75acb503aca11e7d8156646"),
    ("Image.dmg.trustcache", "f0fb56da2829f69b8aae113584349635738e68d77c82a726ed72040890f16bf4"),
];

/// The disk image for a device (`board`, `chip` from its personalization identifiers), from the
/// first of: `source`; `DEVICESTEER_DDI`; Xcode's copy on macOS; the download cache; a download.
/// A folder whose manifest doesn't list the device is skipped, so an old Xcode doesn't block a
/// newer phone.
pub async fn find_disk_image(source: Option<&Path>, board: u64, chip: u64, progress: &ProgressFn) -> Result<DiskImageFiles> {
    if let Some(dir) = source.map(Path::to_path_buf).or_else(|| std::env::var_os("DEVICESTEER_DDI").map(PathBuf::from)) {
        return disk_image_in(&dir, board, chip, &dir.display().to_string())
            .with_context(|| format!("{} holds no developer disk image for this device", dir.display()));
    }
    if cfg!(target_os = "macos")
        && let Ok(f) = disk_image_in(Path::new(SYSTEM_DDI), board, chip, "Xcode's developer disk images")
    {
        return Ok(f);
    }
    let cache = crate::paths::state_dir().join("ddi").join(&DDI_REPO_COMMIT[..12]);
    if !DDI_FILES.iter().all(|(name, sha)| sha256_file(&cache.join(name)).is_some_and(|h| h == *sha)) {
        download_disk_image(&cache, progress).await?;
    }
    disk_image_in(&cache, board, chip, "the downloaded developer disk image")
        .context("the downloaded developer disk image does not support this device; install Xcode once (it puts a newer one in /Library/Developer/DeveloperDiskImages) or pass --ddi")
}

fn sha256_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex(&Sha256::digest(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn download_disk_image(dir: &Path, progress: &ProgressFn) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for (i, (name, sha)) in DDI_FILES.iter().enumerate() {
        progress(Progress {
            step: SetupStep::DiskImage,
            waiting_for_user: false,
            message: format!("Downloading the developer disk image ({name})"),
            fraction: Some(i as f64 / DDI_FILES.len() as f64),
        });
        let url = format!("https://raw.githubusercontent.com/doronz88/DeveloperDiskImage/{DDI_REPO_COMMIT}/{DDI_REPO_DIR}/{name}");
        let res = tokio::time::timeout(Duration::from_secs(300), idevice::http::get(&url))
            .await
            .map_err(|_| anyhow::anyhow!("downloading {name} timed out; check the network, or pass --ddi"))?
            .map_err(|e| anyhow::anyhow!("downloading {name} failed ({e:?}); check the network, or pass --ddi"))?;
        if res.status != 200 {
            bail!("downloading {url} failed with HTTP {}; pass --ddi with a folder that holds the disk image", res.status);
        }
        let got = hex(&Sha256::digest(&res.body));
        if got != *sha {
            bail!("{name} from {url} has SHA-256 {got}, expected {sha}; not using it");
        }
        let tmp = dir.join(format!("{name}.part"));
        std::fs::write(&tmp, &res.body)?;
        std::fs::rename(&tmp, dir.join(name))?;
    }
    Ok(())
}

/// The DDI files in `dir` for the device, as its build manifest names them.
fn disk_image_in(dir: &Path, board: u64, chip: u64, origin: &str) -> Result<DiskImageFiles> {
    let manifest_path = dir.join("BuildManifest.plist");
    let manifest: plist::Dictionary =
        plist::from_file(&manifest_path).with_context(|| format!("no readable {}", manifest_path.display()))?;
    let identity = idevice::tss::select_build_identity(&manifest, board, chip, None)
        .map_err(|_| anyhow::anyhow!("{} lists no build identity for this device", manifest_path.display()))?;
    let path_of = |key: &str| {
        identity
            .get("Manifest")?
            .as_dictionary()?
            .get(key)?
            .as_dictionary()?
            .get("Info")?
            .as_dictionary()?
            .get("Path")?
            .as_string()
            .map(str::to_owned)
    };
    // Xcode's copy keeps the names the manifest uses; the mirror renames them to Image.dmg*.
    let pick = |key: &str, fallback: &str| -> Result<PathBuf> {
        let named = path_of(key).map(|p| dir.join(p));
        match named {
            Some(p) if p.is_file() => Ok(p),
            _ if dir.join(fallback).is_file() => Ok(dir.join(fallback)),
            _ => bail!("{} has no {key} file", dir.display()),
        }
    };
    Ok(DiskImageFiles {
        image: pick("PersonalizedDMG", "Image.dmg")?,
        trust_cache: pick("LoadableTrustCache", "Image.dmg.trustcache")?,
        manifest: manifest_path,
        origin: origin.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(untrusted: bool, dev: Option<bool>, ddi: Option<bool>) -> Probe {
        Probe {
            untrusted: untrusted.then(|| "no pair record".to_string()),
            developer_mode: dev,
            disk_image: ddi,
            ..Default::default()
        }
    }

    #[test]
    fn first_open_step_is_needed_and_later_ones_blocked() {
        let s = describe("U", vec![Transport::Usb], &probe(true, None, None));
        assert_eq!(s.step, Some(SetupStep::Trust));
        assert_eq!(s.steps.iter().map(|s| s.state).collect::<Vec<_>>(), [StepState::Needed, StepState::Blocked, StepState::Blocked]);
        assert!(s.next.contains("setup trust --udid U"));

        let s = describe("U", vec![Transport::Usb], &probe(false, Some(false), None));
        assert_eq!(s.step, Some(SetupStep::DeveloperMode));
        assert_eq!(s.steps[0].state, StepState::Done);
        assert_eq!(s.steps[2].state, StepState::Blocked);

        let s = describe("U", vec![Transport::Usb], &probe(false, Some(true), Some(false)));
        assert_eq!(s.step, Some(SetupStep::DiskImage));
        assert!(!s.ready);

        let s = describe("U", vec![Transport::Wifi], &probe(false, Some(true), Some(true)));
        assert!(s.ready);
        assert!(s.steps.iter().all(|s| s.state == StepState::Done && s.detail.is_none()));
    }

    #[test]
    fn unsupported_device_has_no_step_and_is_not_ready() {
        let p = Probe {
            unsupported: Some("DeviceSteer needs iOS 17 or later; this device runs iOS 15.8.8".into()),
            developer_mode: None,
            disk_image: None,
            ..Default::default()
        };
        assert_eq!(p.step(), None);
        let s = describe("U", vec![Transport::Usb], &p);
        assert!(!s.ready);
        assert_eq!(s.step, None);
        assert!(s.steps.iter().all(|s| s.state == StepState::Blocked));
        assert!(s.next.starts_with("DeviceSteer needs iOS 17"));
        assert!(s.ensure_supported().is_err());
        // Even untrusted: trusting it would not help.
        let p = Probe { untrusted: Some("no pair record".into()), ..p };
        assert_eq!(p.step(), None);
    }

    #[test]
    fn pairing_over_wifi_asks_for_a_cable() {
        let s = describe("U", vec![Transport::Wifi], &probe(true, None, None));
        assert!(s.next.contains("USB cable"));
    }

    #[test]
    fn mounted_image_is_found_by_its_mount_path() {
        let entry = |path: &str, mounted: bool| {
            plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("MountPath".to_string(), plist::Value::String(path.into())),
                ("IsMounted".to_string(), plist::Value::Boolean(mounted)),
            ]))
        };
        assert!(is_mounted(&[entry("/System/Developer", true)], "/System/Developer"));
        assert!(!is_mounted(&[entry("/System/Developer", false)], "/System/Developer"));
        assert!(!is_mounted(&[entry("/private/var/other", true)], "/System/Developer"));
        assert!(!is_mounted(&[], "/System/Developer"));
    }

    #[test]
    fn step_names_parse() {
        for s in SetupStep::ALL {
            assert_eq!(SetupStep::parse(s.command()).unwrap(), s);
        }
        assert_eq!(SetupStep::parse("ddi").unwrap(), SetupStep::DiskImage);
        assert!(SetupStep::parse("reboot").is_err());
    }

    #[test]
    fn finds_files_by_manifest_path_or_mirror_name() {
        let dir = std::env::temp_dir().join(format!("devicesteer-ddi-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Firmware")).unwrap();
        let identity = |board: &str, dmg: &str| {
            plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("ApBoardID".to_string(), plist::Value::String(board.into())),
                ("ApChipID".to_string(), plist::Value::String("0x8101".into())),
                (
                    "Manifest".to_string(),
                    plist::Value::Dictionary(plist::Dictionary::from_iter([
                        ("PersonalizedDMG".to_string(), info(dmg)),
                        ("LoadableTrustCache".to_string(), info(&format!("Firmware/{dmg}.trustcache"))),
                    ])),
                ),
            ]))
        };
        fn info(path: &str) -> plist::Value {
            plist::Value::Dictionary(plist::Dictionary::from_iter([(
                "Info".to_string(),
                plist::Value::Dictionary(plist::Dictionary::from_iter([("Path".to_string(), plist::Value::String(path.into()))])),
            )]))
        }
        let manifest = plist::Dictionary::from_iter([(
            "BuildIdentities".to_string(),
            plist::Value::Array(vec![identity("0x0C", "a.dmg"), identity("0x08", "b.dmg")]),
        )]);
        plist::to_file_xml(dir.join("BuildManifest.plist"), &manifest).unwrap();
        // The named files are missing: the mirror's names stand in.
        std::fs::write(dir.join("Image.dmg"), b"x").unwrap();
        std::fs::write(dir.join("Image.dmg.trustcache"), b"x").unwrap();
        let f = disk_image_in(&dir, 0x08, 0x8101, "test").unwrap();
        assert!(f.image.ends_with("Image.dmg"));
        // Named files win.
        std::fs::write(dir.join("b.dmg"), b"x").unwrap();
        std::fs::write(dir.join("Firmware/b.dmg.trustcache"), b"x").unwrap();
        let f = disk_image_in(&dir, 0x08, 0x8101, "test").unwrap();
        assert!(f.image.ends_with("b.dmg"));
        assert!(f.trust_cache.ends_with("Firmware/b.dmg.trustcache"));
        assert!(disk_image_in(&dir, 0x0E, 0x8101, "test").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
