//! One open tunnel to a device, and the RSD service list read over it.

use anyhow::{Context, Result, bail};
use idevice::{
    IdeviceService, RsdService,
    core_device::{ImageFormat, ScreenCaptureServiceClient},
    core_device_proxy::CoreDeviceProxy,
    heartbeat::HeartbeatClient,
    provider::IdeviceProvider,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
    usbmuxd::UsbmuxdDevice,
};
use serde::Serialize;

use crate::{
    devices::{Transport, mux_addr, mux_devices, transport_of},
    display::{DeviceInfoLink, Display},
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
    _heartbeat: Option<Heartbeat>,
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
    /// USB is preferred when the device is reachable both ways.
    pub async fn open(udid: Option<&str>) -> Result<Self> {
        let device = pick_device(mux_devices().await?, udid)?;
        let transport = transport_of(&device.connection_type).context("unknown usbmuxd connection type")?;
        let provider = device.to_provider(mux_addr(), "iphone-use");

        // Over Wi-Fi the device closes CoreDeviceProxy (and the tunnel with it) unless some client
        // answers its heartbeat. USB needs none.
        let heartbeat = match transport {
            Transport::Wifi => Some(Heartbeat::start(&provider).await?),
            Transport::Usb => None,
        };

        let proxy = step("CoreDeviceProxy connection", CoreDeviceProxy::connect(&provider)).await?;
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy
            .create_software_tunnel()
            .map_err(|e| anyhow::anyhow!("tunnel setup failed: {e:?}"))?;
        let mut adapter = adapter.to_async_handle();
        let stream = step("RSD connection over the tunnel", adapter.connect(rsd_port)).await?;
        let rsd = step("RSD handshake", RsdHandshake::new(stream)).await?;

        Ok(Self { udid: device.udid, transport, adapter, rsd, _heartbeat: heartbeat })
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
                 Disk Image is probably not mounted: open Xcode's Devices window once with the \
                 device connected, then retry"
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
        let mut link: DeviceInfoLink = self.connect().await?;
        step("display info", link.active_display()).await
    }

    /// Captures the active display as PNG.
    ///
    /// `display` is a panel's `uniqueId` (foldables have two panels; without it the device
    /// captures its first panel, which is black while unfolded).
    pub async fn screenshot(&mut self, display: Option<&str>) -> Result<Vec<u8>> {
        let mut client: ScreenCaptureServiceClient<_> = self.connect().await?;
        step("screenshot", client.take_screenshot(display, ImageFormat::Png)).await
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
            Some(u) => bail!("device {u} is not connected (no usbmuxd connection). Run `devices` to see what is"),
            None => bail!("no device is connected (usbmuxd lists none). Connect one over USB"),
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
