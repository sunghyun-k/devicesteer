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
    /// Why DeviceSteer can't work with this device (its OS is older than iOS 17), as a sentence to
    /// show. Such a device has no `setup`: no step would make it usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<String>,
    /// Why the device could not be read (connection dropped, lockdown failed). A device that is
    /// only waiting for setup has `setup` instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The oldest major OS version DeviceSteer works with. Everything goes through CoreDeviceProxy,
/// RSD and RemoteXPC, which the device only has from iOS 17.
pub const MIN_OS_MAJOR: u32 = 17;

/// The major version in a lockdown `ProductVersion` such as `15.8.8`.
fn os_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// Why a device running `version` can't be used, or `None` when it can (or the version doesn't
/// parse: the later steps then say what is wrong). `device_class` is lockdown's `DeviceClass`, to
/// name the OS as the owner knows it.
///
/// The wording must not say "connection" or "tunnel": the daemon would take it for a dropped
/// connection and keep retrying (lib.rs `step`).
pub fn unsupported_reason(version: &str, device_class: Option<&str>) -> Option<String> {
    let major = os_major(version)?;
    if major >= MIN_OS_MAJOR {
        return None;
    }
    let os = if device_class == Some("iPad") { "iPadOS" } else { "iOS" };
    Some(format!("DeviceSteer needs iOS {MIN_OS_MAJOR} or later; this device runs {os} {}", version.trim()))
}

/// [`unsupported_reason`] for a device's lockdown values.
pub(crate) fn unsupported_in(values: &plist::Dictionary) -> Option<String> {
    let s = |k: &str| values.get(k).and_then(|v| v.as_string());
    unsupported_reason(s("ProductVersion")?, s("DeviceClass"))
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

/// Every device usbmuxd lists, with what lockdown says about it, and every device paired over
/// Wi-Fi that answers on the network (PITFALLS #50).
pub async fn list_devices() -> Result<Vec<DeviceInfo>> {
    let stored = crate::remote::pairings();
    // The network search runs while usbmuxd and lockdown are asked, and only when there is a
    // pairing it could find.
    let adverts = async {
        if stored.is_empty() {
            Vec::new()
        } else {
            crate::remote::browse(crate::remote::BROWSE_WAIT).await.unwrap_or_default()
        }
    };
    let (mut list, adverts) = tokio::join!(list_mux_devices(), adverts);
    let (matched, _) = crate::remote::match_adverts(adverts, &stored);
    // No usbmuxd at all (Windows without Apple's device support): Wi-Fi pairings still count.
    if list.is_err() && !matched.is_empty() {
        list = Ok(Vec::new());
    }
    let mut list = list?;
    for (_, p) in matched {
        match list.iter_mut().find(|i| i.udid == p.udid) {
            Some(info) => {
                if !info.transports.contains(&Transport::Wifi) {
                    info.transports.push(Transport::Wifi);
                }
            }
            None => list.push(remote_info(p)),
        }
    }
    Ok(list)
}

/// A device reached only through our Wi-Fi pairing, described from what it said when we last
/// connected (its tunnel isn't opened just to list it).
fn remote_info(p: crate::remote::Pairing) -> DeviceInfo {
    let a = p.about;
    DeviceInfo {
        udid: p.udid,
        transports: vec![Transport::Wifi],
        name: a.name,
        model_name: a.product_type.as_deref().and_then(crate::models::marketing_name).map(str::to_owned),
        product_type: a.product_type,
        device_class: a.device_class,
        unsupported: a.os_version.as_deref().and_then(|v| unsupported_reason(v, None)),
        os_version: a.os_version,
        build_version: a.build_version,
        setup: None,
        error: None,
    }
}

async fn list_mux_devices() -> Result<Vec<DeviceInfo>> {
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
                    unsupported: None,
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
                info.unsupported = p.unsupported.clone();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_below_17_are_unsupported() {
        assert_eq!(os_major("15.8.8"), Some(15));
        assert_eq!(os_major("17"), Some(17));
        assert_eq!(os_major("27.3"), Some(27));
        assert_eq!(os_major(""), None);
        assert_eq!(os_major("beta"), None);

        assert_eq!(
            unsupported_reason("15.8.8", Some("iPhone")).as_deref(),
            Some("DeviceSteer needs iOS 17 or later; this device runs iOS 15.8.8")
        );
        assert_eq!(
            unsupported_reason("16.7.10", Some("iPad")).as_deref(),
            Some("DeviceSteer needs iOS 17 or later; this device runs iPadOS 16.7.10")
        );
        assert!(unsupported_reason("17.0", Some("iPhone")).is_none());
        assert!(unsupported_reason("26.1", None).is_none());
        // Unreadable: not refused here.
        assert!(unsupported_reason("x.y", None).is_none());
    }

    #[test]
    fn reason_does_not_read_as_a_connection_failure() {
        let r = unsupported_reason("12.5.7", None).unwrap();
        assert!(!r.contains("connection") && !r.contains("tunnel"));
    }

    #[test]
    fn reason_comes_from_lockdown_values() {
        let values = plist::Dictionary::from_iter([
            ("ProductVersion".to_string(), plist::Value::String("15.8.8".into())),
            ("DeviceClass".to_string(), plist::Value::String("iPhone".into())),
        ]);
        assert!(unsupported_in(&values).unwrap().ends_with("iOS 15.8.8"));
        assert!(unsupported_in(&plist::Dictionary::new()).is_none());
    }
}
