//! The scene-drawer interface and its one real implementation — the CPU raster
//! backend (a muri-owned raster blitter for fills/blits, plus a muri-owned text
//! layer built directly on the `fontdb` / `harfrust` / `swash` engines)
//! described in the design. The same drawer paints the menu on macOS, Windows,
//! and Linux; per-OS code is confined to the anchoring/dismiss shims (see
//! [`crate::platform`]), never to drawing.
//!
//! [`SceneDrawer`] defines the boundary as a trait so the layout/paint code in
//! [`crate::render::paint`] can be written and tested against it independent of
//! any window. [`RasterDrawer`] is the concrete CPU backend; it renders into an
//! owned [`Framebuffer`] (premultiplied RGBA) which the popup window blits to
//! its surface (and which the headless snapshot test saves straight to PNG).
//!
//! ## The text layer (ADR-0001)
//!
//! `cosmic-text` used to provide font lookup, shaping, rasterization, *and*
//! cross-script fallback behind one `FontSystem`/`Buffer`/`SwashCache` facade.
//! Per [ADR-0001](../../../docs/design/adr/0001-own-the-menu-use-engines-not-frameworks.md)
//! muri now owns that glue and uses the deep engines directly:
//!
//! * [`fontdb::Database`] — the font database + CSS-like family/weight queries.
//! * [`harfrust`] — the HarfBuzz-project's Rust shaper (the maintained
//!   successor to `rustybuzz`) that shapes a run of text against one face
//!   (clusters, kerning, ligatures, complex-script glyphs within a single run).
//! * [`swash`] — the glyph rasterizer (outline and color-emoji), producing the
//!   same `Content::{Mask, SubpixelMask, Color}` coverage muri blits with its
//!   own `raster::blend_pixel`.
//!
//! Everything between those engines — the pinned-UI-family resolver ([§7.1]),
//! the deterministic per-codepoint **font-fallback** layer (`FontStore::segment_faces`),
//! and the persistent rasterized-glyph cache that replaces `SwashCache` — lives
//! in this module. Scope is **LTR + per-run color/weight + complex-script glyphs
//! within a single unstyled run**; bidi/RTL reordering is documented out
//! (spec `10-rendering-layout.md` §7.2).
//!
//! [§7.1]: ../../../docs/design/spec/10-rendering-layout.md

pub mod paint;
// Not `pub`: this module's blit primitives (`blend_pixel`, `fill_round_rect`,
// `fill_rect`, `scaled`) are internal-only implementation details with no
// external caller (platform backends + `paint` are all in-crate). Only
// `decode_png`/`Framebuffer` are muri's actual public raster surface, and they
// stay reachable via the `pub use` re-export just below regardless of this
// module's own visibility.
pub(crate) mod raster;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use fontdb::{
    Database, Family as DbFamily, Query, Source as DbSource, Style as DbStyle, Weight as DbWeight,
    ID as FaceId,
};
use harfrust::{FontRef as HbFontRef, ShapeOptions, ShaperData, UnicodeBuffer};
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::{FontRef, GlyphId};

pub use raster::{decode_png, Framebuffer};
// Crate-internal only: the compat facade encodes its raw-RGBA tray icon to PNG
// through this. Not part of muri's public raster surface (unlike `decode_png`,
// which platform backends call on caller-supplied icon bytes).
pub(crate) use raster::encode_rgba_png;

/// A decoded PNG icon: straight-alpha RGBA bytes plus its `(width, height)`.
type DecodedIcon = Rc<(Vec<u8>, u32, u32)>;

/// Cache key for a shaped run: the text, the resolved primary face, the OpenType
/// weight, and the device pixel size (as raw `f32` bits for exact equality).
type ShapeKey = (String, Option<FaceId>, u16, u32);

/// A [`RasterDrawer::icons`] cache entry: the decoded icon plus a strong
/// clone of the source `Arc<[u8]>` it was decoded from (see the field doc for
/// why retaining that `Arc` is load-bearing, not incidental).
type IconCacheEntry = (Arc<[u8]>, DecodedIcon);

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::platform::{Platform, SystemFont, SystemFontSource};
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

    /// Decode a PNG icon (straight-alpha RGBA + dimensions), reusing a cached
    /// decode when `bytes` is the *same* `Arc` (pointer identity, not content)
    /// as a previous call on this drawer. `Icon::Png` bytes live in an
    /// `Arc<[u8]>` precisely so a menu re-rendered every frame (the live popup
    /// repaints on hover) doesn't re-run the PNG decoder on unchanged icon
    /// bytes each time.
    ///
    /// The default implementation just decodes uncached (correct, only
    /// non-caching); [`RasterDrawer`] overrides it with a real cache.
    fn decode_icon(&self, bytes: &Arc<[u8]>) -> Option<DecodedIcon> {
        raster::decode_png(bytes).map(Rc::new)
    }
}

/// The line-height multiple applied to a font's point size, matching the metric
/// the previous `cosmic-text` path used (`Metrics::new(px, px * 1.3)`).
const LINE_HEIGHT_FACTOR: f32 = 1.3;

/// Prioritized fallback families tried (in order) when the pinned face lacks a
/// glyph for a codepoint, before scanning the whole database. Deliberately
/// cross-platform: emoji, then symbols, then broad CJK coverage. Only families
/// actually present in the db can match; missing ones are skipped cheaply.
const FALLBACK_FAMILIES: &[&str] = &[
    // Color emoji.
    "Apple Color Emoji",
    "Segoe UI Emoji",
    "Noto Color Emoji",
    // Symbol / dingbat coverage.
    "Apple Symbols",
    "Segoe UI Symbol",
    "Symbola",
    // CJK (macOS / Windows / Noto).
    "PingFang SC",
    "Hiragino Sans",
    "Hiragino Sans GB",
    "Microsoft YaHei",
    "Yu Gothic",
    "Malgun Gothic",
    "Noto Sans CJK SC",
    "Noto Sans SC",
    "Noto Sans",
    // Broad Unicode catch-all.
    "Arial Unicode MS",
];

/// The CPU raster backend: a muri-owned blitter fills into an owned
/// [`Framebuffer`] and a muri-owned text layer (`fontdb` + `harfrust` + `swash`)
/// shapes and rasterizes glyphs blitted onto it. This is the shared scene drawer
/// referenced throughout the design.
///
/// Create it with [`RasterDrawer::new`] (at an integer-ish device scale, e.g.
/// `2.0` for Retina), call the [`SceneDrawer`] methods (usually via
/// [`paint::render_menu`]), then read [`RasterDrawer::framebuffer`] to present or
/// save the result.
pub struct RasterDrawer {
    scale: f32,
    fb: Framebuffer,
    fonts: FontStore,
    /// Decoded-PNG-icon cache, keyed by the source `Arc<[u8]>`'s pointer
    /// identity (see [`SceneDrawer::decode_icon`]). Bounded by
    /// [`ICON_CACHE_CAP`] so a very long-lived drawer fed many distinct icon
    /// byte buffers over its lifetime can't grow this without bound.
    ///
    /// Each entry also retains a strong clone of the `Arc<[u8]>` it was keyed
    /// from. This is load-bearing, not incidental: a bare pointer (or
    /// pointer+len) is not a stable identity for a dropped allocation — once
    /// the caller's `Arc` is dropped, a later, *different* icon byte buffer
    /// can be allocated at the exact same address (and even the same length),
    /// producing a false cache hit that draws the wrong icon (ABA). Holding
    /// the `Arc` alive for as long as the entry is cached makes that address
    /// un-reusable while the entry exists, so a pointer match is only ever a
    /// match against the *same* live allocation.
    icons: RefCell<HashMap<usize, IconCacheEntry>>,
}

/// Cap on [`RasterDrawer::icons`]'s entry count. A menu's icon set is tiny
/// (a handful of tray/menu-row icons at most), so this is generous headroom;
/// on overflow the whole cache is cleared rather than tracking per-entry
/// recency (a normal menu never gets close to this, so eviction quality
/// doesn't matter in practice).
const ICON_CACHE_CAP: usize = 64;

impl std::fmt::Debug for RasterDrawer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RasterDrawer")
            .field("scale", &self.scale)
            .field("width", &self.fb.width())
            .field("height", &self.fb.height())
            .finish_non_exhaustive()
    }
}

impl RasterDrawer {
    /// Create a raster drawer at the given device scale factor (device pixels per
    /// logical pixel), backed by the **host's installed fonts**
    /// ([`fontdb::Database::load_system_fonts`]). The backing pixmap starts at
    /// 1×1 and is reallocated by [`SceneDrawer::begin_frame`].
    pub fn new(scale: f32) -> Self {
        Self::with_system_font(scale, None)
    }

    /// Like [`RasterDrawer::new`], but pins [`FontFamily::System`] to the host's
    /// **native menu font** from the per-OS
    /// [`Platform::system_menu_font`](crate::platform::Platform::system_menu_font)
    /// — SF Pro on macOS, Segoe UI on Windows, the GNOME UI family on Linux (#10).
    /// The live popup panels use this so the menu renders in the true OS UI face,
    /// which `fontdb`'s generic discovery can't reach on macOS. Falls back to
    /// installed-font discovery when the platform returns `None` or the face
    /// can't be registered — so it never regresses [`RasterDrawer::new`].
    pub fn new_native(scale: f32) -> Self {
        Self::with_system_font(scale, crate::platform::current().system_menu_font())
    }

    /// Construct a drawer, optionally pinning `FontFamily::System` to a supplied
    /// system font (its face registered into the database). `None` — or a face
    /// that fails to register — falls back to installed-font discovery.
    pub fn with_system_font(scale: f32, system: Option<SystemFont>) -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        let ui_family = system
            .and_then(|sf| register_system_font(&mut db, sf.source))
            .or_else(|| resolve_ui_family(&db));
        Self::from_parts(scale, db, ui_family)
    }

    /// Create a raster drawer whose text is shaped **only** against a
    /// repo-vendored font (`tests/fonts/DejaVuSans{,-Bold}.ttf`), with system
    /// font discovery disabled entirely — no
    /// [`fontdb::Database::load_system_fonts`] call, ever.
    ///
    /// This is the determinism seam the golden-image snapshot suite needs (the
    /// design spec's testing-verification doc, §3.3, "font selection"):
    /// [`RasterDrawer::new`] deliberately resolves to *whatever UI font the host
    /// OS has installed* (San Francisco on macOS, Segoe UI on Windows, DejaVu/
    /// Liberation on Linux) so the live popup looks native — which also means a
    /// golden PNG rendered with `new` can never match across all three CI
    /// runners. `new_headless` instead builds an empty [`fontdb::Database`]
    /// populated with exactly the two vendored DejaVu Sans faces (regular + bold,
    /// same family so a bold section header and a regular row still read as one
    /// typeface), so glyph outlines and metrics are byte-identical on every OS
    /// and every developer machine.
    ///
    /// Intentionally **not** used by [`RasterDrawer::new`] / the live backends —
    /// this constructor exists for the headless snapshot suite (`tests/golden.rs`,
    /// `tests/fonts/`) only; it does not change the live, native-font-matching
    /// popup rendering.
    pub fn new_headless(scale: f32) -> Self {
        const DEJAVU_SANS: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        const DEJAVU_SANS_BOLD: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans-Bold.ttf");

        let mut db = Database::new();
        db.load_font_data(DEJAVU_SANS.to_vec());
        db.load_font_data(DEJAVU_SANS_BOLD.to_vec());

        // Resolve deterministically over just the two vendored faces (yields
        // "DejaVu Sans", whose regular + bold are distinct in-family faces).
        let ui_family = resolve_ui_family(&db).or_else(|| Some("DejaVu Sans".to_string()));
        Self::from_parts(scale, db, ui_family)
    }

    fn from_parts(scale: f32, db: Database, ui_family: Option<String>) -> Self {
        RasterDrawer {
            scale: scale.max(0.1),
            fb: Framebuffer::new(1, 1),
            fonts: FontStore::new(db, ui_family),
            icons: RefCell::new(HashMap::new()),
        }
    }

    /// The device scale factor this drawer rasterizes at.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The rendered framebuffer (premultiplied RGBA, device pixels). Valid after
    /// a [`SceneDrawer::begin_frame`]/paint pass.
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.fb
    }

    /// Device pixel dimensions of the current frame.
    pub fn device_size(&self) -> (u32, u32) {
        (self.fb.width(), self.fb.height())
    }

    /// Encode the current frame as PNG bytes (requires a completed paint pass).
    pub fn encode_png(&self) -> Vec<u8> {
        self.fb.encode_png()
    }

    /// Test-only: how many text runs have actually been shaped (cache misses),
    /// used to assert that a repaint of unchanged text re-shapes nothing.
    #[cfg(test)]
    pub(crate) fn shape_miss_count(&self) -> usize {
        self.fonts.shape_misses.get()
    }
}

/// A glyph positioned along a shaped line: which face rendered it, its glyph id,
/// and its pen position + offset in device pixels (relative to the line start).
struct ShapedGlyph {
    face: FaceId,
    glyph: u16,
    /// Horizontal pen position (already including the shaper's x-offset), px.
    x: f32,
    /// Vertical offset above the baseline (shaper y-offset), px, positive up.
    y: f32,
}

/// One shaped, single-line run: its glyphs plus the total advance width (px).
struct ShapedLine {
    glyphs: Vec<ShapedGlyph>,
    width: f32,
}

/// A rasterized glyph coverage/color bitmap cached across frames (the
/// replacement for `cosmic-text`'s `SwashCache`, satisfying spec §9's "zero new
/// rasterization on re-render" invariant).
struct GlyphImage {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    content: Content,
    data: Vec<u8>,
}

/// Cache key for a rasterized glyph: the face, glyph id, and device-pixel size
/// (as raw `f32` bits so equal sizes hit the cache exactly).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: FaceId,
    glyph: u16,
    size_bits: u32,
}

/// Raw font bytes for one face, kept alive so `harfrust`/`swash` borrows into
/// them are valid, and shared cheaply via [`Rc`].
struct FaceBytes {
    data: Vec<u8>,
    index: u32,
}

/// muri's owned text layer: the font database, the pinned UI family, a
/// per-face byte cache, the shaping-glyph rasterization cache, and the
/// deterministic fallback order. All lookup state is behind [`RefCell`] so the
/// `&self` [`SceneDrawer::measure_text`]/[`SceneDrawer::line_height`] methods can
/// shape without a `&mut self`.
/// Cap on [`FontStore::glyphs`]'s entry count (see [`FontStore::glyph_image`]).
const GLYPH_CACHE_CAP: usize = 512;

/// Cap on [`FontStore::shaped`]'s entry count. A menu has a few dozen distinct
/// runs; the cap only bounds a long-lived drawer whose text changes every tick.
const SHAPED_CACHE_CAP: usize = 1024;

struct FontStore {
    db: Database,
    /// The concrete family name the generic "system" font resolves to, pinned
    /// once at construction. Using an explicit family (rather than the generic
    /// `Family::SansSerif`) for *every* weight keeps a bold section header and a
    /// regular row in the **same typeface** — otherwise `fontdb` can match the
    /// bold request to a different family's face than the regular one, which is
    /// the "different face for bold" glitch (spec §7.1).
    ui_family: Option<String>,
    face_data: RefCell<HashMap<FaceId, Rc<FaceBytes>>>,
    glyphs: RefCell<HashMap<GlyphKey, Option<Rc<GlyphImage>>>>,
    /// Compiled harfrust shaping tables (GSUB/GPOS/cmap caches), one per face,
    /// built once and reused across every `shape` call. `ShaperData::new` is the
    /// expensive step (it compiles the font's OpenType/AAT tables); rebuilding it
    /// per run per frame made a single menu repaint take ~0.5s on real fonts, so
    /// hover-highlight felt laggy. Cached here, only the cheap per-call `Shaper`
    /// is rebuilt. Bounded by the handful of distinct faces a menu uses.
    shaper_data: RefCell<HashMap<FaceId, Rc<ShaperData>>>,
    /// Memoized codepoint coverage per `(face, char)`. `face_has_glyph` otherwise
    /// re-parses the font and its cmap table on every call — and it is called per
    /// char, per candidate face, per render. Caching it (with the fallback cache
    /// below) is what keeps a hover-highlight repaint from re-scanning fonts.
    coverage: RefCell<HashMap<(FaceId, char), bool>>,
    /// Memoized font-fallback decision per `(char, ot_weight)`. `fallback_face_for`
    /// otherwise scans the *entire system font DB* (loading + cmap-parsing each
    /// face) for every codepoint the primary face lacks, on every render — the
    /// dominant cost of a laggy menu with symbol/emoji/logo glyphs. The result is
    /// stable for a given char+weight, so it is cached across frames.
    fallback_cache: RefCell<HashMap<(char, u16), Option<FaceId>>>,
    /// Memoized shaped runs per `(text, primary face, ot_weight, px-bits)`.
    /// `shape` is called several times per run per render (measure pass + draw
    /// pass), and a hover-highlight repaints unchanged text — so caching the
    /// shaped glyph list across frames turns a repaint into a glyph-blit with no
    /// re-shaping. Bounded by [`SHAPED_CACHE_CAP`] (cleared wholesale on overflow)
    /// so a live menu whose text changes each tick can't grow it without limit.
    shaped: RefCell<HashMap<ShapeKey, Rc<ShapedLine>>>,
    /// Test-only counter of actual shaping runs (cache misses), so a test can
    /// assert a repaint of unchanged text re-shapes nothing.
    #[cfg(test)]
    shape_misses: std::cell::Cell<usize>,
    scale_ctx: RefCell<ScaleContext>,
    /// Every face id in the db, in a stable order, scanned as the last-resort
    /// fallback when no [`FALLBACK_FAMILIES`] entry covers a codepoint.
    fallback_order: Vec<FaceId>,
}

impl FontStore {
    fn new(db: Database, ui_family: Option<String>) -> Self {
        let fallback_order: Vec<FaceId> = db.faces().map(|f| f.id).collect();
        FontStore {
            db,
            ui_family,
            face_data: RefCell::new(HashMap::new()),
            glyphs: RefCell::new(HashMap::new()),
            shaper_data: RefCell::new(HashMap::new()),
            coverage: RefCell::new(HashMap::new()),
            fallback_cache: RefCell::new(HashMap::new()),
            shaped: RefCell::new(HashMap::new()),
            #[cfg(test)]
            shape_misses: std::cell::Cell::new(0),
            scale_ctx: RefCell::new(ScaleContext::new()),
            fallback_order,
        }
    }

    /// The raw bytes (+ collection index) for a face, cached after first load.
    fn face_bytes(&self, id: FaceId) -> Option<Rc<FaceBytes>> {
        if let Some(b) = self.face_data.borrow().get(&id) {
            return Some(b.clone());
        }
        let bytes = self.db.with_face_data(id, |data, index| FaceBytes {
            data: data.to_vec(),
            index,
        })?;
        let rc = Rc::new(bytes);
        self.face_data.borrow_mut().insert(id, rc.clone());
        Some(rc)
    }

    /// The db family selector for a muri [`FontFamily`], pinning `System` to the
    /// resolved UI family (spec §7.1).
    fn db_family<'a>(&'a self, family: &'a FontFamily) -> DbFamily<'a> {
        match family {
            FontFamily::System => match &self.ui_family {
                Some(name) => DbFamily::Name(name),
                None => DbFamily::SansSerif,
            },
            FontFamily::SystemMono => DbFamily::Monospace,
            FontFamily::Named(name) => DbFamily::Name(name),
        }
    }

    /// Resolve a muri font + weight to a concrete face in the db.
    fn resolve_face(&self, family: &FontFamily, ot_weight: u16) -> Option<FaceId> {
        query_face(&self.db, self.db_family(family), ot_weight)
    }

    /// Whether `id` has a glyph for `ch` in its cmap.
    fn face_has_glyph(&self, id: FaceId, ch: char) -> bool {
        if let Some(&hit) = self.coverage.borrow().get(&(id, ch)) {
            return hit;
        }
        let hit = self
            .face_bytes(id)
            .and_then(|b| {
                FontRef::from_index(&b.data, b.index as usize).map(|f| f.charmap().map(ch) != 0)
            })
            .unwrap_or(false);
        self.coverage.borrow_mut().insert((id, ch), hit);
        hit
    }

    /// Pick a fallback face that covers `ch`: try the preferred families first,
    /// then scan the whole db in a stable order. Deterministic given the db.
    fn fallback_face_for(&self, ch: char, ot_weight: u16) -> Option<FaceId> {
        if let Some(&cached) = self.fallback_cache.borrow().get(&(ch, ot_weight)) {
            return cached;
        }
        let result = 'find: {
            for fam in FALLBACK_FAMILIES {
                if let Some(id) = query_face(&self.db, DbFamily::Name(fam), ot_weight) {
                    if self.face_has_glyph(id, ch) {
                        break 'find Some(id);
                    }
                }
            }
            self.fallback_order
                .iter()
                .copied()
                .find(|&id| self.face_has_glyph(id, ch))
        };
        self.fallback_cache
            .borrow_mut()
            .insert((ch, ot_weight), result);
        result
    }

    /// Split `text` into consecutive `(face, substring)` runs so each codepoint
    /// is shaped by a face that actually has a glyph for it — the owned
    /// font-fallback layer. Characters the primary face covers stay on it (so a
    /// whole label shapes as one run with kerning/ligatures intact); only
    /// codepoints it lacks divert to a fallback face, keeping tofu out of
    /// CJK/emoji/symbol text.
    fn segment_faces(
        &self,
        text: &str,
        primary: Option<FaceId>,
        ot_weight: u16,
    ) -> Vec<(FaceId, String)> {
        let mut runs: Vec<(FaceId, String)> = Vec::new();
        for ch in text.chars() {
            let face = match primary {
                Some(p) if self.face_has_glyph(p, ch) => Some(p),
                Some(p) => self.fallback_face_for(ch, ot_weight).or(Some(p)),
                None => self.fallback_face_for(ch, ot_weight),
            };
            let Some(face) = face else { continue };
            match runs.last_mut() {
                Some((f, s)) if *f == face => s.push(ch),
                _ => runs.push((face, ch.to_string())),
            }
        }
        runs
    }

    /// Shape a single line of `text` into positioned glyphs (device px), running
    /// each fallback sub-run through `harfrust` with its own face.
    fn shape(
        &self,
        text: &str,
        primary: Option<FaceId>,
        ot_weight: u16,
        px: f32,
    ) -> Rc<ShapedLine> {
        let key = (text.to_owned(), primary, ot_weight, px.to_bits());
        if let Some(cached) = self.shaped.borrow().get(&key) {
            return Rc::clone(cached);
        }
        #[cfg(test)]
        self.shape_misses.set(self.shape_misses.get() + 1);
        let mut glyphs = Vec::new();
        let mut pen = 0.0_f32;
        for (face_id, sub) in self.segment_faces(text, primary, ot_weight) {
            let Some(bytes) = self.face_bytes(face_id) else {
                continue;
            };
            let Ok(font) = HbFontRef::from_index(&bytes.data, bytes.index) else {
                continue;
            };
            // Shape in font units (no scale set on `ShapeOptions`) and convert
            // advances/offsets to device px with `s = px / upem`, matching the
            // previous `rustybuzz` scaling.
            //
            // Reuse this face's compiled `ShaperData` (built once, cached) rather
            // than recompiling the font's OpenType tables on every run — the
            // difference between a snappy and a ~0.5s menu repaint on real fonts.
            let shaper_data = {
                let mut cache = self.shaper_data.borrow_mut();
                Rc::clone(
                    cache
                        .entry(face_id)
                        .or_insert_with(|| Rc::new(ShaperData::new(&font))),
                )
            };
            let shaper = shaper_data.shaper(&font).build();
            let upem = shaper.units_per_em() as f32;
            let s = if upem > 0.0 { px / upem } else { 0.0 };
            let mut buffer = UnicodeBuffer::new();
            buffer.push_str(&sub);
            buffer.guess_segment_properties();
            let shaped = shaper.shape(buffer, ShapeOptions::default());
            for (info, pos) in shaped
                .glyph_infos()
                .iter()
                .zip(shaped.glyph_positions().iter())
            {
                glyphs.push(ShapedGlyph {
                    face: face_id,
                    glyph: info.glyph_id as u16,
                    x: pen + pos.x_offset as f32 * s,
                    y: pos.y_offset as f32 * s,
                });
                pen += pos.x_advance as f32 * s;
            }
        }
        let line = Rc::new(ShapedLine { glyphs, width: pen });
        let mut cache = self.shaped.borrow_mut();
        if cache.len() >= SHAPED_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, Rc::clone(&line));
        line
    }

    /// The (ascent, descent) of the primary face at `px` (descent negative),
    /// used to center a single line within its line box. Falls back to a
    /// reasonable ratio if the face has no usable metrics.
    fn v_metrics(&self, primary: Option<FaceId>, px: f32) -> (f32, f32) {
        if let Some(id) = primary {
            if let Some(bytes) = self.face_bytes(id) {
                if let Some(face) = FontRef::from_index(&bytes.data, bytes.index as usize) {
                    let m = face.metrics(&[]);
                    if m.units_per_em > 0 {
                        // swash reports ascent/descent as positive distances from
                        // the baseline; the caller expects a negative descent
                        // (as `rustybuzz`'s `descender()` returned).
                        let sm = m.scale(px);
                        return (sm.ascent, -sm.descent);
                    }
                }
            }
        }
        (px * 0.8, -px * 0.2)
    }

    /// A rasterized glyph, cached across frames (replaces `SwashCache`).
    ///
    /// Bounded by [`GLYPH_CACHE_CAP`] so a drawer that lives across many
    /// different menus (distinct glyph/size combinations accumulate forever
    /// otherwise) can't grow this cache without bound; a single menu's glyph
    /// set is a few dozen entries at most, far under the cap, so the "zero
    /// re-rasterization on repaint of the same menu" invariant (spec §9)
    /// holds — no menu ever triggers a mid-frame eviction.
    fn glyph_image(&self, face_id: FaceId, glyph: u16, px: f32) -> Option<Rc<GlyphImage>> {
        let key = GlyphKey {
            face: face_id,
            glyph,
            size_bits: px.to_bits(),
        };
        {
            let cache = self.glyphs.borrow();
            if let Some(cached) = cache.get(&key) {
                return cached.clone();
            }
        }
        let rendered = self.render_glyph(face_id, glyph, px);
        let mut cache = self.glyphs.borrow_mut();
        if cache.len() >= GLYPH_CACHE_CAP && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.insert(key, rendered.clone());
        rendered
    }

    /// Rasterize one glyph with `swash`, matching the previous color-emoji +
    /// outline handling: color outline / color bitmap first, then a plain alpha
    /// outline mask.
    fn render_glyph(&self, face_id: FaceId, glyph: u16, px: f32) -> Option<Rc<GlyphImage>> {
        let bytes = self.face_bytes(face_id)?;
        let font = FontRef::from_index(&bytes.data, bytes.index as usize)?;
        let mut ctx = self.scale_ctx.borrow_mut();
        let mut scaler = ctx.builder(font).size(px).hint(false).build();
        let image = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .render(&mut scaler, glyph as GlyphId)?;
        if image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }
        Some(Rc::new(GlyphImage {
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            content: image.content,
            data: image.data,
        }))
    }
}

/// Query the db for the best face matching `family` at `ot_weight` (normal
/// style/stretch). Returns a face *within one of the requested families* — it
/// never falls back across families (that is the caller's job), which is what
/// keeps the pinned-family bold invariant honest (spec §7.1).
fn query_face(db: &Database, family: DbFamily, ot_weight: u16) -> Option<FaceId> {
    db.query(&Query {
        families: &[family],
        weight: DbWeight(ot_weight),
        stretch: fontdb::Stretch::Normal,
        style: DbStyle::Normal,
    })
}

/// Register the host's native menu font into `db` and return the family name
/// [`FontFamily::System`] should pin to, or `None` if it couldn't be made
/// resolvable (so the caller falls back to [`resolve_ui_family`]).
///
/// The OS-agnostic consumer of the per-OS
/// [`SystemFont`](crate::platform::SystemFont): it loads bytes or a file into
/// the database (or, for an already-installed family, validates the name) and
/// returns the concrete family to pin. Unlike [`resolve_ui_family`] it does not
/// require regular/bold to be two distinct faces — the OS menu font is
/// authoritative even when it is a single variable face (macOS SF Pro).
fn register_system_font(db: &mut Database, source: SystemFontSource) -> Option<String> {
    let loaded: Option<FaceId> = match source {
        SystemFontSource::Family(name) => {
            // Already installed (Segoe UI, a Linux UI family): pin by name iff the
            // db actually has a face for it; nothing to load.
            return query_face(db, DbFamily::Name(&name), 400).map(|_| name);
        }
        SystemFontSource::Path(path) => db.load_font_source(DbSource::File(path)).first().copied(),
        SystemFontSource::Data(data) => db
            .load_font_source(DbSource::Binary(Arc::new(data)))
            .first()
            .copied(),
    };
    // Pin the registered face's family only if it is resolvable by that name (so
    // a malformed face falls back).
    let name = db
        .face(loaded?)
        .and_then(|f| f.families.first().map(|(n, _)| n.clone()))?;
    query_face(db, DbFamily::Name(&name), 400).map(|_| name)
}

/// Resolve a concrete UI font family whose **regular and bold are distinct,
/// real faces within that same family**, so a bold section header and a regular
/// row read as one typeface at two weights.
///
/// The generic `Family::SansSerif` is deliberately avoided, and a name-only
/// lookup isn't enough: on macOS the native `.SF NS` (San Francisco) is a single
/// variable face, so a `fontdb` query for regular and for bold return the *same*
/// face — there is no static bold to pair with the regular, the "different face
/// for bold" glitch the Phase 1 report flagged. So each candidate is verified by
/// confirming both weights resolve to **distinct** in-family faces (and that the
/// bold one is actually bold); the first that passes wins (Helvetica Neue on
/// stock macOS, DejaVu Sans in the headless suite).
fn resolve_ui_family(db: &Database) -> Option<String> {
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
        if family_shapes_both_weights(db, cand) {
            return Some(cand.to_string());
        }
    }
    None
}

/// Whether `name` resolves a regular (400) and a bold (700) request to two
/// *distinct* faces that both belong to `name`, the bold one being genuinely
/// bold, and both able to shape a Latin sample. This is the `fontdb`/`swash`
/// port of the shipped invariant; its only former `cosmic-text` touchpoint —
/// `fs.db().face(id).families` — maps 1:1 onto [`fontdb::Database::face`].
fn family_shapes_both_weights(db: &Database, name: &str) -> bool {
    let regular = query_face(db, DbFamily::Name(name), 400);
    let bold = query_face(db, DbFamily::Name(name), 700);
    match (regular, bold) {
        (Some(r), Some(b)) => {
            r != b
                && face_in_family(db, r, name)
                && face_in_family(db, b, name)
                && db
                    .face(b)
                    .map(|f| f.weight >= DbWeight(600))
                    .unwrap_or(false)
                && face_can_shape(db, r)
                && face_can_shape(db, b)
        }
        _ => false,
    }
}

/// Whether face `id`'s family list contains `name` (i.e. the db didn't hand back
/// a different family). The direct `cosmic-text` → `fontdb` mapping of §7.1.
fn face_in_family(db: &Database, id: FaceId, name: &str) -> bool {
    db.face(id)
        .map(|f| f.families.iter().any(|(n, _)| n == name))
        .unwrap_or(false)
}

/// Whether face `id` has glyphs for a small Latin sample (`"Agy0"`), i.e. it can
/// actually shape UI text rather than being, say, a symbol-only face.
fn face_can_shape(db: &Database, id: FaceId) -> bool {
    db.with_face_data(id, |data, index| {
        FontRef::from_index(data, index as usize)
            .map(|f| {
                let cmap = f.charmap();
                "Agy0".chars().all(|c| cmap.map(c) != 0)
            })
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

impl SceneDrawer for RasterDrawer {
    fn begin_frame(&mut self, size: LogicalSize) {
        let w = ((size.width * self.scale).round() as u32).max(1);
        let h = ((size.height * self.scale).round() as u32).max(1);
        // A fresh transparent framebuffer at the device size.
        self.fb = Framebuffer::new(w, h);
    }

    fn fill_round_rect(&mut self, rect: LogicalRect, corner_radius: f32, color: Rgba) {
        let s = self.scale;
        let (x, y, w, h) = raster::scaled(rect, s);
        raster::fill_round_rect(&mut self.fb, x, y, w, h, corner_radius * s, color);
    }

    fn draw_separator(&mut self, rect: LogicalRect, color: Rgba) {
        let s = self.scale;
        // Device-snap the origin and force a ≥1px hairline (spec §6).
        let x = rect.origin.x * s;
        let y = (rect.origin.y * s).round();
        let w = rect.size.width * s;
        let h = (rect.size.height * s).max(1.0);
        raster::fill_rect(&mut self.fb, x, y, w, h, color);
    }

    fn measure_text(&self, text: &str, font: &Font) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let ot_weight = font.weight.ot_weight();
        let primary = self.fonts.resolve_face(&font.family, ot_weight);
        // Logical width: shape at the font's point size (device-scale is applied
        // by `draw_text`; advances scale linearly, so widths stay consistent).
        self.fonts.shape(text, primary, ot_weight, font.size).width
    }

    fn line_height(&self, font: &Font) -> f32 {
        font.size * LINE_HEIGHT_FACTOR
    }

    fn draw_text(&mut self, run: &TextRun<'_>) {
        if run.text.is_empty() {
            return;
        }
        let scale = self.scale;
        let px = run.font.size * scale;
        let ox = run.origin.x * scale;
        let oy = run.origin.y * scale;
        let ot_weight = run.weight.ot_weight();

        let primary = self.fonts.resolve_face(&run.font.family, ot_weight);
        let shaped = self.fonts.shape(run.text, primary, ot_weight, px);
        let (ascent, descent) = self.fonts.v_metrics(primary, px);
        // Baseline that vertically centers the line within its box, matching the
        // previous path's `line_y` placement within `Metrics::line_height`.
        let line_height = px * LINE_HEIGHT_FACTOR;
        let baseline = ascent + (line_height - (ascent - descent)) / 2.0;

        let color = run.color;
        let (pw, ph) = (self.fb.width() as i32, self.fb.height() as i32);

        for g in &shaped.glyphs {
            // Fetch (and cache) the glyph bitmap first, releasing the font-store
            // borrow before the disjoint `&mut self.fb` blit borrow.
            let Some(image) = self.fonts.glyph_image(g.face, g.glyph, px) else {
                continue;
            };
            let gx = (ox + g.x).round() as i32 + image.left;
            let gy = (oy + baseline - g.y).round() as i32 - image.top;
            let pixels = self.fb.pixels_mut();
            blit_glyph(pixels, pw, ph, &image, gx, gy, color);
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
        let (pw, ph) = (self.fb.width() as i32, self.fb.height() as i32);
        let pixels = self.fb.pixels_mut();
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
                raster::blend_pixel(
                    pixels,
                    ((py * pw + px) * 4) as usize,
                    Rgba::new(rgba[idx], rgba[idx + 1], rgba[idx + 2], 255),
                    a,
                );
            }
        }
    }

    fn decode_icon(&self, bytes: &Arc<[u8]>) -> Option<DecodedIcon> {
        let key = Arc::as_ptr(bytes) as *const u8 as usize;
        if let Some(hit) = icon_cache_hit(self.icons.borrow().get(&key), bytes) {
            return Some(hit);
        }
        let decoded = Rc::new(raster::decode_png(bytes)?);
        let mut cache = self.icons.borrow_mut();
        if cache.len() >= ICON_CACHE_CAP && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.insert(key, (bytes.clone(), decoded.clone()));
        Some(decoded)
    }
}

/// Decide whether a pointer-keyed icon-cache `entry` is a genuine hit for
/// `bytes`. The cache key is only an `Arc` pointer *value*; a cleared-and-reused
/// allocation could place a *different* `Arc` at the same address (ABA), so a
/// hit is trusted only when the retained `Arc` is the very same allocation
/// ([`Arc::ptr_eq`]), never merely an equal-looking key. Factored out of
/// [`RasterDrawer::decode_icon`] so the reject path is unit-testable without
/// relying on the allocator to reuse an address.
fn icon_cache_hit(entry: Option<&IconCacheEntry>, bytes: &Arc<[u8]>) -> Option<DecodedIcon> {
    match entry {
        Some((cached_bytes, hit)) if Arc::ptr_eq(cached_bytes, bytes) => Some(hit.clone()),
        _ => None,
    }
}

/// Blit one rasterized glyph onto the framebuffer (premultiplied RGBA bytes) at
/// device-pixel origin `(gx, gy)`, matching the previous `swash` coverage
/// handling: an alpha mask is drawn in `color`; a color-emoji bitmap is drawn
/// with its own straight-alpha RGBA (coverage-tinted per pixel).
fn blit_glyph(
    pixels: &mut [u8],
    pw: i32,
    ph: i32,
    image: &GlyphImage,
    gx: i32,
    gy: i32,
    color: Rgba,
) {
    let iw = image.width as i32;
    let ih = image.height as i32;
    match image.content {
        Content::Mask | Content::SubpixelMask => {
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
                    raster::blend_pixel(pixels, ((py * pw + px) * 4) as usize, color, a);
                }
            }
        }
        Content::Color => {
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
                    raster::blend_pixel(
                        pixels,
                        ((py * pw + px) * 4) as usize,
                        Rgba::new(r, g, b, 255),
                        a,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::FontFamily;

    #[test]
    fn register_system_font_pins_present_family_data_and_falls_back() {
        const DEJAVU: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        let mut db = Database::new();
        db.load_font_data(DEJAVU.to_vec());
        // A family present in the db (#10 Windows/Linux path) pins by name.
        assert_eq!(
            register_system_font(&mut db, SystemFontSource::Family("DejaVu Sans".into()))
                .as_deref(),
            Some("DejaVu Sans")
        );
        // An absent family falls back (None) so the caller keeps its discovered face.
        assert!(register_system_font(
            &mut db,
            SystemFontSource::Family("No Such Family 9x".into())
        )
        .is_none());
        // Raw font bytes (the macOS Data path) register and pin their family.
        assert_eq!(
            register_system_font(&mut db, SystemFontSource::Data(DEJAVU.to_vec())).as_deref(),
            Some("DejaVu Sans")
        );
    }

    /// spec §7.1's font-fallback layer: when a primary face is pinned but has
    /// no glyph for a codepoint, and the headless (DejaVu-only) db has no
    /// dedicated fallback family that covers it either (no color-emoji face
    /// is vendored/installed headless), `segment_faces` falls back to
    /// *keeping* the primary face for that codepoint (`.or(Some(primary))`)
    /// rather than dropping it — so the whole label stays one stable run on
    /// one face instead of splintering around the uncovered codepoint.
    #[test]
    fn segment_faces_keeps_primary_face_when_no_fallback_face_covers_the_gap() {
        let d = RasterDrawer::new_headless(1.0);
        let primary = d.fonts.resolve_face(&FontFamily::System, 400);
        assert!(primary.is_some(), "headless db must resolve a UI family");

        let runs = d.fonts.segment_faces("a\u{1F600}b", primary, 400);
        assert_eq!(runs.len(), 1, "expected one merged run, got {runs:?}");
        assert_eq!(runs[0].0, primary.unwrap());
        assert_eq!(runs[0].1, "a\u{1F600}b");
    }

    /// The `None`-primary path is the one place an uncovered codepoint is
    /// actually dropped rather than kept: with nothing pinned, resolution
    /// goes straight to `fallback_face_for`, whose `None` result (no face in
    /// the db covers the codepoint) skips the character outright, merging its
    /// neighbors into one run instead of splitting around it.
    #[test]
    fn segment_faces_drops_uncovered_codepoints_when_no_primary_face_is_pinned() {
        let d = RasterDrawer::new_headless(1.0);
        let runs = d.fonts.segment_faces("a\u{1F600}b", None, 400);
        assert_eq!(runs.len(), 1, "expected one merged run, got {runs:?}");
        assert_eq!(runs[0].1, "ab");
    }

    /// The common case: every codepoint is covered by the primary face, so
    /// the whole label stays one run on that face (kerning/ligatures intact).
    #[test]
    fn segment_faces_keeps_one_run_when_the_primary_face_covers_everything() {
        let d = RasterDrawer::new_headless(1.0);
        let primary = d.fonts.resolve_face(&FontFamily::System, 400);
        assert!(primary.is_some());

        let runs = d.fonts.segment_faces("hello", primary, 400);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, primary.unwrap());
        assert_eq!(runs[0].1, "hello");
    }

    /// Without a primary face at all, resolution goes straight to fallback
    /// for every codepoint; on the headless (DejaVu-only) db that still finds
    /// coverage for plain Latin text via the last-resort `fallback_order`
    /// scan.
    #[test]
    fn segment_faces_uses_fallback_order_when_no_primary_face_is_given() {
        let d = RasterDrawer::new_headless(1.0);
        let runs = d.fonts.segment_faces("hi", None, 400);
        assert_eq!(runs.len(), 1, "expected one run, got {runs:?}");
        assert_eq!(runs[0].1, "hi");
    }

    fn solid_png(color: Rgba) -> Arc<[u8]> {
        let mut fb = Framebuffer::new(2, 2);
        fb.fill(color);
        let bytes: Vec<u8> = fb.encode_png();
        Arc::from(bytes.into_boxed_slice())
    }

    /// The common, intended cache hit: the *same* `Arc<[u8]>` (same
    /// allocation, same pointer identity) is passed across two frames — the
    /// second call must be a cache hit that returns the identical decode
    /// (same pixels) without re-decoding a different result.
    #[test]
    fn decode_icon_hits_the_cache_for_the_same_arc_across_calls() {
        let d = RasterDrawer::new_headless(1.0);
        let bytes = solid_png(Rgba::opaque(10, 20, 30));

        let first = d.decode_icon(&bytes).expect("valid png decodes");
        let second = d.decode_icon(&bytes).expect("valid png decodes");

        // Same decoded content...
        assert_eq!(first.0, second.0);
        assert_eq!((first.1, first.2), (second.1, second.2));
        // ...and actually served from the cache (same `Rc` allocation), not
        // merely two decodes that happen to agree.
        assert!(
            Rc::ptr_eq(&first, &second),
            "expected a cache hit, got a fresh decode"
        );
    }

    /// The ABA regression this fix closes: two *different* `Arc<[u8]>`
    /// allocations with different icon content must never be confused with
    /// one another by the cache, even though the cache key is derived from
    /// pointer identity. This doesn't (and can't, deterministically) force
    /// the allocator to reuse the first `Arc`'s address for the second, but
    /// it does prove the two decodes are independent and correct — and the
    /// strong-`Arc`-retention fix is exactly what makes address reuse for a
    /// *cached* entry impossible in the first place (the cached clone keeps
    /// the first allocation alive for as long as its entry lives in the
    /// map).
    #[test]
    fn decode_icon_never_conflates_two_different_arcs() {
        let d = RasterDrawer::new_headless(1.0);
        let red = solid_png(Rgba::opaque(255, 0, 0));
        let blue = solid_png(Rgba::opaque(0, 0, 255));

        let red_decoded = d.decode_icon(&red).expect("valid png decodes");
        let blue_decoded = d.decode_icon(&blue).expect("valid png decodes");

        assert_ne!(
            red_decoded.0, blue_decoded.0,
            "distinct icons must decode to distinct pixels"
        );
        assert_eq!(&red_decoded.0[0..4], &[255, 0, 0, 255]);
        assert_eq!(&blue_decoded.0[0..4], &[0, 0, 255, 255]);

        // Re-fetching each by its own (still-live) `Arc` must still return
        // its own content, not the other's — the cache didn't cross-wire the
        // two entries.
        let red_again = d.decode_icon(&red).expect("valid png decodes");
        let blue_again = d.decode_icon(&blue).expect("valid png decodes");
        assert_eq!(red_again.0, red_decoded.0);
        assert_eq!(blue_again.0, blue_decoded.0);
        assert!(
            Rc::ptr_eq(&red_again, &red_decoded),
            "expected a cache hit for `red`"
        );
        assert!(
            Rc::ptr_eq(&blue_again, &blue_decoded),
            "expected a cache hit for `blue`"
        );
    }

    /// The core ABA guard, exercised directly: the cache's retained
    /// `Arc<[u8]>` clone keeps the source allocation alive for as long as
    /// the entry is cached. Dropping the caller's own handle to `bytes`
    /// (as `set_menu` does every ~0.75s in a live app) must not evict, or
    /// corrupt, the cached entry — a later lookup via a *fresh* `Arc` at
    /// (incidentally) the same content still hits the cache keyed on the
    /// still-live, cache-retained allocation's own address, never a
    /// different one.
    #[test]
    fn decode_icon_cache_entry_survives_the_callers_arc_being_dropped() {
        let d = RasterDrawer::new_headless(1.0);
        let bytes = solid_png(Rgba::opaque(4, 5, 6));
        let decoded = d.decode_icon(&bytes).expect("valid png decodes");

        // The caller (e.g. a `set_menu` implementation) drops its handle;
        // the cache's own strong clone must keep the allocation alive.
        drop(bytes);

        assert_eq!(d.icons.borrow().len(), 1);
        let (kept_bytes, kept_decoded) = d.icons.borrow().values().next().cloned().unwrap();
        assert!(Rc::ptr_eq(&kept_decoded, &decoded));
        assert!(!kept_bytes.is_empty());
    }

    /// The ABA guard exercised directly on its reject path: an entry whose
    /// retained `Arc` is a *different* allocation than the querying `Arc` — even
    /// with byte-identical content — must be treated as a miss, not a hit, so a
    /// reused key address can never return another icon's decode. This pins the
    /// `Arc::ptr_eq` guard itself; if a regression reverted it to trust any key
    /// hit, this test fails (unlike the address-reuse-dependent tests above).
    #[test]
    fn icon_cache_hit_rejects_a_key_hit_on_a_different_allocation() {
        let cached: Arc<[u8]> = Arc::from(vec![1u8, 2, 3].into_boxed_slice());
        // Byte-identical content, but a distinct allocation (distinct pointer).
        let query: Arc<[u8]> = Arc::from(vec![1u8, 2, 3].into_boxed_slice());
        assert!(!Arc::ptr_eq(&cached, &query));

        let decoded: DecodedIcon = Rc::new((vec![9, 9, 9, 9], 1, 1));
        let entry: IconCacheEntry = (cached.clone(), decoded.clone());

        // A "key hit" whose Arc is a different allocation -> guard rejects it.
        assert!(icon_cache_hit(Some(&entry), &query).is_none());
        // The genuine same-allocation hit -> served, same `Rc`.
        let hit = icon_cache_hit(Some(&entry), &cached).expect("same-Arc hit");
        assert!(Rc::ptr_eq(&hit, &decoded));
        // No entry at all -> miss.
        assert!(icon_cache_hit(None, &cached).is_none());
    }
}
