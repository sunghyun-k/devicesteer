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

/// The `CoreDevice.output` of an answer, or its `CoreDevice.error` as the error.
pub(crate) fn output(answer: plist::Value) -> Result<plist::Value> {
    let mut res = answer
        .into_dictionary()
        .ok_or_else(|| anyhow!("the CoreDevice answer is not a dictionary"))?;
    match res.remove("CoreDevice.output") {
        Some(out) => Ok(out),
        None => Err(anyhow!(
            "{:?}",
            res.get("CoreDevice.error")
                .unwrap_or(&plist::Value::Dictionary(res.clone()))
        )),
    }
}

/// The CoreDevice error code in an error built by [`output`], if any.
pub(crate) fn error_code(e: &anyhow::Error) -> Option<i64> {
    // The error dictionary is printed with `{:?}`; its `code` is `Integer(N)`.
    let text = format!("{e:?}");
    let at = text.find("\"code\": Integer(")? + "\"code\": Integer(".len();
    text[at..].split(')').next()?.parse().ok()
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
        assert_eq!(error_code(&anyhow!("tunnel closed")), None);
    }
}
