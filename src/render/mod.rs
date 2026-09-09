//! The scene-drawer interface: the single, platform-agnostic drawing API the
//! menu is painted through. Exactly one real implementation is planned — the
//! CPU raster backend (`softbuffer` framebuffer + `tiny-skia` 2D + `cosmic-text`
//! glyphs) described in the design — and it is the same on macOS, Windows, and
//! Linux. Per-OS code is confined to the anchoring/dismiss shims (see
//! [`crate::tray`]), never to drawing.
//!
//! This module defines the boundary as a trait so the layout/paint code can be
//! written and tested against it before the raster backend lands. The backend
//! bodies are still `todo!()` where the framebuffer/glyph work goes.

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::style::{Font, Rgba, Weight};

/// A shaped run of text to blit, with its resolved color and weight. The layout
/// stage produces these from a [`Segment`](crate::Segment)'s text and style
/// runs; the drawer only has to rasterize glyphs at a position.
#[derive(Clone, Debug)]
pub struct TextRun<'a> {
    /// The substring to draw.
    pub text: &'a str,
    /// Baseline-left origin in logical pixels.
    pub origin: LogicalPoint,
    /// The font to shape with.
    pub font: &'a Font,
    /// The resolved color.
    pub color: Rgba,
    /// The resolved weight (may override the font's weight for a style run).
    pub weight: Weight,
}

/// A pre-decoded icon handed to the drawer. The data model's
/// [`Icon`](crate::Icon) is resolved to one of these by the backend (PNG decoded
/// once, SVG rasterized per-DPI) before drawing.
#[derive(Clone, Copy, Debug)]
pub enum IconKind {
    /// An RGBA bitmap that the backend has already prepared.
    Bitmap,
    /// The themed checkmark glyph.
    Checkmark,
}

/// The drawing surface muri paints a menu onto. One implementation (the CPU
/// raster backend) satisfies this on every OS.
///
/// The method set is intentionally tiny — a menu is rounded rects, hairlines,
/// text runs, and icons. Coordinates are logical pixels; the implementation
/// owns DPI scaling.
pub trait SceneDrawer {
    /// Begin a frame of the given logical size, clearing to `background`.
    fn begin_frame(&mut self, size: LogicalSize, background: Rgba);

    /// Fill a rounded rectangle (the popup body, a row highlight).
    fn fill_round_rect(&mut self, rect: LogicalRect, corner_radius: f32, color: Rgba);

    /// Stroke a 1px hairline separator across the given rectangle.
    fn draw_separator(&mut self, rect: LogicalRect, color: Rgba);

    /// Measure the intrinsic width of a string in the given font (logical px).
    /// Feeds [`crate::layout::resolve_segments`].
    fn measure_text(&self, text: &str, font: &Font) -> f32;

    /// Draw a shaped text run.
    fn draw_text(&mut self, run: &TextRun<'_>);

    /// Draw an icon within the given rectangle.
    fn draw_icon(&mut self, kind: IconKind, rect: LogicalRect, tint: Rgba);

    /// Present the finished frame to the window/surface.
    fn present(&mut self);
}

/// The planned CPU raster backend (`softbuffer` + `tiny-skia` + `cosmic-text`).
///
/// This is the shared scene drawer referenced throughout the design; its bodies
/// are `todo!()` until the rendering phase lands. It is defined now so the
/// surrounding layout/paint code can be written against [`SceneDrawer`].
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct RasterDrawer;

impl RasterDrawer {
    /// Create the raster drawer. (Real construction will own a `tiny-skia`
    /// `Pixmap` and a `cosmic-text` `FontSystem`.)
    pub fn new() -> Self {
        RasterDrawer
    }
}

impl SceneDrawer for RasterDrawer {
    fn begin_frame(&mut self, _size: LogicalSize, _background: Rgba) {
        todo!("tiny-skia pixmap allocation + clear — see the muri design doc roadmap")
    }

    fn fill_round_rect(&mut self, _rect: LogicalRect, _corner_radius: f32, _color: Rgba) {
        todo!("tiny-skia rounded-rect fill")
    }

    fn draw_separator(&mut self, _rect: LogicalRect, _color: Rgba) {
        todo!("tiny-skia hairline")
    }

    fn measure_text(&self, _text: &str, _font: &Font) -> f32 {
        todo!("cosmic-text measurement")
    }

    fn draw_text(&mut self, _run: &TextRun<'_>) {
        todo!("cosmic-text shaping + glyph blit")
    }

    fn draw_icon(&mut self, _kind: IconKind, _rect: LogicalRect, _tint: Rgba) {
        todo!("PNG decode / SVG rasterize + blit")
    }

    fn present(&mut self) {
        todo!("softbuffer present")
    }
}
