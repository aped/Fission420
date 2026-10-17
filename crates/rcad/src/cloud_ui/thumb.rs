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

/// Largest thumbnail width or height accepted for display, in pixels.
pub const MAX_DECODE_PX: u32 = 1024;
/// Most memory the PNG decoder may allocate for one thumbnail.
const MAX_DECODE_ALLOC: u64 = 8 * 1024 * 1024;

/// Width and height from a PNG's IHDR chunk (it must come first), read before
/// anything is decoded.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if !png.starts_with(rcad_cloud::client::PNG_SIGNATURE) || png.get(12..16)? != b"IHDR" {
        return None;
    }
    let be = |r: std::ops::Range<usize>| -> Option<u32> {
        Some(u32::from_be_bytes(png.get(r)?.try_into().ok()?))
    };
    Some((be(16..20)?, be(20..24)?))
}

/// Decode a thumbnail for display. None for anything that is not a PNG, is
/// larger than [`MAX_DECODE_PX`] either way (checked in the header before
/// decoding), or would need more than a few MiB to decode.
pub fn decode_png(png: &[u8]) -> Option<egui::ColorImage> {
    rcad_cloud::client::check_thumbnail(png).ok()?;
    let (w, h) = png_dimensions(png)?;
    if w == 0 || h == 0 || w > MAX_DECODE_PX || h > MAX_DECODE_PX {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_PX);
    limits.max_image_height = Some(MAX_DECODE_PX);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

/// Decodes thumbnails on a background thread, so a slow or hostile PNG never
/// stalls the UI.
pub struct Decoder {
    tx: std::sync::mpsc::Sender<(super::ThumbKey, Vec<u8>)>,
    rx: std::sync::mpsc::Receiver<(super::ThumbKey, Option<egui::ColorImage>)>,
    pending: usize,
}

impl Decoder {
    /// Start the decoding thread. If it cannot be started, thumbnails are
    /// simply not shown.
    pub fn start() -> Decoder {
        let (tx, jobs) = std::sync::mpsc::channel::<(super::ThumbKey, Vec<u8>)>();
        let (done, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("rcad-thumbnails".into())
            .spawn(move || {
                for (key, png) in jobs {
                    if done.send((key, decode_png(&png))).is_err() {
                        break;
                    }
                }
            });
        Decoder { tx, rx, pending: 0 }
    }

    pub fn submit(&mut self, key: super::ThumbKey, png: Vec<u8>) {
        if self.tx.send((key, png)).is_ok() {
            self.pending += 1;
        }
    }

    /// Images decoded since the last call (None: not a usable PNG).
    pub fn finished(&mut self) -> Vec<(super::ThumbKey, Option<egui::ColorImage>)> {
        let out: Vec<_> = self.rx.try_iter().collect();
        self.pending = self.pending.saturating_sub(out.len());
        out
    }

    /// Thumbnails submitted and not finished yet.
    pub fn pending(&self) -> usize {
        self.pending
    }
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

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        let img = image::GrayImage::new(w, h);
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn oversized_thumbnails_are_refused_before_decoding() {
        // A 4096x4096 grey PNG is a few KB on the wire and 64 MiB as a texture.
        let big = png_of(4096, 4096);
        assert!(big.len() < 100_000, "{}", big.len());
        assert_eq!(png_dimensions(&big), Some((4096, 4096)));
        assert!(decode_png(&big).is_none());
        assert!(decode_png(&png_of(1025, 16)).is_none());
        assert!(decode_png(&png_of(16, 1025)).is_none());
        let ok = decode_png(&png_of(1024, 256)).unwrap();
        assert_eq!(ok.size, [1024, 256]);
        // A header that lies about its size is caught by the decoder's limits.
        let mut liar = png_of(64, 64);
        liar[16..20].copy_from_slice(&2000u32.to_be_bytes());
        assert!(decode_png(&liar).is_none());
    }

    #[test]
    fn the_decoder_thread_hands_back_images() {
        let mut d = Decoder::start();
        d.submit(("d".into(), 1), png_of(8, 8));
        d.submit(("d".into(), 2), b"junk".to_vec());
        let mut got = Vec::new();
        let start = std::time::Instant::now();
        while got.len() < 2 {
            got.extend(d.finished());
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(d.pending(), 0);
        assert!(got.iter().any(|(k, i)| k.1 == 1 && i.is_some()));
        assert!(got.iter().any(|(k, i)| k.1 == 2 && i.is_none()));
    }

    #[test]
    fn garbage_does_not_decode() {
        assert!(decode_png(b"not a png").is_none());
        let mut fake = rcad_cloud::client::PNG_SIGNATURE.to_vec();
        fake.extend_from_slice(b"truncated");
        assert!(decode_png(&fake).is_none());
    }
}
