//! Painting a [`Menu`] through a [`SceneDrawer`]: the single, platform-agnostic
//! layout + draw pass that turns the declarative menu tree into pixels and a
//! hit-test map. It measures text through the drawer, resolves the
//! [`Flex`](crate::Flex)/[`Align`] layout with
//! [`crate::layout::resolve_segments`] (the flush-right promise), and emits
//! fills, separators, icons, and text runs.
//!
//! [`render_menu`] is called once for a snapshot and once per frame by the live
//! popup (cheap — menus are small); it returns a [`LaidMenu`] describing the
//! popup size and the clickable rows for hit-testing.

use std::collections::HashMap;

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::layout::{resolve_segments, SegmentMetrics};
use crate::menu::{Align, Axis, Content, Icon, Item, MenuId, Row, Segment, Stack};
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
    // Tracking is part of the key: a tracked and untracked measurement of the same
    // text have different widths and must not collide (#42).
    spacing_bits: u32,
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
            spacing_bits: font.letter_spacing.to_bits(),
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
const TRAILING_COLUMN: f32 = 14.0;
const DEFAULT_MIN_WIDTH: f32 = 200.0;
const DEFAULT_MAX_WIDTH: f32 = 380.0;
/// Vertical padding above/below an [`Item::Content`] row's measured stack
/// height, on top of the theme's `row_height` floor (issue #44). Chosen to
/// roughly match the breathing room a text row gets from its line-height
/// centering within `theme.row_height`.
const CONTENT_ROW_VPAD: f32 = 4.0;

/// Opacity a **disabled** row's icon/checkmark is drawn at (issue E): the row's
/// text already dims via `secondary_label`, but its icon and checkmark are drawn
/// in their own (accent/image) colors and would otherwise stay at full opacity —
/// so they are dimmed here to match.
const DISABLED_ALPHA: f32 = 0.5;

/// `v` if it is finite, else `fallback`. Sanitizes a measurement/option before it
/// feeds a `clamp` or geometry: an empty or degenerate input (a `NaN` min width,
/// a non-finite measured stack) must not produce `NaN` geometry (`f32::clamp`
/// even *panics* when its bounds aren't ordered).
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

/// `c` with its alpha scaled by `alpha` (clamped to `[0, 1]`) — the primitive
/// behind [`dim_color`], applied to a checkmark/symbol glyph so a disabled row's
/// glyph dims exactly like a blitted icon does.
fn dim_rgba(c: Rgba, alpha: f32) -> Rgba {
    let alpha = alpha.clamp(0.0, 1.0);
    Rgba::new(c.r, c.g, c.b, (c.a as f32 * alpha).round() as u8)
}

/// `c` unchanged when `enabled`, else dimmed to [`DISABLED_ALPHA`] (issue E).
fn dim_color(c: Rgba, enabled: bool) -> Rgba {
    if enabled {
        c
    } else {
        dim_rgba(c, DISABLED_ALPHA)
    }
}

fn is_submenu(item: &Item) -> bool {
    matches!(item, Item::Submenu { .. })
}

/// The inline leading advance a row consumes for its own icon/checkmark before
/// its text: `ICON_SIZE + gap` when the row carries a leading icon or is checked,
/// else `0`. This is **per-row** and never reserved globally — every row's
/// content starts at the same left x; an icon row simply draws its image first
/// and pushes only *its own* text right (issue #16). A caller wanting a shared
/// checkmark/icon column adds it explicitly.
fn row_leading_width(row: &Row, gap: f32) -> f32 {
    if row.leading.is_some() || row.checked == Some(true) {
        ICON_SIZE + gap
    } else {
        0.0
    }
}

/// Whether the menu reserves a shared leading gutter: true when any row is
/// **checkable** — `checked.is_some()`, i.e. `Some(true)` *or* `Some(false)`, per
/// the `Row::checked` contract that "`Some(true/false)` shows a check column" —
/// so checked *and* currently-unchecked-but-checkable rows align their text past
/// the gutter (the native `NSMenu` look). Testing only `Some(true)` would leave a
/// menu whose checkable rows are all currently unchecked with no reserved column,
/// making every row's text jump right the instant one row is toggled on. A menu
/// with only a section-header icon and no checkable rows reserves nothing and
/// stays per-row inline (#16). This reconciles #16's shared-left-x with native
/// alignment.
fn menu_reserves_gutter(menu: &Menu) -> bool {
    menu.items.iter().any(|it| {
        item_row(it)
            .is_some_and(|r| r.checked.is_some() || matches!(r.leading, Some(Icon::Checkmark)))
    })
}

/// The leading advance a row consumes: the shared gutter width when the menu
/// reserves one (every row, so text aligns), else the row's own inline icon
/// advance (0 for icon-less rows).
fn row_lead(row: &Row, gap: f32, reserve_gutter: bool) -> f32 {
    if reserve_gutter {
        ICON_SIZE + gap
    } else {
        row_leading_width(row, gap)
    }
}

/// The trailing column a row reserves for its own trailing icon/accessory:
/// [`TRAILING_COLUMN`] when the row carries a [`Row::trailing`] icon, else `0`.
/// A submenu's chevron reserves the same column via [`is_submenu`] at the menu
/// level — this is the per-row equivalent for an explicit trailing icon (issue A).
fn row_trailing_width(row: &Row) -> f32 {
    if row.trailing.is_some() {
        TRAILING_COLUMN
    } else {
        0.0
    }
}

/// The font a segment renders in: its own override, else the row's base font.
fn row_font(seg: &Segment, base: &Font) -> Font {
    seg.font.clone().unwrap_or_else(|| base.clone())
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
        let font = row_font(seg, base);
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
        Item::Separator | Item::Content(_) => None,
    }
}

// -----------------------------------------------------------------------------
// Content stacks (issue #44): recursive measure + paint over a `Stack` tree.
// -----------------------------------------------------------------------------

/// The intrinsic (unconstrained) size of one [`Content`] node: text measured
/// through the drawer's shaper at its resolved font, an image at `size`×`size`,
/// a spacer at zero (it only absorbs slack at paint time), and a stack as the
/// recursive sum-along-axis / max-across-axis of its own children.
fn measure_content<D: SceneDrawer>(
    drawer: &D,
    cache: &mut MeasureCache,
    theme: &Theme,
    content: &Content,
) -> LogicalSize {
    match content {
        Content::Text(t) => {
            let font = t.font.clone().unwrap_or_else(|| theme.row_font.clone());
            let w = measure_cached(drawer, cache, &t.text, &font);
            let h = drawer.line_height(&font);
            LogicalSize::new(w, h)
        }
        Content::Image { size, .. } => LogicalSize::new(*size, *size),
        Content::Stack(stack) => measure_stack(drawer, cache, theme, stack),
        Content::Spacer => LogicalSize::new(0.0, 0.0),
    }
}

/// The intrinsic size of a [`Stack`]: children summed (plus inter-child
/// `spacing`) along `axis`, maxed across the cross axis.
fn measure_stack<D: SceneDrawer>(
    drawer: &D,
    cache: &mut MeasureCache,
    theme: &Theme,
    stack: &Stack,
) -> LogicalSize {
    let mut main = 0.0_f32;
    let mut cross = 0.0_f32;
    for (i, child) in stack.children.iter().enumerate() {
        let sz = measure_content(drawer, cache, theme, child);
        let (m, c) = match stack.axis {
            Axis::Horizontal => (sz.width, sz.height),
            Axis::Vertical => (sz.height, sz.width),
        };
        main += m;
        if i + 1 < stack.children.len() {
            main += stack.spacing;
        }
        cross = cross.max(c);
    }
    match stack.axis {
        Axis::Horizontal => LogicalSize::new(main, cross),
        Axis::Vertical => LogicalSize::new(cross, main),
    }
}

/// Paint one [`Content`] node into `rect` (already resolved by the parent
/// stack's layout pass). `base_color` is the row's default text color (used
/// when a [`crate::menu::TextContent`] carries no explicit color override).
fn paint_content<D: SceneDrawer>(
    drawer: &mut D,
    cache: &mut MeasureCache,
    theme: &Theme,
    content: &Content,
    rect: LogicalRect,
    base_color: Rgba,
) {
    match content {
        Content::Text(t) => {
            let font = t.font.clone().unwrap_or_else(|| theme.row_font.clone());
            let color = t.color.map(|c| theme.resolve(c)).unwrap_or(base_color);
            let w = measure_cached(drawer, cache, &t.text, &font);
            let lh = drawer.line_height(&font);
            let x = match t.align {
                Align::Left => rect.origin.x,
                Align::Center => rect.origin.x + (rect.size.width - w) / 2.0,
                Align::Right => rect.origin.x + (rect.size.width - w),
            };
            let y = rect.origin.y + (rect.size.height - lh) / 2.0;
            drawer.draw_text(&TextRun {
                text: &t.text,
                origin: LogicalPoint::new(x, y),
                font: &font,
                color,
                weight: font.weight,
            });
        }
        Content::Image { icon, size } => {
            let x = rect.origin.x + (rect.size.width - size) / 2.0;
            let y = rect.origin.y + (rect.size.height - size) / 2.0;
            let dest = LogicalRect::new(LogicalPoint::new(x, y), LogicalSize::new(*size, *size));
            // Route through the single icon funnel (a content image is enabled and
            // uses the row's base color for a checkmark/symbol glyph).
            draw_icon(drawer, cache, &theme.row_font, icon, dest, base_color, true);
        }
        Content::Stack(s) => paint_stack(drawer, cache, theme, s, rect, base_color),
        Content::Spacer => {}
    }
}

/// Lay out and paint a [`Stack`]'s children into `rect`: each child gets its
/// intrinsic main-axis size (measured via [`measure_content`]) except a
/// [`Content::Spacer`], which receives an equal share of whatever main-axis
/// space is left over after fixed children and `spacing` are subtracted
/// (clamped to zero — an over-full stack simply overflows `rect`, it is never
/// negative-sized). Cross-axis position honors `stack.align`.
fn paint_stack<D: SceneDrawer>(
    drawer: &mut D,
    cache: &mut MeasureCache,
    theme: &Theme,
    stack: &Stack,
    rect: LogicalRect,
    base_color: Rgba,
) {
    if stack.children.is_empty() {
        return;
    }
    let sizes: Vec<LogicalSize> = stack
        .children
        .iter()
        .map(|c| measure_content(drawer, cache, theme, c))
        .collect();

    let n = stack.children.len();
    let spacing_total = stack.spacing * (n.saturating_sub(1)) as f32;
    let main_avail = match stack.axis {
        Axis::Horizontal => rect.size.width,
        Axis::Vertical => rect.size.height,
    } - spacing_total;
    let cross_avail = match stack.axis {
        Axis::Horizontal => rect.size.height,
        Axis::Vertical => rect.size.width,
    };

    let mut fixed_main_sum = 0.0_f32;
    let mut spacer_count = 0usize;
    for (child, sz) in stack.children.iter().zip(&sizes) {
        if matches!(child, Content::Spacer) {
            spacer_count += 1;
        } else {
            fixed_main_sum += match stack.axis {
                Axis::Horizontal => sz.width,
                Axis::Vertical => sz.height,
            };
        }
    }
    let leftover = (main_avail - fixed_main_sum).max(0.0);
    let spacer_share = if spacer_count > 0 {
        leftover / spacer_count as f32
    } else {
        0.0
    };

    let cross_origin = match stack.axis {
        Axis::Horizontal => rect.origin.y,
        Axis::Vertical => rect.origin.x,
    };
    let mut main_pos = match stack.axis {
        Axis::Horizontal => rect.origin.x,
        Axis::Vertical => rect.origin.y,
    };

    for (child, sz) in stack.children.iter().zip(&sizes) {
        let (m, c) = match stack.axis {
            Axis::Horizontal => (sz.width, sz.height),
            Axis::Vertical => (sz.height, sz.width),
        };
        let this_main = if matches!(child, Content::Spacer) {
            spacer_share
        } else {
            m
        };
        let cross_pos = match stack.align {
            Align::Left => cross_origin,
            Align::Center => cross_origin + (cross_avail - c) / 2.0,
            Align::Right => cross_origin + (cross_avail - c),
        };
        let child_rect = match stack.axis {
            Axis::Horizontal => LogicalRect::new(
                LogicalPoint::new(main_pos, cross_pos),
                LogicalSize::new(this_main, c),
            ),
            Axis::Vertical => LogicalRect::new(
                LogicalPoint::new(cross_pos, main_pos),
                LogicalSize::new(c, this_main),
            ),
        };
        paint_content(drawer, cache, theme, child, child_rect, base_color);
        main_pos += this_main + stack.spacing;
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
        // Walk ALL runs (no early `break`): overlapping runs composite with the
        // LATER RUN WINS (issue D) — a run later in `seg.runs` overrides an earlier
        // one on the chars they share, instead of the first match silently
        // suppressing every later overlapping run.
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
            }
        }
        match pieces.last_mut() {
            // Open per-char merge, not a closed enum: extend the current piece when
            // its (color, weight) match, else start a new piece.
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
    // Reserve a shared leading gutter per the caller's policy (#43): `Auto` (the
    // OEM default) reserves it only when the menu has checkmarks so checked and
    // unchecked rows align (#16 reconciliation); `Always`/`Never` force it.
    let reserve_gutter = match opts.gutter {
        crate::theme::GutterPolicy::Always => true,
        crate::theme::GutterPolicy::Never => false,
        crate::theme::GutterPolicy::Auto => menu_reserves_gutter(menu),
    };

    // Reserve the trailing column when any item needs it: a submenu (its chevron)
    // or a row carrying an explicit trailing icon/accessory (issue A). Reserving
    // it menu-wide keeps every row's segment band ending at the same right edge.
    let trailing_w = if menu
        .items
        .iter()
        .any(|it| is_submenu(it) || item_row(it).is_some_and(|r| row_trailing_width(r) > 0.0))
    {
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
            // The row's content width: its leading advance (the shared gutter
            // when reserved, else its own inline icon) plus its segments.
            let content = row_lead(row, gap, reserve_gutter)
                + row_intrinsic(
                    drawer,
                    &mut measure_cache,
                    row,
                    font,
                    base_color,
                    theme,
                    gap,
                );
            max_content = max_content.max(content);
        } else if let Item::Content(stack) = item {
            let stack_size = measure_stack(drawer, &mut measure_cache, theme, stack);
            max_content = max_content.max(stack_size.width);
        }
    }

    // Sanitize every input to the width `clamp` (HIGH): a `NaN`/`inf` `min_width`,
    // `max_width`, or measured `max_content` (a degenerate/empty measurement) must
    // not reach `f32::clamp` — a non-finite bound produces `NaN` geometry, and an
    // out-of-order (`min > max`) bound makes `clamp` *panic*.
    let min_w = finite_or(
        opts.min_width.unwrap_or(DEFAULT_MIN_WIDTH),
        DEFAULT_MIN_WIDTH,
    );
    // `f32::clamp` panics if `min > max`; a consumer can set `min_width >
    // max_width`, so normalize by letting the floor win (#36).
    let max_w = finite_or(
        opts.max_width.unwrap_or(DEFAULT_MAX_WIDTH),
        DEFAULT_MAX_WIDTH,
    )
    .max(min_w);
    let max_content = finite_or(max_content, 0.0);
    let desired = finite_or(pad.left + max_content + trailing_w + pad.right, min_w);
    let width = desired.clamp(min_w, max_w);

    // Every row's content starts at the same left x (`content_left`); a row with a
    // leading icon draws it here and offsets only its own text. `band_right` is
    // the shared right edge segments right-align to (before the submenu column).
    let content_left = pad.left;
    let band_right = width - pad.right - trailing_w;

    let mut y = pad.top;
    let mut rows: Vec<LaidRow> = Vec::new();
    let mut plan: Vec<(usize, f32, f32)> = Vec::new(); // (item index, y, height)
    for (i, item) in menu.items.iter().enumerate() {
        let h = match item {
            Item::Separator => SEPARATOR_HEIGHT,
            Item::SectionHeader(_) => theme.row_height,
            Item::Row(r) | Item::Submenu { label: r, .. } => {
                // Sanitize a caller-supplied `min_height` before the `max` so a
                // `NaN` can't poison the row height (HIGH).
                theme
                    .row_height
                    .max(finite_or(r.min_height.unwrap_or(0.0), 0.0))
            }
            Item::Content(stack) => {
                let stack_size = measure_stack(drawer, &mut measure_cache, theme, stack);
                theme
                    .row_height
                    .max(finite_or(stack_size.height, 0.0) + CONTENT_ROW_VPAD * 2.0)
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
                    content_left,
                    band_right,
                    reserve_gutter,
                    ry,
                    rh,
                    false, // submenu
                    true,  // is_header: never paints a `checked` checkmark
                    false, // highlighted
                    theme.resolve(theme.secondary_label),
                );
            }
            Item::Content(stack) => {
                if let Some(bg) = stack.background {
                    let band =
                        LogicalRect::new(LogicalPoint::new(0.0, ry), LogicalSize::new(width, rh));
                    drawer.fill_round_rect(band, 0.0, theme.resolve(bg));
                }
                let content_rect = LogicalRect::new(
                    LogicalPoint::new(content_left, ry + CONTENT_ROW_VPAD),
                    LogicalSize::new(
                        (band_right - content_left).max(1.0),
                        (rh - CONTENT_ROW_VPAD * 2.0).max(1.0),
                    ),
                );
                paint_stack(
                    drawer,
                    &mut measure_cache,
                    theme,
                    stack,
                    content_rect,
                    theme.resolve(theme.label),
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
                // A row whose model requests an explicit background gets it painted
                // first (issue B), underneath any hover highlight.
                fill_row_background(drawer, theme, row, rect);
                if highlighted {
                    let hl = LogicalRect::new(
                        LogicalPoint::new(pad.left - 2.0, ry + 1.0),
                        LogicalSize::new(width - pad.horizontal() + 4.0, rh - 2.0),
                    );
                    // The hover fill reads `theme.row_highlight` (resolved) rather
                    // than a hard-coded `theme.accent`: the dedicated theme field
                    // exists for exactly this and every preset sets it = Accent, so
                    // the pixels are identical today but a theme can now diverge.
                    drawer.fill_round_rect(hl, 5.0, theme.resolve(theme.row_highlight));
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
                    content_left,
                    band_right,
                    reserve_gutter,
                    ry,
                    rh,
                    submenu,
                    false, // is_header
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
    content_left: f32,
    band_right: f32,
    reserve_gutter: bool,
    ry: f32,
    rh: f32,
    submenu: bool,
    is_header: bool,
    highlighted: bool,
    base_color: Rgba,
) {
    // A disabled row dims its icon/checkmark (issue E); its text already dims via
    // `secondary_label`. Headers/highlighted rows are always drawn "enabled".
    let icon_enabled = highlighted || is_header || row.enabled;
    // The color a checkmark/symbol glyph is drawn in: white on a highlighted row,
    // else the theme accent (dimmed later for a disabled row inside the funnel).
    let glyph_color = if highlighted {
        Rgba::WHITE
    } else {
        theme.resolve(theme.accent)
    };

    // Leading advance: the shared gutter width when the menu reserves one (so
    // checked + unchecked rows align, native look), else this row's own inline
    // icon advance (#16). The icon/checkmark still draws at `content_left`.
    let lead = row_lead(row, theme.column_gap, reserve_gutter);
    let band_x = content_left + lead;
    let band_w = (band_right - band_x).max(1.0);
    if lead > 0.0 {
        let icon_rect = LogicalRect::new(
            LogicalPoint::new(content_left, ry + (rh - ICON_SIZE) / 2.0),
            LogicalSize::new(ICON_SIZE, ICON_SIZE),
        );
        if let Some(icon) = &row.leading {
            // Every leading icon — Png, Svg, Checkmark, Symbol — draws through the
            // single funnel, so an `Icon::Svg`/`Icon::Symbol` in the leading slot
            // renders instead of being swallowed by a wildcard (HIGH, issue: Svg
            // leading icon).
            draw_icon(
                drawer,
                cache,
                base_font,
                icon,
                icon_rect,
                glyph_color,
                icon_enabled,
            );
        } else if row.checked == Some(true) && !is_header {
            // A checked row with no explicit leading icon paints the check glyph in
            // the gutter. Section headers never do (their `checked` is ignored, per
            // `Menu::section_header`); a submenu-parent row does.
            draw_icon(
                drawer,
                cache,
                base_font,
                &Icon::Checkmark,
                icon_rect,
                glyph_color,
                icon_enabled,
            );
        }
    }

    // Segment band.
    if !row.segments.is_empty() {
        let metrics: Vec<SegmentMetrics> = row
            .segments
            .iter()
            .map(|seg| {
                let font = row_font(seg, base_font);
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
            let font = row_font(seg, base_font);
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

    // Trailing icon/accessory (issue A): a row carrying `Row::trailing` draws it in
    // the reserved trailing column, through the same funnel as every other icon.
    if let Some(icon) = &row.trailing {
        let trailing_rect = LogicalRect::new(
            LogicalPoint::new(
                band_x + band_w + (TRAILING_COLUMN - ICON_SIZE) / 2.0,
                ry + (rh - ICON_SIZE) / 2.0,
            ),
            LogicalSize::new(ICON_SIZE, ICON_SIZE),
        );
        draw_icon(
            drawer,
            cache,
            base_font,
            icon,
            trailing_rect,
            glyph_color,
            icon_enabled,
        );
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

/// The single funnel every icon-drawing site routes through (leading slot,
/// trailing slot, standalone checkmark, content-stack image). It matches **every**
/// [`Icon`] variant exhaustively — there is deliberately no `_` arm, so adding a
/// future `Icon` variant is a compile error *here* rather than a silently-undrawn
/// icon (the recurring muri bug class where a model attribute is set but never
/// rendered).
///
/// `glyph_color` is the color a glyph-based icon (checkmark / a symbol's fallback)
/// draws in; it is ignored for image icons. `enabled` dims the whole icon to
/// [`DISABLED_ALPHA`] when `false` (issue E) — both a blitted image and a glyph.
#[allow(clippy::too_many_arguments)]
fn draw_icon<D: SceneDrawer>(
    drawer: &mut D,
    cache: &mut MeasureCache,
    base_font: &Font,
    icon: &Icon,
    rect: LogicalRect,
    glyph_color: Rgba,
    enabled: bool,
) {
    let alpha = if enabled { 1.0 } else { DISABLED_ALPHA };
    match icon {
        // Both raster and SVG icon bytes converge on the same decoded-icon blit
        // path (`decode_icon` tries PNG then falls back to the SVG rasterizer; see
        // `crate::render::decode_icon_bytes`).
        Icon::Png(bytes) | Icon::Svg(bytes) => {
            if let Some(decoded) = drawer.decode_icon(bytes) {
                let (rgba, w, h) = &*decoded;
                drawer.draw_image_alpha(rgba, *w, *h, rect, alpha);
            }
        }
        Icon::Checkmark => draw_glyph_centered(
            drawer,
            cache,
            "\u{2713}",
            base_font,
            Weight::Bold,
            dim_color(glyph_color, enabled),
            rect,
        ),
        // A named symbol has no bundled per-name glyph yet (SF Symbols are macOS-
        // only), but it must NOT be a silent no-op — that is exactly the bug class
        // this funnel guards. Draw a neutral placeholder glyph so a `Symbol` icon
        // is always visibly rendered until a real symbol face is wired up.
        Icon::Symbol(_) => draw_glyph_centered(
            drawer,
            cache,
            "\u{25AA}", // ▪ small filled square placeholder
            base_font,
            Weight::Regular,
            dim_color(glyph_color, enabled),
            rect,
        ),
    }
}

/// Fill a row's explicit background (issue B): a row whose model carries a
/// [`Row::background`] gets that color painted across its full band before its
/// content (and before any hover highlight). A row with no background is a no-op.
fn fill_row_background<D: SceneDrawer>(
    drawer: &mut D,
    theme: &Theme,
    row: &Row,
    rect: LogicalRect,
) {
    if let Some(bg) = row.background {
        drawer.fill_round_rect(rect, 0.0, theme.resolve(bg));
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
    use crate::menu::{Align, Flex, StyleRun, TextContent};
    use crate::style::Color;
    use std::sync::Arc;

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

    /// A drawer that records the geometry of every draw op, with deterministic
    /// monospace metrics (7px/char, 14px line height) so layout is exactly
    /// reproducible in a test. Icons decode to an **opaque** 2x2 stub (not
    /// all-zero) so a dimmed-alpha blit is observable, and every op is recorded:
    /// fills (issue B), text colors (issue E checkmark dimming), and per-image
    /// alpha (issue E icon dimming).
    #[derive(Default)]
    struct RecordingDrawer {
        texts: Vec<(String, f32, f32)>,        // (text, origin.x, origin.y)
        images: Vec<LogicalRect>,              // dest rects of draw_image[_alpha]
        fills: Vec<(LogicalRect, Rgba)>,       // (rect, color) of fill_round_rect
        text_colors: Vec<(String, Rgba)>,      // (text, resolved color) of draw_text
        image_alphas: Vec<(LogicalRect, f32)>, // (dest, alpha) of draw_image_alpha
    }
    impl SceneDrawer for RecordingDrawer {
        fn begin_frame(&mut self, _size: LogicalSize) {}
        fn fill_round_rect(&mut self, r: LogicalRect, _cr: f32, c: Rgba) {
            self.fills.push((r, c));
        }
        fn draw_separator(&mut self, _r: LogicalRect, _c: Rgba) {}
        fn measure_text(&self, text: &str, _font: &Font) -> f32 {
            text.chars().count() as f32 * 7.0
        }
        fn line_height(&self, _font: &Font) -> f32 {
            14.0
        }
        fn draw_text(&mut self, run: &TextRun<'_>) {
            self.texts
                .push((run.text.to_string(), run.origin.x, run.origin.y));
            self.text_colors.push((run.text.to_string(), run.color));
        }
        fn draw_image(&mut self, rgba: &[u8], w: u32, h: u32, dest: LogicalRect) {
            self.draw_image_alpha(rgba, w, h, dest, 1.0);
        }
        fn draw_image_alpha(
            &mut self,
            _rgba: &[u8],
            _w: u32,
            _h: u32,
            dest: LogicalRect,
            alpha: f32,
        ) {
            self.images.push(dest);
            self.image_alphas.push((dest, alpha));
        }
        fn decode_icon(&self, _bytes: &Arc<[u8]>) -> Option<crate::render::DecodedIcon> {
            // Opaque stub (alpha 255) so a dimmed blit differs from a transparent one.
            Some(std::rc::Rc::new((vec![255u8; 16], 2, 2)))
        }
    }

    /// Right edge (`origin.x + measured width`) of the draw_text op whose text
    /// matches `needle`, using the same 7px/char metric the drawer reports.
    fn text_right_edge(d: &RecordingDrawer, needle: &str) -> f32 {
        let (t, x, _) = d
            .texts
            .iter()
            .find(|(t, ..)| t == needle)
            .unwrap_or_else(|| panic!("no draw_text for {needle:?}; got {:?}", d.texts));
        x + t.chars().count() as f32 * 7.0
    }

    /// Regression for #16: no global leading gutter. A menu mixing an icon row
    /// with plain text rows must start every row's content at the SAME left x —
    /// the icon sits at that x (inline) and plain rows are NOT indented past it.
    #[test]
    fn issue16_no_global_leading_gutter_shared_left_x() {
        use std::sync::Arc;
        let logo: Arc<[u8]> = Arc::from(vec![0u8; 8]);
        let menu = Menu::new()
            .row(
                Row::new("hdr")
                    .leading(Icon::Png(logo.clone()))
                    .label("Claude")
                    .enabled(false),
            )
            .row(Row::new("acct").label("matthew@example.com"))
            .row(Row::new("quit").label("Quit"));

        let mut d = RecordingDrawer::default();
        let _ = render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);

        // The icon is drawn at the shared left x.
        assert_eq!(d.images.len(), 1, "one inline icon");
        let icon_x = d.images[0].origin.x;

        // Plain rows' text starts at the same left x as the icon — NOT indented
        // past a reserved gutter.
        let acct_x = d
            .texts
            .iter()
            .find(|(t, ..)| t == "matthew@example.com")
            .map(|(_, x, _)| *x)
            .expect("account row text");
        let quit_x = d
            .texts
            .iter()
            .find(|(t, ..)| t == "Quit")
            .map(|(_, x, _)| *x)
            .expect("quit row text");

        assert!(
            (acct_x - icon_x).abs() < 0.5,
            "plain row must start at the icon's left x, not indented: icon={icon_x} acct={acct_x}"
        );
        assert!(
            (quit_x - icon_x).abs() < 0.5,
            "every plain row shares the same left x: icon={icon_x} quit={quit_x}"
        );

        // The icon row's OWN label is offset past its icon (inline content).
        let hdr_x = d
            .texts
            .iter()
            .find(|(t, ..)| t == "Claude")
            .map(|(_, x, _)| *x)
            .expect("header text");
        assert!(
            hdr_x > icon_x + 8.0,
            "the icon row's text follows its icon inline: icon={icon_x} hdr={hdr_x}"
        );
    }

    /// OEM alignment: when a menu has a checked row, it reserves a shared gutter
    /// so the checked row's text aligns with the *unchecked* rows' text (native
    /// `NSMenu` look), instead of the checkmark pushing only its own row right.
    #[test]
    fn checkable_menu_reserves_gutter_so_rows_align() {
        let menu = Menu::new()
            .row(Row::new("a").checked(true).segments(vec![
                Segment::new("me@example.com").flex(Flex::Grow),
                Segment::new("47%").align(Align::Right),
            ]))
            .row(Row::new("b").segments(vec![
                Segment::new("you@example.com").flex(Flex::Grow),
                Segment::new("20%").align(Align::Right),
            ]));

        let mut d = RecordingDrawer::default();
        let _ = render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);

        let x = |needle: &str| {
            d.texts
                .iter()
                .find(|(t, ..)| t == needle)
                .map(|(_, x, _)| *x)
                .unwrap_or_else(|| panic!("no text {needle:?}"))
        };
        // Checked row's email aligns with the unchecked row's email (shared
        // gutter), not indented by the checkmark.
        assert!(
            (x("me@example.com") - x("you@example.com")).abs() < 0.5,
            "checked and unchecked rows must share a text left x: {} vs {}",
            x("me@example.com"),
            x("you@example.com")
        );
        // And that text starts past the gutter (a checkmark glyph was drawn at
        // the left).
        assert!(x("you@example.com") > 6.0);
    }

    /// Regression: a menu whose checkable rows are ALL currently *unchecked*
    /// (`checked(false)`) must still reserve the shared gutter — `Row::checked`'s
    /// contract is "`Some(true/false)` shows a check column". Testing only
    /// `Some(true)` (the pre-fix bug) reserved nothing here, so the rows' text
    /// sat flush-left and every row jumped right the instant one was toggled on.
    #[test]
    fn all_unchecked_checkable_menu_still_reserves_gutter() {
        let text_x = |checked: bool| {
            let menu = Menu::new()
                .row(Row::new("a").checked(checked).label("Alpha"))
                .row(Row::new("b").checked(checked).label("Beta"));
            let mut d = RecordingDrawer::default();
            let _ = render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);
            d.texts
                .iter()
                .find(|(t, ..)| t == "Alpha")
                .map(|(_, x, _)| *x)
                .expect("Alpha text")
        };
        // The gutter is reserved regardless of the current on/off state, so text
        // starts at the same x whether the checkable rows are on or off — no jump
        // on toggle. (With the bug, the all-unchecked menu reserved nothing and
        // its text x was smaller.)
        let off = text_x(false);
        let on = text_x(true);
        assert!(
            (off - on).abs() < 0.5,
            "checkable rows must reserve the gutter whether checked or not: off={off} on={on}"
        );
        assert!(
            off > 6.0,
            "an all-unchecked checkable menu must still reserve the check column, text x={off}"
        );
    }

    /// Regression for #15: a menu mixing icon-bearing section headers with
    /// `label\tvalue` rows must (1) draw every leading icon at the same left
    /// gutter x (never trailing), and (2) right-align each `\t` value to one
    /// shared column, for both `Row` and `Submenu` items.
    #[test]
    fn issue15_leading_icons_and_tab_values_align_to_shared_columns() {
        let logo: Arc<[u8]> = Arc::from(vec![0u8; 8]);
        let menu = Menu::new()
            .row(
                Row::new("hdr:claude")
                    .leading(Icon::Png(logo.clone()))
                    .label("Claude")
                    .enabled(false),
            )
            .submenu(
                Row::new("acct:short").segments(vec![
                    Segment::new("a@x.com").flex(Flex::Grow),
                    Segment::new("20% / 38%").align(Align::Right),
                ]),
                Menu::new().row(Row::new("d").label("detail")),
            )
            .submenu(
                Row::new("acct:longemail").segments(vec![
                    Segment::new("demo1@example.com").flex(Flex::Grow),
                    Segment::new("47% / 52%").align(Align::Right),
                ]),
                Menu::new().row(Row::new("d2").label("detail")),
            );

        let mut d = RecordingDrawer::default();
        let _ = render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);

        // (1) The header's leading icon is drawn in the left gutter, near x≈0,
        //     never at the right edge of the row.
        assert_eq!(d.images.len(), 1, "one leading icon drawn");
        let icon_x = d.images[0].origin.x;
        assert!(
            icon_x < 12.0,
            "leading icon must sit in the left gutter, got x={icon_x}"
        );

        // (2) Both `\t` values right-align to the same column: their right edges
        //     match despite different value/label widths.
        let r_short = text_right_edge(&d, "20% / 38%");
        let r_long = text_right_edge(&d, "47% / 52%");
        assert!(
            (r_short - r_long).abs() < 0.5,
            "tab-stop values must share a right column: short={r_short} long={r_long}"
        );
    }

    // -------------------------------------------------------------------------
    // Content stacks (issue #44)
    // -------------------------------------------------------------------------

    /// A vertical stack of 3 texts measures to: width = the widest text (the
    /// cross axis, maxed), height = the summed line heights plus inter-child
    /// spacing (the main axis) — using `RecordingDrawer`'s deterministic
    /// 7px/char, 14px-line-height metrics.
    #[test]
    fn measure_vertical_stack_of_three_texts() {
        let d = RecordingDrawer::default();
        let mut cache = MeasureCache::new();
        let theme = Theme::light();
        let stack = Stack::vertical(2.0)
            .child(Content::Text(TextContent::new("a"))) // 1 char -> 7px wide
            .child(Content::Text(TextContent::new("bb"))) // 2 chars -> 14px wide
            .child(Content::Text(TextContent::new("ccc"))); // 3 chars -> 21px wide

        let size = measure_stack(&d, &mut cache, &theme, &stack);

        // Cross axis (width) = widest child = "ccc" at 21px.
        assert_eq!(size.width, 21.0);
        // Main axis (height) = 3 * 14 (line height) + 2 * 2.0 (spacing between
        // the 3 children).
        assert_eq!(size.height, 3.0 * 14.0 + 2.0 * 2.0);
    }

    /// A horizontal strip measures to: width = summed child widths + spacing
    /// (main axis), height = the tallest child (cross axis).
    #[test]
    fn measure_horizontal_strip() {
        let d = RecordingDrawer::default();
        let mut cache = MeasureCache::new();
        let theme = Theme::light();
        let stack = Stack::horizontal(5.0)
            .child(Content::Text(TextContent::new("ab"))) // 14px wide, 14px tall
            .child(Content::Image {
                icon: Icon::Checkmark,
                size: 20.0,
            }) // 20x20
            .child(Content::Text(TextContent::new("c"))); // 7px wide, 14px tall

        let size = measure_stack(&d, &mut cache, &theme, &stack);

        // Main axis (width) = 14 + 20 + 7 + 2 * 5.0 (spacing between 3 children).
        assert_eq!(size.width, 14.0 + 20.0 + 7.0 + 2.0 * 5.0);
        // Cross axis (height) = tallest child = the 20px image.
        assert_eq!(size.height, 20.0);
    }

    /// A `Spacer` has zero intrinsic size and absorbs the leftover main-axis
    /// space at paint time, splitting it equally among sibling spacers — the
    /// same "grow" promise `Flex::Grow` makes for `Segment`s, generalized to
    /// the `Content` tree.
    #[test]
    fn spacer_distributes_leftover_space_equally() {
        let mut d = RecordingDrawer::default();
        let mut cache = MeasureCache::new();
        let theme = Theme::light();
        // "a" (7px) + spacer + "bb" (14px) + spacer, in a 100px-wide row: the
        // two spacers must split (100 - 7 - 14) = 79px evenly (39.5px each).
        let stack = Stack::horizontal(0.0)
            .child(Content::Text(TextContent::new("a")))
            .child(Content::Spacer)
            .child(Content::Text(TextContent::new("bb")))
            .child(Content::Spacer);

        let rect = LogicalRect::new(LogicalPoint::new(0.0, 0.0), LogicalSize::new(100.0, 14.0));
        paint_stack(&mut d, &mut cache, &theme, &stack, rect, Rgba::BLACK);

        let a_x = d
            .texts
            .iter()
            .find(|(t, ..)| t == "a")
            .map(|(_, x, _)| *x)
            .expect("'a' drawn");
        let bb_x = d
            .texts
            .iter()
            .find(|(t, ..)| t == "bb")
            .map(|(_, x, _)| *x)
            .expect("'bb' drawn");

        assert_eq!(a_x, 0.0, "'a' sits flush at the stack's leading edge");
        // "bb" starts after "a" (7px) plus one spacer's share (39.5px).
        assert_eq!(bb_x, 7.0 + 39.5);
        // The trailing spacer pushes the stack's total content to fill the
        // full 100px width: "bb" ends at 100 - 39.5 (its own trailing spacer).
        assert_eq!(bb_x + 2.0 * 7.0, 100.0 - 39.5);
    }

    /// A nested stack's size is the recursive sum: a horizontal strip of
    /// vertical "cells" (time/icon/temp, the Apple-Weather-extra use case)
    /// measures as wide as its cells summed, and as tall as its tallest cell.
    #[test]
    fn nested_stack_size_recurses() {
        let d = RecordingDrawer::default();
        let mut cache = MeasureCache::new();
        let theme = Theme::light();

        let cell = |time: &str, temp: &str| {
            Content::Stack(
                Stack::vertical(1.0)
                    .child(Content::Text(TextContent::new(time)))
                    .child(Content::Image {
                        icon: Icon::Checkmark,
                        size: 16.0,
                    })
                    .child(Content::Text(TextContent::new(temp))),
            )
        };
        // Each cell: width = max(time, 16, temp) widths; height = time_h + 16 +
        // temp_h + 2 * 1.0 spacing = 14 + 16 + 14 + 2 = 46.
        let strip = Stack::horizontal(3.0)
            .child(cell("1PM", "72°"))
            .child(cell("2PM", "70°"));

        let size = measure_stack(&d, &mut cache, &theme, &strip);

        let cell_height = 14.0 + 16.0 + 14.0 + 2.0 * 1.0;
        assert_eq!(size.height, cell_height, "strip height = tallest cell");
        // Each cell's width = widest of its 3 children; "1PM"/"2PM" are 3
        // chars (21px), wider than the 16px icon, so each cell is 21px wide.
        // Strip width = 2 cells + 1 gap of 3.0.
        assert_eq!(size.width, 21.0 * 2.0 + 3.0);
    }

    /// `Item::Content` integrates into `render_menu`: the row's height is
    /// derived from the stack's measured content (plus padding), it paints
    /// through the ordinary `SceneDrawer` text/image path, and it never
    /// appears in `LaidMenu::rows` (non-interactive, like `SectionHeader`).
    #[test]
    fn item_content_sizes_and_paints_through_render_menu() {
        let logo: std::sync::Arc<[u8]> = std::sync::Arc::from(vec![0u8; 8]);
        let mut d = RecordingDrawer::default();
        let menu = Menu::new().content(
            Stack::vertical(2.0)
                .child(Content::Text(TextContent::new("Weather")))
                .child(Content::Image {
                    icon: Icon::Png(logo),
                    size: 16.0,
                }),
        );
        let laid = render_menu(
            &mut d,
            &menu,
            &Theme::light(),
            &MenuOptions::default(),
            None,
        );

        // Non-interactive: no clickable row recorded for a content item.
        assert!(laid.rows.is_empty());
        // The text was actually painted.
        assert!(d.texts.iter().any(|(t, ..)| t == "Weather"));
        assert_eq!(d.images.len(), 1, "the icon image was blitted");
        // The row is tall enough to fit "Weather" (14px) + gap (2px) + the
        // 16px icon, plus the row's own vertical padding.
        let min_expected = 14.0 + 2.0 + 16.0;
        assert!(laid.size.height > min_expected);
    }

    // -------------------------------------------------------------------------
    // Structural paint-layer fixes
    // -------------------------------------------------------------------------

    /// HIGH: a non-finite `min_width` must be sanitized before the width `clamp`
    /// (a `NaN` bound produces `NaN` geometry / panics `f32::clamp`) so the popup
    /// size is always finite.
    #[test]
    fn nan_min_width_is_clamped() {
        let menu = Menu::new().row(Row::new("a").label("Alpha"));
        let opts = MenuOptions::default().min_width(f32::NAN);
        let mut d = RecordingDrawer::default();
        let laid = render_menu(&mut d, &menu, &Theme::dark(), &opts, None);
        assert!(
            laid.size.width.is_finite() && laid.size.height.is_finite(),
            "geometry must be finite despite a NaN min_width: {:?}",
            laid.size
        );
        assert!(laid.size.width > 0.0);
    }

    /// HIGH: an `Icon::Svg` in the LEADING slot must render (via the funnel),
    /// not be swallowed by a wildcard that only handled `Png`/`Checkmark`.
    #[test]
    fn svg_leading_icon_is_rendered() {
        let svg: Arc<[u8]> = Arc::from(vec![1u8, 2, 3]);
        let menu = Menu::new().row(Row::new("a").leading(Icon::Svg(svg)).label("Alpha"));
        let mut d = RecordingDrawer::default();
        render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);
        assert_eq!(
            d.images.len(),
            1,
            "an Svg leading icon must blit, not be dropped"
        );
    }

    /// MEDIUM: a section header must NOT paint a `checked` checkmark (its
    /// `checked` is ignored per `Menu::section_header`), but a submenu-parent row
    /// DOES honor `checked`.
    #[test]
    fn header_ignores_checked_but_submenu_parent_honors_it() {
        let menu = Menu::new()
            .section_header(Row::info().label("Header").checked(true))
            .submenu(Row::new("more").label("More").checked(true), Menu::new());
        let mut d = RecordingDrawer::default();
        render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);
        let checks = d.texts.iter().filter(|(t, ..)| t == "\u{2713}").count();
        assert_eq!(
            checks, 1,
            "only the submenu-parent row paints a checkmark, not the header; texts={:?}",
            d.texts
        );
    }

    /// Issue A: a row with a trailing icon/accessory must draw it in the reserved
    /// trailing column (right of the segment band), not silently omit it.
    #[test]
    fn issue_a_trailing_icon_is_drawn() {
        let icon: Arc<[u8]> = Arc::from(vec![9u8; 4]);
        let menu = Menu::new().row(Row::new("a").label("Alpha").trailing(Icon::Png(icon)));
        let mut d = RecordingDrawer::default();
        let laid = render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);
        assert_eq!(d.images.len(), 1, "the trailing icon must be blitted");
        let icon_x = d.images[0].origin.x;
        let label_x = d
            .texts
            .iter()
            .find(|(t, ..)| t == "Alpha")
            .map(|(_, x, _)| *x)
            .expect("label text");
        assert!(
            icon_x > label_x,
            "trailing icon must sit right of the label: icon={icon_x} label={label_x}"
        );
        assert!(
            icon_x + ICON_SIZE <= laid.size.width + 0.5,
            "trailing icon must stay within the popup width"
        );
    }

    /// Issue B: a row whose model requests an explicit background gets it filled.
    #[test]
    fn issue_b_row_background_is_filled() {
        let menu = Menu::new().row(Row::new("a").label("Alpha").background(Color::SystemRed));
        let mut d = RecordingDrawer::default();
        render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);
        let want = Theme::dark().resolve(Color::SystemRed);
        assert!(
            d.fills.iter().any(|(_, c)| *c == want),
            "a row with Row::background must be filled with it; fills={:?}",
            d.fills
        );
    }

    /// Issue D: overlapping style runs composite with the LATER RUN WINS — an
    /// early `break` used to stop at the first matching run, dropping later ones.
    #[test]
    fn issue_d_overlapping_style_runs_the_later_run_wins() {
        let seg = Segment::new("ab").runs(vec![
            StyleRun::new(0, 2, Color::SystemRed),   // covers a, b
            StyleRun::new(1, 1, Color::SystemGreen), // overlaps on b — later, wins
        ]);
        let theme = Theme::light();
        let pieces = style_pieces(&seg, Rgba::BLACK, Weight::Regular, &theme, false);
        let a = pieces.iter().find(|(s, ..)| s == "a").expect("piece a");
        let b = pieces.iter().find(|(s, ..)| s == "b").expect("piece b");
        assert_eq!(
            a.1,
            theme.resolve(Color::SystemRed),
            "'a' is covered only by the red run"
        );
        assert_eq!(
            b.1,
            theme.resolve(Color::SystemGreen),
            "'b' is covered by both runs; the later (green) run must win"
        );
    }

    /// Issue E: a disabled row dims its checkmark AND its icon (not just text),
    /// applying `DISABLED_ALPHA` to both the glyph color and the image blit.
    #[test]
    fn issue_e_disabled_row_dims_checkmark_and_icon() {
        let icon: Arc<[u8]> = Arc::from(vec![7u8; 4]);
        let menu = Menu::new()
            .row(
                Row::new("i")
                    .label("IconRow")
                    .leading(Icon::Png(icon))
                    .enabled(false),
            )
            .row(Row::new("c").label("CheckRow").checked(true).enabled(false));
        let mut d = RecordingDrawer::default();
        render_menu(&mut d, &menu, &Theme::dark(), &MenuOptions::default(), None);
        assert!(
            d.image_alphas
                .iter()
                .any(|(_, a)| (*a - DISABLED_ALPHA).abs() < 1e-6),
            "a disabled row's icon must blit at DISABLED_ALPHA; got {:?}",
            d.image_alphas
        );
        let check = d
            .text_colors
            .iter()
            .find(|(t, _)| t == "\u{2713}")
            .expect("a checkmark glyph was drawn");
        assert!(
            check.1.a < 255,
            "a disabled row's checkmark must be dimmed, got alpha {}",
            check.1.a
        );
    }

    /// The funnel draws SOMETHING for every `Icon` variant — a table-driven guard
    /// against a future variant becoming a silent no-op (the muri bug class).
    #[test]
    fn draw_icon_funnel_draws_every_icon_variant() {
        let png: Arc<[u8]> = Arc::from(vec![1u8; 4]);
        let svg: Arc<[u8]> = Arc::from(vec![2u8; 4]);
        let cases: Vec<(&str, Icon)> = vec![
            ("Png", Icon::Png(png)),
            ("Svg", Icon::Svg(svg)),
            ("Checkmark", Icon::Checkmark),
            ("Symbol", Icon::Symbol("gear")),
        ];
        let font = Font::default();
        let rect = LogicalRect::new(
            LogicalPoint::new(0.0, 0.0),
            LogicalSize::new(ICON_SIZE, ICON_SIZE),
        );
        for (name, icon) in cases {
            let mut d = RecordingDrawer::default();
            let mut cache = MeasureCache::new();
            draw_icon(&mut d, &mut cache, &font, &icon, rect, Rgba::WHITE, true);
            let ops = d.images.len() + d.texts.len();
            assert!(ops > 0, "Icon::{name} must emit at least one draw op");
        }
    }

    /// The hover highlight fill reads `theme.row_highlight`, not `theme.accent`:
    /// a theme whose two fields differ proves the correct one is used.
    #[test]
    fn row_highlight_uses_the_theme_field() {
        let mut theme = Theme::dark();
        theme.accent = Color::Rgba(1, 2, 3, 255);
        theme.row_highlight = Color::Rgba(9, 8, 7, 255);
        let menu = Menu::new().row(Row::new("a").label("Alpha"));
        let mut d = RecordingDrawer::default();
        render_menu(&mut d, &menu, &theme, &MenuOptions::default(), Some(0));
        let want = theme.resolve(theme.row_highlight);
        let accent = theme.resolve(theme.accent);
        assert!(
            d.fills.iter().any(|(_, c)| *c == want),
            "hover fill must use row_highlight; fills={:?}",
            d.fills
        );
        assert!(
            !d.fills.iter().any(|(_, c)| *c == accent),
            "hover fill must NOT use accent"
        );
    }
}
