//! Device discovery through usbmuxd.

use anyhow::Result;
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice};
use serde::Serialize;

use crate::{setup::SetupStep, step};

/// How a device is reachable through usbmuxd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Usb,
    Wifi,
}

/// One device, merged across transports (a phone on USB with Wi-Fi sync on is listed twice by
/// usbmuxd).
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub udid: String,
    /// Every transport usbmuxd currently lists for this device, USB first.
    pub transports: Vec<Transport>,
    /// The user-visible device name. Personal data: don't log or commit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Hardware model identifier, such as `iPhone17,1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    /// What Apple calls the model, such as `iPhone 16 Pro Max`, when `product_type` is one the
    /// table in `models.rs` knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_class: Option<String>,
    /// iOS / iPadOS version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_version: Option<String>,
    /// The first setup step the device still needs (`devicesteer setup status` says more). Absent
    /// when it is ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupStep>,
    /// Why the device could not be read (connection dropped, lockdown failed). A device that is
    /// only waiting for setup has `setup` instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(crate) fn transport_of(c: &Connection) -> Option<Transport> {
    match c {
        Connection::Usb => Some(Transport::Usb),
        Connection::Network(_) => Some(Transport::Wifi),
        Connection::Unknown(_) => None,
    }
}

pub(crate) fn mux_addr() -> UsbmuxdAddr {
    // `USBMUXD_SOCKET_ADDRESS` lets a remote or non-default usbmuxd be used; fall back to the
    // platform default (Unix socket on macOS/Linux, TCP 27015 on Windows).
    UsbmuxdAddr::from_env_var().unwrap_or_default()
}

/// Raw usbmuxd entries, one per (device, transport).
pub(crate) async fn mux_devices() -> Result<Vec<UsbmuxdDevice>> {
    let mut mux = step("usbmuxd connection", mux_addr().connect(0)).await?;
    step("usbmuxd device list", mux.get_devices()).await
}

/// Every device usbmuxd lists, with what lockdown says about it.
pub async fn list_devices() -> Result<Vec<DeviceInfo>> {
    let raw = mux_devices().await?;
    let mut out: Vec<(DeviceInfo, UsbmuxdDevice)> = Vec::new();
    for d in raw {
        let Some(t) = transport_of(&d.connection_type) else { continue };
        match out.iter_mut().find(|(i, _)| i.udid == d.udid) {
            Some((info, best)) => {
                info.transports.push(t);
                info.transports.sort_by_key(|t| *t != Transport::Usb);
                if t == Transport::Usb {
                    *best = d;
                }
            }
            None => out.push((
                DeviceInfo {
                    udid: d.udid.clone(),
                    transports: vec![t],
                    name: None,
                    product_type: None,
                    model_name: None,
                    device_class: None,
                    os_version: None,
                    build_version: None,
                    setup: None,
                    error: None,
                },
                d,
            )),
        }
    }

    let mut list = Vec::with_capacity(out.len());
    for (mut info, dev) in out {
        let provider = dev.to_provider(mux_addr(), "devicesteer");
        match crate::setup::probe(&provider).await {
            Ok(p) => {
                info.setup = p.step();
                let values = &p.values;
                let s = |k: &str| values.get(k).and_then(|v| v.as_string()).map(str::to_owned);
                info.name = s("DeviceName");
                info.product_type = s("ProductType");
                info.model_name = info.product_type.as_deref().and_then(crate::models::marketing_name).map(str::to_owned);
                info.device_class = s("DeviceClass");
                info.os_version = s("ProductVersion");
                info.build_version = s("BuildVersion");
            }
            Err(e) => info.error = Some(e.to_string()),
        }
        list.push(info);
    }
    Ok(list)
}
