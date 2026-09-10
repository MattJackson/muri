//! The in-house CPU raster surface and its drawing primitives (ADR-0001).
//!
//! muri paints a menu with a deliberately tiny set of raster operations —
//! anti-aliased rounded-rect fills, device-snapped hairline separators, and
//! straight-alpha bitmap blits — so it does not need a general-purpose 2D crate
//! (`tiny-skia`) behind it. This module owns:
//!
//! * [`Framebuffer`] — an owned, **premultiplied-RGBA** pixel buffer (byte order
//!   `R, G, B, A`, row-major). Premultiplied because that is what both live
//!   present paths want (macOS `CGImage` `PremultipliedLast`, Windows
//!   `UpdateLayeredWindow` premultiplied BGRA) and what alpha compositing over it
//!   produces.
//! * The primitives [`fill_round_rect`], [`fill_rect`], and [`blend_pixel`],
//!   plus PNG [`decode_png`] / [`Framebuffer::encode_png`] built on the lean
//!   `png` crate (the same codec `tiny-skia` used internally, now depended on
//!   directly so `tiny-skia`'s own crates drop out of the tree).
//!
//! Anti-aliasing uses an analytic rounded-box **signed distance field**: a
//! pixel's coverage is `clamp(0.5 - sdf, 0, 1)`, giving a ~1px edge band and
//! pixel-crisp axis-aligned edges (a pixel center exactly on an edge lands at
//! full/zero coverage). Glyph and image blits reuse the exact premultiplied
//! `over` arithmetic the previous `tiny-skia` path used, so only the AA fills
//! (panel body, row highlight) differ at all from the old backend.

use crate::geometry::LogicalRect;
use crate::style::Rgba;

/// An owned CPU raster surface: premultiplied straight-alpha RGBA (`R, G, B, A`
/// byte order), row-major, `width * height * 4` bytes. This is the buffer the
/// popup window blits to its surface and the headless snapshot suite encodes to
/// PNG.
#[derive(Clone)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    /// Premultiplied RGBA, row-major.
    data: Vec<u8>,
}

impl std::fmt::Debug for Framebuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Framebuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Framebuffer {
    /// A fully-transparent `width × height` surface.
    pub fn new(width: u32, height: u32) -> Self {
        let (w, h) = (width.max(1), height.max(1));
        Framebuffer {
            width: w,
            height: h,
            data: vec![0u8; (w as usize) * (h as usize) * 4],
        }
    }

    /// Width in device pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in device pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The premultiplied-RGBA pixel bytes (`width * height * 4`, row-major).
    /// This is what the present paths source from.
    pub fn pixels(&self) -> &[u8] {
        &self.data
    }

    /// Mutable access to the premultiplied-RGBA pixel bytes.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Fill the whole surface with an opaque-or-not straight-alpha color,
    /// premultiplying as it writes. Used for a solid backdrop (test composites).
    pub fn fill(&mut self, color: Rgba) {
        let a = color.a as u32;
        let (r, g, b) = (
            (color.r as u32 * a / 255) as u8,
            (color.g as u32 * a / 255) as u8,
            (color.b as u32 * a / 255) as u8,
        );
        for px in self.data.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = color.a;
        }
    }

    /// Alpha-composite `src` (premultiplied) over this surface with its top-left
    /// at device-pixel `(x, y)`. Used to composite a parent popup and its flyout
    /// onto one canvas (mirrors the live per-window blit).
    pub fn draw(&mut self, src: &Framebuffer, x: i32, y: i32) {
        let (dw, dh) = (self.width as i32, self.height as i32);
        let (sw, sh) = (src.width as i32, src.height as i32);
        for row in 0..sh {
            let dy = y + row;
            if dy < 0 || dy >= dh {
                continue;
            }
            for col in 0..sw {
                let dx = x + col;
                if dx < 0 || dx >= dw {
                    continue;
                }
                let s = ((row * sw + col) * 4) as usize;
                let sa = src.data[s + 3];
                if sa == 0 {
                    continue;
                }
                // `src` is premultiplied already; blend it over the destination
                // with a straight-`over` on premultiplied operands.
                let d = ((dy * dw + dx) * 4) as usize;
                let inv = 255 - sa as u32;
                for c in 0..4 {
                    let out = src.data[s + c] as u32 + self.data[d + c] as u32 * inv / 255;
                    self.data[d + c] = out.min(255) as u8;
                }
            }
        }
    }

    /// Straight-alpha (un-premultiplied) RGBA copy of the surface — what a PNG
    /// stores, and the space the golden suite compares in.
    pub fn to_straight_rgba(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.data.len()];
        for (dst, src) in out.chunks_exact_mut(4).zip(self.data.chunks_exact(4)) {
            let a = src[3];
            if a == 0 {
                continue;
            }
            let a32 = a as u32;
            dst[0] = (src[0] as u32 * 255 / a32).min(255) as u8;
            dst[1] = (src[1] as u32 * 255 / a32).min(255) as u8;
            dst[2] = (src[2] as u32 * 255 / a32).min(255) as u8;
            dst[3] = a;
        }
        out
    }

    /// Encode the surface as PNG bytes (straight-alpha RGBA8).
    pub fn encode_png(&self) -> Vec<u8> {
        let straight = self.to_straight_rgba();
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&straight).expect("png data");
        }
        out
    }
}

/// Encode straight-alpha RGBA8 pixels (row-major, 4 bytes per pixel) as PNG
/// bytes. Returns `None` when the dimensions are zero or `rgba.len()` does not
/// equal `width * height * 4`.
///
/// This is the bridge that lets the compat facade's raw-RGBA tray icon
/// ([`Icon::from_rgba`](crate::compat::muda::Icon::from_rgba)) reach muri's
/// encoded-bytes [`Icon::Png`](crate::menu::Icon::Png): the Linux SNI backend
/// (`icon_pixmap`) and the macOS/Windows image paths all consume encoded bytes,
/// so without this the facade icon never reaches the drawn tray (divergence D6)
/// and, on GNOME, the appindicator extension drops an item with an empty pixmap.
pub(crate) fn encode_rgba_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if rgba.len() != expected {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// Blend a straight-alpha source color, scaled by coverage `a`, over one
/// premultiplied destination pixel at byte offset `off`. This is the exact
/// `over` arithmetic the previous `tiny-skia`-backed path used for glyph and
/// image blits, so those outputs are unchanged.
#[inline]
pub(crate) fn blend_pixel(dst: &mut [u8], off: usize, src: Rgba, a: u8) {
    let sa = a as u32;
    let inv = 255 - sa;
    let sr = src.r as u32 * sa / 255;
    let sg = src.g as u32 * sa / 255;
    let sb = src.b as u32 * sa / 255;
    dst[off] = (sr + dst[off] as u32 * inv / 255).min(255) as u8;
    dst[off + 1] = (sg + dst[off + 1] as u32 * inv / 255).min(255) as u8;
    dst[off + 2] = (sb + dst[off + 2] as u32 * inv / 255).min(255) as u8;
    dst[off + 3] = (sa + dst[off + 3] as u32 * inv / 255).min(255) as u8;
}

/// Signed distance (in device pixels) from point `(px, py)` to a rounded rect
/// `[x, y, x+w, y+h]` with corner radius `r`; negative inside.
#[inline]
fn round_rect_sdf(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let qx = (px - cx).abs() - (w / 2.0 - r);
    let qy = (py - cy).abs() - (h / 2.0 - r);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    let outside = (ox * ox + oy * oy).sqrt();
    let inside = qx.max(qy).min(0.0);
    outside + inside - r
}

/// Fill an anti-aliased rounded rectangle (device pixels) into `fb` in the given
/// straight-alpha `color`. `r` is clamped to `min(w/2, h/2)`; `r ≈ 0` yields a
/// pixel-crisp rectangle with a 1px AA edge, matching the old fill's behavior.
pub(crate) fn fill_round_rect(
    fb: &mut Framebuffer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    color: Rgba,
) {
    if w <= 0.0 || h <= 0.0 || color.a == 0 {
        return;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let (fw, fh) = (fb.width as i32, fb.height as i32);
    // Bounding box, padded 1px for the AA band, clamped to the surface.
    let x0 = ((x.floor() as i32) - 1).clamp(0, fw);
    let y0 = ((y.floor() as i32) - 1).clamp(0, fh);
    let x1 = (((x + w).ceil() as i32) + 1).clamp(0, fw);
    let y1 = (((y + h).ceil() as i32) + 1).clamp(0, fh);
    let ca = color.a as f32;
    let pixels = fb.pixels_mut();
    for py in y0..y1 {
        for px in x0..x1 {
            let d = round_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, r);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let a = (ca * cov).round() as u8;
            if a == 0 {
                continue;
            }
            blend_pixel(pixels, ((py * fw + px) * 4) as usize, color, a);
        }
    }
}

/// Fill an aliased, device-snapped rectangle (device pixels rounded to the pixel
/// grid) — the hairline separator primitive. `color` is straight-alpha.
pub(crate) fn fill_rect(fb: &mut Framebuffer, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
    if color.a == 0 {
        return;
    }
    let (fw, fh) = (fb.width as i32, fb.height as i32);
    let x0 = (x.round() as i32).clamp(0, fw);
    let y0 = (y.round() as i32).clamp(0, fh);
    let x1 = ((x + w).round() as i32).clamp(0, fw);
    let y1 = ((y + h).round() as i32).clamp(0, fh);
    let pixels = fb.pixels_mut();
    for py in y0..y1 {
        for px in x0..x1 {
            blend_pixel(pixels, ((py * fw + px) * 4) as usize, color, color.a);
        }
    }
}

/// Hard cap on either PNG dimension `decode_png` will decode. muri only ever
/// decodes small menu/tray icons (16px logical, a handful of device pixels at
/// most even at very high DPI); anything beyond this is not a legitimate icon
/// and is refused *before* any pixel buffer is allocated, so a crafted PNG
/// header with huge IHDR dimensions (a malicious `Icon::Png` reachable from
/// Windows tray `HICON` decode, Linux SNI icons, and menu-row leading icons)
/// cannot force a multi-GB allocation (DoS).
const MAX_ICON_DIM: u32 = 4096;

/// Hard cap on total pixel count (`width * height`), checked alongside
/// [`MAX_ICON_DIM`] and set well below `MAX_ICON_DIM²` so a large-but-square
/// (or wide-but-short / tall-but-thin) image that individually satisfies the
/// per-dimension cap still can't force an excessive allocation — a real
/// menu/tray icon, even generously upscaled for a very high DPI, never
/// approaches this.
const MAX_ICON_PIXELS: u64 = 2048 * 2048;

/// Decode a raster PNG to straight-alpha RGBA (`R, G, B, A` per pixel), returning
/// `(rgba, width, height)` or `None` for non-PNG / undecodable bytes. Palette,
/// grayscale, and 16-bit inputs are normalized to 8-bit RGBA.
///
/// Dimensions are validated against internal per-dimension and total-pixel
/// caps (4096px per side; 2048×2048 pixels total) straight from the
/// IHDR-derived [`png::OutputInfo`] *before* any pixel buffer is allocated,
/// so oversized/malicious inputs are rejected cheaply.
pub fn decode_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let hdr = reader.info();
    let (hw, hh) = (hdr.width, hdr.height);
    if hw == 0 || hh == 0 || hw > MAX_ICON_DIM || hh > MAX_ICON_DIM {
        return None;
    }
    if (hw as u64) * (hh as u64) > MAX_ICON_PIXELS {
        return None;
    }
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let count = (w as usize).checked_mul(h as usize)?;
    let mut out = vec![0u8; count * 4];
    match info.color_type {
        png::ColorType::Rgba => {
            out.copy_from_slice(&buf[..count * 4]);
        }
        png::ColorType::Rgb => {
            for (dst, src) in out.chunks_exact_mut(4).zip(buf.chunks_exact(3)) {
                dst[0] = src[0];
                dst[1] = src[1];
                dst[2] = src[2];
                dst[3] = 255;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for (dst, src) in out.chunks_exact_mut(4).zip(buf.chunks_exact(2)) {
                dst[0] = src[0];
                dst[1] = src[0];
                dst[2] = src[0];
                dst[3] = src[1];
            }
        }
        png::ColorType::Grayscale => {
            for (dst, &g) in out.chunks_exact_mut(4).zip(buf.iter()) {
                dst[0] = g;
                dst[1] = g;
                dst[2] = g;
                dst[3] = 255;
            }
        }
        // `normalize_to_color8` expands Indexed to Rgb/Rgba, so it never reaches
        // here; treat anything unexpected as undecodable rather than panicking.
        png::ColorType::Indexed => return None,
    }
    Some((out, w, h))
}

/// Convenience: the device-pixel bounds of a logical rect scaled by `scale`,
/// returned as `(x, y, w, h)` in device pixels. Kept here so the drawer's scale
/// math has one home.
#[inline]
pub(crate) fn scaled(rect: LogicalRect, scale: f32) -> (f32, f32, f32, f32) {
    (
        rect.origin.x * scale,
        rect.origin.y * scale,
        rect.size.width * scale,
        rect.size.height * scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard PNG chunk CRC (CRC-32/ISO-HDLC over the chunk's type+data
    /// bytes), needed to hand-build a syntactically valid IHDR chunk for the
    /// oversize-dimension test below.
    fn png_crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// A syntactically valid PNG byte stream containing *only* a signature +
    /// IHDR chunk declaring `w × h` (no `IDAT`/`IEND`). `decode_png`'s
    /// dimension cap is checked right after `read_info()` parses the IHDR —
    /// before any pixel buffer is allocated — so this is sufficient to drive
    /// that check without needing a real (potentially huge) encoded image.
    fn png_header_with_dims(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']);
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"IHDR");
        chunk.extend_from_slice(&w.to_be_bytes());
        chunk.extend_from_slice(&h.to_be_bytes());
        chunk.push(8); // bit depth
        chunk.push(6); // color type: RGBA
        chunk.push(0); // compression method
        chunk.push(0); // filter method
        chunk.push(0); // interlace method
        out.extend_from_slice(&13u32.to_be_bytes()); // IHDR data length
        out.extend_from_slice(&chunk);
        out.extend_from_slice(&png_crc32(&chunk).to_be_bytes());
        out
    }

    #[test]
    fn decode_png_rejects_oversize_dimensions_before_allocating() {
        // Well past MAX_ICON_DIM; a naive `vec![0u8; w*h*4]` here would be a
        // multi-GB allocation (60_000² × 4 ≈ 14.4 GB) — the DoS this cap
        // exists to prevent.
        let bytes = png_header_with_dims(60_000, 60_000);
        assert!(decode_png(&bytes).is_none());
    }

    #[test]
    fn decode_png_rejects_a_square_image_over_the_pixel_cap_but_under_the_dim_cap() {
        // 3000x3000 is under MAX_ICON_DIM (4096) individually, but its pixel
        // count (9M) is well over MAX_ICON_PIXELS (2048² = ~4.2M) — this only
        // gets caught by the *pixel* cap, proving it's a real, independent
        // check and not just shadowed by the per-dimension one.
        let bytes = png_header_with_dims(3000, 3000);
        const { assert!((3000u64 * 3000) <= MAX_ICON_DIM as u64 * MAX_ICON_DIM as u64) };
        const { assert!((3000u64 * 3000) > MAX_ICON_PIXELS) };
        assert!(decode_png(&bytes).is_none());
    }

    #[test]
    fn decode_png_rejects_malformed_bytes() {
        assert!(decode_png(&[]).is_none());
        assert!(decode_png(b"not a png at all").is_none());
        assert!(decode_png(&[0x89, b'P', b'N', b'G']).is_none()); // truncated signature
    }

    #[test]
    fn decode_png_roundtrips_a_small_real_image() {
        let mut fb = Framebuffer::new(2, 2);
        fb.fill(Rgba::opaque(255, 0, 0));
        let bytes = fb.encode_png();
        let (rgba, w, h) = decode_png(&bytes).expect("a real, small PNG decodes");
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba.len(), 2 * 2 * 4);
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn encode_rgba_png_roundtrips_through_decode() {
        // A 2x2 straight-alpha image with distinct, partly-transparent pixels.
        let rgba = vec![
            255, 0, 0, 255, // opaque red
            0, 255, 0, 128, // half-alpha green
            0, 0, 255, 255, // opaque blue
            9, 8, 7, 0, // fully transparent
        ];
        let png = encode_rgba_png(&rgba, 2, 2).expect("valid RGBA encodes");
        let (decoded, w, h) = decode_png(&png).expect("the encoded PNG decodes");
        assert_eq!((w, h), (2, 2));
        // A fully-transparent pixel's color channels are not preserved by PNG's
        // straight-alpha storage in every codec path, so compare only the pixels
        // with alpha, plus every alpha channel.
        assert_eq!(&decoded[0..8], &rgba[0..8]);
        assert_eq!(&decoded[8..12], &rgba[8..12]);
        assert_eq!(decoded[15], 0, "the transparent pixel stays transparent");
    }

    #[test]
    fn encode_rgba_png_rejects_bad_dimensions_and_lengths() {
        // Zero dimensions.
        assert!(encode_rgba_png(&[0, 0, 0, 0], 0, 1).is_none());
        assert!(encode_rgba_png(&[0, 0, 0, 0], 1, 0).is_none());
        // Length mismatch: 1x1 needs 4 bytes, not 3.
        assert!(encode_rgba_png(&[0, 0, 0], 1, 1).is_none());
        // Length mismatch: 2x2 needs 16 bytes, not 4.
        assert!(encode_rgba_png(&[0, 0, 0, 0], 2, 2).is_none());
    }

    #[test]
    fn fill_round_rect_corners_are_more_transparent_than_center() {
        let mut fb = Framebuffer::new(20, 20);
        fill_round_rect(&mut fb, 2.0, 2.0, 16.0, 16.0, 6.0, Rgba::opaque(10, 20, 30));
        let alpha_at = |fb: &Framebuffer, x: u32, y: u32| -> u8 {
            fb.pixels()[((y * fb.width() + x) * 4 + 3) as usize]
        };
        let center = alpha_at(&fb, 10, 10);
        let corner = alpha_at(&fb, 2, 2); // the rect's bounding-box corner
        assert_eq!(center, 255, "well inside the rect should be fully opaque");
        assert!(
            corner < center,
            "a rounded corner should be more transparent than the center (corner={corner}, center={center})"
        );
    }

    #[test]
    fn fill_round_rect_clamps_radius_to_half_the_smaller_dimension() {
        // r (1000) is absurdly larger than min(w, h)/2 (= 2.0 for an 8x4
        // rect); the fill must clamp rather than produce a nonsensical/negative
        // effective radius or panic.
        let mut fb = Framebuffer::new(10, 10);
        fill_round_rect(&mut fb, 0.0, 0.0, 8.0, 4.0, 1000.0, Rgba::opaque(1, 2, 3));
        let idx = ((2 * fb.width() + 4) * 4 + 3) as usize;
        assert_eq!(
            fb.pixels()[idx],
            255,
            "center of the clamped fill is opaque"
        );
    }
}
