//! # muri — Menu Utilities for Rust Interfaces
//!
//! `muri` is a cross-platform, fully-styleable **tray-icon + popup-menu** system
//! for Rust: a custom-drawn replacement for the `muda` + `tray-icon` pairing.
//! Unlike native menus (which delegate pixels to AppKit / Win32 USER / GTK and
//! therefore can't be restyled), muri draws **one consistent custom appearance on
//! every OS**. That single owned drawing surface is what makes true
//! left/right/center alignment, arbitrary colors and fonts, embedded logos, and
//! **flush right-aligned values with no reserved chevron column** actually
//! possible.
//!
//! muri owns the whole stack: the **tray icon**, the **styled popup**, and the
//! **anchoring** (where the popup appears relative to the icon). A consuming app
//! drives it with a small builder API:
//!
//! ```no_run
//! use muri::{Tray, Menu, Item, Row, Icon};
//!
//! # fn demo(icon_png: &[u8]) {
//! let menu = Menu::new()
//!     .row(Row::new("open").label("Open"))
//!     .separator()
//!     .row(Row::new("quit").label("Quit"));
//!
//! let _tray = Tray::new(Icon::from_png_bytes(icon_png))
//!     .tooltip("My App")
//!     .menu(menu)
//!     .on_click(|id| println!("clicked {}", id.as_str()));
//! // _tray.run(); // enters the platform event loop (not implemented in the skeleton)
//! # }
//! ```
//!
//! ## Status
//!
//! **Early / macOS-first WIP.** This crate currently ships the **public API
//! surface only** — the types, the builder, and the documentation of how each
//! backend will work. No pixels are drawn yet; methods that require a live
//! renderer (`Tray::run`, `Tray::open`, …) are `todo!()`. See the design
//! document and README for the roadmap (macOS → Windows → Linux).
//!
//! ## Rendering stack
//!
//! The portable drawer is [`winit`] (windowing) + `softbuffer`/`tiny-skia`
//! (CPU 2D raster) + `cosmic-text` (text shaping / fonts). CPU raster means a
//! tiny binary, no GPU warm-up, and instant popups with full pixel control. On
//! macOS the same custom-drawn surface is used for the look; only the tray
//! *anchoring* and transient-dismiss lean on a native AppKit-hosted panel
//! (`NSStatusItem`).
//!
//! ## Platform support (honest matrix)
//!
//! | OS      | Tray icon | Styled anchored popup | Screen-reader a11y |
//! |---------|-----------|-----------------------|--------------------|
//! | macOS   | yes       | yes (NSStatusItem rect) | NSAccessibility  |
//! | Windows | yes       | yes (`Shell_NotifyIconGetRect`) | UIA      |
//! | Linux   | yes       | **no** — see below, native-menu fallback | AT-SPI (fallback) |
//!
//! **Linux caveat:** the SNI / AppIndicator tray *host* owns and draws the icon
//! in its own process, so the app is never told the icon's on-screen rectangle
//! and never receives the click coordinate; Wayland additionally forbids a client
//! from positioning its own toplevel. A tray-*anchored* styled popup is therefore
//! architecturally impossible on Linux/Wayland. muri does not pretend otherwise:
//! [`Tray::run`] returns [`Error::Unsupported`]`(`[`Unsupported::TrayAnchor`]`)`
//! there, and the same [`Menu`] can be rendered through a native menu fallback or
//! shown as a pointer-anchored [`ContextMenu`]. See [`Unsupported`].
//!
//! [`winit`]: https://docs.rs/winit

#![forbid(unsafe_code)]

use std::sync::Arc;

// =============================================================================
// Identity & events
// =============================================================================

/// A click identifier for a row. muri treats this as an opaque string and hands
/// it back verbatim when the row is clicked — the *grammar* of the id (e.g.
/// `"switch:claude:me@x.com"`, `"quit"`) is entirely the consumer's business.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct MenuId(pub String);

impl MenuId {
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

/// The callback invoked on row activation. Stored by [`Tray`]/[`ContextMenu`].
pub type ClickHandler = Box<dyn Fn(&MenuId) + Send + 'static>;

// =============================================================================
// Errors
// =============================================================================

/// Things muri genuinely cannot do on a given platform. Surfaced rather than
/// papered over, so consumers can fall back deliberately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// Anchoring a styled popup to a tray icon. Returned on Linux/Wayland, where
    /// the SNI/AppIndicator host owns the icon and no geometry or click
    /// coordinate reaches the app. Use a native-menu fallback or a
    /// pointer-anchored [`ContextMenu`] instead.
    TrayAnchor,
    /// Positioning a non-activating toplevel, which Wayland forbids by protocol.
    ClientPositioning,
}

/// Errors returned by muri's fallible operations.
#[derive(Debug)]
pub enum Error {
    /// A capability that is not available on the current platform.
    Unsupported(Unsupported),
    /// The supplied icon bytes could not be decoded.
    BadIcon(String),
    /// A platform API call failed while creating or anchoring the surface.
    Platform(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(u) => write!(f, "unsupported on this platform: {u:?}"),
            Error::BadIcon(m) => write!(f, "bad icon: {m}"),
            Error::Platform(m) => write!(f, "platform error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

// =============================================================================
// Geometry
// =============================================================================

/// A logical (DPI-independent) screen point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalPoint {
    /// Horizontal position in logical pixels.
    pub x: f32,
    /// Vertical position in logical pixels.
    pub y: f32,
}

/// Which edge of the anchor the popup should grow from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Edge {
    /// Open below the anchor (typical for a top menu bar). The default.
    #[default]
    Bottom,
    /// Open above the anchor (typical for a bottom taskbar).
    Top,
    /// Open to the left of the anchor.
    Left,
    /// Open to the right of the anchor.
    Right,
}

/// Per-side insets in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    /// Top inset.
    pub top: f32,
    /// Right inset.
    pub right: f32,
    /// Bottom inset.
    pub bottom: f32,
    /// Left inset.
    pub left: f32,
}

impl Insets {
    /// Uniform insets on all four sides.
    pub fn uniform(v: f32) -> Self {
        Insets {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// Separate horizontal and vertical insets.
    pub fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Insets {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

// =============================================================================
// Color
// =============================================================================

/// A color: either a literal RGBA value, or a **semantic** role that resolves
/// against the active [`Theme`] (and, on macOS, to the matching system
/// `NSColor`) so dark/light and accent adapt automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// A literal 8-bit-per-channel RGBA color.
    Rgba(u8, u8, u8, u8),
    /// Primary text color (`labelColor`).
    Label,
    /// De-emphasized text color (`secondaryLabelColor`), e.g. a version tail.
    SecondaryLabel,
    /// The user's accent color (`controlAccentColor`).
    Accent,
    /// Separator / hairline color.
    Separator,
    /// System red — used for critical/over-limit severity.
    SystemRed,
    /// System orange — used for warning severity.
    SystemOrange,
    /// System green — used for healthy severity.
    SystemGreen,
    /// System yellow.
    SystemYellow,
}

impl Color {
    /// An opaque literal color from 8-bit channels.
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgba(r, g, b, 255)
    }
}

// =============================================================================
// Fonts
// =============================================================================

/// A font family selector. `System`/`SystemMono` resolve to the platform UI
/// font so menus match the OS; `Named` looks up an installed family.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum FontFamily {
    /// The platform UI font (San Francisco, Segoe UI, system default).
    #[default]
    System,
    /// The platform monospace UI font.
    SystemMono,
    /// A specific installed family by name.
    Named(String),
}

/// Font weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Weight {
    /// Regular / normal weight. The default.
    #[default]
    Regular,
    /// Medium weight.
    Medium,
    /// Semibold weight.
    Semibold,
    /// Bold weight.
    Bold,
}

/// A resolved font: family, size (in logical points), and weight.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The font family.
    pub family: FontFamily,
    /// Size in logical points.
    pub size: f32,
    /// Weight.
    pub weight: Weight,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            family: FontFamily::System,
            size: 13.0,
            weight: Weight::Regular,
        }
    }
}

impl Font {
    /// A system-font at the given size and weight.
    pub fn system(size: f32, weight: Weight) -> Self {
        Font {
            family: FontFamily::System,
            size,
            weight,
        }
    }

    /// A system monospace font at the given size and weight.
    pub fn mono(size: f32, weight: Weight) -> Self {
        Font {
            family: FontFamily::SystemMono,
            size,
            weight,
        }
    }
}

// =============================================================================
// Icons
// =============================================================================

/// A leading/trailing icon or logo. Raster formats are decoded at load; SVGs
/// are rasterized per-DPI at draw time so they stay crisp on HiDPI.
#[derive(Clone, Debug)]
pub enum Icon {
    /// A PNG (or other auto-detected raster format) from raw bytes.
    Png(Arc<[u8]>),
    /// An SVG from raw bytes, rasterized per-DPI.
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

    /// Build an [`Icon::Svg`] from raw SVG bytes.
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
#[derive(Clone, Copy, Debug)]
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
/// as a [`Item::SectionHeader`].
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
}

// =============================================================================
// Theming
// =============================================================================

/// Where the theme comes from.
#[derive(Clone, Debug, Default)]
pub enum ThemeSource {
    /// Track the OS dark/light appearance and accent color. The default.
    #[default]
    FollowSystem,
    /// Force the light theme.
    Light,
    /// Force the dark theme.
    Dark,
    /// Use a fully custom theme.
    Custom(Theme),
}

/// The full visual theme. Every semantic [`Color`] resolves against one of these,
/// and spacing/radius/row-height are tunable so a consumer can fully restyle.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Popup background fill.
    pub background: Color,
    /// Primary text color.
    pub label: Color,
    /// De-emphasized text color.
    pub secondary_label: Color,
    /// Accent color (selection, checkmarks).
    pub accent: Color,
    /// Separator / hairline color.
    pub separator: Color,
    /// Row background when hovered/selected.
    pub row_highlight: Color,
    /// Default row font.
    pub row_font: Font,
    /// Section-header font.
    pub header_font: Font,
    /// Default row height in logical points.
    pub row_height: f32,
    /// Corner radius of the popup and highlight in logical points.
    pub corner_radius: f32,
    /// Inner padding of the popup.
    pub padding: Insets,
    /// Horizontal gap between leading icon, segments, and trailing column.
    pub column_gap: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}

impl Theme {
    /// The default light theme.
    pub fn light() -> Self {
        Theme {
            background: Color::rgb(246, 246, 246),
            label: Color::rgb(0, 0, 0),
            secondary_label: Color::rgb(140, 140, 140),
            accent: Color::Accent,
            separator: Color::rgb(210, 210, 210),
            row_highlight: Color::Accent,
            row_font: Font::system(13.0, Weight::Regular),
            header_font: Font::system(13.0, Weight::Bold),
            row_height: 22.0,
            corner_radius: 8.0,
            padding: Insets::symmetric(6.0, 5.0),
            column_gap: 10.0,
        }
    }

    /// The default dark theme.
    pub fn dark() -> Self {
        Theme {
            background: Color::rgb(40, 40, 40),
            label: Color::rgb(255, 255, 255),
            secondary_label: Color::rgb(150, 150, 150),
            separator: Color::rgb(70, 70, 70),
            ..Theme::light()
        }
    }
}

/// Tunable popup options layered on top of the [`Theme`].
#[derive(Clone, Debug, Default)]
pub struct MenuOptions {
    /// Minimum popup width in logical points.
    pub min_width: Option<f32>,
    /// Maximum popup width in logical points.
    pub max_width: Option<f32>,
    /// Theme source.
    pub theme: ThemeSource,
}

// =============================================================================
// Tray
// =============================================================================

/// A live tray icon with an attached styled menu.
///
/// Construct with [`Tray::new`], configure via the builder methods, then call
/// [`Tray::run`] to enter the platform event loop (or integrate the backend with
/// an existing `winit` loop in a later phase). Content can be swapped at runtime
/// with [`Tray::set_menu`] — usagio rebuilds its menu on a ~0.75s tick.
///
/// ## Anchoring, per OS
///
/// - **macOS:** anchored to the `NSStatusItem` button; AppKit computes the
///   on-screen rect and which display the menu bar is on.
/// - **Windows:** anchored via `Shell_NotifyIconGetRect`, positioning a
///   `WS_EX_NOACTIVATE` layered window toward screen center.
/// - **Linux:** [`Tray::run`] returns
///   [`Error::Unsupported`]`(`[`Unsupported::TrayAnchor`]`)`; use the native-menu
///   fallback or a [`ContextMenu`].
pub struct Tray {
    icon: Icon,
    menu: Menu,
    tooltip: Option<String>,
    options: MenuOptions,
    on_click: Option<ClickHandler>,
}

impl Tray {
    /// Create a tray with the given status-bar icon.
    pub fn new(icon: Icon) -> Self {
        Tray {
            icon,
            menu: Menu::new(),
            tooltip: None,
            options: MenuOptions::default(),
            on_click: None,
        }
    }

    /// Attach the menu shown when the icon is clicked.
    pub fn menu(mut self, menu: Menu) -> Self {
        self.menu = menu;
        self
    }

    /// Set the tray icon tooltip / accessible name.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Set popup options (width bounds, theme source).
    pub fn options(mut self, options: MenuOptions) -> Self {
        self.options = options;
        self
    }

    /// Convenience: set just the theme source.
    pub fn theme(mut self, theme: ThemeSource) -> Self {
        self.options.theme = theme;
        self
    }

    /// Register a click handler invoked with the activated row's [`MenuId`].
    pub fn on_click(mut self, handler: impl Fn(&MenuId) + Send + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Replace the menu content at runtime (cheap; re-rendered on next open).
    pub fn set_menu(&mut self, menu: Menu) {
        self.menu = menu;
    }

    /// The tooltip, if set.
    pub fn tooltip_text(&self) -> Option<&str> {
        self.tooltip.as_deref()
    }

    /// Borrow the current menu.
    pub fn current_menu(&self) -> &Menu {
        &self.menu
    }

    /// Borrow the icon.
    pub fn icon(&self) -> &Icon {
        &self.icon
    }

    /// Borrow the options.
    pub fn menu_options(&self) -> &MenuOptions {
        &self.options
    }

    /// Programmatically show the popup anchored to the tray icon.
    ///
    /// Not implemented in the API skeleton; requires the rendering/anchoring
    /// backend built in a later phase.
    pub fn open(&self) -> Result<(), Error> {
        todo!("anchoring/render backend — see the muri design doc roadmap")
    }

    /// Hide the popup if shown.
    pub fn close(&self) {
        todo!("render backend")
    }

    /// Install the tray icon and run the platform event loop, dispatching clicks
    /// to the registered handler. On Linux this returns
    /// [`Error::Unsupported`]`(`[`Unsupported::TrayAnchor`]`)`.
    ///
    /// Not implemented in the API skeleton.
    pub fn run(self) -> Result<(), Error> {
        todo!("tray install + event loop — see the muri design doc roadmap")
    }
}

// =============================================================================
// ContextMenu
// =============================================================================

/// A free-standing styled menu shown at an explicit screen point. Unlike a
/// tray-anchored popup, this works anywhere a pointer coordinate is available —
/// including Linux/Wayland via `xdg_positioner` relative to the caller's own
/// surface — so it is muri's portable styled-menu primitive.
pub struct ContextMenu {
    menu: Menu,
    options: MenuOptions,
    on_click: Option<ClickHandler>,
}

impl ContextMenu {
    /// Create a context menu from a [`Menu`].
    pub fn new(menu: Menu) -> Self {
        ContextMenu {
            menu,
            options: MenuOptions::default(),
            on_click: None,
        }
    }

    /// Set popup options.
    pub fn options(mut self, options: MenuOptions) -> Self {
        self.options = options;
        self
    }

    /// Register a click handler.
    pub fn on_click(mut self, handler: impl Fn(&MenuId) + Send + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Borrow the menu.
    pub fn menu(&self) -> &Menu {
        &self.menu
    }

    /// Borrow the options.
    pub fn menu_options(&self) -> &MenuOptions {
        &self.options
    }

    /// Show the menu at the given screen point, growing from `edge`.
    ///
    /// Not implemented in the API skeleton.
    pub fn open_at(&self, _point: LogicalPoint, _edge: Edge) -> Result<(), Error> {
        todo!("render backend — see the muri design doc roadmap")
    }
}
