//! Screenshots: the capture request, and the device's PNG made 8-bit.

use std::{borrow::Cow, time::Duration};

use anyhow::{Context, Result, bail};
use idevice::{
    IdeviceError, RsdService,
    core_device::{CoreDeviceError, ImageFormat, ScreenCaptureServiceClient},
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};
use serde::Serialize;

/// Longest wait for one capture. Longer than the usual step timeout: the device takes 1.5 s to
/// encode a 16-bit PNG of a large panel before it sends anything, then sends about 12 MB
/// (PITFALLS #63), and a busy Wi-Fi network can stretch that past 12 s.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);

/// A screenshot as it will be saved.
#[derive(Debug, Clone, Serialize)]
pub struct Screenshot {
    #[serde(skip)]
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Bits per channel of the PNG the device sent (16 on wide-colour panels, PITFALLS #63).
    pub device_bits: u8,
    /// Size of the PNG the device sent.
    pub device_bytes: usize,
}

/// Asks the device for a PNG of the panel `display` (its `uniqueId`; `None` for the first panel)
/// on a connection of its own, so the caller can do it without holding the session.
pub async fn capture(adapter: &mut AdapterHandle, rsd: &mut RsdHandshake, display: Option<&str>) -> Result<Vec<u8>> {
    let name = ScreenCaptureServiceClient::rsd_service_name();
    if !rsd.services.contains_key(name.as_ref()) {
        bail!(
            "the device does not advertise {name}. The Developer Disk Image is probably not mounted (a restart \
             can unmount it): run `devicesteer setup`, then retry"
        );
    }
    let mut client = crate::step(&format!("{name} connection"), ScreenCaptureServiceClient::connect_rsd(adapter, rsd)).await?;
    // The device sends nothing else than PNG: it refuses "jpeg", "jpg", "heic" and "tiff".
    match tokio::time::timeout(CAPTURE_TIMEOUT, client.take_screenshot(display, ImageFormat::Png)).await {
        Ok(Ok(png)) => Ok(png),
        // The device's own refusal, such as another client's capture in progress. Not worded as a
        // connection error: the daemon would drop a tunnel that is fine.
        Ok(Err(IdeviceError::CoreDevice(CoreDeviceError::DeviceError(detail)))) => {
            bail!("the device did not take the screenshot: {}; retry", device_description(&detail))
        }
        Ok(Err(e)) => bail!("screenshot failed (connection error: {e:?})"),
        Err(_) => bail!("screenshot timed out after {}s (connection stalled)", CAPTURE_TIMEOUT.as_secs()),
    }
}

/// The `NSLocalizedDescription` in a CoreDevice error's dump, or the whole dump.
fn device_description(detail: &str) -> &str {
    const KEY: &str = "\"NSLocalizedDescription\": String(\"";
    detail
        .find(KEY)
        .map(|i| &detail[i + KEY.len()..])
        .and_then(|rest| rest.find("\")").map(|end| &rest[..end]))
        .unwrap_or(detail)
}

/// The screenshot to save from the device's `png`: 8 bits per channel and no alpha channel
/// that is opaque everywhere, unless `raw`. Colour profile chunks are kept.
pub fn finish(png: Vec<u8>, raw: bool) -> Result<Screenshot> {
    let device_bytes = png.len();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&png[..]));
    decoder.set_transformations(png::Transformations::IDENTITY);
    if raw {
        let info = decoder.read_header_info().context("the device sent an image that is not a PNG")?;
        let (width, height, device_bits) = (info.width, info.height, info.bit_depth as u8);
        return Ok(Screenshot { png, width, height, device_bits, device_bytes });
    }
    let mut reader = decoder.read_info().context("the device sent an image that is not a PNG")?;
    let info = reader.info().clone();
    let device_bits = info.bit_depth as u8;
    let (width, height) = (info.width, info.height);
    let mut pixels = vec![0; reader.output_buffer_size().context("the device's PNG is too large")?];
    let frame = reader.next_frame(&mut pixels).context("the device's PNG is damaged")?;
    pixels.truncate(frame.buffer_size());

    let Some((color, pixels)) = to_8bit_opaque(info.color_type, info.bit_depth, pixels) else {
        return Ok(Screenshot { png, width, height, device_bits, device_bytes });
    };
    let mut out_info = png::Info::with_size(width, height);
    out_info.color_type = color;
    out_info.bit_depth = png::BitDepth::Eight;
    // The device tags its pixels Display P3 (an ICC profile and cICP); without the tags a viewer
    // would show them as sRGB, a little dull.
    out_info.icc_profile = info.icc_profile.clone();
    out_info.coding_independent_code_points = info.coding_independent_code_points;
    out_info.srgb = info.srgb;
    out_info.gama_chunk = info.gama_chunk;
    out_info.chrm_chunk = info.chrm_chunk;
    out_info.pixel_dims = info.pixel_dims;
    out_info.exif_metadata = info.exif_metadata.as_ref().map(|e| Cow::Owned(e.to_vec()));
    let mut out = Vec::with_capacity(device_bytes / 8);
    {
        let mut encoder = png::Encoder::with_info(&mut out, out_info).context("could not encode the screenshot")?;
        // zlib level 3: 3.3 MB in 0.13 s for a busy 2007×2853 screen (release build). fdeflate
        // (`Compression::Fast`) took 25 ms but left 4.7 MB; level 6 (`Balanced`) 3.1 MB in 0.6 s.
        encoder.set_deflate_compression(png::DeflateCompression::Level(3));
        encoder.set_filter(png::Filter::Adaptive);
        let mut writer = encoder.write_header().context("could not encode the screenshot")?;
        writer.write_image_data(&pixels).context("could not encode the screenshot")?;
        writer.finish().context("could not encode the screenshot")?;
    }
    Ok(Screenshot { png: out, width, height, device_bits, device_bytes })
}

/// `pixels` at 8 bits per channel, without an alpha channel that is opaque everywhere, or `None`
/// when they are that already (or of a kind this doesn't handle).
fn to_8bit_opaque(color: png::ColorType, depth: png::BitDepth, pixels: Vec<u8>) -> Option<(png::ColorType, Vec<u8>)> {
    use png::{BitDepth, ColorType};
    let channels = match color {
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Indexed => return None,
    };
    let has_alpha = matches!(color, ColorType::Rgba | ColorType::GrayscaleAlpha);
    let pixels: Vec<u8> = match depth {
        // Rounded, not truncated: the high byte alone darkens every sample a little.
        BitDepth::Sixteen => pixels.as_chunks::<2>().0.iter().map(|s| ((u32::from(u16::from_be_bytes(*s)) * 255 + 32767) / 65535) as u8).collect(),
        BitDepth::Eight if has_alpha => pixels,
        _ => return None,
    };
    let opaque = has_alpha && pixels.chunks_exact(channels).all(|p| p[channels - 1] == 255);
    if !opaque {
        return Some((color, pixels));
    }
    let color = if color == ColorType::Rgba { ColorType::Rgb } else { ColorType::Grayscale };
    let pixels = pixels.chunks_exact(channels).flat_map(|p| p[..channels - 1].iter().copied()).collect();
    Some((color, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(color: png::ColorType, depth: png::BitDepth, w: u32, h: u32, data: &[u8], icc: Option<&[u8]>) -> Vec<u8> {
        let mut info = png::Info::with_size(w, h);
        info.color_type = color;
        info.bit_depth = depth;
        info.icc_profile = icc.map(|p| Cow::Owned(p.to_vec()));
        let mut out = Vec::new();
        let mut w = png::Encoder::with_info(&mut out, info).unwrap().write_header().unwrap();
        w.write_image_data(data).unwrap();
        w.finish().unwrap();
        out
    }

    fn decode(png: &[u8]) -> (png::OutputInfo, Vec<u8>, Option<Vec<u8>>) {
        let mut d = png::Decoder::new(std::io::Cursor::new(png));
        d.set_transformations(png::Transformations::IDENTITY);
        let mut r = d.read_info().unwrap();
        let icc = r.info().icc_profile.as_ref().map(|p| p.to_vec());
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info, buf, icc)
    }

    #[test]
    fn a_16_bit_capture_becomes_8_bit_with_its_profile() {
        // Two pixels: 0x0000/0x8080/0xFFFF and 0x00FF/0x7F7F/0x0181 per channel.
        let data = [0, 0, 0x80, 0x80, 0xFF, 0xFF, 0x00, 0xFF, 0x7F, 0x7F, 0x01, 0x81];
        let icc = b"not really a profile".as_slice();
        let png = encode(png::ColorType::Rgb, png::BitDepth::Sixteen, 2, 1, &data, Some(icc));
        let shot = finish(png.clone(), false).unwrap();
        assert_eq!((shot.width, shot.height, shot.device_bits, shot.device_bytes), (2, 1, 16, png.len()));
        let (info, pixels, kept) = decode(&shot.png);
        assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Rgb, png::BitDepth::Eight));
        assert_eq!(pixels, [0, 128, 255, 1, 127, 1]);
        assert_eq!(kept.as_deref(), Some(icc));
    }

    #[test]
    fn an_opaque_alpha_channel_is_dropped() {
        let data = [10, 20, 30, 255, 40, 50, 60, 255];
        let shot = finish(encode(png::ColorType::Rgba, png::BitDepth::Eight, 2, 1, &data, None), false).unwrap();
        let (info, pixels, _) = decode(&shot.png);
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(pixels, [10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn a_translucent_alpha_channel_stays() {
        let data = [10, 20, 30, 128];
        let shot = finish(encode(png::ColorType::Rgba, png::BitDepth::Eight, 1, 1, &data, None), false).unwrap();
        let (info, pixels, _) = decode(&shot.png);
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(pixels, data);
    }

    #[test]
    fn an_8_bit_rgb_capture_and_a_raw_one_are_kept_as_sent() {
        let png = encode(png::ColorType::Rgb, png::BitDepth::Eight, 1, 1, &[1, 2, 3], None);
        assert_eq!(finish(png.clone(), false).unwrap().png, png);
        let deep = encode(png::ColorType::Rgb, png::BitDepth::Sixteen, 1, 1, &[1, 2, 3, 4, 5, 6], None);
        let raw = finish(deep.clone(), true).unwrap();
        assert_eq!((raw.png, raw.device_bits), (deep, 16));
    }

    #[test]
    fn a_device_refusal_is_told_by_its_description() {
        // Shape of the dump seen when another client's capture was running (iOS 27.3).
        let dump = r#"Dictionary({"code": Integer(25003), "userInfo": Dictionary({"NSLocalizedDescription": String("A screenshot capture is already in progress.")}), "domain": String("com.apple.dt.CoreDeviceError")})"#;
        assert_eq!(device_description(dump), "A screenshot capture is already in progress.");
        assert_eq!(device_description("something else"), "something else");
    }

    #[test]
    fn something_else_than_png_is_an_error() {
        assert!(finish(b"\xFF\xD8\xFF not a png".to_vec(), false).is_err());
    }
}
