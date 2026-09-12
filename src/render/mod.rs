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

/// Display-free, one-call offscreen rendering of a built menu to pixels
/// (issue #59): [`render_menu_to_png`] / [`render_menu_to_rgba`], re-exported at
/// the crate root. No tray/window/display/TCC required; resolves the theme from
/// [`MenuOptions`](crate::MenuOptions) internally.
mod offscreen;
pub use offscreen::{render_menu_to_png, render_menu_to_rgba};

pub mod paint;
// Not `pub`: this module's blit primitives (`blend_pixel`, `fill_round_rect`,
// `fill_rect`, `scaled`) are internal-only implementation details with no
// external caller (platform backends + `paint` are all in-crate). Only
// `decode_png`/`Framebuffer` are muri's actual public raster surface, and they
// stay reachable via the `pub use` re-export just below regardless of this
// module's own visibility.
pub(crate) mod raster;
// The `Icon::Svg` rasterizer (a restricted SVG subset → straight-alpha RGBA,
// built on `zeno`, already in the tree via `swash`). Same output shape as
// `raster::decode_png`, so it slots into the shared icon path; `rasterize_svg` is
// re-exported for the platform backends (Windows HICON / Linux SNI / macOS
// NSImage) to reuse.
pub(crate) mod svg;
pub use svg::rasterize_svg;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use fontdb::{
    Database, Family as DbFamily, Query, Source as DbSource, Style as DbStyle, Weight as DbWeight,
    ID as FaceId,
};
use harfrust::{FontRef as HbFontRef, ShapeOptions, ShaperData, ShaperInstance, UnicodeBuffer};
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
// (text, primary face, ot_weight, px-bits, tracking-bits, opsz-bits). Tracking is
// part of the key so a tracked and untracked shaping of the same text don't
// collide (#42); optical size likewise, so the same text at two optical masters
// keeps distinct advances (#77).
type ShapeKey = (String, Option<FaceId>, u16, u32, u32, u32);

/// A [`RasterDrawer::icons`] cache entry: the decoded icon plus a strong
/// clone of the source `Arc<[u8]>` it was decoded from (see the field doc for
/// why retaining that `Arc` is load-bearing, not incidental).
type IconCacheEntry = (Arc<[u8]>, DecodedIcon);

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::platform::{Platform, SystemFont, SystemFontSource};
use crate::style::{Font, FontFamily, Rgba, Weight};
use crate::theme::OsFamily;

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

    /// Like [`draw_image`](Self::draw_image), but with the whole blit's opacity
    /// scaled by `alpha` (`1.0` = opaque, `0.0` = invisible). The icon funnel
    /// (the `draw_icon` funnel in `paint`) routes through this so a **disabled** row dims its
    /// icon exactly as it dims its checkmark/text (issue E). The default forwards
    /// to the opaque [`draw_image`](Self::draw_image) so drawers that don't care
    /// about dimming need not implement it.
    fn draw_image_alpha(
        &mut self,
        rgba: &[u8],
        src_w: u32,
        src_h: u32,
        dest: LogicalRect,
        alpha: f32,
    ) {
        let _ = alpha;
        self.draw_image(rgba, src_w, src_h, dest);
    }

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
        decode_icon_bytes(bytes).map(Rc::new)
    }
}

/// Decode icon bytes to straight-alpha RGBA `(rgba, width, height)`, trying the
/// PNG codec first and falling back to the [`svg`] rasterizer — the one place
/// `Icon::Png` and `Icon::Svg` bytes converge onto the shared decoded-icon shape
/// the drawer's `draw_image` blit consumes.
pub(crate) fn decode_icon_bytes(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    raster::decode_png(bytes).or_else(|| svg::rasterize_svg(bytes))
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
    /// The shared text layer (font DB + face/shaping/glyph caches). Held behind
    /// an [`Rc`] so a fresh drawer for each popup/flyout open (the platform
    /// backends build one per `for_menu_options` call) reuses the *same*
    /// already-built [`FontStore`] — the expensive font-DB scan/registration and
    /// UI-family resolution happen once per configuration, not once per open, and
    /// the shaping/glyph caches stay warm across opens (#1). Per-frame layout
    /// state (the framebuffer, `scale`) is still per-drawer, so a theme/scale
    /// change between opens is honored; only the config-independent font DB is
    /// shared (see [`shared_font_store`]).
    fonts: Rc<FontStore>,
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
    /// **native menu font** from the per-OS [`Platform::system_menu_font`]
    /// — SF Pro on macOS, Segoe UI on Windows, the GNOME UI family on Linux (#10).
    /// The live popup panels use this so the menu renders in the true OS UI face,
    /// which `fontdb`'s generic discovery can't reach on macOS. Falls back to
    /// installed-font discovery when the platform returns `None` or the face
    /// can't be registered — so it never regresses [`RasterDrawer::new`].
    pub fn new_native(scale: f32) -> Self {
        // The host-native font store is config-independent (the OS menu font is
        // stable for the process), so build it once and share it across every
        // popup/flyout open rather than re-scanning + re-resolving per open (#1).
        let fonts = shared_font_store(FontStoreKey::Native, || {
            let mut db = cached_system_fonts_db();
            let ui_family = crate::platform::current()
                .system_menu_font()
                .and_then(|sf| register_system_font(&mut db, sf.source))
                .or_else(|| resolve_ui_family(&db));
            FontStore::new(db, ui_family)
        });
        Self::from_shared(scale, fonts)
    }

    /// Construct a drawer, optionally pinning `FontFamily::System` to a supplied
    /// system font (its face registered into the database). `None` — or a face
    /// that fails to register — falls back to installed-font discovery.
    pub fn with_system_font(scale: f32, system: Option<SystemFont>) -> Self {
        // `load_system_fonts` walks every system font directory and parses each
        // face — tens-to-hundreds of ms, and it ran on *every* popup open. Cache
        // the scanned database once per thread and clone it (fontdb `Arc`s the
        // font data, so a clone is a cheap metadata copy, no disk I/O) (#22).
        let mut db = cached_system_fonts_db();
        let ui_family = system
            .and_then(|sf| register_system_font(&mut db, sf.source))
            .or_else(|| resolve_ui_family(&db));
        Self::from_parts(scale, db, ui_family)
    }

    /// Create a raster drawer for a **forced** OS theme
    /// ([`ThemeSource::MacOs`](crate::theme::ThemeSource::MacOs) /
    /// [`Windows`](crate::theme::ThemeSource::Windows) /
    /// [`Gnome`](crate::theme::ThemeSource::Gnome), i.e. whenever
    /// [`ThemeSource::forced_family`](crate::theme::ThemeSource::forced_family)
    /// returns `Some`), pinning `FontFamily::System` to the *target* OS's UI
    /// font rather than the host's (issue #54).
    ///
    /// [`RasterDrawer::new_native`] is correct only for `System(..)`: it pins
    /// whatever native menu font [`Platform::system_menu_font`] reports for
    /// the **host** OS. A forced theme must render in the *target* OS's font
    /// on *any* host — a `Windows`-forced menu must use Segoe UI even when
    /// running on macOS — so this constructor resolves against
    /// [`OsFamily::ui_font_families`] / [`OsFamily::fallback_font_families`]
    /// instead of the host-native path, and — the honest limit this issue is
    /// about — **never** falls back to the host's resolved native UI family
    /// (`resolve_ui_family`). See `resolve_forced_ui_family` for the exact
    /// three-step resolution and the caveat about proprietary target faces
    /// (Segoe UI / SF Pro) not being redistributable.
    ///
    /// Platform-backend wiring note: a popup's construction site should choose
    /// between this and [`RasterDrawer::new_native`] based on
    /// `theme_source.forced_family()` — `Some(family)` calls this,
    /// `None` calls `new_native`.
    pub fn with_forced_theme(scale: f32, family: OsFamily) -> Self {
        // A forced theme's store depends only on the target `OsFamily`, so it too
        // is built once per family and shared across opens (#1) — keyed by family
        // so switching the forced OS between opens still resolves correctly.
        let fonts = shared_font_store(FontStoreKey::Forced(forced_family_tag(family)), || {
            // With `bundled-fonts` on, resolution gains a middle tier: the vendored
            // OSS substitute (registered into `db`, hence the `&mut`) is tried after
            // the real target font is found absent and before any free host fallback
            // (issue #54). With the feature off this is byte-for-byte the original
            // read-only two-tier resolution — no regression.
            #[cfg(feature = "bundled-fonts")]
            let (db, ui_family) = {
                let mut db = cached_system_fonts_db();
                let ui_family = resolve_forced_ui_family_bundled(&mut db, family);
                (db, ui_family)
            };
            #[cfg(not(feature = "bundled-fonts"))]
            let (db, ui_family) = {
                let db = cached_system_fonts_db();
                let ui_family = resolve_forced_ui_family(
                    &db,
                    family.ui_font_families(),
                    family.fallback_font_families(),
                );
                (db, ui_family)
            };
            FontStore::new(db, ui_family)
        });
        Self::from_shared(scale, fonts)
    }

    /// The live drawer for a menu about to be painted, selected from its
    /// [`MenuOptions`](crate::MenuOptions): a forced-OS theme
    /// (`ThemeSource::MacOs`/`Windows`/`Gnome`) renders the **target** OS font via
    /// [`with_forced_theme`](Self::with_forced_theme); `System`/`Preset`/`Custom`
    /// use the host-native [`new_native`](Self::new_native). Shared by every
    /// backend's popup/flyout construction so the forced-vs-native choice can't
    /// drift between macOS, Windows, and X11 (#54).
    pub fn for_menu_options(scale: f32, options: &crate::theme::MenuOptions) -> Self {
        match options.theme.forced_family() {
            Some(family) => Self::with_forced_theme(scale, family),
            None => Self::new_native(scale),
        }
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
        Self::from_shared(scale, Rc::new(FontStore::new(db, ui_family)))
    }

    /// Build a drawer around an already-constructed (possibly shared) font store.
    /// The per-frame state (framebuffer, scale, icon cache) is always fresh per
    /// drawer; only the `fonts` text layer may be shared across drawers (#1).
    fn from_shared(scale: f32, fonts: Rc<FontStore>) -> Self {
        RasterDrawer {
            scale: scale.max(0.1),
            fb: Framebuffer::new(1, 1),
            fonts,
            icons: RefCell::new(HashMap::new()),
        }
    }

    /// Test-only: the shared [`FontStore`]'s allocation address, so a test can
    /// assert two drawers built for the *same* configuration reuse the very same
    /// store (built once, not rebuilt per open — #1).
    #[cfg(test)]
    pub(crate) fn font_store_ptr(&self) -> usize {
        Rc::as_ptr(&self.fonts) as usize
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

    /// Test-only: how many times a heavy-weight font request (e.g.
    /// `Weight::Bold`) silently resolved to a light face instead — see
    /// [`FontStore::weight_downgrade_count`] (issue #56), proving the
    /// downgrade is now detectable/assertable rather than silent.
    #[cfg(test)]
    pub(crate) fn weight_downgrade_count(&self) -> usize {
        self.fonts.weight_downgrade_count()
    }

    /// Test-only: how many owned [`ShapeKey`] `String`s have been built (i.e.
    /// fresh `shaped`-cache inserts), so a test can assert the cache-hit path
    /// allocates no owned key (#4).
    #[cfg(test)]
    pub(crate) fn owned_key_build_count(&self) -> usize {
        self.fonts.owned_key_builds.get()
    }
}

/// How a heavy-weight (bold) request is satisfied on a face that `fontdb`'s
/// weight matching resolved to a *lighter* face than asked — the live-macOS
/// single-SF-variable-file case (#63/#56), where a `Weight::Bold` request and a
/// `Weight::Regular` request both land on the one registered SF face.
///
/// `fontdb`'s CSS `find_best_match` never fails on weight, so a family with no
/// discrete bold face silently returns its regular face for a bold request.
/// This enum records what to do about that so bold still renders visibly bold:
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Embolden {
    /// Nothing to do — either the request wasn't heavy, or a genuinely bold
    /// (discrete or already-heavy) face resolved, so it renders as-is.
    None,
    /// The resolved face is a **variable font with a `wght` axis**: instance it
    /// at this OpenType weight (e.g. `700`) through the shaper *and* the glyph
    /// rasterizer, so it renders at the real bold master (the correct, native
    /// result on modern macOS, whose SF face is variable).
    Variable(u16),
    /// The resolved face is a plain single-weight face with no usable `wght`
    /// axis: synthesize bold (faux-bold) by dilating the glyph coverage at
    /// rasterization, so a bold request still visibly bolds.
    Synthetic,
}

/// The OpenType `wght` variation-axis tag (`b"wght"` as a big-endian `u32`),
/// used to detect a variable font's weight axis via `swash`'s `Variations`.
/// The OpenType `wght` variation-axis tag, `pub(crate)` so the macOS
/// system-font path can single-source the same detection when deciding whether a
/// regular face is variable (bold via axis instancing, #63/#65) rather than
/// needing a discrete bold file.
pub(crate) const WGHT_AXIS_TAG: u32 =
    ((b'w' as u32) << 24) | ((b'g' as u32) << 16) | ((b'h' as u32) << 8) | (b't' as u32);

/// OpenType `opsz` (optical-size) variation-axis tag, packed big-endian like
/// [`WGHT_AXIS_TAG`]. A variable UI font (San Francisco, Segoe UI Variable) tunes
/// its outlines per optical master; muri instances this axis to the menu point
/// size so text renders at the correct master rather than the face's (often
/// condensed *Display*) default — SFNS defaults to `opsz` 28 but a 13pt menu wants
/// the *Text* master at `opsz` 17 (#77).
pub(crate) const OPSZ_AXIS_TAG: u32 =
    ((b'o' as u32) << 24) | ((b'p' as u32) << 16) | ((b's' as u32) << 8) | (b'z' as u32);

/// A glyph positioned along a shaped line: which face rendered it, its glyph id,
/// and its pen position + offset in device pixels (relative to the line start).
struct ShapedGlyph {
    face: FaceId,
    glyph: u16,
    /// How this glyph's face achieves the run's (possibly bold) weight — carried
    /// through to rasterization so the variation / synthetic-bold treatment and
    /// the glyph cache key match how it was shaped (#63).
    emb: Embolden,
    /// The resolved optical size (`opsz` axis value) this glyph was shaped at, or
    /// `None` for the face's default master. Carried through to rasterization so
    /// the outline is scaled at the *same* master the shaper positioned it at, and
    /// so the glyph cache key distinguishes optical masters (#77).
    opsz: Option<f32>,
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
    /// The emboldening treatment (#63): a bold and a regular glyph on the *same*
    /// variable/single face at the same size would otherwise collide on this key
    /// and serve each other's bitmap, so the variation/synthetic-bold state is
    /// part of the identity.
    emb: Embolden,
    /// The optical master (`opsz` axis) the outline is instanced at, as raw `f32`
    /// bits (`0` = the face's default master), so glyphs at different optical sizes
    /// don't collide on this key (#77).
    opsz_bits: u32,
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

/// Cap on [`FontStore::coverage`]'s entry count (`(FaceId, char)` → covered).
/// Like the other resolution caches this is process-wide + thread-local, so a
/// long session touching many distinct codepoints across many candidate faces
/// would otherwise grow it without bound; on overflow the whole map is cleared
/// (a menu's live glyph set is far under the cap, so this never evicts mid-render).
const COVERAGE_CACHE_CAP: usize = 4096;

/// Cap on [`FontStore::fallback_cache`]'s entry count (`(char, ot_weight)` →
/// fallback face). Bounded so a long-lived store that fields many distinct
/// symbol/emoji codepoints over its lifetime can't grow it forever; cleared
/// wholesale on overflow.
const FALLBACK_CACHE_CAP: usize = 1024;

/// Cap on [`FontStore::embolden_cache`]'s entry count (`(FaceId, ot_weight)` →
/// [`Embolden`]). A menu uses a handful of faces at a handful of weights, so the
/// cap only bounds pathological long-session growth; cleared on overflow.
const EMBOLDEN_CACHE_CAP: usize = 256;

/// Cap on [`FontStore::face_cache`]'s entry count (`(FontFamily, ot_weight)` →
/// resolved face). Bounded like the others; a real menu resolves only a few
/// family/weight pairs, so overflow (and the wholesale clear) never fires in
/// practice.
const FACE_CACHE_CAP: usize = 256;

/// Cap on [`FontStore::shaper_instances`]'s entry count
/// (`(FaceId, Embolden, opsz-bits)` → [`ShaperInstance`]). A variable-font run's
/// `from_variations` instance depends only on the face, its emboldening, and the
/// optical master, so it is memoized here rather than rebuilt on every `shape`
/// cache miss; bounded and cleared on overflow.
const SHAPER_INSTANCE_CACHE_CAP: usize = 256;

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
    /// Memoized variable-font shaper instances per `(face, emboldening, opsz-bits)`
    /// (#13/#77). `ShaperInstance::from_variations` reparses the font's variation
    /// tables and so is rebuilt on *every* `shape` cache miss for a variable run
    /// even though it depends only on the face, its [`Embolden`], and the optical
    /// master; memoized here (mirroring how `shaper_data` is memoized per face),
    /// bounded by [`SHAPER_INSTANCE_CACHE_CAP`] and cleared wholesale on overflow.
    shaper_instances: RefCell<HashMap<(FaceId, Embolden, u32), Rc<ShaperInstance>>>,
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
    /// Memoized emboldening decision per `(face, ot_weight)` (#63). Computing it
    /// parses the face to check for a `wght` variation axis, so — like the other
    /// resolution caches — the result is memoized (it's stable for the drawer's
    /// lifetime) rather than recomputed per glyph per repaint.
    embolden_cache: RefCell<HashMap<(FaceId, u16), Embolden>>,
    /// Memoized primary-face resolution per `(family, ot_weight)`. `resolve_face`
    /// otherwise runs a `fontdb::Database::query` scan on every measure_text /
    /// draw_text call (once per segment per row, every repaint); the result is
    /// stable for the drawer's lifetime, so it is cached like the others.
    face_cache: RefCell<HashMap<(FontFamily, u16), Option<FaceId>>>,
    /// Memoized shaped runs, keyed by a **hash** of `(text, primary face,
    /// ot_weight, px-bits, tracking-bits)` so the hot lookup path (measure +
    /// draw both call `shape`, and a hover repaint re-shapes unchanged text)
    /// never allocates an owned [`String`] key — it hashes the borrowed `&str`
    /// and, on a hash hit, verifies the retained owned [`ShapeKey`] matches
    /// field-by-field (guarding against a hash collision returning the wrong
    /// glyphs). The owned key's `String` is built only when inserting a genuinely
    /// new entry (#4). Bounded by [`SHAPED_CACHE_CAP`] (cleared wholesale on
    /// overflow) so a live menu whose text changes each tick can't grow it
    /// without limit.
    shaped: RefCell<HashMap<u64, (ShapeKey, Rc<ShapedLine>)>>,
    /// Reused scratch for `shape`'s face segmentation, so the per-run
    /// `(FaceId, String)` buffer (and its `String` allocations) is recycled
    /// across shaping runs instead of freshly allocated each call (#3).
    seg_scratch: RefCell<Vec<(FaceId, String)>>,
    /// Test-only counter of actual shaping runs (cache misses), so a test can
    /// assert a repaint of unchanged text re-shapes nothing.
    #[cfg(test)]
    shape_misses: std::cell::Cell<usize>,
    /// Test-only counter of owned [`ShapeKey`] `String` allocations — bumped
    /// only when inserting a fresh `shaped` entry, never on a cache hit — so a
    /// test can assert the hit path doesn't build a new owned key (#4).
    #[cfg(test)]
    owned_key_builds: std::cell::Cell<usize>,
    /// Test-only counter of times [`FontStore::resolve_face`] requested a
    /// heavy weight (`ot_weight >= 600`, e.g. `Weight::Bold`) but the
    /// database's best match resolved to a substantially lighter face
    /// (`< 500`) — a silent weight downgrade, the exact failure mode behind
    /// issue #56 (`fontdb::Database::query`'s CSS `find_best_match` never
    /// fails on weight; with only a regular face registered it just returns
    /// that face for every request). Per this crate's error philosophy (no
    /// `log` crate dependency), this is how that downgrade is surfaced
    /// instead of logged: a queryable counter so a test can assert on it
    /// rather than it being silent. See [`FontStore::weight_downgrade_count`].
    #[cfg(test)]
    weight_downgrades: std::cell::Cell<usize>,
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
            shaper_instances: RefCell::new(HashMap::new()),
            coverage: RefCell::new(HashMap::new()),
            fallback_cache: RefCell::new(HashMap::new()),
            embolden_cache: RefCell::new(HashMap::new()),
            face_cache: RefCell::new(HashMap::new()),
            shaped: RefCell::new(HashMap::new()),
            seg_scratch: RefCell::new(Vec::new()),
            #[cfg(test)]
            shape_misses: std::cell::Cell::new(0),
            #[cfg(test)]
            owned_key_builds: std::cell::Cell::new(0),
            #[cfg(test)]
            weight_downgrades: std::cell::Cell::new(0),
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
        // Memoize the fontdb query: a given (family, ot_weight) always resolves to
        // the same FaceId for the drawer's lifetime (the db and pinned UI family
        // never change after construction), so caching avoids re-scanning the
        // system-font database on every measure_text/draw_text call — i.e. once per
        // segment per row per repaint, undermining the "shaped once per open" work
        // (#22/#23) one layer earlier than shaping.
        let key = (family.clone(), ot_weight);
        if let Some(&cached) = self.face_cache.borrow().get(&key) {
            return cached;
        }
        let result = query_face(&self.db, self.db_family(family), ot_weight);
        #[cfg(test)]
        if let Some(id) = result {
            self.record_weight_downgrade(ot_weight, id);
        }
        let mut cache = self.face_cache.borrow_mut();
        if cache.len() >= FACE_CACHE_CAP && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.insert(key, result);
        result
    }

    /// Weight requested >= this is "asking for bold or heavier" (matches
    /// `Weight::Bold`'s `700`, with headroom for `Weight::Custom` in between).
    const HEAVY_WEIGHT: u16 = 600;
    /// A resolved face below this is "substantially lighter than requested" —
    /// i.e. the caller got a regular-ish face back for a heavy-weight ask.
    const LIGHT_WEIGHT: u16 = 500;

    /// The OpenType weight applied for a variable-font bold instance (#63) — the
    /// `wght` axis value a `Weight::Bold` heavy run is rendered at when the face
    /// downgraded to a single/variable master, clamped to the axis's own max.
    const VARIABLE_BOLD_WEIGHT: u16 = 700;

    /// Decide how a `(face, ot_weight)` pair should be emboldened (#63), memoized.
    ///
    /// Returns [`Embolden::None`] unless the request is heavy
    /// ([`HEAVY_WEIGHT`](Self::HEAVY_WEIGHT)) *and* the resolved face's own
    /// registered weight is substantially lighter
    /// ([`LIGHT_WEIGHT`](Self::LIGHT_WEIGHT)) — i.e. `fontdb` silently downgraded
    /// a bold request to a regular-ish face (no discrete bold in the family).
    /// In that case: if the face is a variable font exposing a `wght` axis, it
    /// is instanced at bold ([`Embolden::Variable`], clamped to the axis max);
    /// otherwise bold is synthesized ([`Embolden::Synthetic`]).
    fn face_embolden(&self, id: FaceId, ot_weight: u16) -> Embolden {
        if ot_weight < Self::HEAVY_WEIGHT {
            return Embolden::None;
        }
        let key = (id, ot_weight);
        if let Some(&cached) = self.embolden_cache.borrow().get(&key) {
            return cached;
        }
        let emb = match self.face_wght_axis_max(id) {
            // A **variable** face instances its `wght` axis to the requested bold
            // weight regardless of its registered *default-instance* weight — the
            // registered weight is only the default (the live macOS SFNS System
            // face defaults to Regular but must bold *up* the axis), so a variable
            // face must never be treated as "already bold" and skip instancing.
            // This is the #65 live-System bold fix: when `fontdb` registers the
            // resolved variable face at a mid/heavy default weight (>= LIGHT_WEIGHT),
            // the old `actual_weight >= LIGHT_WEIGHT -> None` short-circuit dropped
            // the instance entirely, so a bold row rendered identical to regular.
            Some(max) => Embolden::Variable(Self::variable_bold_wght(ot_weight, max as u16)),
            // A **static** face has no axis: only faux-bold when `fontdb` silently
            // *downgraded* a heavy request to a light face; a genuinely heavy static
            // face (a discrete bold) is already bold and needs nothing.
            None => {
                let actual_weight = self.db.face(id).map_or(ot_weight, |f| f.weight.0);
                if actual_weight >= Self::LIGHT_WEIGHT {
                    Embolden::None
                } else {
                    Embolden::Synthetic
                }
            }
        };
        let mut cache = self.embolden_cache.borrow_mut();
        if cache.len() >= EMBOLDEN_CACHE_CAP && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.insert(key, emb);
        emb
    }

    /// The `wght` axis value a variable-bold instance is created at, given the
    /// requested `ot_weight` and the face's own `wght` axis maximum (#F18). The
    /// bold floor is raised FIRST and the axis-max clamp applied LAST, so a face
    /// whose axis tops out below [`HEAVY_WEIGHT`](Self::HEAVY_WEIGHT) is instanced
    /// at its own max rather than pushed above it (the earlier
    /// `.min(max).max(HEAVY_WEIGHT)` order could exceed the axis max).
    fn variable_bold_wght(ot_weight: u16, axis_max: u16) -> u16 {
        // Raise to the bold floor / cap at the bold target first
        // (`HEAVY_WEIGHT <= VARIABLE_BOLD_WEIGHT`, a valid clamp), THEN clamp to the
        // axis max LAST so a low axis max is never exceeded.
        ot_weight
            .clamp(Self::HEAVY_WEIGHT, Self::VARIABLE_BOLD_WEIGHT)
            .min(axis_max)
    }

    /// The maximum value of the face's `wght` variation axis, or `None` when the
    /// face has no such axis (a plain static face). Used to decide between a real
    /// variable-weight instance and synthetic faux-bold, and to clamp the
    /// requested bold weight into the axis's own range (#63).
    fn face_wght_axis_max(&self, id: FaceId) -> Option<f32> {
        let bytes = self.face_bytes(id)?;
        let font = FontRef::from_index(&bytes.data, bytes.index as usize)?;
        font.variations()
            .find_by_tag(WGHT_AXIS_TAG)
            .map(|axis| axis.max_value())
    }

    /// Resolve the `opsz` (optical-size) axis value to instance `id` at for a
    /// `points`-point render, clamped into the face's own `opsz` range, or `None`
    /// when the face has no `opsz` axis (nothing to instance) (#77). Clamping
    /// mirrors CoreText: a 13pt menu on SFNS (axis min 17) resolves to the *Text*
    /// master at 17, not the default *Display* master at 28.
    fn face_opsz(&self, id: FaceId, points: f32) -> Option<f32> {
        let bytes = self.face_bytes(id)?;
        let font = FontRef::from_index(&bytes.data, bytes.index as usize)?;
        font.variations()
            .find_by_tag(OPSZ_AXIS_TAG)
            .map(|axis| points.clamp(axis.min_value(), axis.max_value()))
    }

    /// Diagnostic (#65): a one-line description of the resolved face — its `fontdb`
    /// families/index/registered weight and the parsed `wght` axis min/default/max
    /// plus named-instance count. The live full-system DB resolves `System` to a
    /// face whose axis *default* may already be heavy, in which case a
    /// [`Embolden::Variable`] instance to 700 lightens rather than thickens it; this
    /// surfaces exactly that. Only ever called under `MURI_DEBUG_TEXT`.
    fn face_debug(&self, id: FaceId) -> String {
        let (families, db_weight, db_index) = self
            .db
            .face(id)
            .map(|f| {
                let fams = f
                    .families
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join("/");
                (fams, f.weight.0, f.index)
            })
            .unwrap_or_else(|| ("<none>".into(), 0, 0));
        let axis = self
            .face_bytes(id)
            .and_then(|b| {
                let font = FontRef::from_index(&b.data, b.index as usize)?;
                let wght = font.variations().find_by_tag(WGHT_AXIS_TAG);
                let n_instances = font.instances().count();
                Some(match wght {
                    Some(a) => format!(
                        "wght[min={} def={} max={}] named_instances={n_instances}",
                        a.min_value(),
                        a.default_value(),
                        a.max_value(),
                    ),
                    None => format!("wght[none] named_instances={n_instances}"),
                })
            })
            .unwrap_or_else(|| "wght[unparsed]".into());
        format!("families={families:?} db_index={db_index} db_weight={db_weight} {axis}")
    }

    /// Record a weight downgrade (#56): `ot_weight` was heavy
    /// (`>= HEAVY_WEIGHT`) but `resolved`'s actual registered weight in `db`
    /// is light (`< LIGHT_WEIGHT`) — `fontdb`'s CSS matching returned the
    /// closest available face rather than failing, so this is the only place
    /// that mismatch can be caught. Called once per fresh (non-cached)
    /// `resolve_face` resolution, never per cache hit.
    #[cfg(test)]
    fn record_weight_downgrade(&self, ot_weight: u16, resolved: FaceId) {
        if ot_weight < Self::HEAVY_WEIGHT {
            return;
        }
        let actual_weight = self.db.face(resolved).map_or(ot_weight, |f| f.weight.0);
        if actual_weight < Self::LIGHT_WEIGHT {
            self.weight_downgrades.set(self.weight_downgrades.get() + 1);
        }
    }

    /// Test-only: how many times [`resolve_face`](Self::resolve_face) silently
    /// downgraded a heavy-weight request (e.g. `Weight::Bold`) to a light
    /// resolved face — see [`weight_downgrades`](Self::weight_downgrades) for
    /// why this exists instead of a log line.
    #[cfg(test)]
    fn weight_downgrade_count(&self) -> usize {
        self.weight_downgrades.get()
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
        let mut cache = self.coverage.borrow_mut();
        if cache.len() >= COVERAGE_CACHE_CAP && !cache.contains_key(&(id, ch)) {
            cache.clear();
        }
        cache.insert((id, ch), hit);
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
        let mut cache = self.fallback_cache.borrow_mut();
        if cache.len() >= FALLBACK_CACHE_CAP && !cache.contains_key(&(ch, ot_weight)) {
            cache.clear();
        }
        cache.insert((ch, ot_weight), result);
        result
    }

    /// Split `text` into consecutive `(face, substring)` runs so each codepoint
    /// is shaped by a face that actually has a glyph for it — the owned
    /// font-fallback layer. Characters the primary face covers stay on it (so a
    /// whole label shapes as one run with kerning/ligatures intact); only
    /// codepoints it lacks divert to a fallback face, keeping tofu out of
    /// CJK/emoji/symbol text.
    #[cfg(test)]
    fn segment_faces(
        &self,
        text: &str,
        primary: Option<FaceId>,
        ot_weight: u16,
    ) -> Vec<(FaceId, String)> {
        let mut runs: Vec<(FaceId, String)> = Vec::new();
        let used = self.segment_faces_into(text, primary, ot_weight, &mut runs);
        runs.truncate(used);
        runs
    }

    /// Fill `runs` with the `(face, substring)` segmentation of `text` and return
    /// how many entries are live. `runs` may carry entries (and their `String`
    /// allocations) from a previous call: they are recycled in place (cleared and
    /// rewritten) rather than reallocated, so `shape`'s hot path reuses one
    /// scratch buffer across runs (#3). Entries past the returned length are stale
    /// and must be ignored by the caller (they keep their capacity for reuse).
    fn segment_faces_into(
        &self,
        text: &str,
        primary: Option<FaceId>,
        ot_weight: u16,
        runs: &mut Vec<(FaceId, String)>,
    ) -> usize {
        let mut used = 0usize;
        for ch in text.chars() {
            let face = match primary {
                Some(p) if self.face_has_glyph(p, ch) => Some(p),
                Some(p) => self.fallback_face_for(ch, ot_weight).or(Some(p)),
                None => self.fallback_face_for(ch, ot_weight),
            };
            let Some(face) = face else { continue };
            // Merge into the current live run when the face matches (keeps a whole
            // label on one face so kerning/ligatures survive).
            if used > 0 && runs[used - 1].0 == face {
                runs[used - 1].1.push(ch);
                continue;
            }
            // Start a new run, recycling an existing slot's `String` allocation
            // when the scratch already has one, else growing the buffer.
            if used < runs.len() {
                let slot = &mut runs[used];
                slot.0 = face;
                slot.1.clear();
                slot.1.push(ch);
            } else {
                let mut s = String::new();
                s.push(ch);
                runs.push((face, s));
            }
            used += 1;
        }
        used
    }

    /// Shape a single line of `text` into positioned glyphs (device px), running
    /// each fallback sub-run through `harfrust` with its own face.
    fn shape(
        &self,
        text: &str,
        primary: Option<FaceId>,
        ot_weight: u16,
        px: f32,
        // Extra tracking added to every glyph advance, in the SAME units as `px`
        // (logical points when measuring, device px when drawing) so measure and
        // draw stay proportional (#42). `0.0` is the metrics-only default.
        tracking: f32,
        // Optical size (`opsz` axis) to instance every face at, in LOGICAL points
        // (scale-independent — the same design master serves 1x and 2x), or `None`
        // to leave each face at its default master (#77).
        optical_size: Option<f32>,
    ) -> Rc<ShapedLine> {
        let px_bits = px.to_bits();
        let tracking_bits = tracking.to_bits();
        // Optical size keys the shape cache in raw bits (`0` = default master), so
        // the same text at two optical masters keeps distinct advances.
        let opsz_bits = optical_size.map(f32::to_bits).unwrap_or(0);
        // Hash the borrowed components — no owned `String` on the lookup path.
        let hash = shape_key_hash(text, primary, ot_weight, px_bits, tracking_bits, opsz_bits);
        if let Some((k, cached)) = self.shaped.borrow().get(&hash) {
            // Verify the retained owned key really matches (a hash collision must
            // not return another run's glyphs). Field-by-field so `String == &str`
            // compares without allocating.
            if k.0 == text
                && k.1 == primary
                && k.2 == ot_weight
                && k.3 == px_bits
                && k.4 == tracking_bits
                && k.5 == opsz_bits
            {
                return Rc::clone(cached);
            }
        }
        #[cfg(test)]
        self.shape_misses.set(self.shape_misses.get() + 1);
        let mut glyphs = Vec::new();
        let mut pen = 0.0_f32;
        let mut scratch = self.seg_scratch.borrow_mut();
        let runs = &mut *scratch;
        let used = self.segment_faces_into(text, primary, ot_weight, runs);
        for (face_id, sub) in runs.iter().take(used) {
            let face_id = *face_id;
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
            // A heavy run on a face that downgraded to a single/variable master
            // is instanced at the bold `wght` axis value so the shaper positions
            // the *bold* glyphs (advances included), not the regular ones (#63).
            let emb = self.face_embolden(face_id, ot_weight);
            // Resolve the optical master for THIS face (clamped into its own `opsz`
            // range; `None` for a face with no `opsz` axis or an opt-out caller).
            let opsz = optical_size.and_then(|pts| self.face_opsz(face_id, pts));
            let opsz_bits = opsz.map(f32::to_bits).unwrap_or(0);
            // Memoize the variable-font instance per `(face, emb, opsz)` (#13/#77):
            // it depends only on those, so building it fresh on every shape cache
            // miss (as the code used to) needlessly reparsed the font's variation
            // tables. A single instance carries BOTH the bold `wght` and the `opsz`
            // master so advances and outline stay on the same coordinates.
            let wght = if let Embolden::Variable(w) = emb {
                Some(w as f32)
            } else {
                None
            };
            let instance: Option<Rc<ShaperInstance>> = if wght.is_some() || opsz.is_some() {
                let key = (face_id, emb, opsz_bits);
                let mut cache = self.shaper_instances.borrow_mut();
                let inst = if let Some(inst) = cache.get(&key) {
                    Rc::clone(inst)
                } else {
                    let mut vars: Vec<(&str, f32)> = Vec::with_capacity(2);
                    if let Some(w) = wght {
                        vars.push(("wght", w));
                    }
                    if let Some(o) = opsz {
                        vars.push(("opsz", o));
                    }
                    let inst = Rc::new(ShaperInstance::from_variations(&font, vars));
                    if cache.len() >= SHAPER_INSTANCE_CACHE_CAP {
                        cache.clear();
                    }
                    cache.insert(key, Rc::clone(&inst));
                    inst
                };
                Some(inst)
            } else {
                None
            };
            let shaper = shaper_data
                .shaper(&font)
                .instance(instance.as_deref())
                .build();
            let upem = shaper.units_per_em() as f32;
            let s = if upem > 0.0 { px / upem } else { 0.0 };
            let mut buffer = UnicodeBuffer::new();
            buffer.push_str(sub);
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
                    emb,
                    opsz,
                    x: pen + pos.x_offset as f32 * s,
                    y: pos.y_offset as f32 * s,
                });
                // Advance + tracking (native UI engines add size-dependent tracking
                // a bare shaper doesn't — the macOS System theme sets it, #42).
                pen += pos.x_advance as f32 * s + tracking;
            }
        }
        drop(scratch);
        let line = Rc::new(ShapedLine { glyphs, width: pen });
        // Build the owned key only now, on a genuine insert (never on a hit) (#4).
        let key: ShapeKey = (
            text.to_owned(),
            primary,
            ot_weight,
            px_bits,
            tracking_bits,
            opsz_bits,
        );
        #[cfg(test)]
        self.owned_key_builds.set(self.owned_key_builds.get() + 1);
        let mut cache = self.shaped.borrow_mut();
        if cache.len() >= SHAPED_CACHE_CAP {
            cache.clear();
        }
        cache.insert(hash, (key, Rc::clone(&line)));
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
    fn glyph_image(
        &self,
        face_id: FaceId,
        glyph: u16,
        px: f32,
        emb: Embolden,
        opsz: Option<f32>,
    ) -> Option<Rc<GlyphImage>> {
        let key = GlyphKey {
            face: face_id,
            glyph,
            size_bits: px.to_bits(),
            emb,
            opsz_bits: opsz.map(f32::to_bits).unwrap_or(0),
        };
        {
            let cache = self.glyphs.borrow();
            if let Some(cached) = cache.get(&key) {
                return cached.clone();
            }
        }
        let rendered = self.render_glyph(face_id, glyph, px, emb, opsz);
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
    fn render_glyph(
        &self,
        face_id: FaceId,
        glyph: u16,
        px: f32,
        emb: Embolden,
        opsz: Option<f32>,
    ) -> Option<Rc<GlyphImage>> {
        let bytes = self.face_bytes(face_id)?;
        let font = FontRef::from_index(&bytes.data, bytes.index as usize)?;
        let mut ctx = self.scale_ctx.borrow_mut();
        // Instance the variable-font `wght` and `opsz` axes so the outline is
        // scaled at the SAME masters the shaper positioned the advances at (#63 for
        // wght, #77 for opsz). Both are no-ops for a face lacking the axis. A
        // single `variations([...])` call carries whichever apply.
        let mut builder = ctx.builder(font).size(px).hint(false);
        let mut vars: Vec<(&str, f32)> = Vec::with_capacity(2);
        if let Embolden::Variable(w) = emb {
            vars.push(("wght", w as f32));
        }
        if let Some(o) = opsz {
            vars.push(("opsz", o));
        }
        if !vars.is_empty() {
            builder = builder.variations(vars);
        }
        let mut scaler = builder.build();
        let image = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .render(&mut scaler, glyph as GlyphId)?;
        if image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }
        let mut glyph_image = GlyphImage {
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            content: image.content,
            data: image.data,
        };
        // Faux-bold for a plain face with no `wght` axis: dilate the alpha
        // coverage one device pixel horizontally so a bold request still visibly
        // bolds where no real bold master exists (#63). Color glyphs (emoji) are
        // left untouched — synthetic emboldening doesn't apply to them.
        if emb == Embolden::Synthetic {
            embolden_mask(&mut glyph_image);
        }
        Some(Rc::new(glyph_image))
    }
}

/// Hash the components of a [`ShapeKey`] from borrowed parts, so `shape`'s cache
/// lookup never allocates an owned `String` (#4). The `shaped` map keys on this
/// hash and verifies the stored owned key on a hit, so a collision only costs a
/// re-shape, never wrong output.
fn shape_key_hash(
    text: &str,
    primary: Option<FaceId>,
    ot_weight: u16,
    px_bits: u32,
    tracking_bits: u32,
    opsz_bits: u32,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    primary.hash(&mut h);
    ot_weight.hash(&mut h);
    px_bits.hash(&mut h);
    tracking_bits.hash(&mut h);
    opsz_bits.hash(&mut h);
    h.finish()
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
/// A clone of the process's system-font database, scanned **once** per thread and
/// cached (the scan — `load_system_fonts` — is the dominant popup-open cost; a
/// clone is a cheap metadata copy since fontdb `Arc`s the actual font data) (#22).
/// Identifies a shareable [`FontStore`] configuration for the process-wide
/// (per-thread) store cache. Two drawers with the same key resolve to the exact
/// same font DB + pinned UI family, so they can share one built store (#1). The
/// forced variant carries a small tag rather than [`OsFamily`] itself only so
/// this key can `derive(Hash)` without depending on `OsFamily: Hash`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum FontStoreKey {
    /// The host-native menu font store ([`RasterDrawer::new_native`]).
    Native,
    /// A forced-OS-theme store ([`RasterDrawer::with_forced_theme`]), one per
    /// target [`OsFamily`] (tag from [`forced_family_tag`]).
    Forced(u8),
}

/// Stable tag for an [`OsFamily`] used as a [`FontStoreKey::Forced`] discriminant.
fn forced_family_tag(family: OsFamily) -> u8 {
    match family {
        OsFamily::MacOs => 0,
        OsFamily::Windows => 1,
        OsFamily::Gnome => 2,
    }
}

/// Return the shared [`FontStore`] for `key`, building it with `build` exactly
/// once per key per thread and reusing it (as an [`Rc`]) on every later open (#1).
///
/// The store's contents (font DB, pinned UI family, and the shaping/glyph/face
/// caches) depend only on `key`, never on the per-open device scale or theme
/// colors, so sharing is behavior-preserving: the glyph/shaped caches are keyed
/// by device-pixel size, so different scales never collide, and per-frame layout
/// runs entirely in the per-drawer framebuffer.
fn shared_font_store(key: FontStoreKey, build: impl FnOnce() -> FontStore) -> Rc<FontStore> {
    thread_local! {
        static SHARED_FONT_STORES: RefCell<HashMap<FontStoreKey, Rc<FontStore>>> =
            RefCell::new(HashMap::new());
    }
    SHARED_FONT_STORES.with(|cell| {
        if let Some(fs) = cell.borrow().get(&key) {
            return Rc::clone(fs);
        }
        let fs = Rc::new(build());
        cell.borrow_mut().insert(key, Rc::clone(&fs));
        fs
    })
}

fn cached_system_fonts_db() -> Database {
    thread_local! {
        static SYSTEM_FONTS_DB: std::cell::OnceCell<Database> = const { std::cell::OnceCell::new() };
    }
    SYSTEM_FONTS_DB.with(|cell| {
        cell.get_or_init(|| {
            let mut db = Database::new();
            db.load_system_fonts();
            db
        })
        .clone()
    })
}

fn register_system_font(db: &mut Database, source: SystemFontSource) -> Option<String> {
    let loaded: Option<FaceId> = match source {
        SystemFontSource::Family(name) => {
            // Already installed (Segoe UI, a Linux UI family): pin by name iff the
            // db actually has a face for it; nothing to load.
            return query_face(db, DbFamily::Name(&name), 400).map(|_| name);
        }
        SystemFontSource::Path(path) => db.load_font_source(DbSource::File(path)).first().copied(),
        SystemFontSource::Data(data) => {
            // A macOS regular+bold pair packed by `pack_dual_face` (#56) gets its
            // own two-face registration path; anything else is the plain
            // single-face byte load this variant has always supported.
            if let Some((regular, bold)) = unpack_dual_face(&data) {
                return register_dual_face(db, regular, bold);
            }
            db.load_font_source(DbSource::Binary(Arc::new(data)))
                .first()
                .copied()
        }
    };
    // Pin the registered face's family only if it is resolvable by that name (so
    // a malformed face falls back).
    let name = db
        .face(loaded?)
        .and_then(|f| f.families.first().map(|(n, _)| n.clone()))?;
    query_face(db, DbFamily::Name(&name), 400).map(|_| name)
}

/// Magic prefix identifying a [`SystemFontSource::Data`] payload as a packed
/// regular+bold pair rather than a single face's raw bytes (issue #56).
///
/// [`SystemFontSource`] (defined in `src/platform/mod.rs`, part of the
/// cross-platform `Platform` seam) has no field for a second face, so the
/// live macOS backend (`src/platform/mac.rs`) that resolves a distinct bold
/// system-menu face packs both faces' bytes into one `Data` blob with this
/// header (its `pack_dual_face` producer — the only writer — is macOS-only and
/// lives beside that backend in `src/platform/mac.rs`); [`unpack_dual_face`] is
/// the matching decoder read only here, on the render side of the same seam.
/// Not a real font-container format (no other platform produces or needs to
/// parse it) — a private encoding between exactly those two call sites, so this
/// magic is `pub(crate)` to let the macOS producer share the one definition.
pub(crate) const DUAL_FACE_MAGIC: &[u8; 8] = b"MURIDUOF";

/// Decode a `pack_dual_face` blob (packed by the macOS backend in
/// `src/platform/mac.rs`) back into its `(regular, bold)` byte
/// slices. `None` if `data` doesn't start with [`DUAL_FACE_MAGIC`] or is
/// truncated — callers treat that as "not a dual-face blob, load it as a
/// single plain face" rather than an error.
fn unpack_dual_face(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = data.strip_prefix(DUAL_FACE_MAGIC.as_slice())?;
    let (len_bytes, rest) = rest.split_first_chunk::<4>()?;
    let regular_len = u32::from_le_bytes(*len_bytes) as usize;
    if regular_len > rest.len() {
        return None;
    }
    let (regular, bold) = rest.split_at(regular_len);
    if bold.is_empty() {
        return None;
    }
    Some((regular, bold))
}

/// Load a macOS regular+bold face pair (packed by `pack_dual_face` in
/// `src/platform/mac.rs`) into `db` and
/// pin the family the regular face resolves to (#56). Registers the bold
/// bytes best-effort: if the bold face's own name-table family doesn't match
/// the regular one's — so `query_face` at weight 700 still can't find it —
/// this still returns the regular face's family rather than failing outright,
/// exactly like a bold-less single-face registration always has; the
/// resulting weight downgrade is then caught (not silent) by
/// [`FontStore::resolve_face`]'s downgrade detection instead of by this
/// function refusing to register anything.
fn register_dual_face(db: &mut Database, regular: &[u8], bold: &[u8]) -> Option<String> {
    let regular_id = db
        .load_font_source(DbSource::Binary(Arc::new(regular.to_vec())))
        .first()
        .copied()?;
    let name = db
        .face(regular_id)
        .and_then(|f| f.families.first().map(|(n, _)| n.clone()))?;
    query_face(db, DbFamily::Name(&name), 400)?;
    // Best-effort: a parse failure or family mismatch here just means the
    // bold weight later resolves back to the regular face (caught by
    // `resolve_face`'s downgrade detection), not a registration failure.
    db.load_font_source(DbSource::Binary(Arc::new(bold.to_vec())));
    Some(name)
}

/// Resolve the UI family to pin for a **forced** OS theme (issue #54):
/// [`RasterDrawer::with_forced_theme`]'s pure resolution step.
///
/// Tries, in order, each name in `target_families` (the target OS's real UI
/// font, e.g. `["Segoe UI"]`), then each name in `fallback_families` (a free
/// face broadly available regardless of host OS), returning the first that is
/// actually installed in `db` (a plain family-query hit, at weight 400 — a
/// forced theme's font doesn't need the regular/bold-distinct-face invariant
/// [`resolve_ui_family`] enforces, since it's naming one specific real family
/// rather than discovering *some* usable UI face).
///
/// Deliberately does **not** call [`resolve_ui_family`] (the *host's* native
/// UI font resolver) as a last resort: doing so is exactly the bug issue #54
/// reports — a forced Windows theme silently rendering in the host's SF Pro on
/// macOS. Returns `None` when neither list has an installed hit, in which
/// case the caller leaves `FontFamily::System` unpinned (falling through to
/// `fontdb`'s own generic `SansSerif` family, not to a host-specific pin).
///
/// Honesty caveat: Segoe UI and SF Pro are proprietary and not bundled by
/// muri, so `target_families` only wins when the real target font happens to
/// be installed on this host; otherwise the free `fallback_families` face is
/// used, which is *not* metrically identical to the target font.
// Under `bundled-fonts` the library routes forced resolution through
// [`resolve_forced_ui_family_bundled`] instead, so this read-only two-tier
// resolver is reached only by the feature-off build and by unit tests — hence
// the `dead_code` allow for the feature-on lib build (tests still use it).
#[cfg_attr(feature = "bundled-fonts", allow(dead_code))]
pub(crate) fn resolve_forced_ui_family(
    db: &Database,
    target_families: &[&str],
    fallback_families: &[&str],
) -> Option<String> {
    first_installed_family(db, target_families)
        .or_else(|| first_installed_family(db, fallback_families))
}

/// The first family name in `families` that is actually installed in `db` (a
/// plain family-query hit at weight 400), as an owned `String`. The shared
/// building block of forced-UI-family resolution: [`resolve_forced_ui_family`]
/// chains a target tier and a free-fallback tier through it, and (with the
/// `bundled-fonts` feature) [`resolve_forced_ui_family_bundled`] slots the
/// vendored OSS substitute *between* those two tiers.
fn first_installed_family(db: &Database, families: &[&str]) -> Option<String> {
    families
        .iter()
        .find(|name| query_face(db, DbFamily::Name(name), 400).is_some())
        .map(|name| name.to_string())
}

/// Forced-UI-family resolution **with the `bundled-fonts` fallback tier**
/// (issue #54). The resolution order is exactly:
///
/// 1. the real target OS font ([`OsFamily::ui_font_families`]) if installed on
///    the host (OEM parity — e.g. an actual Segoe UI / SF Pro present) — used;
/// 2. else the vendored OSS substitute for this [`OsFamily`]
///    ([`bundled_fonts`]: Inter for macOS, Microsoft's OFL Selawik for Windows,
///    genuine Cantarell for GNOME), registered into `db` and used;
/// 3. else the free host fallback family ([`OsFamily::fallback_font_families`]);
/// 4. else `None` (never the wrong-platform host UI font).
///
/// Registering the substitute needs `&mut db`, which is why this is a distinct
/// entry point from the read-only [`resolve_forced_ui_family`]; the feature-off
/// build never compiles it (and `with_forced_theme` uses the read-only path
/// unchanged, so behavior with the feature off is exactly as before).
#[cfg(feature = "bundled-fonts")]
pub(crate) fn resolve_forced_ui_family_bundled(
    db: &mut Database,
    family: OsFamily,
) -> Option<String> {
    first_installed_family(db, family.ui_font_families())
        .or_else(|| bundled_fonts::register_substitute(db, family))
        .or_else(|| first_installed_family(db, family.fallback_font_families()))
}

/// The freely-redistributable OSS UI-font substitutes embedded by the
/// `bundled-fonts` feature, and the wiring that registers the right one for a
/// forced [`OsFamily`] into a [`fontdb::Database`].
///
/// muri cannot bundle the proprietary originals (Apple SF Pro / Microsoft Segoe
/// UI forbid redistribution), so it vendors the closest OFL substitutes instead —
/// Inter (SF Pro), Microsoft's own metric-compatible Selawik (Segoe UI), and the
/// genuine, already-OFL Cantarell (GNOME). All are SIL OFL 1.1; see
/// `assets/fonts/README.md` for licenses and sources. The `.ttf` bytes are
/// `include_bytes!`-embedded only under this feature, so the default build
/// carries none of them. This is cross-platform, feature-gated code — never
/// `cfg(target_os)`-gated (ADR-0002) — living in the render layer, not
/// `src/platform/`.
#[cfg(feature = "bundled-fonts")]
pub(crate) mod bundled_fonts {
    use super::{query_face, Arc, Database, DbFamily, DbSource};
    use crate::theme::OsFamily;

    const INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/inter/Inter-Regular.ttf");
    const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/inter/Inter-SemiBold.ttf");
    const INTER_BOLD: &[u8] = include_bytes!("../../assets/fonts/inter/Inter-Bold.ttf");

    const SELAWIK_REGULAR: &[u8] = include_bytes!("../../assets/fonts/selawik/Selawik-Regular.ttf");
    const SELAWIK_SEMIBOLD: &[u8] =
        include_bytes!("../../assets/fonts/selawik/Selawik-SemiBold.ttf");
    const SELAWIK_BOLD: &[u8] = include_bytes!("../../assets/fonts/selawik/Selawik-Bold.ttf");

    const CANTARELL_REGULAR: &[u8] =
        include_bytes!("../../assets/fonts/cantarell/Cantarell-Regular.ttf");
    const CANTARELL_BOLD: &[u8] = include_bytes!("../../assets/fonts/cantarell/Cantarell-Bold.ttf");

    /// The vendored substitute (family name + embedded face bytes) for a forced
    /// [`OsFamily`]: macOS → Inter, Windows → Selawik, GNOME → Cantarell.
    fn substitute(family: OsFamily) -> (&'static str, &'static [&'static [u8]]) {
        match family {
            OsFamily::MacOs => ("Inter", &[INTER_REGULAR, INTER_SEMIBOLD, INTER_BOLD]),
            OsFamily::Windows => (
                "Selawik",
                &[SELAWIK_REGULAR, SELAWIK_SEMIBOLD, SELAWIK_BOLD],
            ),
            OsFamily::Gnome => ("Cantarell", &[CANTARELL_REGULAR, CANTARELL_BOLD]),
        }
    }

    /// Register the vendored substitute's faces for `family` into `db` and return
    /// the family name to pin (`Some` iff the family is resolvable afterwards, so
    /// a parse failure falls through to the free host fallback rather than
    /// pinning an unusable name). Idempotent enough for muri's use: `db` is a
    /// fresh per-drawer clone, registered into once.
    pub(crate) fn register_substitute(db: &mut Database, family: OsFamily) -> Option<String> {
        let (name, faces) = substitute(family);
        for face in faces {
            db.load_font_source(DbSource::Binary(Arc::new(face.to_vec())));
        }
        query_face(db, DbFamily::Name(name), 400).map(|_| name.to_string())
    }
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
        // Open `(Option, Option)` tuple, not a closed enum: any missing weight
        // means the family can't supply a distinct regular+bold pair.
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
        // Reset to a transparent surface, reusing the existing buffer when the
        // size is unchanged (the common per-hover repaint) instead of allocating
        // a fresh framebuffer each frame.
        self.fb.reset(w, h);
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
        // Tracking is in logical points here, matching the logical `font.size`.
        self.fonts
            .shape(
                text,
                primary,
                ot_weight,
                font.size,
                font.letter_spacing,
                font.optical_size,
            )
            .width
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
        // Tracking in device px, matching the device `px` size (measure uses the
        // logical equivalent, so the two stay proportional).
        let tracking = run.font.letter_spacing * scale;
        // Diagnostic (0.12.10, #65/#66): the live on-screen popup reportedly
        // renders bold/tracking differently than the offscreen renderer, which no
        // static trace explains — so print the exact per-run inputs on the real
        // live path. Inert unless `MURI_DEBUG_TEXT` is set; removed once diagnosed.
        if std::env::var_os("MURI_DEBUG_TEXT").is_some() {
            let emb = primary.map(|f| self.fonts.face_embolden(f, ot_weight));
            let face_dbg = primary
                .map(|f| self.fonts.face_debug(f))
                .unwrap_or_default();
            eprintln!(
                "MURI_TEXT text={:?} weight={ot_weight} letter_spacing={} tracking={tracking} \
                 face={primary:?} embolden={emb:?} px={px} [{face_dbg}]",
                run.text, run.font.letter_spacing,
            );
        }
        let shaped = self.fonts.shape(
            run.text,
            primary,
            ot_weight,
            px,
            tracking,
            run.font.optical_size,
        );
        let (ascent, descent) = self.fonts.v_metrics(primary, px);
        // Baseline that vertically centers the line within its box, matching the
        // previous path's `line_y` placement within `Metrics::line_height`.
        let line_height = px * LINE_HEIGHT_FACTOR;
        let baseline = ascent + (line_height - (ascent - descent)) / 2.0;

        let color = run.color;
        let (pw, ph) = (self.fb.width() as i32, self.fb.height() as i32);

        // Diagnostic (#65): log the (face, emb, opsz) actually reaching glyph
        // rasterization for this run — the live-vs-offscreen bold divergence is
        // between the (correct) `Variable(700)` decision and the on-screen pixels,
        // so this confirms whether the bold treatment survives to `glyph_image`.
        // Inert unless `MURI_DEBUG_TEXT` is set.
        if std::env::var_os("MURI_DEBUG_TEXT").is_some() {
            if let Some(g0) = shaped.glyphs.first() {
                eprintln!(
                    "MURI_RASTER text={:?} glyphs={} first_face={:?} first_emb={:?} \
                     first_opsz={:?} px={px}",
                    run.text,
                    shaped.glyphs.len(),
                    g0.face,
                    g0.emb,
                    g0.opsz,
                );
            }
        }

        for g in &shaped.glyphs {
            // Fetch (and cache) the glyph bitmap first, releasing the font-store
            // borrow before the disjoint `&mut self.fb` blit borrow.
            let Some(image) = self.fonts.glyph_image(g.face, g.glyph, px, g.emb, g.opsz) else {
                continue;
            };
            let gx = (ox + g.x).round() as i32 + image.left;
            let gy = (oy + baseline - g.y).round() as i32 - image.top;
            let pixels = self.fb.pixels_mut();
            blit_glyph(pixels, pw, ph, &image, gx, gy, color);
        }
    }

    fn draw_image(&mut self, rgba: &[u8], src_w: u32, src_h: u32, dest: LogicalRect) {
        self.draw_image_alpha(rgba, src_w, src_h, dest, 1.0);
    }

    fn draw_image_alpha(
        &mut self,
        rgba: &[u8],
        src_w: u32,
        src_h: u32,
        dest: LogicalRect,
        alpha: f32,
    ) {
        if src_w == 0 || src_h == 0 {
            return;
        }
        // Clamp the opacity multiplier to [0, 1] so a degenerate caller can't
        // overshoot the per-pixel `u8` alpha (issue E dims with `0.5`).
        let alpha = alpha.clamp(0.0, 1.0);
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
                let a = (rgba[idx + 3] as f32 * alpha).round() as u8;
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
        let decoded = Rc::new(decode_icon_bytes(bytes)?);
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
        // Open `Option`/guard match, not a closed enum: a missing entry, or a key
        // hit whose retained `Arc` is a different allocation (ABA), is a miss.
        _ => None,
    }
}

/// Synthesize bold on an alpha-coverage glyph mask (faux-bold) by dilating it one
/// device pixel horizontally: the mask widens by one column and each output pixel
/// takes the max coverage of itself and its left neighbor. This thickens stems
/// visibly without a real bold master — the fallback for a bold request on a plain
/// single-weight face with no `wght` axis (#63). Only [`Content::Mask`] /
/// [`Content::SubpixelMask`] glyphs are dilated; color glyphs are left as-is.
fn embolden_mask(image: &mut GlyphImage) {
    if !matches!(image.content, Content::Mask | Content::SubpixelMask) {
        return;
    }
    let w = image.width as usize;
    let h = image.height as usize;
    if w == 0 || h == 0 {
        return;
    }
    let new_w = w + 1;
    let mut out = vec![0u8; new_w * h];
    for row in 0..h {
        let src = &image.data[row * w..row * w + w];
        let dst = &mut out[row * new_w..row * new_w + new_w];
        for col in 0..new_w {
            let here = if col < w { src[col] } else { 0 };
            let left = if col > 0 { src[col - 1] } else { 0 };
            dst[col] = here.max(left);
        }
    }
    image.data = out;
    image.width = new_w as u32;
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
                    let off = ((py * pw + px) * 4) as usize;
                    // Polarity-aware font smoothing (#71): thin the AA coverage of
                    // light-on-dark glyph pixels so strokes don't overshoot vs
                    // macOS's luminance-dependent smoothing. Dark-on-light is left
                    // on the plain linear blend.
                    let cov = raster::smooth_glyph_coverage(cov, color, pixels, off);
                    let a = (cov as u16 * color.a as u16 / 255) as u8;
                    raster::blend_pixel(pixels, off, color, a);
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
    fn letter_spacing_widens_measured_text_proportionally() {
        // #42: a non-zero `letter_spacing` adds tracking to every glyph advance, so
        // the text measures wider; zero is the metrics-only default (goldens rely
        // on it). "Quit" is 4 glyphs → ~4 * spacing wider (tracking added per glyph).
        let drawer = RasterDrawer::new_headless(1.0);
        let base = Font::system(13.0, crate::style::Weight::Regular);
        let tracked = base.clone().with_letter_spacing(3.0);
        let w0 = drawer.measure_text("Quit", &base);
        let w1 = drawer.measure_text("Quit", &tracked);
        assert!(w0 > 0.0);
        let delta = w1 - w0;
        // 4 glyphs * 3.0 pt of tracking = ~12pt wider (allow slack for shaping).
        assert!(
            (delta - 12.0).abs() < 2.0,
            "expected ~12pt wider with 3pt tracking over 4 glyphs, got {delta}"
        );
        // A negative tracking (the macOS System theme's direction) tightens it.
        let tight = drawer.measure_text("Quit", &base.clone().with_letter_spacing(-1.0));
        assert!(
            tight < w0,
            "negative tracking must tighten: {tight} !< {w0}"
        );
    }

    /// #F18: a variable face whose `wght` axis max is below the bold floor (600)
    /// must be instanced at its own axis max, never *above* it. The old
    /// `.min(max).max(HEAVY_WEIGHT)` order raised the value back above the axis
    /// max; the fixed order clamps the axis max last.
    #[test]
    fn variable_bold_wght_never_exceeds_a_low_axis_max() {
        // Axis tops out below the 600 floor: instance at the axis max, not 600.
        assert_eq!(FontStore::variable_bold_wght(700, 500), 500);
        assert_eq!(FontStore::variable_bold_wght(700, 400), 400);
        // Axis between the floor and the 700 target: the axis max still wins.
        assert_eq!(FontStore::variable_bold_wght(700, 650), 650);
        // A normal wide axis lands at the 700 bold target.
        assert_eq!(FontStore::variable_bold_wght(700, 900), 700);
        assert_eq!(FontStore::variable_bold_wght(800, 1000), 700);
    }

    /// #F14/F15/F16: the resolution caches are bounded — they clear wholesale on
    /// overflow rather than growing forever over a long session. Exercised on the
    /// `embolden_cache` (the smallest cap): inserting one past the cap must leave
    /// the map at or below the cap, not `cap + 1`.
    #[test]
    fn embolden_cache_clears_when_it_exceeds_its_cap() {
        let d = RasterDrawer::new_headless(1.0);
        let id = d.fonts.resolve_face(&FontFamily::System, 400).unwrap();
        // Distinct heavy weights are distinct `(face, ot_weight)` keys; drive one
        // past the cap so the clear-on-overflow guard fires.
        for w in 0..=(EMBOLDEN_CACHE_CAP as u16) {
            let _ = d.fonts.face_embolden(id, FontStore::HEAVY_WEIGHT + w);
        }
        let len = d.fonts.embolden_cache.borrow().len();
        assert!(
            len <= EMBOLDEN_CACHE_CAP,
            "embolden cache must clear on overflow, got {len} > {EMBOLDEN_CACHE_CAP}"
        );
        assert!(len >= 1, "it keeps caching after the clear");
    }

    /// #1: the expensive [`FontStore`] (font DB + shaping/glyph caches) is built
    /// once per configuration and shared across drawers, not rebuilt on every
    /// popup/flyout open. Two drawers built for the *same* forced theme — even at
    /// different device scales — must reference the very same store allocation
    /// (scale is per-drawer and does not affect the shared store).
    #[test]
    fn font_store_is_reused_across_drawers_of_the_same_config() {
        let d1 = RasterDrawer::with_forced_theme(1.0, OsFamily::Windows);
        let d2 = RasterDrawer::with_forced_theme(2.0, OsFamily::Windows);
        assert_eq!(
            d1.font_store_ptr(),
            d2.font_store_ptr(),
            "same-config drawers must share one built FontStore, not rebuild it per open"
        );
        // The build closure must not run again for a cached key: passing a
        // panicking builder for the same key proves the store came from the cache.
        let key = FontStoreKey::Forced(forced_family_tag(OsFamily::Windows));
        let reused = shared_font_store(key, || {
            panic!("FontStore must not be rebuilt for a cached key")
        });
        assert_eq!(Rc::as_ptr(&reused) as usize, d1.font_store_ptr());
    }

    /// #4: the `shaped`-cache lookup path must not allocate an owned `String`
    /// key on a cache hit. `owned_key_build_count` bumps only on a fresh insert,
    /// so a repeated identical measure (a hit) must leave it unchanged.
    #[test]
    fn shape_cache_hit_does_not_build_a_new_owned_key() {
        let d = RasterDrawer::new_headless(1.0);
        let font = Font::system(13.0, crate::style::Weight::Regular);

        assert_eq!(d.owned_key_build_count(), 0);
        let w1 = d.measure_text("Reuse", &font);
        assert!(w1 > 0.0);
        assert_eq!(
            d.owned_key_build_count(),
            1,
            "the first (miss) measure builds exactly one owned key"
        );

        let w2 = d.measure_text("Reuse", &font);
        assert_eq!(w1, w2, "the cached measure must be identical");
        assert_eq!(
            d.owned_key_build_count(),
            1,
            "a cache hit must not build (allocate) a new owned key"
        );
        assert_eq!(d.shape_miss_count(), 1, "and it must not re-shape either");
    }

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

    /// Issue #56: the live macOS System-font path now pairs the regular menu
    /// face with its bold counterpart, packed into one
    /// `SystemFontSource::Data` blob (see `dual_face_source` in
    /// `src/platform/mac.rs`). `register_system_font` must unpack that and
    /// register BOTH faces under the same family, so `resolve_face(..., 700)`
    /// has a real bold candidate instead of only ever finding the regular one.
    #[test]
    fn register_system_font_unpacks_a_dual_face_blob_into_both_weights() {
        const DEJAVU: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        const DEJAVU_BOLD: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans-Bold.ttf");
        let mut db = Database::new();
        // Build a dual-face blob the way the macOS producer (`pack_dual_face` in
        // `src/platform/mac.rs`) does, to exercise the decoder side that lives
        // here: magic, LE u32 regular length, regular bytes, then bold bytes.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(DUAL_FACE_MAGIC);
        bytes.extend_from_slice(&(DEJAVU.len() as u32).to_le_bytes());
        bytes.extend_from_slice(DEJAVU);
        bytes.extend_from_slice(DEJAVU_BOLD);
        let name = register_system_font(&mut db, SystemFontSource::Data(bytes));
        assert_eq!(name.as_deref(), Some("DejaVu Sans"));

        let regular = query_face(&db, DbFamily::Name("DejaVu Sans"), 400);
        let bold = query_face(&db, DbFamily::Name("DejaVu Sans"), 700);
        assert!(regular.is_some() && bold.is_some());
        assert_ne!(
            regular, bold,
            "a packed bold face must resolve as a distinct face from regular"
        );
        assert!(db.face(bold.unwrap()).unwrap().weight >= DbWeight(600));
    }

    /// A `Data` blob that doesn't start with the dual-face magic (every other
    /// platform's plain single-face bytes, and macOS's own pre-#56 fallback
    /// when no distinct bold file was found) must still register exactly as
    /// before — `unpack_dual_face` returning `None` must not be mistaken for
    /// a registration failure.
    #[test]
    fn register_system_font_treats_a_plain_data_blob_as_single_face() {
        const DEJAVU: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        let mut db = Database::new();
        assert_eq!(
            register_system_font(&mut db, SystemFontSource::Data(DEJAVU.to_vec())).as_deref(),
            Some("DejaVu Sans")
        );
    }

    /// Core #56 regression test: with a real bold face registered alongside
    /// regular (exactly what the fixed macOS path now does), resolving weight
    /// 700 must return a face DISTINCT from weight 400 — proving resolution
    /// actually picks bold instead of degrading to regular — and must not
    /// record a weight downgrade.
    #[test]
    fn resolve_face_picks_the_bold_face_when_one_is_registered() {
        let d = RasterDrawer::new_headless(1.0);
        let regular = d.fonts.resolve_face(&FontFamily::System, 400);
        let bold = d.fonts.resolve_face(&FontFamily::System, 700);
        assert!(regular.is_some(), "headless db must resolve a regular face");
        assert!(bold.is_some(), "headless db must resolve a bold face");
        assert_ne!(
            regular, bold,
            "bold must resolve to a face distinct from regular, not degrade to it (#56)"
        );
        assert_eq!(
            d.weight_downgrade_count(),
            0,
            "a real bold face is available — this must not count as a downgrade"
        );
    }

    /// The failure #56 reports: when a family has ONLY a regular face (no
    /// bold), `fontdb`'s CSS weight matching never fails — it returns the
    /// closest available face, silently, for a bold request too. This must no
    /// longer be silent: `weight_downgrade_count` must record it.
    #[test]
    fn resolve_face_records_a_downgrade_when_no_bold_face_is_available() {
        const DEJAVU: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        let mut db = Database::new();
        db.load_font_data(DEJAVU.to_vec());
        let d = RasterDrawer::from_parts(1.0, db, Some("DejaVu Sans".to_string()));

        let regular = d.fonts.resolve_face(&FontFamily::System, 400);
        assert!(regular.is_some());
        assert_eq!(
            d.weight_downgrade_count(),
            0,
            "a regular-weight request is never a downgrade"
        );

        let bold_request = d.fonts.resolve_face(&FontFamily::System, 700);
        assert_eq!(
            bold_request, regular,
            "with no bold face, fontdb's best-match falls back to the only face"
        );
        assert_eq!(
            d.weight_downgrade_count(),
            1,
            "requesting bold with no bold face registered must be recorded, never silent (#56)"
        );

        // Memoized: re-resolving the same (family, weight) must not double-count.
        let _ = d.fonts.resolve_face(&FontFamily::System, 700);
        assert_eq!(d.weight_downgrade_count(), 1);
    }

    /// Sum the alpha coverage of a shaped run rendered at `ot_weight` on the
    /// store `d`, so a test can compare how much ink a bold vs regular render of
    /// the same text lays down on the *same* face (#63).
    #[cfg(test)]
    fn run_ink(
        d: &RasterDrawer,
        primary: Option<FaceId>,
        text: &str,
        ot_weight: u16,
        px: f32,
    ) -> u64 {
        let line = d.fonts.shape(text, primary, ot_weight, px, 0.0, None);
        let mut sum = 0u64;
        for g in &line.glyphs {
            if let Some(img) = d.fonts.glyph_image(g.face, g.glyph, px, g.emb, g.opsz) {
                if matches!(img.content, Content::Mask | Content::SubpixelMask) {
                    sum += img.data.iter().map(|&b| b as u64).sum::<u64>();
                }
            }
        }
        sum
    }

    /// #63 core (synthetic path, portable): a family with only a single static
    /// face (no `wght` axis) still renders a `Weight::Bold` request visibly bold.
    /// `fontdb` resolves bold to the same regular face (it never fails on
    /// weight), so [`FontStore::face_embolden`] must choose faux-bold and the
    /// bold render must ink strictly heavier than regular.
    #[test]
    fn single_static_face_synthesizes_bold_when_no_wght_axis() {
        const DEJAVU: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        let mut db = Database::new();
        db.load_font_data(DEJAVU.to_vec());
        let d = RasterDrawer::from_parts(1.0, db, Some("DejaVu Sans".to_string()));

        let regular = d.fonts.resolve_face(&FontFamily::System, 400).unwrap();
        let bold = d.fonts.resolve_face(&FontFamily::System, 700).unwrap();
        assert_eq!(
            regular, bold,
            "one static face: fontdb resolves bold back to the only face"
        );
        assert_eq!(
            d.fonts.face_embolden(bold, 700),
            Embolden::Synthetic,
            "a static face with no wght axis must synthesize bold (faux-bold), #63"
        );

        let px = 32.0;
        let reg_ink = run_ink(&d, Some(regular), "Bold", 400, px);
        let bold_ink = run_ink(&d, Some(regular), "Bold", 700, px);
        assert!(
            bold_ink > reg_ink,
            "#63: synthesized bold must ink heavier than regular (bold {bold_ink} vs regular {reg_ink})"
        );
    }

    /// #63 core (variable path, the real live-macOS scenario): the system UI
    /// font on modern macOS is a single SF **variable** file (`SFNS.ttf`),
    /// registered as ONE face with a `wght` axis. A `Weight::Bold` request
    /// resolves to that same regular face, but [`FontStore::face_embolden`] must
    /// now instance the `wght` axis at bold instead of downgrading, so the bold
    /// render inks strictly heavier. Gated at runtime on the real SF file
    /// existing (present on macOS hosts, absent elsewhere) rather than a
    /// `cfg(target_os)` — ADR-0002 keeps `target_os` out of the render layer.
    #[test]
    fn native_sf_variable_font_renders_bold_via_wght_instancing() {
        const SFNS: &str = "/System/Library/Fonts/SFNS.ttf";
        if !std::path::Path::new(SFNS).exists() {
            return; // SF variable file not present on this host
        }
        let mut db = Database::new();
        let ids = db.load_font_source(DbSource::File(std::path::PathBuf::from(SFNS)));
        let Some(&fid) = ids.first() else {
            return;
        };
        let Some(family) = db
            .face(fid)
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
        else {
            return;
        };
        let d = RasterDrawer::from_parts(2.0, db, Some(family));

        let regular = d.fonts.resolve_face(&FontFamily::System, 400).unwrap();
        let bold = d.fonts.resolve_face(&FontFamily::System, 700).unwrap();
        assert_eq!(
            regular, bold,
            "SFNS is one face; a bold request resolves to the same variable file"
        );
        let emb = d.fonts.face_embolden(bold, 700);
        assert!(
            matches!(emb, Embolden::Variable(_)),
            "SFNS exposes a wght axis, so bold must be a variable-weight instance, not synthetic or downgraded: got {emb:?}"
        );

        let px = 30.0;
        let reg_ink = run_ink(&d, Some(regular), "Bold", 400, px);
        let bold_ink = run_ink(&d, Some(regular), "Bold", 700, px);
        assert!(
            bold_ink > reg_ink,
            "#63: SF variable bold must ink heavier than regular on the live System path \
             (bold {bold_ink} vs regular {reg_ink})"
        );
    }

    /// #65 (hermetic): a single registered **variable** face with a `wght` axis
    /// must resolve a bold request to a variable-weight instance — never
    /// downgrade to the regular face or fall back to synthetic faux-bold. This is
    /// the exact modern-macOS SF scenario (one variable file, no discrete bold),
    /// but hermetic: it uses a bundled variable DejaVu fixture (a `wght` fvar axis
    /// added to the DejaVu master) so the wght-instancing DECISION is covered on
    /// EVERY host, not only where the live `SFNS.ttf` exists. Pixel-level bold
    /// heaviness stays covered by
    /// `native_sf_variable_font_renders_bold_via_wght_instancing` on macOS hosts
    /// (the fixture carries no `gvar` deltas, so instancing it doesn't move ink —
    /// only the *decision* to instance is under test here).
    #[test]
    fn variable_face_resolves_bold_via_wght_instancing_not_downgrade() {
        const VAR: &[u8] = include_bytes!("../../tests/fonts/variable-wght-test.ttf");
        let mut db = Database::new();
        db.load_font_data(VAR.to_vec());
        // A modified, Basic-Latin subset of the DejaVu master with a synthetic
        // `wght` fvar axis added — renamed off the reserved "DejaVu" name since
        // it is a derivative test artifact, not the real font product.
        let d = RasterDrawer::from_parts(1.0, db, Some("Muri Var Test".to_string()));

        let regular = d.fonts.resolve_face(&FontFamily::System, 400).unwrap();
        let bold = d.fonts.resolve_face(&FontFamily::System, 700).unwrap();
        assert_eq!(
            regular, bold,
            "one variable face: a bold request resolves to the same master"
        );
        let emb = d.fonts.face_embolden(bold, 700);
        assert!(
            matches!(emb, Embolden::Variable(_)),
            "a variable face with a wght axis must instance the axis for bold, \
             not downgrade or synthesize (#65): got {emb:?}"
        );
    }

    /// #65 regression (the live-System bold bug): a variable face registered at a
    /// **heavy default weight** (OS/2 `usWeightClass` >= `LIGHT_WEIGHT`) must STILL
    /// instance its `wght` axis for a bold request — the registered weight is only
    /// the default instance, not "already bold". This is the exact shape that made
    /// the live macOS SFNS System menu render bold rows identical to regular: the
    /// old `actual_weight >= LIGHT_WEIGHT -> Embolden::None` short-circuit dropped
    /// the instance whenever `fontdb` registered the resolved variable face at a
    /// mid/heavy default, so `face_embolden(700)` returned `None` (ink ratio 1.00).
    #[test]
    fn variable_face_with_heavy_default_still_instances_bold() {
        const VAR: &[u8] = include_bytes!("../../tests/fonts/variable-wght-heavy-test.ttf");
        let mut db = Database::new();
        db.load_font_data(VAR.to_vec());
        let d = RasterDrawer::from_parts(1.0, db, Some("Muri Var Heavy".to_string()));
        let bold = d.fonts.resolve_face(&FontFamily::System, 700).unwrap();
        // Registered at OS/2 weight 600 (>= LIGHT_WEIGHT) yet has a wght axis, so a
        // bold request must instance the axis rather than be treated as already-bold.
        let emb = d.fonts.face_embolden(bold, 700);
        assert!(
            matches!(emb, Embolden::Variable(_)),
            "a variable face with a heavy default weight must still instance bold (#65): got {emb:?}"
        );
    }

    /// #77: a face carrying an `opsz` axis clamps a requested optical size into the
    /// axis's own range — the mechanism behind the "squished menu" fix. SFNS's axis
    /// min is 17, so a 13pt menu resolves to 17 (the *Text* master), not the
    /// default *Display* master. The test font has an `opsz` axis min=17/def=28/max=96.
    #[test]
    fn optical_size_clamps_into_the_faces_opsz_range() {
        const VAR: &[u8] = include_bytes!("../../tests/fonts/variable-opsz-test.ttf");
        let mut db = Database::new();
        db.load_font_data(VAR.to_vec());
        let d = RasterDrawer::from_parts(1.0, db, Some("Muri Var Test".to_string()));
        let face = d.fonts.resolve_face(&FontFamily::System, 400).unwrap();
        // Below the axis min -> clamped up to the min (the #77 "13 -> 17" case).
        assert_eq!(d.fonts.face_opsz(face, 13.0), Some(17.0));
        // Inside the range -> passed through unchanged.
        assert_eq!(d.fonts.face_opsz(face, 50.0), Some(50.0));
        // Above the axis max -> clamped down to the max.
        assert_eq!(d.fonts.face_opsz(face, 200.0), Some(96.0));
    }

    /// #77: a face with no `opsz` axis yields `None` (nothing to instance), so the
    /// optical-size feature is inert on plain faces — a `with_optical_size` caller
    /// never perturbs a font that has no optical masters.
    #[test]
    fn face_without_opsz_axis_yields_no_optical_size() {
        // DejaVu (headless default) is a static face with no variation axes.
        let d = RasterDrawer::new_headless(1.0);
        let face = d.fonts.resolve_face(&FontFamily::System, 400).unwrap();
        assert_eq!(d.fonts.face_opsz(face, 13.0), None);
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

    /// Build a db containing only the vendored DejaVu Sans faces — used as a
    /// stand-in "installed target family" for [`resolve_forced_ui_family`]
    /// tests, since we can't rename a real font's family table entry to
    /// literally read "Segoe UI" for a unit test.
    fn dejavu_only_db() -> Database {
        const DEJAVU_SANS: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        const DEJAVU_SANS_BOLD: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans-Bold.ttf");
        let mut db = Database::new();
        db.load_font_data(DEJAVU_SANS.to_vec());
        db.load_font_data(DEJAVU_SANS_BOLD.to_vec());
        db
    }

    /// Issue #54, step (a): when the target family is actually installed on
    /// the host db, it wins over every fallback.
    #[test]
    fn resolve_forced_ui_family_picks_the_target_family_when_installed() {
        let db = dejavu_only_db();
        // Stand in for "Segoe UI is installed": the target list's first hit
        // ("DejaVu Sans" here) must be returned, not a later fallback entry.
        let resolved = resolve_forced_ui_family(&db, &["DejaVu Sans"], &["Liberation Sans"]);
        assert_eq!(resolved.as_deref(), Some("DejaVu Sans"));
    }

    /// Issue #54, step (b): when the target family isn't installed, resolution
    /// falls through to the free fallback list — not to nothing.
    #[test]
    fn resolve_forced_ui_family_falls_back_to_the_free_face_when_target_is_absent() {
        let db = dejavu_only_db();
        let resolved =
            resolve_forced_ui_family(&db, &["Segoe UI"], &["DejaVu Sans", "Liberation Sans"]);
        assert_eq!(resolved.as_deref(), Some("DejaVu Sans"));
    }

    /// Issue #54, step (c) — the core requirement this issue is about: when
    /// NEITHER the target family NOR any free fallback is installed, the
    /// resolver returns `None` rather than ever reaching for the HOST's
    /// resolved native UI family. This is what `resolve_ui_family` itself
    /// *would* return on this same db (it happily resolves "DejaVu Sans" as a
    /// usable host UI family) — proving `resolve_forced_ui_family` never
    /// silently substitutes it.
    #[test]
    fn resolve_forced_ui_family_never_falls_back_to_the_host_ui_family() {
        let db = dejavu_only_db();

        // Sanity: the host-native resolver *would* find a usable UI family on
        // this db (DejaVu Sans qualifies for both weights).
        assert_eq!(resolve_ui_family(&db).as_deref(), Some("DejaVu Sans"));

        // But when a forced theme's own target + fallback lists both miss,
        // the forced resolver must NOT reach for that host family.
        let resolved = resolve_forced_ui_family(&db, &["Segoe UI"], &["Nonexistent Free Face"]);
        assert_eq!(resolved, None);
        assert_ne!(resolved, resolve_ui_family(&db));
    }

    /// Each [`OsFamily`]'s target family list is what actually gets tried
    /// first end-to-end through `with_forced_theme`'s resolution helper.
    #[test]
    fn os_family_target_lists_are_tried_before_their_fallbacks() {
        let db = dejavu_only_db();
        // None of Windows/macOS/GNOME's real target families are the
        // vendored DejaVu Sans, so on this db every forced family must miss
        // its target list and land on its OWN fallback list's DejaVu Sans
        // entry (Windows/macOS list it; GNOME's list also includes it) —
        // never on some unrelated host pin.
        for family in [OsFamily::MacOs, OsFamily::Windows, OsFamily::Gnome] {
            let resolved = resolve_forced_ui_family(
                &db,
                family.ui_font_families(),
                family.fallback_font_families(),
            );
            assert_eq!(
                resolved.as_deref(),
                Some("DejaVu Sans"),
                "{family:?} should land on its free fallback face on a DejaVu-only db"
            );
        }
    }

    /// `bundled-fonts`, tier (2): with the feature on and NEITHER the real
    /// target font NOR any free fallback installed (an empty db), forcing each
    /// OS look registers and resolves to that OS's vendored OSS substitute —
    /// Inter for macOS, Selawik for Windows, Cantarell for GNOME — never a
    /// wrong-platform host face.
    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn bundled_forced_family_resolves_to_the_vendored_substitute_when_target_absent() {
        for (family, expected) in [
            (OsFamily::MacOs, "Inter"),
            (OsFamily::Windows, "Selawik"),
            (OsFamily::Gnome, "Cantarell"),
        ] {
            // Empty db: no SF Pro / Segoe UI / Cantarell, and no free fallback
            // face either — the substitute is the only thing that can resolve.
            let mut db = Database::new();
            let resolved = resolve_forced_ui_family_bundled(&mut db, family);
            assert_eq!(
                resolved.as_deref(),
                Some(expected),
                "{family:?} with no target/fallback installed must use its bundled substitute"
            );
            // And the substitute really is resolvable at both weights (regular +
            // bold are distinct faces, so a bold header and regular row match).
            let regular = query_face(&db, DbFamily::Name(expected), 400);
            let bold = query_face(&db, DbFamily::Name(expected), 700);
            assert!(regular.is_some() && bold.is_some());
            assert_ne!(regular, bold, "{expected} must expose a distinct bold face");
        }
    }

    /// `bundled-fonts`, tier ordering (2) before (3): the bundled substitute is
    /// tried BEFORE the free host fallback list, so even when a fallback-list
    /// face is installed the substitute still wins. DejaVu Sans is in Windows'
    /// `fallback_font_families`, yet forcing Windows must land on Selawik, not
    /// DejaVu.
    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn bundled_substitute_beats_the_free_host_fallback() {
        const DEJAVU: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        let mut db = Database::new();
        db.load_font_data(DEJAVU.to_vec()); // a Windows free-fallback face is present
        assert!(
            query_face(&db, DbFamily::Name("DejaVu Sans"), 400).is_some(),
            "precondition: the free fallback face is installed"
        );
        let resolved = resolve_forced_ui_family_bundled(&mut db, OsFamily::Windows);
        assert_eq!(
            resolved.as_deref(),
            Some("Selawik"),
            "the bundled substitute must be preferred over the free host fallback"
        );
    }

    /// `bundled-fonts`, tier (1) OEM-present wins: when the real target font IS
    /// installed, it is used and the bundled substitute is never reached. On
    /// macOS the target list's "Helvetica Neue"/SF faces are installed, so
    /// forcing the macOS look over the live system db must resolve to a real
    /// target face, not "Inter".
    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn bundled_oem_target_present_wins_over_the_substitute() {
        let mut db = cached_system_fonts_db();
        // Precondition for this assertion to be meaningful: at least one macOS
        // target face is actually installed on the host.
        if first_installed_family(&db, OsFamily::MacOs.ui_font_families()).is_none() {
            return; // no target face installed (non-macOS host); nothing to prove
        }
        let resolved = resolve_forced_ui_family_bundled(&mut db, OsFamily::MacOs);
        assert!(resolved.is_some());
        assert_ne!(
            resolved.as_deref(),
            Some("Inter"),
            "an installed real target face must win over the bundled substitute"
        );
    }

    /// Feature-off contrast (always compiled): the read-only two-tier resolver
    /// has no bundled middle tier, so with only a free-fallback face installed
    /// (DejaVu Sans, in Windows' fallback list) forcing Windows lands on that
    /// fallback — proving the bundled substitute is purely additive and the
    /// feature-off path is unchanged.
    #[test]
    fn forced_resolution_without_bundling_uses_the_free_fallback() {
        const DEJAVU: &[u8] = include_bytes!("../../tests/fonts/DejaVuSans.ttf");
        let mut db = Database::new();
        db.load_font_data(DEJAVU.to_vec());
        let resolved = resolve_forced_ui_family(
            &db,
            OsFamily::Windows.ui_font_families(),
            OsFamily::Windows.fallback_font_families(),
        );
        assert_eq!(resolved.as_deref(), Some("DejaVu Sans"));
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
