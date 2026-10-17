//! Version thumbnails: a square PNG made from a screenshot of the 3D view
//! when a version is saved, and decoding of the thumbnails the server sends
//! back for display.
use eframe::egui;

/// Edge length of the thumbnails R-CAD makes, in pixels.
pub const THUMBNAIL_PX: u32 = 256;

/// Marker carried by the screenshot request, so the answer is told apart from
/// other screenshots (F12, `--screenshot`).
#[derive(Debug, Clone, Copy)]
pub struct ThumbnailShot;

/// Whether a screenshot event answers a thumbnail request.
pub fn is_thumbnail_shot(user_data: &egui::UserData) -> bool {
    user_data
        .data
        .as_ref()
        .is_some_and(|d| d.downcast_ref::<ThumbnailShot>().is_some())
}

/// A thumbnail PNG from `image` (a whole-window screenshot): the region
/// `crop` (in physical pixels: min x, min y, max x, max y; None = the whole
/// image) is clamped to the image, center-cropped to a square, scaled to
/// [`THUMBNAIL_PX`] and encoded. None when the region is empty or the PNG
/// would exceed the protocol's limit.
pub fn thumbnail_png(image: &egui::ColorImage, crop: Option<[f32; 4]>) -> Option<Vec<u8>> {
    let [w, h] = image.size;
    if w == 0 || h == 0 || image.pixels.len() != w * h {
        return None;
    }
    let clamp = |v: f32, max: usize| (v.max(0.0).round() as usize).min(max);
    let [x0, y0, x1, y1] = crop.map_or([0, 0, w, h], |[a, b, c, d]| {
        [clamp(a, w), clamp(b, h), clamp(c, w), clamp(d, h)]
    });
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let side = (x1 - x0).min(y1 - y0);
    let sx = x0 + (x1 - x0 - side) / 2;
    let sy = y0 + (y1 - y0 - side) / 2;
    let side32 = u32::try_from(side).ok()?;
    let mut rgba = image::RgbaImage::new(side32, side32);
    for (y, row) in (sy..sy + side).enumerate() {
        for (x, col) in (sx..sx + side).enumerate() {
            let c = image.pixels[row * w + col];
            // Opaque: the 3D view has no meaningful transparency.
            rgba.put_pixel(x as u32, y as u32, image::Rgba([c.r(), c.g(), c.b(), 255]));
        }
    }
    let scaled = image::imageops::resize(
        &rgba,
        THUMBNAIL_PX,
        THUMBNAIL_PX,
        image::imageops::FilterType::Triangle,
    );
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

    fn gradient(w: usize, h: usize) -> egui::ColorImage {
        let mut img = egui::ColorImage::new([w, h], vec![egui::Color32::BLACK; w * h]);
        for y in 0..h {
            for x in 0..w {
                img.pixels[y * w + x] =
                    egui::Color32::from_rgb((x % 256) as u8, (y % 256) as u8, 128);
            }
        }
        img
    }

    #[test]
    fn screenshot_becomes_a_square_png_within_the_limit() {
        let shot = gradient(1400, 900);
        // A view rect away from the window's edges, wider than tall.
        let png = thumbnail_png(&shot, Some([300.0, 100.0, 1100.0, 700.0])).unwrap();
        assert!(png.starts_with(rcad_cloud::client::PNG_SIGNATURE));
        assert!(png.len() as u64 <= rcad_cloud::proto::MAX_THUMBNAIL_BYTES);
        let back = decode_png(&png).unwrap();
        assert_eq!(back.size, [256, 256]);
        // Center-cropped: the square spans x 400..1000, y 100..700, so its
        // top-left pixel carries the source's (400 % 256, 100) colour.
        let c = back.pixels[0];
        assert!((i32::from(c.r()) - 400 % 256).abs() <= 8, "{c:?}");
        assert!((i32::from(c.g()) - 100).abs() <= 8, "{c:?}");
        // The whole image works too, and an empty region gives nothing.
        assert!(thumbnail_png(&shot, None).is_some());
        assert!(thumbnail_png(&shot, Some([500.0, 500.0, 500.0, 900.0])).is_none());
        assert!(thumbnail_png(&shot, Some([2000.0, 0.0, 3000.0, 10.0])).is_none());
    }

    #[test]
    fn garbage_does_not_decode() {
        assert!(decode_png(b"not a png").is_none());
        let mut fake = rcad_cloud::client::PNG_SIGNATURE.to_vec();
        fake.extend_from_slice(b"truncated");
        assert!(decode_png(&fake).is_none());
    }
}
