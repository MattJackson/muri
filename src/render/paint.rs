//! Painting a [`Menu`] through a [`SceneDrawer`]: the single, platform-agnostic
//! layout + draw pass that turns the declarative menu tree into pixels and a
//! hit-test map. It measures text through the drawer, resolves the
//! [`Flex`](crate::Flex)/[`Align`](crate::Align) layout with
//! [`crate::layout::resolve_segments`] (the flush-right promise), and emits
//! fills, separators, icons, and text runs.
//!
//! [`render_menu`] is called once for a snapshot and once per frame by the live
//! popup (cheap — menus are small); it returns a [`LaidMenu`] describing the
//! popup size and the clickable rows for hit-testing.

use std::collections::HashMap;

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::layout::{resolve_segments, SegmentMetrics};
use crate::menu::{Icon, Item, MenuId, Row, Segment};
use crate::render::{SceneDrawer, TextRun};
use crate::style::{Font, FontFamily, Rgba, Weight};
use crate::theme::{MenuOptions, Theme};
use crate::Menu;

/// A scratch memo of `(text, font)` -> measured width, cleared at the start of
/// every [`render_menu`] call. A row's segments are measured once for the width
/// pass (`row_intrinsic`) and again while laying out the draw pass
/// (`draw_row_content`'s `SegmentMetrics` + per-styled-piece widths); without
/// this cache the same `(text, font)` gets re-shaped through the drawer's text
/// engine up to 3× per frame for no behavioral difference.
type MeasureCache = HashMap<MeasureKey, f32>;

#[derive(PartialEq, Eq, Hash)]
struct MeasureKey {
    text: String,
    family_tag: u8,
    family_name: String,
    size_bits: u32,
    weight: u16,
}

impl MeasureKey {
    fn new(text: &str, font: &Font) -> Self {
        let (family_tag, family_name) = match &font.family {
            FontFamily::System => (0u8, String::new()),
            FontFamily::SystemMono => (1u8, String::new()),
            FontFamily::Named(name) => (2u8, name.clone()),
        };
        MeasureKey {
            text: text.to_string(),
            family_tag,
            family_name,
            size_bits: font.size.to_bits(),
            weight: font.weight.ot_weight(),
        }
    }
}

/// Measure `text` in `font` through `drawer`, memoizing in `cache` so an
/// identical `(text, font)` pair within the same frame is only shaped once.
/// Output is identical to calling `drawer.measure_text` directly every time.
fn measure_cached<D: SceneDrawer>(
    drawer: &D,
    cache: &mut MeasureCache,
    text: &str,
    font: &Font,
) -> f32 {
    let key = MeasureKey::new(text, font);
    if let Some(&w) = cache.get(&key) {
        return w;
    }
    let w = drawer.measure_text(text, font);
    cache.insert(key, w);
    w
}

/// A resolved, clickable row in a painted menu (window-relative logical coords).
#[derive(Clone, Debug)]
pub struct LaidRow {
    /// Index of the item within the menu's top-level `items`.
    pub index: usize,
    /// The row's bounding rectangle in logical, window-relative coordinates.
    pub rect: LogicalRect,
    /// The row's click id (may be [`MenuId::none`] for inert rows).
    pub id: MenuId,
    /// Whether the row is interactive (clickable / highlightable).
    pub interactive: bool,
}

/// The result of painting a menu: its logical size and the clickable rows.
#[derive(Clone, Debug, Default)]
pub struct LaidMenu {
    /// Total popup size in logical pixels.
    pub size: LogicalSize,
    /// One entry per top-level item, in order (for hit-testing / highlight).
    pub rows: Vec<LaidRow>,
}

impl LaidMenu {
    /// The top-level item index at a window-relative logical point, if any
    /// interactive row contains it.
    pub fn hit(&self, point: LogicalPoint) -> Option<usize> {
        self.rows
            .iter()
            .find(|r| r.interactive && r.rect.contains(point))
            .map(|r| r.index)
    }

    /// The clickable [`MenuId`] at a point, if it lands on an interactive row.
    pub fn id_at(&self, point: LogicalPoint) -> Option<MenuId> {
        self.rows
            .iter()
            .find(|r| r.interactive && r.rect.contains(point))
            .map(|r| r.id.clone())
    }
}

const SEPARATOR_HEIGHT: f32 = 11.0;
const ICON_SIZE: f32 = 16.0;
const LEADING_COLUMN: f32 = 20.0;
const TRAILING_COLUMN: f32 = 14.0;
const DEFAULT_MIN_WIDTH: f32 = 200.0;
const DEFAULT_MAX_WIDTH: f32 = 380.0;

fn is_submenu(item: &Item) -> bool {
    matches!(item, Item::Submenu { .. })
}

/// Whether any item needs a leading column (icon or check state).
fn needs_leading(menu: &Menu) -> bool {
    menu.items.iter().any(|it| match it {
        Item::Row(r) | Item::SectionHeader(r) => r.leading.is_some() || r.checked.is_some(),
        Item::Submenu { label, .. } => label.leading.is_some() || label.checked.is_some(),
        Item::Separator => false,
    })
}

fn row_font(row: &Row, seg: &Segment, base: &Font) -> Font {
    if let Some(f) = &seg.font {
        f.clone()
    } else {
        let mut f = base.clone();
        // A bold-labelled row (usagio's active account) carries weight on the
        // segment font; nothing extra to do here.
        let _ = row;
        f.weight = base.weight;
        f
    }
}

/// Measure the intrinsic width a row's segments want (sum of segment widths plus
/// inter-segment gaps), using the drawer's text metrics. Weight-aware and split
/// exactly as the (un-highlighted) draw pass will render the row, so a segment
/// carrying a bolder `StyleRun` is sized at the bold advance — the popup width
/// then never under-fits the text. `base_color` is the row's resolved base color
/// (the highlight overlay is a hover-time repaint that must not resize the
/// popup, so the width pass always measures the un-highlighted split).
fn row_intrinsic<D: SceneDrawer>(
    d: &D,
    cache: &mut MeasureCache,
    row: &Row,
    base: &Font,
    base_color: Rgba,
    theme: &Theme,
    gap: f32,
) -> f32 {
    if row.segments.is_empty() {
        return 0.0;
    }
    let mut w = 0.0;
    for (i, seg) in row.segments.iter().enumerate() {
        let font = row_font(row, seg, base);
        let seg_base = seg_base_color(seg, theme, base_color, false);
        w += measure_segment(d, cache, seg, &font, theme, seg_base, false);
        if i + 1 < row.segments.len() {
            w += gap;
        }
    }
    w
}

fn item_row(item: &Item) -> Option<&Row> {
    match item {
        Item::Row(r) | Item::SectionHeader(r) => Some(r),
        Item::Submenu { label, .. } => Some(label),
        Item::Separator => None,
    }
}

/// Split a segment's text into consecutive styled pieces by its UTF-16
/// [`StyleRun`](crate::StyleRun) spans, falling back to `base_color`/`base_weight`.
///
/// `StyleRun` colors resolve against the **live** `theme` so a semantic run color
/// (`Label`/`SecondaryLabel`/`Accent`/`Separator`) is correct in dark mode, not
/// baked against a hard-coded light palette (spec §7.3).
///
/// When `highlighted` (the row is filled with the accent and every other glyph —
/// label, checkmark, chevron — is forced to `base_color`, i.e. white), a run's
/// semantic color is **suppressed** so styled runs invert with the rest of the
/// row instead of rendering, say, saturated red on accent blue. Per-run *weight*
/// overrides still apply in both states.
fn style_pieces(
    seg: &Segment,
    base_color: Rgba,
    base_weight: Weight,
    theme: &Theme,
    highlighted: bool,
) -> Vec<(String, Rgba, Weight)> {
    if seg.runs.is_empty() {
        return vec![(seg.text.clone(), base_color, base_weight)];
    }
    // Map each char to (color, weight) by walking UTF-16 offsets.
    let mut pieces: Vec<(String, Rgba, Weight)> = Vec::new();
    let mut u16_idx = 0usize;
    for ch in seg.text.chars() {
        let mut color = base_color;
        let mut weight = base_weight;
        for run in &seg.runs {
            // `start`/`len` are public `StyleRun` fields set by the caller;
            // saturating add so a pathological `start + len` can't overflow
            // `usize` and panic under debug overflow checks.
            if u16_idx >= run.start && u16_idx < run.start.saturating_add(run.len) {
                if !highlighted {
                    color = theme.resolve(run.color);
                }
                if let Some(w) = run.weight {
                    weight = w;
                }
                break;
            }
        }
        match pieces.last_mut() {
            Some((s, c, w)) if *c == color && *w == weight => s.push(ch),
            _ => pieces.push((ch.to_string(), color, weight)),
        }
        u16_idx += ch.len_utf16();
    }
    pieces
}

/// The effective base color for a segment's un-styled chars, matching what the
/// draw pass uses: the highlight override (white) wins; otherwise an explicit
/// per-segment color; else the row's base color. Shared by the width pass, the
/// draw-pass metrics, and the draw loop so all three split into *identical*
/// pieces (piece boundaries depend on this color, and a different split changes
/// the summed width via cross-piece kerning).
fn seg_base_color(seg: &Segment, theme: &Theme, base_color: Rgba, highlighted: bool) -> Rgba {
    if highlighted {
        Rgba::WHITE
    } else if let Some(c) = seg.color {
        theme.resolve(c)
    } else {
        base_color
    }
}

/// The rendered width of a segment, measured the *same way it is drawn*: each
/// `StyleRun` weight override changes glyph advances, so the width is the sum of
/// the per-piece advances at each piece's weight — not the whole string measured
/// once at the base weight (which under-sizes a segment containing a bolder run
/// and lets a right-aligned / flex run overflow its box). `base_color` and
/// `highlighted` must be the same values the draw pass will use for this segment
/// so the piece split — and hence the summed width — is identical to the drawn
/// advance.
fn measure_segment<D: SceneDrawer>(
    drawer: &D,
    cache: &mut MeasureCache,
    seg: &Segment,
    font: &Font,
    theme: &Theme,
    base_color: Rgba,
    highlighted: bool,
) -> f32 {
    style_pieces(seg, base_color, font.weight, theme, highlighted)
        .iter()
        .map(|(text, _color, weight)| {
            let pf = font.clone().with_weight(*weight);
            measure_cached(drawer, cache, text, &pf)
        })
        .sum()
}

/// Lay out and paint `menu` through `drawer`, highlighting the top-level item at
/// `highlight` (usually the row under the cursor). Returns the popup size and
/// clickable row rectangles for hit-testing.
pub fn render_menu<D: SceneDrawer>(
    drawer: &mut D,
    menu: &Menu,
    theme: &Theme,
    opts: &MenuOptions,
    highlight: Option<usize>,
) -> LaidMenu {
    let pad = theme.padding;
    let gap = theme.column_gap;
    let base_font = theme.row_font.clone();

    let leading_w = if needs_leading(menu) {
        LEADING_COLUMN
    } else {
        0.0
    };
    let trailing_w = if menu.items.iter().any(is_submenu) {
        TRAILING_COLUMN
    } else {
        0.0
    };

    // Scratch text-measurement memo, cleared every call (see `MeasureCache`).
    let mut measure_cache: MeasureCache = HashMap::new();

    // ---- Pass 1: width & height ----
    let mut max_content = 0.0_f32;
    for item in &menu.items {
        if let Some(row) = item_row(item) {
            let is_header = matches!(item, Item::SectionHeader(_));
            let font = if is_header {
                &theme.header_font
            } else {
                &base_font
            };
            // The row's un-highlighted base color, matching draw_row_content so
            // the width pass splits into the same pieces the draw pass advances.
            let base_color = if is_header || !row.enabled {
                theme.resolve(theme.secondary_label)
            } else {
                theme.resolve(theme.label)
            };
            max_content = max_content.max(row_intrinsic(
                drawer,
                &mut measure_cache,
                row,
                font,
                base_color,
                theme,
                gap,
            ));
        }
    }

    let min_w = opts.min_width.unwrap_or(DEFAULT_MIN_WIDTH);
    let max_w = opts.max_width.unwrap_or(DEFAULT_MAX_WIDTH);
    let desired = pad.left + leading_w + max_content + trailing_w + pad.right;
    let width = desired.clamp(min_w, max_w);

    let band_x = pad.left + leading_w;
    let band_w = (width - pad.right - trailing_w - band_x).max(1.0);

    let mut y = pad.top;
    let mut rows: Vec<LaidRow> = Vec::new();
    let mut plan: Vec<(usize, f32, f32)> = Vec::new(); // (item index, y, height)
    for (i, item) in menu.items.iter().enumerate() {
        let h = match item {
            Item::Separator => SEPARATOR_HEIGHT,
            Item::SectionHeader(_) => theme.row_height,
            Item::Row(r) | Item::Submenu { label: r, .. } => {
                theme.row_height.max(r.min_height.unwrap_or(0.0))
            }
        };
        plan.push((i, y, h));
        y += h;
    }
    let height = y + pad.bottom;
    let size = LogicalSize::new(width, height);

    // ---- Pass 2: draw ----
    drawer.begin_frame(size);
    let bg = theme.resolve(theme.background);
    drawer.fill_round_rect(
        LogicalRect::new(LogicalPoint::new(0.0, 0.0), size),
        theme.corner_radius,
        bg,
    );

    for (i, ry, rh) in plan {
        let item = &menu.items[i];
        match item {
            Item::Separator => {
                let sep = LogicalRect::new(
                    LogicalPoint::new(pad.left, ry + rh / 2.0),
                    LogicalSize::new(width - pad.horizontal(), 1.0),
                );
                drawer.draw_separator(sep, theme.resolve(theme.separator));
            }
            Item::SectionHeader(row) => {
                draw_row_content(
                    drawer,
                    &mut measure_cache,
                    row,
                    theme,
                    &theme.header_font,
                    band_x,
                    band_w,
                    leading_w,
                    ry,
                    rh,
                    false,
                    false,
                    theme.resolve(theme.secondary_label),
                );
            }
            Item::Row(row) | Item::Submenu { label: row, .. } => {
                let submenu = is_submenu(item);
                let interactive = if submenu {
                    row.enabled
                } else {
                    row.enabled && !row.id.is_none()
                };
                let highlighted = interactive && highlight == Some(i);
                let rect =
                    LogicalRect::new(LogicalPoint::new(0.0, ry), LogicalSize::new(width, rh));
                if highlighted {
                    let hl = LogicalRect::new(
                        LogicalPoint::new(pad.left - 2.0, ry + 1.0),
                        LogicalSize::new(width - pad.horizontal() + 4.0, rh - 2.0),
                    );
                    drawer.fill_round_rect(hl, 5.0, theme.resolve(theme.accent));
                }
                let base_color = if highlighted {
                    Rgba::WHITE
                } else if !row.enabled {
                    theme.resolve(theme.secondary_label)
                } else {
                    theme.resolve(theme.label)
                };
                draw_row_content(
                    drawer,
                    &mut measure_cache,
                    row,
                    theme,
                    &base_font,
                    band_x,
                    band_w,
                    leading_w,
                    ry,
                    rh,
                    submenu,
                    highlighted,
                    base_color,
                );
                rows.push(LaidRow {
                    index: i,
                    rect,
                    id: row.id.clone(),
                    interactive,
                });
            }
        }
    }

    LaidMenu { size, rows }
}

#[allow(clippy::too_many_arguments)]
fn draw_row_content<D: SceneDrawer>(
    drawer: &mut D,
    cache: &mut MeasureCache,
    row: &Row,
    theme: &Theme,
    base_font: &Font,
    band_x: f32,
    band_w: f32,
    leading_w: f32,
    ry: f32,
    rh: f32,
    submenu: bool,
    highlighted: bool,
    base_color: Rgba,
) {
    // Leading icon / checkmark column.
    if leading_w > 0.0 {
        let icon_rect = LogicalRect::new(
            LogicalPoint::new(
                theme.padding.left + (LEADING_COLUMN - ICON_SIZE) / 2.0,
                ry + (rh - ICON_SIZE) / 2.0,
            ),
            LogicalSize::new(ICON_SIZE, ICON_SIZE),
        );
        match &row.leading {
            Some(Icon::Png(bytes)) => {
                if let Some(decoded) = drawer.decode_icon(bytes) {
                    let (rgba, w, h) = &*decoded;
                    drawer.draw_image(rgba, *w, *h, icon_rect);
                }
            }
            Some(Icon::Checkmark) => draw_glyph_centered(
                drawer,
                cache,
                "\u{2713}",
                base_font,
                Weight::Bold,
                if highlighted {
                    Rgba::WHITE
                } else {
                    theme.resolve(theme.accent)
                },
                icon_rect,
            ),
            // SVG / Symbol icons are Phase 2 (rasterize per-DPI); skip for now.
            _ => {
                if row.checked == Some(true) {
                    draw_glyph_centered(
                        drawer,
                        cache,
                        "\u{2713}",
                        base_font,
                        Weight::Bold,
                        if highlighted {
                            Rgba::WHITE
                        } else {
                            theme.resolve(theme.accent)
                        },
                        icon_rect,
                    );
                }
            }
        }
    }

    // Segment band.
    if !row.segments.is_empty() {
        let metrics: Vec<SegmentMetrics> = row
            .segments
            .iter()
            .map(|seg| {
                let font = row_font(row, seg, base_font);
                let seg_base = seg_base_color(seg, theme, base_color, highlighted);
                SegmentMetrics::new(
                    measure_segment(drawer, cache, seg, &font, theme, seg_base, highlighted),
                    seg.flex,
                    seg.align,
                )
            })
            .collect();
        let boxes = resolve_segments(&metrics, band_w);
        for (seg, bx) in row.segments.iter().zip(boxes.iter()) {
            let font = row_font(row, seg, base_font);
            let lh = drawer.line_height(&font);
            let text_top = ry + (rh - lh) / 2.0;
            let seg_base = seg_base_color(seg, theme, base_color, highlighted);
            let pieces = style_pieces(seg, seg_base, font.weight, theme, highlighted);
            let mut px = band_x + bx.text_x;
            for (text, color, weight) in pieces {
                let pf = font.clone().with_weight(weight);
                let w = measure_cached(drawer, cache, &text, &pf);
                drawer.draw_text(&TextRun {
                    text: &text,
                    origin: LogicalPoint::new(px, text_top),
                    font: &pf,
                    color,
                    weight,
                });
                px += w;
            }
        }
    }

    // Trailing submenu chevron.
    if submenu {
        let chev_rect = LogicalRect::new(
            LogicalPoint::new(band_x + band_w, ry),
            LogicalSize::new(TRAILING_COLUMN, rh),
        );
        draw_glyph_centered(
            drawer,
            cache,
            "\u{203A}", // ›
            base_font,
            Weight::Regular,
            if highlighted {
                Rgba::WHITE
            } else {
                theme.resolve(theme.secondary_label)
            },
            chev_rect,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_glyph_centered<D: SceneDrawer>(
    drawer: &mut D,
    cache: &mut MeasureCache,
    glyph: &str,
    base_font: &Font,
    weight: Weight,
    color: Rgba,
    rect: LogicalRect,
) {
    let font = base_font.clone().with_weight(weight);
    let w = measure_cached(drawer, cache, glyph, &font);
    let lh = drawer.line_height(&font);
    let origin = LogicalPoint::new(
        rect.origin.x + (rect.size.width - w) / 2.0,
        rect.origin.y + (rect.size.height - lh) / 2.0,
    );
    drawer.draw_text(&TextRun {
        text: glyph,
        origin,
        font: &font,
        color,
        weight,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Align, Flex, StyleRun};
    use crate::style::Color;

    fn demo_menu() -> Menu {
        Menu::new()
            .section_header(Row::info().label("Claude"))
            .row(
                Row::new("switch:claude:me")
                    .leading(Icon::Checkmark)
                    .checked(true)
                    .segments(vec![
                        Segment::new("me@example.com")
                            .flex(Flex::Grow)
                            .font(Font::system(13.0, Weight::Bold)),
                        Segment::new("47% / 89%")
                            .align(Align::Right)
                            .runs(vec![StyleRun::new(6, 3, Color::SystemRed)]),
                    ]),
            )
            .separator()
            .submenu(Row::new("settings").label("Settings"), Menu::new())
            .row(Row::new("quit").segments(vec![
                Segment::new("Quit").flex(Flex::Grow),
                Segment::new("usagio v1")
                    .align(Align::Right)
                    .color(Color::SecondaryLabel),
            ]))
    }

    #[test]
    fn repeated_render_reuses_shaped_runs_no_reshaping() {
        // A hover-highlight repaints unchanged text; after the first render, a
        // second identical render must re-shape nothing (all runs served from the
        // shaped cache) — the fix for the ~0.5s hover latency. Instruments the
        // real shape() miss counter rather than wall-clock (non-flaky).
        let mut d = crate::render::RasterDrawer::new(2.0);
        let menu = demo_menu();
        let _ = render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);
        let after_first = d.shape_miss_count();
        assert!(after_first > 0, "the first render shapes its runs");
        // Re-render the same menu with a different highlight (a hover change).
        let _ = render_menu(
            &mut d,
            &menu,
            &Theme::dark(),
            &MenuOptions::default(),
            Some(0),
        );
        assert_eq!(
            d.shape_miss_count(),
            after_first,
            "a repaint of unchanged text must not re-shape any run"
        );
    }

    #[test]
    fn render_produces_nonempty_size_and_hits() {
        let mut d = crate::render::RasterDrawer::new(2.0);
        let laid = render_menu(
            &mut d,
            &demo_menu(),
            &Theme::dark(),
            &MenuOptions::default(),
            None,
        );
        assert!(laid.size.width > 100.0 && laid.size.height > 60.0);
        // Interactive rows: the account, the submenu, and quit (not header/sep).
        assert_eq!(laid.rows.iter().filter(|r| r.interactive).count(), 3);
        // Device pixmap matches logical size * scale, and is non-blank.
        let (dw, dh) = d.device_size();
        assert_eq!(dw, (laid.size.width * 2.0).round() as u32);
        assert_eq!(dh, (laid.size.height * 2.0).round() as u32);
        let any_opaque = d.framebuffer().pixels().chunks_exact(4).any(|p| p[3] > 0);
        assert!(
            any_opaque,
            "rendered pixmap should not be fully transparent"
        );
    }

    /// Regression for spec §7.3: a `StyleRun` carrying a **semantic** color must
    /// resolve against the *active* theme, not a hard-coded `Theme::light()`, so
    /// a themed run reads correctly in dark mode. `Color::Label` is white on dark
    /// and black on light; the old seam returned black in both.
    #[test]
    fn style_run_semantic_color_follows_active_theme() {
        // A two-char segment whose second char is a `Color::Label` style run.
        let seg = Segment::new("ab").runs(vec![StyleRun::new(1, 1, Color::Label)]);
        let base = Rgba::opaque(1, 2, 3);

        let dark = style_pieces(&seg, base, Weight::Regular, &Theme::dark(), false);
        let light = style_pieces(&seg, base, Weight::Regular, &Theme::light(), false);

        // The styled piece ("b") resolves Label against each theme.
        let dark_b = dark.iter().find(|(s, ..)| s == "b").expect("styled piece");
        let light_b = light.iter().find(|(s, ..)| s == "b").expect("styled piece");
        assert_eq!(dark_b.1, Theme::dark().resolve(Color::Label));
        assert_eq!(light_b.1, Theme::light().resolve(Color::Label));
        // And they actually differ (white vs black), proving the theme is live.
        assert_ne!(dark_b.1, light_b.1);
    }

    /// On a highlighted (accent-filled) row every glyph inverts to the base
    /// color; a `StyleRun`'s semantic color must be suppressed so it doesn't
    /// render, e.g., saturated red on accent blue — but weight overrides stay.
    #[test]
    fn style_run_color_is_suppressed_when_highlighted_but_weight_is_kept() {
        let seg = Segment::new("ab").runs(vec![
            StyleRun::new(1, 1, Color::SystemRed).weight(Weight::Bold)
        ]);
        let white = Rgba::WHITE;

        let hot = style_pieces(&seg, white, Weight::Regular, &Theme::light(), true);
        for (_s, c, _w) in &hot {
            assert_eq!(
                *c, white,
                "highlighted row keeps every piece at the base color"
            );
        }
        let hot_b = hot.iter().find(|(s, ..)| s == "b").expect("styled piece");
        assert_eq!(hot_b.2, Weight::Bold, "weight override survives highlight");

        // Un-highlighted, the semantic color resolves as before.
        let cold = style_pieces(&seg, white, Weight::Regular, &Theme::light(), false);
        let cold_b = cold.iter().find(|(s, ..)| s == "b").expect("styled piece");
        assert_eq!(cold_b.1, Theme::light().resolve(Color::SystemRed));
    }

    /// A segment carrying a bolder `StyleRun` must be measured at the run's
    /// weight, not the base weight, so its box matches what `draw_row_content`
    /// actually advances (else a right-aligned run overflows). The measured
    /// width equals the sum of the per-piece advances the draw path uses.
    #[test]
    fn measure_segment_accounts_for_per_run_weight() {
        let d = crate::render::RasterDrawer::new_headless(1.0);
        let mut cache = MeasureCache::new();
        let font = Font::default();
        let theme = Theme::light();

        let seg =
            Segment::new("Quit").runs(vec![StyleRun::new(0, 4, Color::Label).weight(Weight::Bold)]);
        let measured = measure_segment(&d, &mut cache, &seg, &font, &theme, Rgba::WHITE, false);

        // Same computation the draw loop performs, piece by piece.
        let drawn: f32 = style_pieces(&seg, Rgba::WHITE, font.weight, &theme, false)
            .iter()
            .map(|(t, _c, w)| measure_cached(&d, &mut cache, t, &font.clone().with_weight(*w)))
            .sum();
        assert_eq!(measured, drawn);

        // And it is never narrower than the naive base-weight measure the old
        // code used (bold advances are >= regular for the vendored face).
        let naive = measure_cached(&d, &mut cache, &seg.text, &font);
        assert!(measured >= naive);
    }

    #[test]
    fn hit_testing_maps_points_to_rows() {
        let mut d = crate::render::RasterDrawer::new(1.0);
        let laid = render_menu(
            &mut d,
            &demo_menu(),
            &Theme::light(),
            &MenuOptions::default(),
            None,
        );
        let account = &laid.rows[0];
        let center = LogicalPoint::new(
            account.rect.origin.x + account.rect.size.width / 2.0,
            account.rect.origin.y + account.rect.size.height / 2.0,
        );
        assert_eq!(laid.hit(center), Some(account.index));
        assert_eq!(laid.id_at(center).unwrap().as_str(), "switch:claude:me");
    }

    /// `measure_cached`'s doc promise ("output is identical to calling
    /// `drawer.measure_text` directly every time") is what makes the
    /// per-frame memo safe: prove both the cold path (first call, a miss)
    /// and the warm path (second call, a hit) return exactly what a direct,
    /// uncached `measure_text` call would — i.e. the memoization can't drift
    /// from the ground truth it's short-circuiting.
    #[test]
    fn measure_cached_matches_a_direct_measure_text_call() {
        let d = crate::render::RasterDrawer::new_headless(1.0);
        let font = Font::system(13.0, Weight::Regular);
        let text = "Settings";

        let direct = d.measure_text(text, &font);

        let mut cache: MeasureCache = HashMap::new();
        let cold = measure_cached(&d, &mut cache, text, &font);
        assert_eq!(
            cold, direct,
            "first (cache-miss) call must match measure_text"
        );
        assert_eq!(cache.len(), 1);

        let warm = measure_cached(&d, &mut cache, text, &font);
        assert_eq!(
            warm, direct,
            "second (cache-hit) call must still match measure_text"
        );
        assert_eq!(
            cache.len(),
            1,
            "a repeat (text, font) must not grow the cache"
        );
    }

    /// Distinct `(text, font)` keys must not collide in the cache — a
    /// different font size for the same text is a different measurement and
    /// must get its own entry and its own (independently correct) value.
    #[test]
    fn measure_cached_distinguishes_different_fonts_for_the_same_text() {
        let d = crate::render::RasterDrawer::new_headless(1.0);
        let text = "Settings";
        let small = Font::system(11.0, Weight::Regular);
        let large = Font::system(22.0, Weight::Regular);

        let mut cache: MeasureCache = HashMap::new();
        let w_small = measure_cached(&d, &mut cache, text, &small);
        let w_large = measure_cached(&d, &mut cache, text, &large);

        assert_eq!(w_small, d.measure_text(text, &small));
        assert_eq!(w_large, d.measure_text(text, &large));
        assert!(w_large > w_small, "a larger font must measure wider");
        assert_eq!(cache.len(), 2);
    }
}
