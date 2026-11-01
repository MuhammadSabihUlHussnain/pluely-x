//! Screenshot downscale + encode (pure).
//!
//! Long edge ≤ `max_edge` (Triangle filter). JPEG is RGB only (the encoder rejects RGBA), so
//! alpha is dropped. `auto` picks PNG when the *source* is small (< 1 MP, i.e. a region
//! shot, where crisp text matters and the file stays small) and JPEG otherwise.

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::FilterType;
use image::{DynamicImage, ImageEncoder, RgbaImage};
use serde::Deserialize;

pub const DEFAULT_MAX_EDGE: u32 = 1568;
pub const HIFI_MAX_EDGE: u32 = 2576;
pub const DEFAULT_QUALITY: u8 = 82;
pub const THUMB_EDGE: u32 = 320;
const PNG_MAX_PIXELS: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Auto,
    Jpeg,
    Png,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Target size so the long edge is ≤ `max_edge` (never upscales, never 0).
pub fn fit_long_edge(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    let max_edge = max_edge.max(1);
    let long = w.max(h);
    if long <= max_edge || w == 0 || h == 0 {
        return (w.max(1), h.max(1));
    }
    let s = max_edge as f64 / long as f64;
    (
        ((w as f64 * s).round() as u32).clamp(1, max_edge),
        ((h as f64 * s).round() as u32).clamp(1, max_edge),
    )
}

pub fn clamp_max_edge(v: Option<u32>) -> u32 {
    v.unwrap_or(DEFAULT_MAX_EDGE).clamp(64, HIFI_MAX_EDGE)
}

pub fn clamp_quality(v: Option<u8>) -> u8 {
    v.unwrap_or(DEFAULT_QUALITY).clamp(30, 95)
}

fn downscale(img: &RgbaImage, max_edge: u32) -> DynamicImage {
    let (w, h) = fit_long_edge(img.width(), img.height(), max_edge);
    if (w, h) == img.dimensions() {
        DynamicImage::ImageRgba8(img.clone())
    } else {
        DynamicImage::ImageRgba8(image::imageops::resize(img, w, h, FilterType::Triangle))
    }
}

fn encode_rgb(
    rgb: &image::RgbImage,
    png: bool,
    quality: u8,
) -> Result<(Vec<u8>, &'static str), String> {
    let mut out = Vec::new();
    if png {
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilter::Adaptive)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("PNG encode failed: {e}"))?;
        Ok((out, "image/png"))
    } else {
        JpegEncoder::new_with_quality(&mut out, quality)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("JPEG encode failed: {e}"))?;
        Ok((out, "image/jpeg"))
    }
}

/// Downscale to `max_edge` and encode.
pub fn encode(
    img: &RgbaImage,
    max_edge: u32,
    format: Format,
    quality: u8,
) -> Result<Encoded, String> {
    if img.width() == 0 || img.height() == 0 {
        return Err("empty image".into());
    }
    let png = match format {
        Format::Png => true,
        Format::Jpeg => false,
        Format::Auto => (img.width() as u64 * img.height() as u64) < PNG_MAX_PIXELS,
    };
    let rgb = downscale(img, max_edge).to_rgb8();
    let (bytes, mime) = encode_rgb(&rgb, png, quality)?;
    Ok(Encoded {
        bytes,
        mime,
        width: rgb.width(),
        height: rgb.height(),
    })
}

/// ≤ 320 px JPEG thumbnail as a data URL.
pub fn thumb_data_url(img: &RgbaImage) -> Result<String, String> {
    let rgb = downscale(img, THUMB_EDGE).to_rgb8();
    let (bytes, mime) = encode_rgb(&rgb, false, 70)?;
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        })
    }

    #[test]
    fn fit_long_edge_math() {
        assert_eq!(fit_long_edge(5000, 2000, 1568), (1568, 627));
        assert_eq!(fit_long_edge(2000, 5000, 1568), (627, 1568));
        assert_eq!(fit_long_edge(3840, 2160, 1568), (1568, 882));
        assert_eq!(fit_long_edge(800, 600, 1568), (800, 600)); // never upscale
        assert_eq!(fit_long_edge(10000, 1, 1568), (1568, 1));
        assert_eq!(clamp_max_edge(None), 1568);
        assert_eq!(clamp_max_edge(Some(99_999)), 2576);
        assert_eq!(clamp_quality(Some(200)), 95);
    }

    #[test]
    fn big_rgba_becomes_decodable_jpeg() {
        let img = gradient(5000, 2000);
        let e = encode(&img, DEFAULT_MAX_EDGE, Format::Auto, 82).unwrap();
        assert_eq!(e.mime, "image/jpeg");
        assert_eq!((e.width, e.height), (1568, 627));
        let back = image::load_from_memory_with_format(&e.bytes, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (1568, 627));
    }

    #[test]
    fn small_region_auto_is_png_and_format_can_be_forced() {
        let img = gradient(600, 400);
        let e = encode(&img, DEFAULT_MAX_EDGE, Format::Auto, 82).unwrap();
        assert_eq!(e.mime, "image/png");
        let back = image::load_from_memory_with_format(&e.bytes, image::ImageFormat::Png).unwrap();
        assert_eq!((back.width(), back.height()), (600, 400));
        assert_eq!(back.color(), image::ColorType::Rgb8); // alpha dropped
        let j = encode(&img, DEFAULT_MAX_EDGE, Format::Jpeg, 82).unwrap();
        assert_eq!(j.mime, "image/jpeg");
        let p = encode(&gradient(3000, 3000), 1000, Format::Png, 82).unwrap();
        assert_eq!((p.mime, p.width, p.height), ("image/png", 1000, 1000));
    }

    #[test]
    fn thumbnail_is_small_data_url() {
        let t = thumb_data_url(&gradient(1920, 1080)).unwrap();
        assert!(t.starts_with("data:image/jpeg;base64,"));
        let b64 = t.trim_start_matches("data:image/jpeg;base64,");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (320, 180));
    }

    #[test]
    fn empty_image_errors() {
        assert!(encode(&RgbaImage::new(0, 0), 1568, Format::Auto, 82).is_err());
    }
}
