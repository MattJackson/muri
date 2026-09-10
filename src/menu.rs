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
}

/// The callback invoked on row activation. Stored by
/// [`Tray`](crate::Tray)/[`ContextMenu`](crate::ContextMenu).
pub type ClickHandler = Box<dyn Fn(&MenuId) + Send + 'static>;

// =============================================================================
// Icons
// =============================================================================

/// A leading/trailing icon or logo. Raster (PNG) bytes are decoded at load.
///
/// **Note:** muri does not yet ship an SVG rasterizer (it keeps a minimal
/// dependency set — see the ADR), so [`Icon::Svg`] is currently **not rendered**
/// on any backend and is treated as "no drawable image"; supply a PNG for now.
/// The variant exists so the API is forward-compatible once SVG rasterization
/// lands.
#[derive(Clone, Debug)]
pub enum Icon {
    /// A PNG (or other auto-detected raster format) from raw bytes.
    Png(Arc<[u8]>),
    /// An SVG from raw bytes. **Not yet rasterized** — see the [`Icon`] note; a
    /// future release will render these per-DPI. Today it draws nothing.
    Svg(Arc<[u8]>),
    /// The themed checkmark glyph, drawn in the leading column.
    Checkmark,
    /// A named symbol: an SF Symbol on macOS, with a bundled fallback elsewhere.
    Symbol(&'static str),
}

impl Icon {
    /// Build an [`Icon::Png`] from raw image bytes.
    pub fn from_png_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        Icon::Png(bytes.into())
    }

    /// Build an [`Icon::Svg`] from raw SVG bytes. **Not yet rasterized** (see the
    /// [`Icon`] note) — it currently draws nothing; supply a PNG for now.
    pub fn from_svg_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
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

    /// A non-interactive row (id = [`MenuId::none`]); handy for section headers
    /// and pure info lines.
    pub fn info() -> Self {
        Row::default()
    }

    /// Append a plain-text left-aligned segment (convenience).
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.segments.push(Segment::new(text));
        self
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
}

impl Item {
    /// Whether this item can receive focus/clicks (rows and submenus that carry
    /// a real id). Separators and section headers are never interactive.
    pub fn is_interactive(&self) -> bool {
        match self {
            Item::Row(row) => row.enabled && !row.id.is_none(),
            Item::Submenu { label, .. } => label.enabled,
            Item::Separator | Item::SectionHeader(_) => false,
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

    /// Append any [`Item`].
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
    pub fn section_header(mut self, row: Row) -> Self {
        self.items.push(Item::SectionHeader(row));
        self
    }

    /// Append an [`Item::Submenu`].
    pub fn submenu(mut self, label: Row, menu: Menu) -> Self {
        self.items.push(Item::Submenu { label, menu });
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
            .section_header(Row::info().label("Claude"))
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
        let header = Item::SectionHeader(Row::info().label("H"));
        let sep = Item::Separator;
        let info = Item::Row(Row::info().label("info"));
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
            .section_header(Row::info().label("H"))
            .row(Row::new("a").label("A"))
            .row(Row::info().label("info only"))
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

        // `disabled_but_white` info rows → Row::info() (id none), Color::Label.
        let info = Row::info();
        assert!(info.id.is_none());
        assert!(info.enabled);
    }
}
