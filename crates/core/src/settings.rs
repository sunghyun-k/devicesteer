//! The device's settings (the ones Xcode's Device Hub offers): appearance, text size, Liquid
//! Glass, accessibility switches, VoiceOver, and the simulated location.
//!
//! All but the location are actions of `com.apple.coredevice.configuration`, a get and a set per
//! item, with the same input and output shape (`{"reduceMotion": {"enabled": true}}`). idevice's
//! `ConfigurationServiceClient` covers most of them; the rest (VoiceOver, reading Liquid Glass and
//! Increase Contrast) go out as raw actions on the same service. The location is a feature of
//! `com.apple.coredevice.locationservice`, which idevice only lists scenarios of.
//!
//! The old Swift implementation found that the configuration service closes its connection after
//! one reply, so every call opens a connection of its own. Some calls get a bodyless reply and
//! nothing after it (PITFALLS #24), so each has a short timeout and one retry.

use std::{borrow::Cow, time::Duration};

use anyhow::{Result, bail};
use idevice::{
    IdeviceError, ReadWrite, RsdService,
    core_device::{ConfigurationServiceClient, CoreDeviceError, CoreDeviceServiceClient, UserInterfaceStyle},
};
use serde::Serialize;

use crate::Session;

const CONFIGURATION: &str = "com.apple.coredevice.configuration";
const LOCATION: &str = "com.apple.coredevice.locationservice";
const SIMULATE_LOCATION: &str = "com.apple.coredevice.feature.simulatelocation";
const ACTION: &str = "com.apple.coredevice.action.";
/// A call answers in 10 to 50 ms; one that hasn't in this long got the bodyless reply of
/// PITFALLS #24 and is tried again on a new connection.
const CALL_TIMEOUT: Duration = Duration::from_millis(1500);
/// Tries per call. Two stalls in a row were seen once in about a hundred calls.
const CALL_TRIES: usize = 3;
/// Setting VoiceOver answers only once it has started or stopped: 3.1 s both ways.
const VOICEOVER_SET_TIMEOUT: Duration = Duration::from_secs(8);

/// What one call gives: the outer error is the connection failing, the inner one the device's
/// answer (a refusal).
type Call<T> = Result<Result<T, IdeviceError>>;

/// The configuration service as a plain CoreDevice client, for the actions idevice lacks.
struct ConfigLink(CoreDeviceServiceClient<Box<dyn ReadWrite>>);

impl RsdService for ConfigLink {
    fn rsd_service_name() -> Cow<'static, str> {
        CONFIGURATION.into()
    }

    async fn from_stream(stream: Box<dyn ReadWrite>) -> Result<Self, IdeviceError> {
        Ok(Self(CoreDeviceServiceClient::new(stream).await?))
    }
}

struct LocationLink(CoreDeviceServiceClient<Box<dyn ReadWrite>>);

impl RsdService for LocationLink {
    fn rsd_service_name() -> Cow<'static, str> {
        LOCATION.into()
    }

    async fn from_stream(stream: Box<dyn ReadWrite>) -> Result<Self, IdeviceError> {
        Ok(Self(CoreDeviceServiceClient::new(stream).await?))
    }
}

/// An on/off item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    ReduceMotion,
    IncreaseContrast,
    ShowBorders,
    ReduceTransparency,
    VoiceOver,
}

impl Toggle {
    pub const ALL: [Toggle; 5] = [
        Toggle::ReduceMotion,
        Toggle::IncreaseContrast,
        Toggle::ShowBorders,
        Toggle::ReduceTransparency,
        Toggle::VoiceOver,
    ];

    /// The name on the command line and in the JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Toggle::ReduceMotion => "reduce-motion",
            Toggle::IncreaseContrast => "increase-contrast",
            Toggle::ShowBorders => "show-borders",
            Toggle::ReduceTransparency => "reduce-transparency",
            Toggle::VoiceOver => "voiceover",
        }
    }

    /// The action without its `get`/`set` prefix, and the input/output key.
    fn wire(self) -> (&'static str, &'static str) {
        match self {
            Toggle::ReduceMotion => ("reducemotion", "reduceMotion"),
            Toggle::IncreaseContrast => ("deviceincreasecontrast", "increaseContrast"),
            Toggle::ShowBorders => ("showborders", "showBorders"),
            Toggle::ReduceTransparency => ("reducetransparency", "reduceTransparency"),
            Toggle::VoiceOver => ("voiceover", "voiceOverConfiguration"),
        }
    }
}

/// Text sizes from smallest to largest: the device's name (a `UIContentSizeCategory` case) and
/// the short name the command line takes.
pub const TEXT_SIZES: [(&str, &str); 12] = [
    ("extraSmall", "xs"),
    ("small", "s"),
    ("medium", "m"),
    ("large", "l"),
    ("extraLarge", "xl"),
    ("extraExtraLarge", "xxl"),
    ("extraExtraExtraLarge", "xxxl"),
    ("accessibilityMedium", "ax1"),
    ("accessibilityLarge", "ax2"),
    ("accessibilityExtraLarge", "ax3"),
    ("accessibilityExtraExtraLarge", "ax4"),
    ("accessibilityExtraExtraExtraLarge", "ax5"),
];

/// The short name of a device text size (`large` -> `l`); unknown names pass through.
fn short_text_size(device: &str) -> String {
    TEXT_SIZES
        .iter()
        .find(|(name, _)| *name == device)
        .map_or_else(|| device.to_string(), |(_, short)| short.to_string())
}

/// The panel's current values. An item the device refused to read is `null` and named in
/// `unsupported`: one refusal (an iPad on iOS 26.6 had no Liquid Glass) must not hide the rest.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DeviceSettings {
    /// `light` or `dark`.
    pub appearance: Option<String>,
    /// `xs` .. `xxxl`, `ax1` .. `ax5`.
    pub text_size: Option<String>,
    /// Liquid Glass opacity, 0 to 1.
    pub liquid_glass: Option<f64>,
    pub reduce_motion: Option<bool>,
    pub increase_contrast: Option<bool>,
    pub show_borders: Option<bool>,
    pub reduce_transparency: Option<bool>,
    pub voiceover: Option<bool>,
    pub unsupported: Vec<&'static str>,
}

impl DeviceSettings {
    pub fn toggle(&self, t: Toggle) -> Option<bool> {
        match t {
            Toggle::ReduceMotion => self.reduce_motion,
            Toggle::IncreaseContrast => self.increase_contrast,
            Toggle::ShowBorders => self.show_borders,
            Toggle::ReduceTransparency => self.reduce_transparency,
            Toggle::VoiceOver => self.voiceover,
        }
    }

    fn toggle_mut(&mut self, t: Toggle) -> &mut Option<bool> {
        match t {
            Toggle::ReduceMotion => &mut self.reduce_motion,
            Toggle::IncreaseContrast => &mut self.increase_contrast,
            Toggle::ShowBorders => &mut self.show_borders,
            Toggle::ReduceTransparency => &mut self.reduce_transparency,
            Toggle::VoiceOver => &mut self.voiceover,
        }
    }
}

/// One change to the panel or the location.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting {
    /// `true` for dark.
    Dark(bool),
    /// A device text size name (`large`).
    TextSize(&'static str),
    LiquidGlass(f32),
    Toggle(Toggle, bool),
    /// Latitude, longitude in degrees.
    Location(f64, f64),
    ClearLocation,
}

const SETTING_NAMES: &str = "appearance, text-size, liquid-glass, reduce-motion, increase-contrast, show-borders, \
                             reduce-transparency, voiceover, location";

impl Setting {
    /// Parses `name value...` as the command line takes it (`text-size ax1`, `location set 1 2`).
    pub fn parse(name: &str, values: &[String]) -> Result<Self> {
        let name = name.to_ascii_lowercase().replace('_', "-");
        let one = || -> Result<String> {
            match values {
                [v] => Ok(v.to_ascii_lowercase()),
                _ => bail!("`{name}` takes one value"),
            }
        };
        if let Some(t) = Toggle::ALL.into_iter().find(|t| t.name() == name) {
            let on = match one()?.as_str() {
                "on" | "true" | "1" | "yes" => true,
                "off" | "false" | "0" | "no" => false,
                v => bail!("`{v}` is not on or off"),
            };
            return Ok(Setting::Toggle(t, on));
        }
        Ok(match name.as_str() {
            "appearance" => match one()?.as_str() {
                "dark" => Setting::Dark(true),
                "light" => Setting::Dark(false),
                v => bail!("appearance is dark or light, not `{v}`"),
            },
            "text-size" => {
                let v = one()?;
                let size = TEXT_SIZES
                    .iter()
                    .find(|(device, short)| *short == v || device.eq_ignore_ascii_case(&v))
                    .map(|(device, _)| *device);
                match size {
                    Some(s) => Setting::TextSize(s),
                    None => bail!("text size is one of xs, s, m, l, xl, xxl, xxxl, ax1 .. ax5, not `{v}`"),
                }
            }
            "liquid-glass" => {
                let v = one()?;
                match v.parse::<f32>() {
                    Ok(o) if (0.0..=1.0).contains(&o) => Setting::LiquidGlass(o),
                    _ => bail!("liquid-glass is an opacity from 0 to 1, not `{v}`"),
                }
            }
            "location" => match values {
                [c] if c.eq_ignore_ascii_case("clear") => Setting::ClearLocation,
                [s, lat, lon] if s.eq_ignore_ascii_case("set") => {
                    let num = |v: &str, what: &str, max: f64| -> Result<f64> {
                        match v.parse::<f64>() {
                            Ok(n) if n.abs() <= max => Ok(n),
                            _ => bail!("`{v}` is not a {what} in degrees (-{max} to {max})"),
                        }
                    };
                    Setting::Location(num(lat, "latitude", 90.0)?, num(lon, "longitude", 180.0)?)
                }
                _ => bail!("location takes `set <latitude> <longitude>` or `clear`"),
            },
            other => bail!("unknown setting `{other}`; settings are {SETTING_NAMES}"),
        })
    }

    /// The name of the item it changes, as in [`DeviceSettings`]'s fields.
    pub fn name(&self) -> &'static str {
        match self {
            Setting::Dark(_) => "appearance",
            Setting::TextSize(_) => "text_size",
            Setting::LiquidGlass(_) => "liquid_glass",
            Setting::Toggle(t, _) => match t {
                Toggle::ReduceMotion => "reduce_motion",
                Toggle::IncreaseContrast => "increase_contrast",
                Toggle::ShowBorders => "show_borders",
                Toggle::ReduceTransparency => "reduce_transparency",
                Toggle::VoiceOver => "voiceover",
            },
            Setting::Location(..) | Setting::ClearLocation => "location",
        }
    }
}

/// Whether the device answered with a refusal (it doesn't have the item), as opposed to the
/// connection failing.
fn refused(e: &IdeviceError) -> bool {
    matches!(
        e,
        IdeviceError::CoreDevice(
            CoreDeviceError::DeviceError(_) | CoreDeviceError::MissingField(_) | CoreDeviceError::MalformedField(_)
        )
    )
}

/// The device's own explanation of a refusal (`userInfo.NSLocalizedDescription`), or the whole
/// error when it has none.
fn reason(e: &IdeviceError) -> String {
    let text = e.to_string();
    const KEY: &str = "\"NSLocalizedDescription\": String(\"";
    text.find(KEY)
        .and_then(|at| text[at + KEY.len()..].split("\")").next())
        .map_or(text.clone(), |r| r.replace("\\\"", "\""))
}

/// `{key: {"enabled": bool}}` from an output.
fn enabled(out: &plist::Value, key: &'static str) -> Result<bool, IdeviceError> {
    out.as_dictionary()
        .and_then(|d| d.get(key))
        .and_then(|v| v.as_dictionary())
        .and_then(|d| d.get("enabled"))
        .and_then(|v| v.as_boolean())
        .ok_or(CoreDeviceError::MissingField(key).into())
}

impl Session {
    /// Runs one call with [`CALL_TIMEOUT`], again on a new connection if it times out.
    async fn attempt<T>(&mut self, what: &str, call: impl AsyncFnMut(&mut Session) -> Call<T>) -> Call<T> {
        self.attempt_within(CALL_TIMEOUT, what, call).await
    }

    async fn attempt_within<T>(
        &mut self,
        limit: Duration,
        what: &str,
        mut call: impl AsyncFnMut(&mut Session) -> Call<T>,
    ) -> Call<T> {
        for _ in 0..CALL_TRIES {
            match tokio::time::timeout(limit, call(self)).await {
                Ok(r) => return r,
                Err(_) => tracing::debug!("{what}: no answer in {limit:?}; trying a new connection"),
            }
        }
        // Not worded as a connection failure: the tunnel is fine (other calls on it answer), and
        // the daemon drops its whole session on those words.
        bail!("the device did not answer {what} ({CALL_TRIES} tries); run the command again")
    }

    async fn configuration(&mut self) -> Result<ConfigurationServiceClient<Box<dyn ReadWrite>>> {
        self.connect().await
    }

    /// Calls a configuration action idevice has no method for.
    async fn configuration_action(&mut self, action: &str, input: plist::Dictionary) -> Call<plist::Value> {
        let mut link: ConfigLink = self.connect().await?;
        Ok(link.0.invoke_action_with_plist(format!("{ACTION}{action}"), input).await)
    }

    async fn read_toggle(&mut self, t: Toggle) -> Call<bool> {
        self.attempt(t.name(), async |s| {
            Ok(match t {
                Toggle::ReduceMotion => s.configuration().await?.get_reduce_motion().await,
                Toggle::ShowBorders => s.configuration().await?.get_show_borders().await,
                Toggle::ReduceTransparency => s.configuration().await?.get_reduce_transparency().await,
                Toggle::IncreaseContrast | Toggle::VoiceOver => {
                    let (action, key) = t.wire();
                    s.configuration_action(&format!("get{action}"), plist::Dictionary::new())
                        .await?
                        .and_then(|out| enabled(&out, key))
                }
            })
        })
        .await
    }

    /// Reads one on/off item. `None` when the device refused it.
    pub async fn setting_toggle(&mut self, t: Toggle) -> Result<Option<bool>> {
        if !self.rsd().services.contains_key(CONFIGURATION) {
            return Ok(None);
        }
        match self.read_toggle(t).await? {
            Ok(v) => Ok(Some(v)),
            Err(e) if refused(&e) => Ok(None),
            Err(e) => bail!("reading {} failed (connection error: {e:?})", t.name()),
        }
    }

    /// Reads the whole panel.
    pub async fn settings(&mut self) -> Result<DeviceSettings> {
        let mut s = DeviceSettings::default();
        if !self.rsd().services.contains_key(CONFIGURATION) {
            s.unsupported = vec![
                "appearance",
                "text_size",
                "liquid_glass",
                "reduce_motion",
                "increase_contrast",
                "show_borders",
                "reduce_transparency",
                "voiceover",
            ];
            return Ok(s);
        }
        // A refusal marks the item unsupported; anything else is a broken connection.
        macro_rules! read {
            ($name:literal, $call:expr) => {
                match $call {
                    Ok(v) => Some(v),
                    Err(e) if refused(&e) => {
                        tracing::debug!("{} refused: {}", $name, reason(&e));
                        s.unsupported.push($name);
                        None
                    }
                    Err(e) => bail!("reading {} failed (connection error: {e:?})", $name),
                }
            };
        }
        let style = self
            .attempt("appearance", async |s| Ok(s.configuration().await?.get_user_interface_style().await))
            .await?;
        s.appearance = read!("appearance", style).map(|st| st.as_str().to_string());
        let size = self
            .attempt("text_size", async |s| Ok(s.configuration().await?.get_device_text_size().await))
            .await?;
        s.text_size = read!("text_size", size).map(|t| short_text_size(&t));
        let glass = self
            .attempt("liquid_glass", async |s| {
                Ok(s.configuration_action("getliquidglassconfiguration", plist::Dictionary::new())
                    .await?
                    .and_then(|out| {
                        out.as_dictionary()
                            .and_then(|d| d.get("configuration"))
                            .and_then(|v| v.as_dictionary())
                            .and_then(|d| d.get("opacity"))
                            .and_then(|v| v.as_real())
                            .ok_or(CoreDeviceError::MissingField("configuration").into())
                    }))
            })
            .await?;
        // It is a Float on the device: 0.8 comes back as 0.800000011920929.
        s.liquid_glass = read!("liquid_glass", glass).map(|o| (o * 100.0).round() / 100.0);
        for t in Toggle::ALL {
            let v = match self.read_toggle(t).await? {
                Ok(v) => Some(v),
                Err(e) if refused(&e) => {
                    tracing::debug!("{} refused: {}", t.name(), reason(&e));
                    s.unsupported.push(Setting::Toggle(t, false).name());
                    None
                }
                Err(e) => bail!("reading {} failed (connection error: {e:?})", t.name()),
            };
            *s.toggle_mut(t) = v;
        }
        Ok(s)
    }

    async fn apply_once(&mut self, setting: &Setting) -> Call<()> {
        Ok(match *setting {
            Setting::Location(lat, lon) => {
                let mut link: LocationLink = self.connect().await?;
                let input = plist::Dictionary::from_iter([
                    ("latitude".to_string(), plist::Value::Real(lat)),
                    ("longitude".to_string(), plist::Value::Real(lon)),
                ]);
                link.0
                    .invoke_with_plist_action(SIMULATE_LOCATION, input, format!("{ACTION}setsimulatedlocation"))
                    .await
                    .map(drop)
            }
            Setting::ClearLocation => {
                let mut link: LocationLink = self.connect().await?;
                link.0
                    .invoke_with_plist_action(SIMULATE_LOCATION, plist::Dictionary::new(), format!("{ACTION}clearsimulatedlocation"))
                    .await
                    .map(drop)
            }
            Setting::Dark(dark) => {
                let style = if dark { UserInterfaceStyle::Dark } else { UserInterfaceStyle::Light };
                self.configuration().await?.set_user_interface_style(style).await
            }
            Setting::TextSize(size) => self.configuration().await?.set_device_text_size(size).await,
            Setting::LiquidGlass(o) => self.configuration().await?.set_liquid_glass_opacity(o).await,
            Setting::Toggle(t, on) => match t {
                Toggle::ReduceMotion => self.configuration().await?.set_reduce_motion(on).await,
                Toggle::IncreaseContrast => self.configuration().await?.set_increase_contrast(on).await,
                Toggle::ShowBorders => self.configuration().await?.set_show_borders(on).await,
                Toggle::ReduceTransparency => self.configuration().await?.set_reduce_transparency(on).await,
                Toggle::VoiceOver => {
                    let (action, key) = t.wire();
                    let input = plist::Dictionary::from_iter([(
                        key.to_string(),
                        plist::Value::Dictionary(plist::Dictionary::from_iter([(
                            "enabled".to_string(),
                            plist::Value::Boolean(on),
                        )])),
                    )]);
                    self.configuration_action(&format!("set{action}"), input).await?.map(drop)
                }
            },
        })
    }

    /// Changes one item, or sets or clears the simulated location (which stays until cleared,
    /// across disconnects). Setting a value twice is harmless, so a timed-out call is repeated.
    pub async fn apply_setting(&mut self, setting: &Setting) -> Result<()> {
        let limit = match setting {
            Setting::Toggle(Toggle::VoiceOver, _) => VOICEOVER_SET_TIMEOUT,
            _ => CALL_TIMEOUT,
        };
        match self.attempt_within(limit, setting.name(), async |s| s.apply_once(setting).await).await? {
            Ok(()) => Ok(()),
            Err(e) if refused(&e) => bail!("the device refused {}: {}", setting.name(), reason(&e)),
            Err(e) => bail!("setting {} failed (connection error: {e:?})", setting.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(name: &str, values: &[&str]) -> Result<Setting> {
        Setting::parse(name, &values.iter().map(|v| v.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_command_line_settings() {
        assert_eq!(parse("appearance", &["Dark"]).unwrap(), Setting::Dark(true));
        assert_eq!(parse("text-size", &["ax1"]).unwrap(), Setting::TextSize("accessibilityMedium"));
        assert_eq!(parse("text_size", &["large"]).unwrap(), Setting::TextSize("large"));
        assert_eq!(parse("liquid-glass", &["0.5"]).unwrap(), Setting::LiquidGlass(0.5));
        assert_eq!(parse("voiceover", &["on"]).unwrap(), Setting::Toggle(Toggle::VoiceOver, true));
        assert_eq!(parse("reduce-motion", &["off"]).unwrap(), Setting::Toggle(Toggle::ReduceMotion, false));
        assert_eq!(parse("location", &["set", "37.33", "-122.01"]).unwrap(), Setting::Location(37.33, -122.01));
        assert_eq!(parse("location", &["clear"]).unwrap(), Setting::ClearLocation);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse("liquid-glass", &["1.5"]).is_err());
        assert!(parse("text-size", &["huge"]).is_err());
        assert!(parse("voiceover", &["maybe"]).is_err());
        assert!(parse("location", &["set", "91", "0"]).is_err());
        assert!(parse("brightness", &["1"]).is_err());
    }

    #[test]
    fn a_refusal_reads_as_the_devices_explanation() {
        let e: IdeviceError = CoreDeviceError::DeviceError(
            r#"Dictionary({"code": Integer(21063), "userInfo": Dictionary({"NSLocalizedDescription": String("Enable it first.")})})"#
                .into(),
        )
        .into();
        assert_eq!(reason(&e), "Enable it first.");
    }

    #[test]
    fn text_sizes_have_short_names() {
        assert_eq!(short_text_size("accessibilityExtraExtraExtraLarge"), "ax5");
        assert_eq!(short_text_size("somethingNew"), "somethingNew");
    }
}
