//! Raw CoreDevice invocations over RemoteXPC.
//!
//! idevice's per-service clients cover most requests, but some need a shape they don't build
//! (stopping streams by name instead of `stopAll`, PITFALLS #1). These helpers build a request the
//! way idevice's `CoreDeviceServiceClient` does, so one XPC connection can carry any of them.
//!
//! Adapted from iphone-cli `src/device/media.rs` (`request`, `output`), MIT, see
//! THIRD_PARTY_NOTICES.md.

use anyhow::{Result, anyhow};
use idevice::xpc::{Dictionary, XPCObject};

/// The CoreDevice version the requests claim, as idevice sends it.
const CORE_DEVICE_VERSION: [u64; 2] = [629, 3];

/// A CoreDevice invocation of `feature` (with `action` for features that take one).
pub(crate) fn request(feature: &str, input: Dictionary, action: Option<&str>) -> Dictionary {
    let mut version = Dictionary::new();
    version.insert(
        "originalComponentsCount".into(),
        XPCObject::Int64(CORE_DEVICE_VERSION.len() as i64),
    );
    version.insert(
        "components".into(),
        XPCObject::Array(CORE_DEVICE_VERSION.map(XPCObject::UInt64).to_vec()),
    );
    version.insert(
        "stringValue".into(),
        XPCObject::String(CORE_DEVICE_VERSION.map(|c| c.to_string()).join(".")),
    );
    let mut req = Dictionary::new();
    req.insert(
        "CoreDevice.CoreDeviceDDIProtocolVersion".into(),
        XPCObject::Int64(if action.is_some() { 2 } else { 0 }),
    );
    req.insert("CoreDevice.action".into(), XPCObject::Dictionary(Dictionary::new()));
    req.insert("CoreDevice.coreDeviceVersion".into(), XPCObject::Dictionary(version));
    req.insert(
        "CoreDevice.deviceIdentifier".into(),
        XPCObject::String(uuid::Uuid::new_v4().to_string()),
    );
    req.insert("CoreDevice.featureIdentifier".into(), XPCObject::String(feature.into()));
    req.insert("CoreDevice.input".into(), XPCObject::Dictionary(input));
    req.insert(
        "CoreDevice.invocationIdentifier".into(),
        XPCObject::String(uuid::Uuid::new_v4().to_string()),
    );
    if let Some(action) = action {
        req.insert("CoreDevice.actionIdentifier".into(), XPCObject::String(action.into()));
    }
    req
}

/// An error the device answered a CoreDevice invocation with, such as 9021 (screen sharing needs a
/// newer iOS) or 9022 (microphone or camera in use).
#[derive(Debug)]
pub(crate) struct CoreDeviceError {
    pub(crate) domain: Option<String>,
    pub(crate) code: Option<i64>,
    /// The device's own sentence (`NSLocalizedDescription`).
    pub(crate) description: Option<String>,
}

impl CoreDeviceError {
    fn from_value(v: Option<&plist::Value>) -> Self {
        let d = v.and_then(plist::Value::as_dictionary);
        let get = |k: &str| d.and_then(|d| d.get(k));
        let info = |k: &str| get("userInfo").and_then(plist::Value::as_dictionary).and_then(|u| u.get(k));
        Self {
            domain: get("domain").and_then(plist::Value::as_string).map(str::to_owned),
            code: get("code").and_then(|c| c.as_signed_integer().or_else(|| c.as_unsigned_integer().map(|u| u as i64))),
            description: info("NSLocalizedDescription")
                .or_else(|| info("NSLocalizedFailureReason"))
                .and_then(plist::Value::as_string)
                .map(str::to_owned),
        }
    }
}

// The whole dictionary used to be printed: a wall of nested XPC values (some with byte arrays)
// that hid the one sentence that says what is wrong.
impl std::fmt::Display for CoreDeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.description.as_deref().unwrap_or("the device refused the request"))?;
        match (&self.domain, self.code) {
            (Some(d), Some(c)) => write!(f, " ({d} {c})"),
            (None, Some(c)) => write!(f, " (CoreDevice error {c})"),
            (Some(d), None) => write!(f, " ({d})"),
            (None, None) => Ok(()),
        }
    }
}

impl std::error::Error for CoreDeviceError {}

/// The `CoreDevice.output` of an answer, or its `CoreDevice.error` as a [`CoreDeviceError`].
pub(crate) fn output(answer: plist::Value) -> Result<plist::Value> {
    let mut res = answer
        .into_dictionary()
        .ok_or_else(|| anyhow!("the CoreDevice answer is not a dictionary"))?;
    match res.remove("CoreDevice.output") {
        Some(out) => Ok(out),
        None => Err(CoreDeviceError::from_value(res.get("CoreDevice.error")).into()),
    }
}

/// The CoreDevice error code in an error built by [`output`], if any, through any context.
pub(crate) fn error_code(e: &anyhow::Error) -> Option<i64> {
    e.chain().find_map(|c| c.downcast_ref::<CoreDeviceError>()).and_then(|c| c.code)
}

/// A RemoteXPC connection to one CoreDevice service, for [`request`]s.
macro_rules! xpc_link {
    ($name:ident, $service:literal) => {
        pub(crate) struct $name(
            pub(crate) idevice::RemoteXpcClient<Box<dyn idevice::ReadWrite>>,
        );

        impl idevice::RsdService for $name {
            fn rsd_service_name() -> std::borrow::Cow<'static, str> {
                $service.into()
            }

            async fn from_stream(
                stream: Box<dyn idevice::ReadWrite>,
            ) -> Result<Self, idevice::IdeviceError> {
                let mut xpc = idevice::RemoteXpcClient::new(stream).await?;
                xpc.do_handshake().await?;
                Ok(Self(xpc))
            }
        }

        impl $name {
            /// Sends one invocation and returns its output.
            pub(crate) async fn invoke(
                &mut self,
                feature: &str,
                input: idevice::xpc::Dictionary,
                action: Option<&str>,
            ) -> anyhow::Result<plist::Value> {
                self.0
                    .send_object($crate::coredevice::request(feature, input, action), true)
                    .await
                    .map_err(|e| anyhow::anyhow!("{} request failed (connection error: {e:?})", $service))?;
                let answer = self
                    .0
                    .recv()
                    .await
                    .map_err(|e| anyhow::anyhow!("{} answer failed (connection error: {e:?})", $service))?;
                $crate::coredevice::output(answer)
            }
        }
    };
}
pub(crate) use xpc_link;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_reads_the_code_of_a_coredevice_error() {
        let mut err = plist::Dictionary::new();
        err.insert("code".into(), plist::Value::Integer(9022.into()));
        err.insert("domain".into(), plist::Value::String("com.apple.CoreDevice.ControlChannelConnectionError".into()));
        let mut answer = plist::Dictionary::new();
        answer.insert("CoreDevice.error".into(), plist::Value::Dictionary(err));
        let e = output(plist::Value::Dictionary(answer)).unwrap_err();
        assert_eq!(error_code(&e), Some(9022));
        assert_eq!(error_code(&e.context("starting the audio stream failed")), Some(9022));
        assert_eq!(error_code(&anyhow!("tunnel closed")), None);
    }

    #[test]
    fn the_message_is_the_device_sentence_not_the_dictionary() {
        // As iPadOS 17 answers a media start (PITFALLS #65), with a byte array like the real one.
        let mut info = plist::Dictionary::new();
        info.insert(
            "NSLocalizedDescription".into(),
            plist::Value::String("Remote control requires iOS 27.0 or later on this device.".into()),
        );
        info.insert("NSUnderlyingError".into(), plist::Value::Data(vec![0; 64]));
        let mut err = plist::Dictionary::new();
        err.insert("code".into(), plist::Value::Integer(9021.into()));
        err.insert("domain".into(), plist::Value::String("com.apple.dt.CoreDeviceError".into()));
        err.insert("userInfo".into(), plist::Value::Dictionary(info));
        let mut answer = plist::Dictionary::new();
        answer.insert("CoreDevice.error".into(), plist::Value::Dictionary(err));
        let e = output(plist::Value::Dictionary(answer)).unwrap_err();
        assert_eq!(
            e.to_string(),
            "Remote control requires iOS 27.0 or later on this device. (com.apple.dt.CoreDeviceError 9021)"
        );
        assert_eq!(error_code(&e), Some(9021));

        let e = output(plist::Value::Dictionary(plist::Dictionary::new())).unwrap_err();
        assert_eq!(e.to_string(), "the device refused the request");
        assert_eq!(error_code(&e), None);
    }
}
