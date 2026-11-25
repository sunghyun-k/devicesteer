//! One open tunnel to a device, and the RSD service list read over it.

use anyhow::{Context, Result, bail};
use idevice::{
    IdeviceService, RsdService,
    core_device::{AppServiceClient, GENERAL_PASTEBOARD, PasteboardServiceClient},
    core_device_proxy::CoreDeviceProxy,
    installation_proxy::InstallationProxyClient,
    heartbeat::HeartbeatClient,
    provider::IdeviceProvider,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
    usbmuxd::UsbmuxdDevice,
};
use serde::Serialize;

use crate::{
    devices::{Transport, mux_addr, mux_devices, transport_of},
    display::{Display, DisplayPoller},
    step,
};

/// How long to wait for the first heartbeat "Marco", and the slack on later ones.
const HEARTBEAT_FIRST_SECS: u64 = 10;
const HEARTBEAT_GRACE_SECS: u64 = 5;

/// A service the device advertises over RSD.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub port: u16,
    pub uses_remote_xpc: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_version: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// An open userspace tunnel to one device.
///
/// Dropping it tears down the tunnel task and the heartbeat.
pub struct Session {
    pub udid: String,
    pub transport: Transport,
    adapter: AdapterHandle,
    rsd: RsdHandshake,
    _keep: Keep,
}

/// What keeps the tunnel open besides the adapter.
enum Keep {
    Nothing,
    /// usbmuxd's Wi-Fi: the device drops CoreDeviceProxy unless its heartbeat is answered.
    Heartbeat { _beat: Heartbeat },
    /// Our own Wi-Fi pairing: the pairing connection the tunnel was asked on (PITFALLS #55).
    Pairing { _tunnel: Box<crate::remote::RemoteTunnel> },
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("udid", &self.udid)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Opens a tunnel to `udid`, or to the only connected device when `udid` is `None`.
    ///
    /// USB is preferred when the device is reachable both ways; then usbmuxd's Wi-Fi; then our
    /// own Wi-Fi pairing for a device usbmuxd doesn't list (PITFALLS #50).
    pub async fn open(udid: Option<&str>) -> Result<Self> {
        match pick_target(udid).await? {
            Target::Mux(device) => {
                // Before the tunnel: an iOS 16 device fails somewhere inside it with an error
                // that reads like a dropped connection, which the daemon would keep retrying.
                let provider = device.to_provider(mux_addr(), "devicesteer");
                if let Some(r) = crate::devices::unsupported_in(&crate::setup::public_values(&provider).await) {
                    bail!("{r}, so it can't be used: tell the user");
                }
                let udid = device.udid.clone();
                match Self::open_tunnel(device).await {
                    Ok(s) => Ok(s),
                    // A device that isn't set up fails somewhere along the way with an error
                    // that doesn't say why; the setup state does. The original words stay,
                    // "connection" included.
                    Err(e) => match Box::pin(crate::setup::status(Some(&udid))).await {
                        Ok(s) if !s.ready => Err(e.context(format!("the device is not set up: {}", s.next))),
                        _ => Err(e),
                    },
                }
            }
            Target::Remote(udid) => Self::open_remote(&udid).await,
        }
    }

    async fn open_tunnel(device: UsbmuxdDevice) -> Result<Self> {
        let transport = transport_of(&device.connection_type).context("unknown usbmuxd connection type")?;
        let provider = device.to_provider(mux_addr(), "devicesteer");

        // Over Wi-Fi the device closes CoreDeviceProxy (and the tunnel with it) unless some client
        // answers its heartbeat. USB needs none.
        let keep = match transport {
            Transport::Wifi => Keep::Heartbeat { _beat: Heartbeat::start(&provider).await? },
            Transport::Usb => Keep::Nothing,
        };

        let proxy = step("CoreDeviceProxy connection", CoreDeviceProxy::connect(&provider)).await?;
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy
            .create_software_tunnel()
            .map_err(|e| anyhow::anyhow!("tunnel setup failed: {e:?}"))?;
        let mut adapter = adapter.to_async_handle();
        let stream = step("RSD connection over the tunnel", adapter.connect(rsd_port)).await?;
        let rsd = step("RSD handshake", RsdHandshake::new(stream)).await?;

        Ok(Self { udid: device.udid, transport, adapter, rsd, _keep: keep })
    }

    /// Over the network with our own pairing, for a device usbmuxd doesn't list.
    async fn open_remote(udid: &str) -> Result<Self> {
        let tunnel = crate::remote::open(udid).await?;
        let mut adapter = tunnel.adapter.clone();
        let stream = step("RSD connection over the tunnel", adapter.connect(tunnel.rsd_port)).await?;
        let rsd = step("RSD handshake", RsdHandshake::new(stream)).await?;
        let prop = |k: &str| rsd.properties.get(k).and_then(|v| v.as_string()).map(str::to_owned);
        // The pairing was matched by its advert's tag; this is the device's own word for it.
        if let Some(seen) = prop("UniqueDeviceID")
            && seen != udid
        {
            bail!("the device on the network is not {udid} (it says it is {seen}); run `devices` and retry");
        }
        let version = prop("ProductVersion").or_else(|| prop("OSVersion"));
        if let Some(r) = version.as_deref().and_then(|v| crate::devices::unsupported_reason(v, prop("DeviceClass").as_deref())) {
            bail!("{r}, so it can't be used: tell the user");
        }
        crate::remote::remember(
            udid,
            crate::remote::About {
                name: None,
                product_type: prop("ProductType"),
                device_class: prop("DeviceClass"),
                os_version: version,
                build_version: prop("BuildVersion"),
            },
        );
        Ok(Self { udid: udid.to_string(), transport: Transport::Wifi, adapter, rsd, _keep: Keep::Pairing { _tunnel: Box::new(tunnel) } })
    }

    /// The services RSD lists, sorted by name.
    pub fn services(&self) -> Vec<ServiceInfo> {
        let mut list: Vec<ServiceInfo> = self
            .rsd
            .services
            .iter()
            .map(|(name, s)| ServiceInfo {
                name: name.clone(),
                port: s.port,
                uses_remote_xpc: s.uses_remote_xpc,
                service_version: s.service_version,
                features: s.features.clone().unwrap_or_default(),
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// RSD properties of the device (OS version, model, ...), as the handshake reported them.
    pub fn properties(&self) -> &std::collections::HashMap<String, plist::Value> {
        &self.rsd.properties
    }

    /// Opens an RSD service over this tunnel.
    pub async fn connect<T: RsdService>(&mut self) -> Result<T> {
        let name = T::rsd_service_name();
        if !self.rsd.services.contains_key(name.as_ref()) {
            bail!(
                "the device does not advertise {name}. If it is a developer service, the Developer \
                 Disk Image is probably not mounted (a restart can unmount it): run \
                 `devicesteer setup`, then retry"
            );
        }
        step(&format!("{name} connection"), T::connect_rsd(&mut self.adapter, &mut self.rsd)).await
    }

    /// The tunnel, for services that open their own connections over it.
    pub fn adapter(&self) -> &AdapterHandle {
        &self.adapter
    }

    /// The RSD service list, for services that open their own connections over it.
    pub fn rsd(&self) -> &RsdHandshake {
        &self.rsd
    }

    /// The display in use: size in points, rotation, and the id its video streams by.
    pub async fn display(&mut self) -> Result<Display> {
        DisplayPoller::new(self.adapter.clone(), self.rsd.clone()).ask().await
    }

    /// Launches an app by bundle id (bringing it to the front if it runs). Returns its pid.
    pub async fn launch(&mut self, bundle_id: &str, relaunch: bool) -> Result<u64> {
        let mut client: AppServiceClient<_> = self.connect().await?;
        let r = step(
            "app launch",
            client.launch_application(bundle_id, &[], relaunch, false, None, None, None),
        )
        .await?;
        Ok(u64::from(r.pid))
    }

    /// Sends `SIGKILL` to a device process. Only for system daemons that launchd starts again on
    /// demand (PITFALLS #25); never for apps the owner is using.
    pub async fn kill(&mut self, pid: i64) -> Result<()> {
        let mut client: AppServiceClient<_> = self.connect().await?;
        let pid = u32::try_from(pid).map_err(|_| anyhow::anyhow!("no such process: {pid}"))?;
        step("process signal", client.send_signal(pid, 9)).await?;
        Ok(())
    }

    /// Running processes as (pid, executable path on the device).
    pub async fn processes(&mut self) -> Result<Vec<(i64, String)>> {
        let mut client: AppServiceClient<_> = self.connect().await?;
        let list = step("process list", client.list_processes()).await?;
        Ok(list
            .into_iter()
            .filter_map(|p| Some((i64::from(p.pid), p.executable_url?.relative)))
            .collect())
    }

    /// Every installed app, system ones included: (bundle path, display name, bundle id).
    ///
    /// Through installation_proxy's `Browse` with only these attributes: CoreDevice's
    /// `listapps` did not answer within 12 s on the test iPhone, whatever the options.
    pub async fn installed_apps(&mut self) -> Result<Vec<(String, String, String)>> {
        let mut client: InstallationProxyClient = self.connect().await?;
        let options = plist::Value::Dictionary(plist::Dictionary::from_iter([
            ("ApplicationType".to_string(), plist::Value::String("Any".into())),
            (
                "ReturnAttributes".to_string(),
                plist::Value::Array(
                    ["CFBundleIdentifier", "CFBundleDisplayName", "CFBundleName", "Path"]
                        .map(|a| plist::Value::String(a.into()))
                        .to_vec(),
                ),
            ),
        ]));
        let list = step("app list", client.browse(Some(options))).await?;
        Ok(list
            .into_iter()
            .filter_map(|v| {
                let d = v.into_dictionary()?;
                let get = |k: &str| d.get(k).and_then(|v| v.as_string()).map(str::to_string);
                let id = get("CFBundleIdentifier")?;
                let path = get("Path")?;
                let name = get("CFBundleDisplayName").or_else(|| get("CFBundleName")).unwrap_or_else(|| id.clone());
                Some((path, name, id))
            })
            .collect())
    }

    /// Captures the active display. 8-bit unless `raw` (PITFALLS #63).
    ///
    /// `display` is a panel's `uniqueId`. Without one, the panel in use now is captured: a
    /// foldable has two panels, and the device on its own captures its first (the cover panel),
    /// which is black while unfolded (PITFALLS #5, #47). A phone with one panel has no id to pass.
    pub async fn screenshot(&mut self, display: Option<&str>, raw: bool) -> Result<crate::Screenshot> {
        let active = match display {
            Some(_) => None,
            None => self.display().await?.capture_id().map(str::to_owned),
        };
        let display = display.or(active.as_deref());
        let png = crate::screenshot::capture(&mut self.adapter, &mut self.rsd, display).await?;
        crate::screenshot::finish(png, raw)
    }

    /// What the device's clipboard holds, as far as it can be put back: text or one image.
    pub async fn clipboard(&mut self) -> Result<Clipboard> {
        let mut client: PasteboardServiceClient<_> = self.connect().await?;
        let snap = step("clipboard read", client.get(GENERAL_PASTEBOARD)).await?;
        Ok(if let Some(t) = snap.text() {
            Clipboard::Text(t)
        } else if let Some((uti, bytes)) = snap.image() {
            Clipboard::Image { uti: uti.to_string(), bytes }
        } else if snap.items.is_empty() {
            Clipboard::Empty
        } else {
            Clipboard::Other
        })
    }

    /// Replaces the clipboard with `text`. A new connection per request: the service has
    /// answered one request per connection in the Swift implementation's use (old PITFALLS #14).
    pub async fn set_clipboard_text(&mut self, text: &str) -> Result<()> {
        let mut client: PasteboardServiceClient<_> = self.connect().await?;
        step("clipboard write", client.set_text(text, GENERAL_PASTEBOARD)).await
    }

    /// Puts back what [`Self::clipboard`] read. Empty and unknown contents become an empty text.
    pub async fn restore_clipboard(&mut self, saved: &Clipboard) -> Result<()> {
        match saved {
            Clipboard::Text(t) => self.set_clipboard_text(t).await,
            Clipboard::Image { uti, bytes } => {
                let mut client: PasteboardServiceClient<_> = self.connect().await?;
                step("clipboard write", client.set_image(bytes, uti, GENERAL_PASTEBOARD)).await
            }
            Clipboard::Empty | Clipboard::Other => self.set_clipboard_text("").await,
        }
    }
}

/// A clipboard's contents, kept to put back after a paste. Never printed or logged: it is the
/// owner's.
#[derive(Clone)]
pub enum Clipboard {
    Text(String),
    Image { uti: String, bytes: Vec<u8> },
    Empty,
    /// Something that is neither text nor an image (it can't be put back).
    Other,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Clipboard::Text(_) => "Clipboard::Text(<redacted>)",
            Clipboard::Image { .. } => "Clipboard::Image(<redacted>)",
            Clipboard::Empty => "Clipboard::Empty",
            Clipboard::Other => "Clipboard::Other",
        })
    }
}

impl Clipboard {
    /// What kind of contents it is, for output.
    pub fn kind(&self) -> &'static str {
        match self {
            Clipboard::Text(_) => "text",
            Clipboard::Image { .. } => "image",
            Clipboard::Empty => "empty",
            Clipboard::Other => "other",
        }
    }
}

/// Errors when the device `udid` names (or the only one) is too old for DeviceSteer, with a
/// message that doesn't read as a connection failure. Reads only what lockdown tells anyone, so it
/// works before trust; when even that can't be read it lets the caller go on and fail as it would.
/// A device on our own Wi-Fi pairing runs iOS 27 or later (it paired that way), so it passes.
pub async fn ensure_supported(udid: Option<&str>) -> Result<()> {
    let Target::Mux(device) = pick_target(udid).await? else { return Ok(()) };
    let provider = device.to_provider(mux_addr(), "devicesteer");
    match crate::devices::unsupported_in(&crate::setup::public_values(&provider).await) {
        Some(r) => bail!("{r}, so it can't be used: tell the user"),
        None => Ok(()),
    }
}

/// What to add when usbmuxd doesn't list a device that may well be plugged in: on Windows, Apple
/// Devices' usbmuxd can miss a device that was connected before it started (PITFALLS #53).
pub fn missing_device_hint() -> &'static str {
    if cfg!(windows) {
        ". If it is plugged in, unplug it and plug it back in: Apple Devices may not have seen it if it was connected before Apple Devices' service started"
    } else {
        ""
    }
}

/// The UDID `udid` names, or the only connected device's when it is `None`.
pub async fn resolve_udid(udid: Option<&str>) -> Result<String> {
    Ok(match pick_target(udid).await? {
        Target::Mux(d) => d.udid,
        Target::Remote(u) => u,
    })
}

/// How a device is reached.
pub(crate) enum Target {
    /// Through usbmuxd (USB, or Wi-Fi when usbmuxd lists it).
    Mux(UsbmuxdDevice),
    /// Through our own Wi-Fi pairing.
    Remote(String),
}

/// The device `udid` names, or the only one: usbmuxd first, then our Wi-Fi pairings.
///
/// Without `udid`, the network is searched only when we hold pairings, so a USB-only setup pays
/// nothing for it.
pub(crate) async fn pick_target(udid: Option<&str>) -> Result<Target> {
    // No usbmuxd (Windows without Apple's device support) still leaves Wi-Fi pairings usable.
    let (mux, mux_err) = match mux_devices().await {
        Ok(l) => (l, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    if let Some(u) = udid {
        if let Ok(d) = pick_device(mux, Some(u)) {
            return Ok(Target::Mux(d));
        }
        if crate::remote::pairing(u).is_some() {
            return Ok(Target::Remote(u.to_string()));
        }
        if let Some(e) = mux_err {
            return Err(e);
        }
        bail!("device {u} is not connected (no usbmuxd connection and no Wi-Fi pairing). Run `devices` to see what is");
    }

    let stored = crate::remote::pairings();
    let mut remote: Vec<String> = Vec::new();
    if !stored.is_empty() {
        let adverts = crate::remote::browse(crate::remote::BROWSE_WAIT).await.unwrap_or_default();
        let (matched, _) = crate::remote::match_adverts(adverts, &stored);
        remote = matched.into_iter().map(|(_, p)| p.udid).collect();
    }
    let usable: Vec<&UsbmuxdDevice> = mux.iter().filter(|d| transport_of(&d.connection_type).is_some()).collect();
    remote.retain(|u| !usable.iter().any(|d| &d.udid == u));
    let mut all: Vec<String> = usable.iter().map(|d| d.udid.clone()).chain(remote.iter().cloned()).collect();
    all.sort();
    all.dedup();
    match all.len() {
        0 => match mux_err {
            Some(e) => Err(e),
            None => bail!(
                "no device is connected (usbmuxd lists none, and no Wi-Fi-paired device answers). Connect one over \
                 USB, or pair one over Wi-Fi with `devicesteer setup wifi`"
            ),
        },
        1 if !remote.is_empty() => Ok(Target::Remote(all.swap_remove(0))),
        1 => Ok(Target::Mux(pick_device(mux, None)?)),
        n => bail!("{n} devices are connected; pass --udid with one of: {}", all.join(", ")),
    }
}

fn pick_device(all: Vec<UsbmuxdDevice>, udid: Option<&str>) -> Result<UsbmuxdDevice> {
    let usable = |d: &UsbmuxdDevice| transport_of(&d.connection_type).is_some();
    let mut candidates: Vec<UsbmuxdDevice> = match udid {
        Some(u) => all.into_iter().filter(|d| d.udid == u && usable(d)).collect(),
        None => all.into_iter().filter(usable).collect(),
    };
    // USB first: lower latency, and no heartbeat to keep alive.
    candidates.sort_by_key(|d| transport_of(&d.connection_type) != Some(Transport::Usb));

    let mut udids: Vec<&str> = candidates.iter().map(|d| d.udid.as_str()).collect();
    udids.dedup();
    match (udid, udids.len()) {
        (_, 0) => match udid {
            Some(u) => bail!("device {u} is not connected (no usbmuxd connection). Run `devices` to see what is{}", missing_device_hint()),
            None => bail!("no device is connected (usbmuxd lists none). Connect one over USB{}", missing_device_hint()),
        },
        (None, n) if n > 1 => bail!(
            "{n} devices are connected; pass --udid with one of: {}",
            udids.join(", ")
        ),
        _ => Ok(candidates.swap_remove(0)),
    }
}

/// Answers the device's heartbeat in the background for as long as it lives.
struct Heartbeat(tokio::task::JoinHandle<()>);

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Heartbeat {
    /// Answers the first "Marco" before returning, so the device sees a live client before any
    /// service is asked for.
    async fn start(provider: &dyn IdeviceProvider) -> Result<Self> {
        let mut client = step("heartbeat connection", HeartbeatClient::connect(provider)).await?;
        let mut interval = step("heartbeat", client.get_marco(HEARTBEAT_FIRST_SECS)).await?;
        step("heartbeat reply", client.send_polo()).await?;
        Ok(Self(tokio::spawn(async move {
            loop {
                let next = client.get_marco(interval.saturating_add(HEARTBEAT_GRACE_SECS)).await;
                match next {
                    Ok(i) => interval = i,
                    Err(e) => {
                        tracing::debug!("heartbeat ended: {e:?}");
                        break;
                    }
                }
                if client.send_polo().await.is_err() {
                    break;
                }
            }
        })))
    }
}
