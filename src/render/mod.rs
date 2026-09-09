//! The scene-drawer interface and its one real implementation — the CPU raster
//! backend (`softbuffer` framebuffer target + `tiny-skia` 2D + `cosmic-text`
//! glyphs) described in the design. The same drawer paints the menu on macOS,
//! Windows, and Linux; per-OS code is confined to the anchoring/dismiss shims
//! (see [`crate::tray`]), never to drawing.
//!
//! [`SceneDrawer`] defines the boundary as a trait so the layout/paint code in
//! [`crate::render::paint`] can be written and tested against it independent of
//! any window. [`RasterDrawer`] is the concrete CPU backend; it renders into an
//! owned [`tiny_skia::Pixmap`] which the popup window blits to its `softbuffer`
//! surface (and which the headless snapshot test saves straight to PNG).

pub mod paint;

use std::cell::RefCell;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent,
    Weight as CtWeight,
};
use tiny_skia::{
    FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Transform as SkTransform,
};

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::style::{Font, FontFamily, Rgba, Weight};

/// A shaped run of text to blit, with its resolved color and weight. The layout
/// stage produces these from a [`Segment`](crate::Segment)'s text and style
/// runs; the drawer only has to rasterize glyphs at a position.
#[derive(Clone, Debug)]
pub struct TextRun<'a> {
    /// The substring to draw.
    pub text: &'a str,
    /// Top-left origin of the text box in logical pixels. (The drawer centers
    /// glyphs on their own line within the font's line height from here.)
    pub origin: LogicalPoint,
    /// The font to shape with.
    pub font: &'a Font,
    /// The resolved color.
    pub color: Rgba,
    /// The resolved weight (may override the font's weight for a style run).
    pub weight: Weight,
}

/// The drawing surface muri paints a menu onto. One implementation (the CPU
/// raster [`RasterDrawer`]) satisfies this on every OS.
///
/// The method set is intentionally tiny — a menu is rounded rects, hairlines,
/// text runs, and bitmap icons. Coordinates are logical pixels; the
/// implementation owns DPI scaling.
pub trait SceneDrawer {
    /// Begin a frame of the given logical size, clearing to fully transparent so
    /// the rounded-rect panel corners show through.
    fn begin_frame(&mut self, size: LogicalSize);

    /// Fill a rounded rectangle (the popup body, a row highlight).
    fn fill_round_rect(&mut self, rect: LogicalRect, corner_radius: f32, color: Rgba);

    /// Stroke a 1px hairline separator across the given rectangle.
    fn draw_separator(&mut self, rect: LogicalRect, color: Rgba);

    /// Measure the intrinsic width of a string in the given font (logical px).
    /// Feeds [`crate::layout::resolve_segments`].
    fn measure_text(&self, text: &str, font: &Font) -> f32;

    /// The line height of the given font in logical pixels.
    fn line_height(&self, font: &Font) -> f32;

    /// Draw a shaped text run.
    fn draw_text(&mut self, run: &TextRun<'_>);

    /// Draw a pre-decoded, straight-alpha RGBA bitmap (an icon/logo) scaled to
    /// fit `dest`. `rgba` is `src_w * src_h * 4` bytes, row-major.
    fn draw_image(&mut self, rgba: &[u8], src_w: u32, src_h: u32, dest: LogicalRect);
}

/// The CPU raster backend: `tiny-skia` shapes/fills into an owned [`Pixmap`] and
/// `cosmic-text` shapes/rasterizes glyphs blitted onto it. This is the shared
/// scene drawer referenced throughout the design.
///
/// Create it with [`RasterDrawer::new`] (at an integer-ish device scale, e.g.
/// `2.0` for Retina), call the [`SceneDrawer`] methods (usually via
/// [`paint::render_menu`]), then read [`RasterDrawer::pixmap`] to present or
/// save the result.
pub struct RasterDrawer {
    scale: f32,
    pixmap: Pixmap,
    font_system: RefCell<FontSystem>,
    swash_cache: RefCell<SwashCache>,
    /// The concrete family name the generic "system" font resolves to, pinned
    /// once at construction. Using an explicit family (rather than the generic
    /// `Family::SansSerif`) for *every* weight keeps a bold section header and a
    /// regular row in the **same typeface** — otherwise `fontdb` can match the
    /// bold request to a different family's bold face than the regular one, which
    /// is the "different face for bold" glitch the Phase 1 report flagged.
    ui_family: Option<String>,
}

impl std::fmt::Debug for RasterDrawer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RasterDrawer")
            .field("scale", &self.scale)
            .field("width", &self.pixmap.width())
            .field("height", &self.pixmap.height())
            .finish_non_exhaustive()
    }
}

impl RasterDrawer {
    /// Create a raster drawer at the given device scale factor (device pixels per
    /// logical pixel). The backing pixmap starts at 1×1 and is reallocated by
    /// [`SceneDrawer::begin_frame`].
    pub fn new(scale: f32) -> Self {
        let mut font_system = FontSystem::new();
        let ui_family = resolve_ui_family(&mut font_system);
        RasterDrawer {
            scale: scale.max(0.1),
            pixmap: Pixmap::new(1, 1).expect("1x1 pixmap"),
            font_system: RefCell::new(font_system),
            swash_cache: RefCell::new(SwashCache::new()),
            ui_family,
        }
    }

    /// The device scale factor this drawer rasterizes at.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The rendered pixmap (device pixels). Valid after a
    /// [`SceneDrawer::begin_frame`]/paint pass.
    pub fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }

    /// Device pixel dimensions of the current frame.
    pub fn device_size(&self) -> (u32, u32) {
        (self.pixmap.width(), self.pixmap.height())
    }

    /// Encode the current frame as PNG bytes (requires a completed paint pass).
    pub fn encode_png(&self) -> Vec<u8> {
        self.pixmap.encode_png().expect("encode png")
    }

    fn metrics(&self, font: &Font, device: bool) -> Metrics {
        let s = if device { self.scale } else { 1.0 };
        let px = font.size * s;
        Metrics::new(px, px * 1.3)
    }

    fn attrs<'a>(&'a self, font: &'a Font, weight: Weight) -> Attrs<'a> {
        let family = match &font.family {
            // Pin the generic "system" font to a concrete family so bold and
            // regular text always come from the same typeface (see `ui_family`).
            FontFamily::System => match &self.ui_family {
                Some(name) => Family::Name(name),
                None => Family::SansSerif,
            },
            FontFamily::SystemMono => Family::Monospace,
            FontFamily::Named(name) => Family::Name(name),
        };
        Attrs::new()
            .family(family)
            .weight(CtWeight(weight.ot_weight()))
    }
}

/// Resolve a concrete UI font family whose **regular and bold both actually
/// shape within that same family**, so a bold section header and a regular row
/// read as one typeface at two weights.
///
/// The generic `Family::SansSerif` is deliberately avoided, and a name-only
/// lookup isn't enough: on macOS the native `.SF NS` (San Francisco) resolves
/// for regular but has no static bold face `cosmic-text` will match, so a bold
/// request silently falls back to **Menlo** (a monospace) — the exact "different
/// face for bold" glitch the Phase 1 report flagged. So each candidate is
/// verified by *actually shaping* both weights and confirming every glyph stays
/// in-family; the first that passes wins (Helvetica Neue on stock macOS).
fn resolve_ui_family(fs: &mut FontSystem) -> Option<String> {
    for cand in [
        ".SF NS",
        "SF Pro Text",
        "SF Pro",
        "Segoe UI",
        "Helvetica Neue",
        "Arial",
        "DejaVu Sans",
        "Liberation Sans",
    ] {
        if family_shapes_both_weights(fs, cand) {
            return Some(cand.to_string());
        }
    }
    None
}

/// Whether `name` shapes both a regular and a bold sample entirely within its own
/// family (i.e. `cosmic-text` doesn't substitute a fallback face for either).
fn family_shapes_both_weights(fs: &mut FontSystem, name: &str) -> bool {
    for weight in [CtWeight(400), CtWeight(700)] {
        let mut buffer = Buffer::new(fs, Metrics::new(13.0, 16.0));
        buffer.set_size(fs, None, None);
        let attrs = Attrs::new().family(Family::Name(name)).weight(weight);
        buffer.set_text(fs, "Agy0", attrs, Shaping::Advanced);
        buffer.shape_until_scroll(fs, false);
        let mut saw_glyph = false;
        let all_in_family = buffer.layout_runs().all(|run| {
            run.glyphs.iter().all(|g| {
                saw_glyph = true;
                fs.db()
                    .face(g.font_id)
                    .map(|f| f.families.iter().any(|(n, _)| n == name))
                    .unwrap_or(false)
            })
        });
        if !saw_glyph || !all_in_family {
            return false;
        }
    }
    true
}

fn sk_color(c: Rgba) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a)
}

impl SceneDrawer for RasterDrawer {
    fn begin_frame(&mut self, size: LogicalSize) {
        let w = ((size.width * self.scale).round() as u32).max(1);
        let h = ((size.height * self.scale).round() as u32).max(1);
        let mut pm = Pixmap::new(w, h).expect("allocate pixmap");
        pm.fill(tiny_skia::Color::TRANSPARENT);
        self.pixmap = pm;
    }

    fn fill_round_rect(&mut self, rect: LogicalRect, corner_radius: f32, color: Rgba) {
        let s = self.scale;
        let x = rect.origin.x * s;
        let y = rect.origin.y * s;
        let w = rect.size.width * s;
        let h = rect.size.height * s;
        let r = (corner_radius * s).min(w / 2.0).min(h / 2.0).max(0.0);
        let mut pb = PathBuilder::new();
        if r <= 0.25 {
            pb.push_rect(Rect::from_xywh(x, y, w, h).expect("rect"));
        } else {
            // Rounded rect via four quadratic corners.
            let (l, t, rt, b) = (x, y, x + w, y + h);
            pb.move_to(l + r, t);
            pb.line_to(rt - r, t);
            pb.quad_to(rt, t, rt, t + r);
            pb.line_to(rt, b - r);
            pb.quad_to(rt, b, rt - r, b);
            pb.line_to(l + r, b);
            pb.quad_to(l, b, l, b - r);
            pb.line_to(l, t + r);
            pb.quad_to(l, t, l + r, t);
            pb.close();
        }
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(sk_color(color));
            paint.anti_alias = true;
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                SkTransform::identity(),
                None,
            );
        }
    }

    fn draw_separator(&mut self, rect: LogicalRect, color: Rgba) {
        let s = self.scale;
        let x = rect.origin.x * s;
        let y = (rect.origin.y * s).round();
        let w = rect.size.width * s;
        let h = (rect.size.height * s).max(1.0);
        if let Some(r) = Rect::from_xywh(x, y, w, h) {
            let mut paint = Paint::default();
            paint.set_color(sk_color(color));
            self.pixmap
                .fill_rect(r, &paint, SkTransform::identity(), None);
        }
    }

    fn measure_text(&self, text: &str, font: &Font) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let mut fs = self.font_system.borrow_mut();
        let metrics = self.metrics(font, false);
        let mut buffer = Buffer::new(&mut fs, metrics);
        buffer.set_size(&mut fs, None, None);
        let attrs = self.attrs(font, font.weight);
        buffer.set_text(&mut fs, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut fs, false);
        buffer
            .layout_runs()
            .map(|r| r.line_w)
            .fold(0.0_f32, f32::max)
    }

    fn line_height(&self, font: &Font) -> f32 {
        self.metrics(font, false).line_height
    }

    fn draw_text(&mut self, run: &TextRun<'_>) {
        if run.text.is_empty() {
            return;
        }
        let scale = self.scale;
        let metrics = self.metrics(run.font, true);
        let ox = run.origin.x * scale;
        let oy = run.origin.y * scale;

        let mut fs = self.font_system.borrow_mut();
        let mut cache = self.swash_cache.borrow_mut();
        let mut buffer = Buffer::new(&mut fs, metrics);
        buffer.set_size(&mut fs, None, None);
        let attrs = self.attrs(run.font, run.weight);
        buffer.set_text(&mut fs, run.text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut fs, false);

        let color = run.color;
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let pixels = self.pixmap.pixels_mut();

        for layout_run in buffer.layout_runs() {
            let baseline = layout_run.line_y;
            for glyph in layout_run.glyphs.iter() {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                let Some(image) = cache.get_image(&mut fs, physical.cache_key).as_ref() else {
                    continue;
                };
                if image.placement.width == 0 || image.placement.height == 0 {
                    continue;
                }
                let gx = ox as i32 + physical.x + image.placement.left;
                let gy = oy as i32 + baseline as i32 + physical.y - image.placement.top;
                match image.content {
                    SwashContent::Mask | SwashContent::SubpixelMask => {
                        let iw = image.placement.width as i32;
                        let ih = image.placement.height as i32;
                        for row in 0..ih {
                            for col in 0..iw {
                                let cov = image.data[(row * iw + col) as usize];
                                if cov == 0 {
                                    continue;
                                }
                                let px = gx + col;
                                let py = gy + row;
                                if px < 0 || py < 0 || px >= pw || py >= ph {
                                    continue;
                                }
                                let a = (cov as u16 * color.a as u16 / 255) as u8;
                                blend_over(&mut pixels[(py * pw + px) as usize], color, a);
                            }
                        }
                    }
                    SwashContent::Color => {
                        let iw = image.placement.width as i32;
                        let ih = image.placement.height as i32;
                        for row in 0..ih {
                            for col in 0..iw {
                                let idx = ((row * iw + col) * 4) as usize;
                                let (r, g, b, a) = (
                                    image.data[idx],
                                    image.data[idx + 1],
                                    image.data[idx + 2],
                                    image.data[idx + 3],
                                );
                                if a == 0 {
                                    continue;
                                }
                                let px = gx + col;
                                let py = gy + row;
                                if px < 0 || py < 0 || px >= pw || py >= ph {
                                    continue;
                                }
                                blend_over(
                                    &mut pixels[(py * pw + px) as usize],
                                    Rgba::new(r, g, b, 255),
                                    a,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn draw_image(&mut self, rgba: &[u8], src_w: u32, src_h: u32, dest: LogicalRect) {
        if src_w == 0 || src_h == 0 {
            return;
        }
        let s = self.scale;
        let dx = (dest.origin.x * s).round() as i32;
        let dy = (dest.origin.y * s).round() as i32;
        let dw = (dest.size.width * s).round().max(1.0) as i32;
        let dh = (dest.size.height * s).round().max(1.0) as i32;
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let pixels = self.pixmap.pixels_mut();
        for row in 0..dh {
            for col in 0..dw {
                // Nearest-neighbour sample from the source bitmap.
                let sx = (col * src_w as i32 / dw).clamp(0, src_w as i32 - 1);
                let sy = (row * src_h as i32 / dh).clamp(0, src_h as i32 - 1);
                let idx = ((sy * src_w as i32 + sx) * 4) as usize;
                if idx + 3 >= rgba.len() {
                    continue;
                }
                let a = rgba[idx + 3];
                if a == 0 {
                    continue;
                }
                let px = dx + col;
                let py = dy + row;
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    continue;
                }
                blend_over(
                    &mut pixels[(py * pw + px) as usize],
                    Rgba::new(rgba[idx], rgba[idx + 1], rgba[idx + 2], 255),
                    a,
                );
            }
        }
    }
}

/// Alpha-blend a straight-alpha source color (with coverage `a`) over a
/// premultiplied destination pixel.
fn blend_over(dst: &mut PremultipliedColorU8, src: Rgba, a: u8) {
    let sa = a as u32;
    let inv = 255 - sa;
    // Source, premultiplied by its coverage.
    let sr = src.r as u32 * sa / 255;
    let sg = src.g as u32 * sa / 255;
    let sb = src.b as u32 * sa / 255;
    let out_r = (sr + dst.red() as u32 * inv / 255).min(255) as u8;
    let out_g = (sg + dst.green() as u32 * inv / 255).min(255) as u8;
    let out_b = (sb + dst.blue() as u32 * inv / 255).min(255) as u8;
    let out_a = (sa + dst.alpha() as u32 * inv / 255).min(255) as u8;
    *dst = PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a)
        .unwrap_or_else(|| PremultipliedColorU8::from_rgba(0, 0, 0, 0).unwrap());
}
