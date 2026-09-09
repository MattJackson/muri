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

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::layout::{resolve_segments, SegmentMetrics};
use crate::menu::{Icon, Item, MenuId, Row, Segment};
use crate::render::{SceneDrawer, TextRun};
use crate::style::{Font, Rgba, Weight};
use crate::theme::{MenuOptions, Theme};
use crate::Menu;

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
/// inter-segment gaps), using the drawer's text metrics.
fn row_intrinsic<D: SceneDrawer>(d: &D, row: &Row, base: &Font, gap: f32) -> f32 {
    if row.segments.is_empty() {
        return 0.0;
    }
    let mut w = 0.0;
    for (i, seg) in row.segments.iter().enumerate() {
        let font = row_font(row, seg, base);
        w += d.measure_text(&seg.text, &font);
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
fn style_pieces(
    seg: &Segment,
    base_color: Rgba,
    base_weight: Weight,
) -> Vec<(String, Rgba, Weight)> {
    if seg.runs.is_empty() {
        return vec![(seg.text.clone(), base_color, base_weight)];
    }
    // Map each char to (color, weight) by walking UTF-16 offsets.
    let theme = Theme::light(); // literal/system colors resolve theme-independently
    let mut pieces: Vec<(String, Rgba, Weight)> = Vec::new();
    let mut u16_idx = 0usize;
    for ch in seg.text.chars() {
        let mut color = base_color;
        let mut weight = base_weight;
        for run in &seg.runs {
            if u16_idx >= run.start && u16_idx < run.start + run.len {
                color = theme.resolve(run.color);
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

fn decode_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let pm = tiny_skia::Pixmap::decode_png(bytes).ok()?;
    let (w, h) = (pm.width(), pm.height());
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (i, px) in pm.pixels().iter().enumerate() {
        let a = px.alpha();
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else {
            let a32 = a as u32;
            (
                (px.red() as u32 * 255 / a32).min(255) as u8,
                (px.green() as u32 * 255 / a32).min(255) as u8,
                (px.blue() as u32 * 255 / a32).min(255) as u8,
            )
        };
        out[i * 4] = r;
        out[i * 4 + 1] = g;
        out[i * 4 + 2] = b;
        out[i * 4 + 3] = a;
    }
    Some((out, w, h))
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

    // ---- Pass 1: width & height ----
    let mut max_content = 0.0_f32;
    for item in &menu.items {
        if let Some(row) = item_row(item) {
            let font = if matches!(item, Item::SectionHeader(_)) {
                &theme.header_font
            } else {
                &base_font
            };
            max_content = max_content.max(row_intrinsic(drawer, row, font, gap));
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
                if let Some((rgba, w, h)) = decode_png(bytes) {
                    drawer.draw_image(&rgba, w, h, icon_rect);
                }
            }
            Some(Icon::Checkmark) => draw_glyph_centered(
                drawer,
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
                SegmentMetrics::new(drawer.measure_text(&seg.text, &font), seg.flex, seg.align)
            })
            .collect();
        let boxes = resolve_segments(&metrics, band_w);
        for (seg, bx) in row.segments.iter().zip(boxes.iter()) {
            let font = row_font(row, seg, base_font);
            let lh = drawer.line_height(&font);
            let text_top = ry + (rh - lh) / 2.0;
            let seg_base = if highlighted {
                Rgba::WHITE
            } else if let Some(c) = seg.color {
                theme.resolve(c)
            } else {
                base_color
            };
            let pieces = style_pieces(seg, seg_base, font.weight);
            let mut px = band_x + bx.text_x;
            for (text, color, weight) in pieces {
                let pf = font.clone().with_weight(weight);
                let w = drawer.measure_text(&text, &pf);
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

fn draw_glyph_centered<D: SceneDrawer>(
    drawer: &mut D,
    glyph: &str,
    base_font: &Font,
    weight: Weight,
    color: Rgba,
    rect: LogicalRect,
) {
    let font = base_font.clone().with_weight(weight);
    let w = drawer.measure_text(glyph, &font);
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
        let any_opaque = d.pixmap().pixels().iter().any(|p| p.alpha() > 0);
        assert!(
            any_opaque,
            "rendered pixmap should not be fully transparent"
        );
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
}
