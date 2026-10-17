//! Version thumbnails: the PNG made from the offscreen model render when a
//! version is saved, and decoding of the thumbnails the server sends back for
//! display.
use eframe::egui;

/// Edge length of the thumbnails R-CAD makes, in pixels.
pub const THUMBNAIL_PX: u32 = 256;

/// A thumbnail PNG from a square `side`×`side` RGBA8 render (rendered larger
/// than [`THUMBNAIL_PX`] for smoother edges): scaled to [`THUMBNAIL_PX`],
/// made opaque and encoded. None when the buffer does not match `side` or the
/// PNG would exceed the protocol's limit.
pub fn thumbnail_png(rgba: &[u8], side: u32) -> Option<Vec<u8>> {
    let len = usize::try_from(side)
        .ok()?
        .checked_mul(usize::try_from(side).ok()?)?;
    if side == 0 || rgba.len() != len.checked_mul(4)? {
        return None;
    }
    let mut img = image::RgbaImage::from_raw(side, side, rgba.to_vec())?;
    // Opaque: the render has no meaningful transparency.
    for p in img.pixels_mut() {
        p.0[3] = 255;
    }
    let scaled = if side == THUMBNAIL_PX {
        img
    } else {
        image::imageops::resize(
            &img,
            THUMBNAIL_PX,
            THUMBNAIL_PX,
            image::imageops::FilterType::Triangle,
        )
    };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(scaled)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    rcad_cloud::client::check_thumbnail(&png).ok()?;
    Some(png)
}

/// Decode a thumbnail for display. None for anything that is not a PNG of a
/// sensible size.
pub fn decode_png(png: &[u8]) -> Option<egui::ColorImage> {
    rcad_cloud::client::check_thumbnail(png).ok()?;
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    if img.width() > 4096 || img.height() > 4096 {
        return None;
    }
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(side: u32) -> Vec<u8> {
        let mut px = Vec::new();
        for y in 0..side {
            for x in 0..side {
                px.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 128, 0]);
            }
        }
        px
    }

    #[test]
    fn a_render_becomes_an_opaque_png_within_the_limit() {
        let png = thumbnail_png(&gradient(512), 512).unwrap();
        assert!(png.starts_with(rcad_cloud::client::PNG_SIGNATURE));
        assert!(png.len() as u64 <= rcad_cloud::proto::MAX_THUMBNAIL_BYTES);
        let back = decode_png(&png).unwrap();
        assert_eq!(back.size, [256, 256]);
        // Scaled by half: pixel (10, 20) comes from about (21, 41); and opaque.
        let c = back.pixels[20 * 256 + 10];
        assert!((i32::from(c.r()) - 21).abs() <= 3, "{c:?}");
        assert!((i32::from(c.g()) - 41).abs() <= 3, "{c:?}");
        assert_eq!(c.a(), 255);
        // Already at size: kept as is. A buffer that does not match gives nothing.
        assert!(thumbnail_png(&gradient(256), 256).is_some());
        assert!(thumbnail_png(&gradient(256), 512).is_none());
        assert!(thumbnail_png(&[], 0).is_none());
    }

    #[test]
    fn garbage_does_not_decode() {
        assert!(decode_png(b"not a png").is_none());
        let mut fake = rcad_cloud::client::PNG_SIGNATURE.to_vec();
        fake.extend_from_slice(b"truncated");
        assert!(decode_png(&fake).is_none());
    }
}
