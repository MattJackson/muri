//! The declarative menu data model: identifiers and events, icons, and the
//! `Segment` → `Row` → `Item` → `Menu` tree a consumer builds. This layer is
//! entirely pure (no I/O, no platform calls) and is what both the renderer and
//! the accessibility tree are derived from.

use std::sync::Arc;

use crate::style::{Color, Font, Weight};

// =============================================================================
// Identity & events
// =============================================================================

/// A click identifier for a row. muri treats this as an opaque string and hands
/// it back verbatim when the row is clicked — the *grammar* of the id (e.g.
/// `"switch:claude:me@x.com"`, `"quit"`) is entirely the consumer's business.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct MenuId(pub String);

impl MenuId {
    /// A `MenuId` from any string-like value. Mirrors real muda's
    /// `MenuId::new(impl Into<String>)` so `MenuId::new(id)` migrates through the
    /// `muda-compat` facade with a pure import swap.
    pub fn new(id: impl Into<String>) -> Self {
        MenuId(id.into())
    }

    /// A sentinel id for non-interactive rows (section headers, separators).
    /// Rows carrying this id never emit a click event.
    pub fn none() -> Self {
        MenuId(String::new())
    }

    /// Borrow the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the non-interactive sentinel ([`MenuId::none`]).
    pub fn is_none(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&str> for MenuId {
    fn from(s: &str) -> Self {
        MenuId(s.to_owned())
    }
}

impl From<String> for MenuId {
    fn from(s: String) -> Self {
        MenuId(s)
    }
}

/// Emitted when a row is activated (click, Enter, or `AXPress`).
#[derive(Clone, Debug)]
pub struct MenuEvent {
    /// The [`MenuId`] of the activated row.
    pub id: MenuId,
    /// Which surface — [`Tray`](crate::Tray), [`ContextMenu`](crate::ContextMenu),
    /// or [`Popup`](crate::Popup) — the activation came from (issue #51). A
    /// consumer with more than one live surface uses this to tell them apart on
    /// the process-global channel; the muda-compat facade ignores it (structural
    /// [`MenuId`] equality is unaffected).
    pub source: crate::SurfaceId,
}

/// The callback invoked on row activation. Stored by
/// [`Tray`](crate::Tray)/[`ContextMenu`](crate::ContextMenu).
pub type ClickHandler = Box<dyn Fn(&MenuId) + Send + 'static>;

// =============================================================================
// Icons
// =============================================================================

/// A leading/trailing icon or logo. Raster (PNG) bytes are decoded at load; SVG
/// bytes are rasterized by muri's own `zeno`-backed **restricted-subset**
/// rasterizer (paths, basic shapes, solid fills, `transform`; no filters/text/
/// gradients — those should ship as PNG). Both feed the same icon path.
#[derive(Clone, Debug)]
pub enum Icon {
    /// A PNG (or other auto-detected raster format) from raw bytes.
    Png(Arc<[u8]>),
    /// An SVG from raw bytes, rasterized per target size via muri's restricted
    /// SVG subset (see the [`Icon`] note for what's supported).
    Svg(Arc<[u8]>),
    /// The themed checkmark glyph, drawn in the leading column.
    Checkmark,
    /// A named symbol: an SF Symbol on macOS, with a bundled fallback elsewhere.
    Symbol(&'static str),
}

impl Icon {
    /// **The one obvious way** to build a raster icon: from encoded PNG (or any
    /// auto-detected raster format) bytes. The 90% path for a logo/avatar.
    pub fn from_png(bytes: impl Into<Arc<[u8]>>) -> Self {
        Icon::Png(bytes.into())
    }

    /// **The one obvious way** to build an icon from raw straight-alpha RGBA8
    /// pixels: `width * height * 4` bytes, row-major. muri keeps a single
    /// encoded-bytes representation internally (the pixels are encoded to PNG),
    /// so a consumer never has to choose a representation — hence this returns a
    /// plain [`Icon`], indistinguishable at the type level from one built with
    /// [`Icon::from_png`]. Returns [`Error::BadIcon`](crate::Error::BadIcon) when
    /// the buffer length doesn't equal `width * height * 4` (or either dimension
    /// is zero).
    pub fn from_rgba(rgba: &[u8], width: u32, height: u32) -> crate::Result<Self> {
        crate::render::encode_rgba_png(rgba, width, height)
            .map(|png| Icon::Png(png.into()))
            .ok_or_else(|| {
                crate::Error::BadIcon(format!(
                    "RGBA buffer is {} bytes but {width}x{height} needs {}",
                    rgba.len(),
                    (width as usize)
                        .saturating_mul(height as usize)
                        .saturating_mul(4),
                ))
            })
    }

    /// **The one obvious way** to build an icon from raw SVG bytes (rasterized
    /// per target size via muri's restricted SVG subset; see the [`Icon`] note).
    pub fn from_svg(bytes: impl Into<Arc<[u8]>>) -> Self {
        Icon::Svg(bytes.into())
    }
}

// =============================================================================
// Segments & rows
// =============================================================================

/// Horizontal alignment of a [`Segment`] within the width it is allotted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    /// Align to the leading edge. The default.
    #[default]
    Left,
    /// Center within the allotted width.
    Center,
    /// Align to the trailing edge (true flush-right).
    Right,
}

/// How a [`Segment`] claims horizontal space during row layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Flex {
    /// Occupy only the segment's intrinsic width. The default.
    #[default]
    Fixed,
    /// Absorb all leftover row width. A `Grow` label followed by an
    /// `Align::Right` value yields a truly flush-right value with **no reserved
    /// chevron column** — the core reason muri exists.
    Grow,
}

/// A per-substring style span within a [`Segment`]'s text, used for severity
/// coloring (e.g. a red over-limit percentage inside an otherwise normal line).
///
/// `start`/`len` are measured in **UTF-16 code units**, matching `NSRange` and
/// usagio's existing span model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StyleRun {
    /// Start offset, in UTF-16 code units.
    pub start: usize,
    /// Length, in UTF-16 code units.
    pub len: usize,
    /// The color applied to this span.
    pub color: Color,
    /// An optional weight override for this span.
    pub weight: Option<Weight>,
}

impl StyleRun {
    /// A span covering an explicit range with a color.
    pub fn new(start: usize, len: usize, color: Color) -> Self {
        StyleRun {
            start,
            len,
            color,
            weight: None,
        }
    }

    /// A span from a Rust byte range into `text`, converted to the UTF-16 code
    /// units `start`/`len` are measured in (see the [`StyleRun`] note). `range`
    /// is clamped to `text`'s bounds and, if an endpoint doesn't land on a char
    /// boundary, snapped **outward** to the enclosing boundary — so the whole
    /// character a partial range touches is styled — rather than panicking.
    pub fn from_byte_range(text: &str, range: std::ops::Range<usize>, color: Color) -> Self {
        let byte_len = text.len();
        let start_byte = range.start.min(byte_len);
        let end_byte = range.end.min(byte_len).max(start_byte);

        // Snap each endpoint outward to the enclosing char boundary (start moves
        // earlier, end moves later) so a partially-covered char is fully styled.
        let start_byte = (0..=start_byte)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        let end_byte = (end_byte..=byte_len)
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(byte_len);

        let start = text[..start_byte].encode_utf16().count();
        let len = text[start_byte..end_byte].encode_utf16().count();

        StyleRun {
            start,
            len,
            color,
            weight: None,
        }
    }

    /// Set an explicit weight for this span.
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = Some(weight);
        self
    }
}

/// One horizontal piece of a row. Rows are composed left→right from segments so
/// multi-column layouts (`label ............ value`) are first-class rather than
/// a tab-stop hack.
#[derive(Clone, Debug, Default)]
pub struct Segment {
    /// The text to draw.
    pub text: String,
    /// Per-substring style spans (colors/weights). Empty → whole-segment style.
    pub runs: Vec<StyleRun>,
    /// Alignment within the segment's allotted width.
    pub align: Align,
    /// How the segment claims horizontal space.
    pub flex: Flex,
    /// Optional per-segment font override (else the row/theme font is used).
    pub font: Option<Font>,
    /// Optional whole-segment color (else [`Color::Label`]).
    pub color: Option<Color>,
}

impl Segment {
    /// A new segment with the given text (left-aligned, fixed width).
    pub fn new(text: impl Into<String>) -> Self {
        Segment {
            text: text.into(),
            ..Segment::default()
        }
    }

    /// A segment that absorbs leftover row width (`Segment::new(text).flex(Flex::Grow)`).
    /// Pair with [`Segment::trailing_value`] for a flush-right label/value row.
    pub fn grow(text: impl Into<String>) -> Self {
        Segment::new(text).flex(Flex::Grow)
    }

    /// A right-aligned segment (`Segment::new(text).align(Align::Right)`), for
    /// the trailing value column of a label/value row.
    pub fn trailing_value(text: impl Into<String>) -> Self {
        Segment::new(text).align(Align::Right)
    }

    /// Set the alignment.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Set the flex behavior.
    pub fn flex(mut self, flex: Flex) -> Self {
        self.flex = flex;
        self
    }

    /// Replace the per-substring style runs.
    pub fn runs(mut self, runs: Vec<StyleRun>) -> Self {
        self.runs = runs;
        self
    }

    /// Append a single per-substring style run (sibling of [`Segment::runs`]
    /// for building up spans one at a time).
    pub fn run(mut self, run: StyleRun) -> Self {
        self.runs.push(run);
        self
    }

    /// Set a per-segment font override.
    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Set a whole-segment color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

/// One menu row. A row is interactive (carries a [`MenuId`]) unless it is used
/// as an [`Item::SectionHeader`].
#[derive(Clone, Debug)]
pub struct Row {
    /// The click id. [`MenuId::none`] marks a non-interactive row.
    pub id: MenuId,
    /// Left→right segments (multi-column layout).
    pub segments: Vec<Segment>,
    /// Optional leading icon column (logo, avatar, checkmark).
    pub leading: Option<Icon>,
    /// Optional trailing icon column.
    pub trailing: Option<Icon>,
    /// Whether the row is clickable. Disabled rows are dimmed and inert.
    pub enabled: bool,
    /// `Some(true/false)` shows a check column; `None` reserves no check column.
    pub checked: Option<bool>,
    /// Optional explicit row background (else theme hover/selection handling).
    pub background: Option<Color>,
    /// Optional minimum row height in logical points (else the theme default).
    pub min_height: Option<f32>,
    /// Optional override for the accessible name announced by screen readers,
    /// used in place of [`Row::accessible_name`]. Needed for icon-only rows
    /// (no segments), whose derived name would otherwise be empty and silent
    /// to an assistive technology (spec 30 §1.4).
    pub accessibility_label: Option<String>,
}

impl Default for Row {
    fn default() -> Self {
        Row {
            id: MenuId::none(),
            segments: Vec::new(),
            leading: None,
            trailing: None,
            enabled: true,
            checked: None,
            background: None,
            min_height: None,
            accessibility_label: None,
        }
    }
}

impl Row {
    /// A new enabled row with the given click id and no segments.
    pub fn new(id: impl Into<MenuId>) -> Self {
        Row {
            id: id.into(),
            ..Row::default()
        }
    }

    /// A non-interactive row (id = [`MenuId::none`]).
    ///
    /// Deprecated (issue #62): redundant with [`Row::label_only`] (the canonical
    /// non-interactive constructor, which also adds the label in one call) and
    /// with [`Row::default`] (an empty non-interactive row to chain onto).
    #[deprecated(
        since = "0.11.0",
        note = "use Row::label_only(text) for header/label/info rows, or Row::default() for an empty non-interactive row to chain segments onto"
    )]
    pub fn info() -> Self {
        Row::default()
    }

    /// A non-interactive row (id = [`MenuId::none`], no check column) carrying
    /// only text, an optional leading icon, and the enabled flag. Intended for
    /// the label [`Row`] of an [`Item::Submenu`] or [`Item::SectionHeader`],
    /// where the [`MenuId`] and `checked` state are discarded anyway (see the
    /// note on [`Menu::submenu`]/[`Menu::section_header`]) — using this
    /// constructor makes that discard explicit at the call site. **The one
    /// obvious way** to build a non-interactive header/label/info row.
    pub fn label_only(text: impl Into<String>) -> Self {
        Row::default().label(text)
    }

    /// **The one obvious way** to add text to a row: append a plain-text
    /// left-aligned segment.
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.segments.push(Segment::new(text));
        self
    }

    /// **Convenience (issue #62):** bold the row's label — its first segment —
    /// without changing its size or family. This is the native styling home for
    /// the common "make this row bold" case; it renders identically to
    /// hand-building the equivalent whole-label [`StyleRun`] with
    /// [`Weight::Bold`]. A no-op on a row with no segments. It **replaces** the
    /// first segment's runs; for partial or mixed-weight styling, build
    /// [`StyleRun`]s directly (the full-control escape hatch).
    pub fn bold(mut self) -> Self {
        if let Some(seg) = self.segments.first_mut() {
            let len = seg.text.encode_utf16().count();
            seg.runs = vec![StyleRun::new(0, len, Color::Label).weight(Weight::Bold)];
        }
        self
    }

    /// **Convenience (issue #62):** color the row's value — its last segment,
    /// i.e. the trailing value of a [`label_value`](Row::label_value) row — with
    /// a whole-segment color. This is the native styling home for the common
    /// "tint this row's value" case. A no-op on a row with no segments. For a
    /// per-substring tint (e.g. only an over-limit percentage), build
    /// [`StyleRun`]s directly (the full-control escape hatch).
    pub fn value_color(mut self, color: Color) -> Self {
        if let Some(seg) = self.segments.last_mut() {
            seg.color = Some(color);
        }
        self
    }

    /// Append the common two-column pair: a growing left-aligned label segment
    /// followed by a right-aligned value segment (`Segment::grow(label)` +
    /// `Segment::trailing_value(value)`), yielding a flush-right value with no
    /// reserved chevron column.
    pub fn label_value(self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.segment(Segment::grow(label))
            .segment(Segment::trailing_value(value))
    }

    /// Append a pre-built segment.
    pub fn segment(mut self, segment: Segment) -> Self {
        self.segments.push(segment);
        self
    }

    /// Replace all segments.
    pub fn segments(mut self, segments: Vec<Segment>) -> Self {
        self.segments = segments;
        self
    }

    /// Set the leading icon.
    pub fn leading(mut self, icon: Icon) -> Self {
        self.leading = Some(icon);
        self
    }

    /// Set the trailing icon.
    pub fn trailing(mut self, icon: Icon) -> Self {
        self.trailing = Some(icon);
        self
    }

    /// Set enabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Show a check column in the given state.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Set an explicit background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set an explicit minimum row height.
    pub fn min_height(mut self, height: f32) -> Self {
        self.min_height = Some(height);
        self
    }

    /// Override the accessible name announced by screen readers, in place of
    /// [`Row::accessible_name`]. Required for icon-only rows (no segments),
    /// whose derived name would otherwise be empty (spec 30 §1.4).
    pub fn accessibility_label(mut self, label: impl Into<String>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// The row's accessible name: its segment texts concatenated with spaces.
    /// This is what screen readers announce for the row (see the design's a11y
    /// section).
    pub fn accessible_name(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// =============================================================================
// Content stacks (issue #44)
// =============================================================================
//
// A declarative, opt-in layout primitive for a row whose body is an arbitrary
// nested stack rather than the `Segment`-based column model above — the
// motivating case is an Apple-Weather-style "extra" row: a horizontal hourly
// strip whose cells each stack a time label, an icon, and a temperature
// vertically. This is purely additive: every existing `Item`/`Row`/`Segment`
// path is untouched, and a `Content` row is opted into via the new
// [`Item::Content`] variant only.
//
// Rendering (recursive measure + paint over this tree) lives in
// `crate::render::paint`, which is the only consumer of these types outside
// this module.

/// The main axis a [`Stack`] lays its children out along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Left-to-right.
    Horizontal,
    /// Top-to-bottom.
    Vertical,
}

/// A run of plain text inside a [`Content`] tree, styled independently of the
/// [`Segment`] column model (no `Flex`, no per-substring [`StyleRun`]s — a
/// content cell is expected to be small and simple; nest a [`Stack`] if a
/// cell needs more than one styled run).
#[derive(Clone, Debug)]
pub struct TextContent {
    /// The text to draw.
    pub text: String,
    /// Optional font override (else the theme's row font).
    pub font: Option<Font>,
    /// Optional color override (else the row's resolved base color).
    pub color: Option<Color>,
    /// Alignment within the box this content is given.
    pub align: Align,
}

impl TextContent {
    /// A new, unstyled, left-aligned text content.
    pub fn new(text: impl Into<String>) -> Self {
        TextContent {
            text: text.into(),
            font: None,
            color: None,
            align: Align::Left,
        }
    }

    /// Set a font override.
    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Set a color override.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Set the alignment within this content's allotted box.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
}

/// One node in a [`Stack`] tree.
#[derive(Clone, Debug)]
pub enum Content {
    /// A run of text (see [`TextContent`]).
    Text(TextContent),
    /// A square-ish icon, drawn at `size` logical points on each side.
    Image {
        /// The icon to draw.
        icon: Icon,
        /// The side length, in logical points.
        size: f32,
    },
    /// A nested stack (arbitrary depth).
    Stack(Stack),
    /// A flexible gap: zero intrinsic size, absorbs an equal share of any
    /// leftover main-axis space alongside sibling spacers.
    Spacer,
}

/// A declarative box-model stack: children laid out along `axis`, separated by
/// `spacing`, aligned on the cross axis per `align`. The body of an
/// [`Item::Content`] row, and freely nestable (a horizontal strip of vertical
/// cells, e.g. the Apple-Weather hourly extra).
#[derive(Clone, Debug)]
pub struct Stack {
    /// The main axis children are laid out along.
    pub axis: Axis,
    /// The gap between consecutive children, in logical points.
    pub spacing: f32,
    /// Cross-axis alignment of children within the stack's cross-axis extent.
    pub align: Align,
    /// The stack's children, in order.
    pub children: Vec<Content>,
    /// Optional background fill, painted behind this stack's own bounds
    /// before its children. Only the **top-level** stack of an
    /// [`Item::Content`] row paints this (a nested stack's `background` is
    /// currently ignored) — kept simple rather than churning the row-tint
    /// story for nested cells.
    pub background: Option<Color>,
}

impl Stack {
    /// A new empty horizontal stack with the given inter-child spacing.
    pub fn horizontal(spacing: f32) -> Self {
        Stack {
            axis: Axis::Horizontal,
            spacing,
            align: Align::Left,
            children: Vec::new(),
            background: None,
        }
    }

    /// A new empty vertical stack with the given inter-child spacing.
    pub fn vertical(spacing: f32) -> Self {
        Stack {
            axis: Axis::Vertical,
            spacing,
            align: Align::Left,
            children: Vec::new(),
            background: None,
        }
    }

    /// Set the cross-axis alignment.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Set an explicit background fill (top-level stack only; see the field doc).
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Append a single child.
    pub fn child(mut self, content: Content) -> Self {
        self.children.push(content);
        self
    }

    /// Replace all children.
    pub fn children(mut self, children: Vec<Content>) -> Self {
        self.children = children;
        self
    }
}

// =============================================================================
// Menu tree
// =============================================================================

/// A single entry in a [`Menu`].
#[derive(Clone, Debug)]
pub enum Item {
    /// An interactive (or info) row.
    Row(Row),
    /// A horizontal divider.
    Separator,
    /// A styled, non-interactive group heading.
    SectionHeader(Row),
    /// A row that opens a nested flyout panel beside it.
    Submenu {
        /// The row shown in the parent menu (drawn with a flyout affordance).
        label: Row,
        /// The nested menu shown in the flyout.
        menu: Menu,
    },
    /// A non-interactive, non-clickable row whose body is an arbitrary
    /// declarative [`Stack`] (issue #44) rather than the `Segment` column
    /// model — e.g. a horizontal hourly-forecast strip. Row height is derived
    /// from the stack's measured content. Never carries a [`MenuId`]: like
    /// [`Item::SectionHeader`], it is always non-interactive (see
    /// [`Item::is_interactive`]) and never appears in
    /// [`crate::render::paint::LaidMenu::rows`] — a future revision could add
    /// an id if an interactive content row is ever needed.
    Content(Stack),
}

impl Item {
    /// Whether this item can receive focus/clicks (rows and submenus that carry
    /// a real id). Separators, section headers, and content rows are never
    /// interactive.
    pub fn is_interactive(&self) -> bool {
        match self {
            Item::Row(row) => row.enabled && !row.id.is_none(),
            Item::Submenu { label, .. } => label.enabled,
            Item::Separator | Item::SectionHeader(_) | Item::Content(_) => false,
        }
    }
}

/// Descend `root` through a chain of `parent` indices, following one
/// [`Item::Submenu`] per step, and borrow the menu at the end of the path
/// (`root` itself for an empty chain). Returns `None` if any index is out of
/// range or does not name a submenu.
///
/// Shared by every platform backend's flyout stack (macOS/Windows/X11) so the
/// open-submenu path is resolved by borrowing, never cloning, the nested menus.
// A tray-only Linux build (`default-features = false`) compiles no flyout backend
// at all — the SNI tray defers submenu rendering to the host — so this helper has
// no caller there; allow it rather than warn in that one configuration (#21).
#[cfg_attr(not(feature = "x11-popup"), allow(dead_code))]
pub(crate) fn descend(root: &Menu, parents: impl IntoIterator<Item = usize>) -> Option<&Menu> {
    let mut menu = root;
    for parent in parents {
        menu = match menu.items.get(parent) {
            Some(Item::Submenu { menu, .. }) => menu,
            _ => return None,
        };
    }
    Some(menu)
}

/// A declarative menu: an ordered list of [`Item`]s. Build it with the fluent
/// methods, or populate [`Menu::items`] directly.
#[derive(Clone, Debug, Default)]
pub struct Menu {
    /// The ordered items.
    pub items: Vec<Item>,
}

impl Menu {
    /// An empty menu.
    pub fn new() -> Self {
        Menu::default()
    }

    /// Append any [`Item`] (the escape hatch). Prefer the labeled builder
    /// methods — [`row`](Menu::row), [`separator`](Menu::separator),
    /// [`section_header`](Menu::section_header), [`submenu`](Menu::submenu),
    /// [`content`](Menu::content) — which are the canonical, one-obvious-way path
    /// (issue #62); reach for `item` only to append a hand-built [`Item`].
    pub fn item(mut self, item: Item) -> Self {
        self.items.push(item);
        self
    }

    /// Append an [`Item::Row`].
    pub fn row(mut self, row: Row) -> Self {
        self.items.push(Item::Row(row));
        self
    }

    /// Append an [`Item::Separator`].
    pub fn separator(mut self) -> Self {
        self.items.push(Item::Separator);
        self
    }

    /// Append an [`Item::SectionHeader`].
    ///
    /// Note: the label `Row`'s [`MenuId`] and `checked` state are ignored for
    /// this position; only its text, leading icon, and `enabled` flag are
    /// used. Consider [`Row::label_only`] to make that explicit at the call
    /// site.
    pub fn section_header(mut self, row: Row) -> Self {
        self.items.push(Item::SectionHeader(row));
        self
    }

    /// Append an [`Item::Submenu`].
    ///
    /// Note: the label `Row`'s [`MenuId`] and `checked` state are ignored for
    /// this position; only its text, leading icon, and `enabled` flag are
    /// used. Consider [`Row::label_only`] to make that explicit at the call
    /// site.
    pub fn submenu(mut self, label: Row, menu: Menu) -> Self {
        self.items.push(Item::Submenu { label, menu });
        self
    }

    /// Append an [`Item::Content`] row (issue #44): a non-interactive row
    /// whose body is an arbitrary declarative [`Stack`].
    pub fn content(mut self, stack: Stack) -> Self {
        self.items.push(Item::Content(stack));
        self
    }

    /// The number of items at this level (not counting nested submenu items).
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the menu has no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Total number of interactive (focusable) items at this level.
    pub fn interactive_count(&self) -> usize {
        self.items.iter().filter(|i| i.is_interactive()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_id_none_is_inert() {
        assert!(MenuId::none().is_none());
        assert!(MenuId::from("quit").as_str() == "quit");
        assert!(!MenuId::from("quit").is_none());
    }

    #[test]
    fn menu_id_new_accepts_str_and_string() {
        // muda-compatible constructor: builds a MenuId from any string-like value
        // (issue #4 — `MenuId::new(...)` must work through the facade).
        assert_eq!(MenuId::new("quit").as_str(), "quit");
        assert_eq!(MenuId::new(String::from("open")).as_str(), "open");
        assert!(!MenuId::new("open").is_none());
    }

    #[test]
    fn builder_produces_expected_item_sequence() {
        let menu = Menu::new()
            .section_header(Row::label_only("Claude"))
            .row(Row::new("a").label("Account A"))
            .separator()
            .submenu(
                Row::new("more").label("More"),
                Menu::new().row(Row::new("x").label("X")),
            );

        assert_eq!(menu.len(), 4);
        assert!(matches!(menu.items[0], Item::SectionHeader(_)));
        assert!(matches!(menu.items[1], Item::Row(_)));
        assert!(matches!(menu.items[2], Item::Separator));
        assert!(matches!(menu.items[3], Item::Submenu { .. }));
    }

    #[test]
    fn interactivity_rules() {
        let header = Item::SectionHeader(Row::label_only("H"));
        let sep = Item::Separator;
        let info = Item::Row(Row::label_only("info"));
        let disabled = Item::Row(Row::new("x").label("X").enabled(false));
        let live = Item::Row(Row::new("x").label("X"));

        assert!(!header.is_interactive());
        assert!(!sep.is_interactive());
        assert!(!info.is_interactive()); // id == none
        assert!(!disabled.is_interactive());
        assert!(live.is_interactive());
    }

    #[test]
    fn interactive_count_skips_headers_and_separators() {
        let menu = Menu::new()
            .section_header(Row::label_only("H"))
            .row(Row::new("a").label("A"))
            .row(Row::label_only("info only"))
            .separator()
            .row(Row::new("b").label("B"));
        assert_eq!(menu.interactive_count(), 2);
    }

    #[test]
    fn accessible_name_joins_segments() {
        let row = Row::new("q").segments(vec![Segment::new("Quit"), Segment::new("usagio v1")]);
        assert_eq!(row.accessible_name(), "Quit usagio v1");
    }

    #[test]
    fn row_defaults_are_enabled_with_no_check_column() {
        let row = Row::new("x");
        assert!(row.enabled);
        assert_eq!(row.checked, None);
        assert!(row.leading.is_none());
    }

    #[test]
    fn accessibility_label_defaults_to_none_and_is_settable() {
        let row = Row::new("x");
        assert_eq!(row.accessibility_label, None);
        let labeled = Row::new("x").accessibility_label("Custom name");
        assert_eq!(labeled.accessibility_label.as_deref(), Some("Custom name"));
    }

    // The usagio `RowStyle` → muri mapping from the design doc, exercised as a
    // data-model test (no GUI needed).
    #[test]
    fn usagio_rowstyle_mapping() {
        // `bold` → Segment.font.weight = Bold
        let bold = Segment::new("label").font(Font::system(13.0, Weight::Bold));
        assert_eq!(bold.font.unwrap().weight, Weight::Bold);

        // `colors: Vec<(off, len, Severity)>` → Segment.runs with system colors;
        // offsets are UTF-16 code units.
        let value = Segment::new("47% / 89%")
            .align(Align::Right)
            .runs(vec![StyleRun::new(6, 3, Color::SystemRed)]);
        assert_eq!(value.align, Align::Right);
        assert_eq!(value.runs.len(), 1);
        assert_eq!(value.runs[0].color, Color::SystemRed);

        // `tab_x_kind: MenuRight` measuring hack → Flex::Grow + Align::Right.
        let label = Segment::new("me@example.com").flex(Flex::Grow);
        assert_eq!(label.flex, Flex::Grow);

        // `checkmark` → Row.checked = Some(true) + leading Icon::Checkmark.
        let active = Row::new("switch:claude:me")
            .checked(true)
            .leading(Icon::Checkmark);
        assert_eq!(active.checked, Some(true));
        assert!(matches!(active.leading, Some(Icon::Checkmark)));

        // `grey_tail_from` → trailing Segment with Color::SecondaryLabel.
        let tail = Segment::new("usagio v1").color(Color::SecondaryLabel);
        assert_eq!(tail.color, Some(Color::SecondaryLabel));

        // `disabled_but_white` info rows → Row::default() (id none), Color::Label.
        let info = Row::default();
        assert!(info.id.is_none());
        assert!(info.enabled);
    }

    // Issue #49 — convenience constructors for the two-column / per-span build path.
    #[test]
    fn segment_grow_and_trailing_value() {
        let label = Segment::grow("me@example.com");
        assert_eq!(label.flex, Flex::Grow);
        assert_eq!(label.align, Align::Left);

        let value = Segment::trailing_value("99%");
        assert_eq!(value.align, Align::Right);
        assert_eq!(value.flex, Flex::Fixed);
    }

    #[test]
    fn segment_run_appends_single_style_run() {
        let seg = Segment::new("47% / 89%")
            .run(StyleRun::new(0, 3, Color::SystemRed))
            .run(StyleRun::new(6, 3, Color::SystemGreen));
        assert_eq!(seg.runs.len(), 2);
        assert_eq!(seg.runs[0].color, Color::SystemRed);
        assert_eq!(seg.runs[1].color, Color::SystemGreen);
    }

    #[test]
    fn row_label_value_produces_grow_and_right_segments() {
        let row = Row::default().label_value("me@example.com", "Active");
        assert_eq!(row.segments.len(), 2);
        assert_eq!(row.segments[0].text, "me@example.com");
        assert_eq!(row.segments[0].flex, Flex::Grow);
        assert_eq!(row.segments[1].text, "Active");
        assert_eq!(row.segments[1].align, Align::Right);
    }

    #[test]
    fn style_run_from_byte_range_ascii() {
        let text = "47% / 89%";
        // "89%" starts at byte 6, len 3 — pure ASCII, so byte offsets == UTF-16 units.
        let run = StyleRun::from_byte_range(text, 6..9, Color::SystemRed);
        assert_eq!(run.start, 6);
        assert_eq!(run.len, 3);
    }

    #[test]
    fn style_run_from_byte_range_multibyte() {
        // "café" — 'é' is a 2-byte UTF-8 char but a single UTF-16 unit, so byte
        // offset 4 (start of 'é') is UTF-16 offset 3, and its byte length 2
        // is UTF-16 length 1.
        let text = "café";
        let run = StyleRun::from_byte_range(text, 4..6, Color::SystemRed);
        assert_eq!(run.start, 3);
        assert_eq!(run.len, 1);

        // Emoji (4-byte UTF-8, but 2 UTF-16 code units — a surrogate pair).
        let text = "hi 🎉!";
        let emoji_byte_start = text.find('🎉').unwrap();
        let emoji_byte_len = '🎉'.len_utf8();
        let run = StyleRun::from_byte_range(
            text,
            emoji_byte_start..emoji_byte_start + emoji_byte_len,
            Color::SystemRed,
        );
        assert_eq!(run.start, "hi ".encode_utf16().count());
        assert_eq!(run.len, 2);
    }

    #[test]
    fn style_run_from_byte_range_clamps_to_char_boundary() {
        // Range that lands mid-codepoint (byte 3 is inside 'é' which spans
        // bytes 3..5 in "café") should snap inward rather than panicking.
        let text = "café";
        let run = StyleRun::from_byte_range(text, 3..text.len() + 10, Color::SystemRed);
        // Start snaps back to the nearest boundary at or before byte 3 (byte 3
        // itself is not a boundary, so it snaps to byte 3's preceding boundary).
        assert!(run.start <= text.encode_utf16().count());
        // End clamps to the string's own byte length.
        assert_eq!(run.start + run.len, text.encode_utf16().count());
    }

    // Issue #50 — Row::label_only makes the MenuId/checked discard explicit.
    #[test]
    fn row_label_only_has_no_id_and_no_checked() {
        let row = Row::label_only("Section");
        assert!(row.id.is_none());
        assert_eq!(row.checked, None);
        assert_eq!(row.accessible_name(), "Section");
        assert!(row.enabled);
    }

    // Issue #62 — Row-level styling conveniences. `Row::bold()` must produce the
    // same whole-label bold StyleRun a consumer would hand-build, so it renders
    // identically (the compat set_bold path lands on this same shape).
    #[test]
    fn row_bold_bolds_the_label_segment() {
        let row = Row::new("x").label("Hi").bold();
        let runs = &row.segments[0].runs;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start, 0);
        assert_eq!(runs[0].len, "Hi".encode_utf16().count());
        assert_eq!(runs[0].weight, Some(Weight::Bold));
        assert_eq!(runs[0].color, Color::Label);
    }

    #[test]
    fn row_bold_is_identical_to_hand_built_style_run() {
        let convenient = Row::new("x").label("Account").bold();
        let hand_built = Row::new("x").segment(Segment::new("Account").run(
            StyleRun::new(0, "Account".encode_utf16().count(), Color::Label).weight(Weight::Bold),
        ));
        assert_eq!(convenient.segments[0].runs, hand_built.segments[0].runs);
        assert_eq!(convenient.segments[0].text, hand_built.segments[0].text);
    }

    #[test]
    fn row_bold_on_empty_row_is_a_noop() {
        let row = Row::new("x").bold();
        assert!(row.segments.is_empty());
    }

    #[test]
    fn row_value_color_colors_the_value_segment() {
        let row = Row::new("acct")
            .label_value("me@example.com", "Active")
            .value_color(Color::SystemRed);
        assert_eq!(row.segments.len(), 2);
        // Colors the *last* segment (the value), not the label.
        assert_eq!(row.segments[1].color, Some(Color::SystemRed));
        assert_eq!(row.segments[0].color, None);
    }

    #[test]
    fn row_value_color_on_empty_row_is_a_noop() {
        let row = Row::new("x").value_color(Color::SystemRed);
        assert!(row.segments.is_empty());
    }

    // Issue #62 — unified Icon constructors.
    #[test]
    fn icon_from_svg_builds_an_svg_icon() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#f00"/></svg>"##;
        let icon = Icon::from_svg(svg.to_vec());
        assert!(matches!(icon, Icon::Svg(_)));
    }

    #[test]
    fn icon_from_png_builds_a_png_icon() {
        let icon = Icon::from_png(vec![1u8, 2, 3]);
        assert!(matches!(icon, Icon::Png(_)));
    }

    #[test]
    fn icon_from_rgba_encodes_to_a_single_representation() {
        // A 2x2 opaque-white RGBA buffer becomes an (encoded-PNG) Icon — the
        // consumer never sees a separate raw-RGBA representation.
        let rgba = vec![255u8; 2 * 2 * 4];
        let icon = Icon::from_rgba(&rgba, 2, 2).expect("valid rgba");
        assert!(matches!(icon, Icon::Png(_)));
    }

    #[test]
    fn icon_from_rgba_rejects_mismatched_buffer() {
        // Wrong length for the stated dimensions → BadIcon rather than a panic.
        let err = Icon::from_rgba(&[0u8; 3], 2, 2).unwrap_err();
        assert!(matches!(err, crate::Error::BadIcon(_)));
    }
}
