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
//! use muri::{Tray, Menu, Row, Icon};
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
//! **macOS-first WIP.** On **macOS** the crate draws a real styled popup:
//! `Tray::run` installs the `NSStatusItem`, opens the custom-drawn menu anchored
//! to it, opens **flyout submenu** panels beside submenu rows, and supports
//! **keyboard navigation** ([`keynav`]) over the same hover-stack the mouse
//! drives. muri also publishes a parallel **accessibility tree** ([`a11y`]) —
//! [`Tray::accessibility_tree`] — mapping the menu onto menu/menuitem roles, with
//! an AccessKit `TreeUpdate` bridge behind the `a11y` feature. The shared scene
//! drawer, the `Flex`/`Align` flush-right layout, the flyout placement/hover-stack
//! logic, the keyboard-nav state machine, and the a11y-tree construction are all
//! pure and unit-tested. `Tray::open`, `ContextMenu::open_at`, the Windows/Linux
//! backends, and attaching the AccessKit platform adapter to the event loop are
//! still `todo!()`/follow-up. See the README for the roadmap
//! (macOS → Windows → Linux).
//!
//! ## Crate layout
//!
//! - [`menu`] — the declarative `Segment` → `Row` → `Item` → `Menu` tree,
//!   identifiers, events, and icons (pure data model).
//! - [`style`] / [`theme`] — visual primitives ([`Color`], [`Font`]) and the
//!   [`Theme`] surface with pure semantic-color resolution.
//! - [`layout`] — pure `Flex`/`Align` width resolution (the flush-right layout).
//! - [`flyout`] — pure flyout-submenu placement (right/left flip, clamp) and
//!   hover-stack transitions.
//! - [`geometry`] — logical points/sizes/rects and [`Insets`]/[`Edge`].
//! - [`render`] — the [`SceneDrawer`](render::SceneDrawer) interface shared by
//!   the one CPU-raster backend on every OS.
//! - [`tray`] — the per-OS [`TrayAnchor`](tray::TrayAnchor) shims.
//! - [`error`] — [`Error`] / [`Unsupported`].
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

// The macOS backend (NSStatusItem via objc2, the winit/softbuffer popup surface)
// and the Windows backend (Shell_NotifyIcon via windows-sys) genuinely require
// `unsafe`; the portable scene drawer and data model do not. So `unsafe` is
// forbidden everywhere except the macOS and Windows builds.
#![cfg_attr(
    all(not(target_os = "macos"), not(target_os = "windows")),
    forbid(unsafe_code)
)]
#![deny(missing_docs)]

pub mod a11y;
pub mod anchor;
pub mod error;
pub mod flyout;
pub mod geometry;
pub mod keynav;
pub mod layout;
pub mod menu;
pub mod render;
pub mod style;
pub mod theme;
pub mod tray;

pub use a11y::{announcement, build_tree, focused_id, locate, AxId, AxNode, AxRole, AxTree};
pub use anchor::place_popup;
pub use error::{Error, Result, Unsupported};
pub use flyout::{next_flyout, place_flyout, FlyoutPlacement, FlyoutSide, HoverTarget};
pub use geometry::{Edge, Insets, LogicalPoint, LogicalRect, LogicalSize};
pub use keynav::{handle_key, FlyoutFocus, MenuFocus, NavAction, NavKey};
pub use menu::{
    Align, ClickHandler, Flex, Icon, Item, Menu, MenuEvent, MenuId, Row, Segment, StyleRun,
};
pub use style::{Color, Font, FontFamily, Rgba, Weight};
pub use theme::{MenuOptions, Theme, ThemeSource};

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

    /// Build the [`AxTree`] the platform screen reader walks over the current
    /// menu. The backend rebuilds this whenever the menu is (re)opened or swapped
    /// and feeds it to the platform accessibility API (via AccessKit).
    pub fn accessibility_tree(&self) -> AxTree {
        a11y::build_tree(&self.menu)
    }

    /// Dispatch a click to the registered handler, if any. Used by the backend
    /// when a row is activated; exposed so the data flow is testable without a
    /// live event loop.
    pub fn dispatch(&self, id: &MenuId) {
        if let Some(handler) = &self.on_click {
            handler(id);
        }
    }

    /// Programmatically show the popup anchored to the tray icon.
    ///
    /// Not implemented in the API skeleton; requires the rendering/anchoring
    /// backend built in a later phase.
    pub fn open(&self) -> Result<()> {
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
    /// On macOS this installs the `NSStatusItem` and runs the winit event loop.
    /// On other platforms it is not implemented yet (Windows) / returns
    /// [`Unsupported::TrayAnchor`] (Linux).
    pub fn run(self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            crate::tray::macos::run_tray(self)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self;
            todo!("tray install + event loop — see the muri design doc roadmap")
        }
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

    /// Build the [`AxTree`] the platform screen reader walks over this menu.
    pub fn accessibility_tree(&self) -> AxTree {
        a11y::build_tree(&self.menu)
    }

    /// Dispatch a click to the registered handler, if any.
    pub fn dispatch(&self, id: &MenuId) {
        if let Some(handler) = &self.on_click {
            handler(id);
        }
    }

    /// Show the menu at the given screen point, growing from `edge`.
    ///
    /// Not implemented in the API skeleton.
    pub fn open_at(&self, _point: LogicalPoint, _edge: Edge) -> Result<()> {
        todo!("render backend — see the muri design doc roadmap")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn tray_builder_stores_configuration() {
        let tray = Tray::new(Icon::Checkmark)
            .tooltip("usagio")
            .menu(Menu::new().row(Row::new("quit").label("Quit")))
            .theme(ThemeSource::Dark);
        assert_eq!(tray.tooltip_text(), Some("usagio"));
        assert_eq!(tray.current_menu().len(), 1);
        assert!(matches!(tray.menu_options().theme, ThemeSource::Dark));
    }

    #[test]
    fn tray_dispatch_invokes_handler_with_id() {
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = Arc::clone(&seen);
        let tray = Tray::new(Icon::Checkmark).on_click(move |id| {
            if id.as_str() == "quit" {
                seen2.fetch_add(1, Ordering::SeqCst);
            }
        });
        tray.dispatch(&MenuId::from("quit"));
        tray.dispatch(&MenuId::from("other"));
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn context_menu_dispatch_works() {
        let hit = Arc::new(AtomicUsize::new(0));
        let hit2 = Arc::clone(&hit);
        let cm = ContextMenu::new(Menu::new()).on_click(move |_| {
            hit2.fetch_add(1, Ordering::SeqCst);
        });
        cm.dispatch(&MenuId::from("x"));
        assert_eq!(hit.load(Ordering::SeqCst), 1);
    }
}
