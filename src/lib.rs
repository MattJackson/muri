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
//! // let _m = muri::MainThreadMarker::new().unwrap(); // call this on the real main thread
//! // _tray.run(_m); // installs the tray icon and enters the platform event loop
//! # }
//! ```
//!
//! ## Status
//!
//! **0.9.0 testing release — all three backends implemented.** On **macOS**
//! the crate draws a real styled popup: `Tray::run` installs the
//! `NSStatusItem`, opens the custom-drawn menu anchored to it in a
//! non-activating `NSPanel`, opens **flyout submenu** panels beside submenu
//! rows, and supports **keyboard navigation** ([`keynav`]) over the same
//! hover-stack the mouse drives. muri also publishes a parallel
//! **accessibility tree** ([`a11y`]) — [`Tray::accessibility_tree`] — mapping
//! the menu onto menu/menuitem roles, with an AccessKit `TreeUpdate` bridge
//! behind the `a11y` feature — and (also behind `a11y`) a per-window AccessKit
//! adapter, so the tree is exposed to NSAccessibility / VoiceOver. The shared
//! scene drawer, the `Flex`/`Align` flush-right layout, the flyout
//! placement/hover-stack logic, the keyboard-nav state machine, and the
//! a11y-tree construction are all pure and unit-tested. The **Windows**
//! backend installs the notification-area icon, anchors a `WS_EX_NOACTIVATE`
//! layered popup via `UpdateLayeredWindow`, and exposes UIA through
//! `accesskit_windows`. The **Linux** backend installs an SNI/AppIndicator
//! native menu and supports pointer-anchored popups via an X11
//! override-redirect `open_at`. [`ContextMenu::open_at`] / [`Popup::anchored_to`]
//! and, once a tray loop is running, [`TrayHandle::open`], work on all three
//! platforms; on-device verification (real hardware, real screen readers) is
//! exactly what the 0.9.0 testing release is for. See the README for the honest
//! platform matrix.
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
//! - [`platform`] — the single per-OS [`Platform`] seam
//!   (tray anchor, popup event loop, environment) selected once.
//! - [`error`] — [`Error`] / [`Unsupported`].
//!
//! ## Rendering stack
//!
//! Text is shaped and rasterized with `fontdb` (font discovery) + `harfrust`
//! (shaping) + `swash` (glyph rendering). Everything else — glyphs, fills,
//! strokes, images — is composited by muri's own in-house CPU
//! [`Framebuffer`](render::Framebuffer) blitter. CPU raster means a tiny
//! binary, no GPU warm-up, and
//! instant popups with full pixel control. Windowing is native per-OS: macOS
//! uses a non-activating `NSPanel` presented via `CALayer`; Windows uses a
//! `WS_EX_NOACTIVATE` layered window presented via `UpdateLayeredWindow`;
//! Linux uses an SNI tray plus an X11 override-redirect window for
//! [`ContextMenu::open_at`].
//!
//! ## Platform support (honest matrix)
//!
//! Legend: ✅ working (automated-tested / live) · 🔬 code-complete, on-device
//! verification pending (this is what the 0.9.0 testing release is for) ·
//! ❌ not offered (by design).
//!
//! | OS      | Tray icon | Styled anchored popup | Context menu (`open_at`) | Screen reader |
//! |---------|-----------|-----------------------|---------------------------|---------------|
//! | macOS   | 🔬 `NSStatusItem` | 🔬 non-activating `NSPanel`, N-level flyouts, mouse + keyboard nav | 🔬 `open_at` + `Popup` (shared `PopupSession`) | 🔬 VoiceOver (per-window AccessKit adapters wired) |
//! | Windows | 🔬 `Shell_NotifyIcon` | 🔬 `WS_EX_NOACTIVATE` layered popup, `WH_MOUSE_LL` dismiss | 🔬 `open_at` + `Popup` (reuses the layered popup) | 🔬 NVDA + Narrator (UIA via `accesskit_windows`) |
//! | Linux   | 🔬 SNI/AppIndicator native menu | ❌ tray-anchored (by design — see below); use pointer `ContextMenu` | 🔬 X11 override-redirect `open_at` (Wayland: `Unsupported::ClientPositioning`) | 🔬 Orca (AT-SPI via the native menu) |
//!
//! The 🔬 cells are muri code that *builds* and is clippy-clean on its target
//! in CI but hasn't been exercised on real hardware yet — verifying them, and
//! the four screen readers, is exactly what the 0.9.0 testing release is for;
//! each flips to ✅ as it's confirmed on the road to 1.0. The Linux
//! tray-anchored styled popup stays ❌ permanently.
//!
//! **Linux caveat:** the SNI / AppIndicator tray *host* owns and draws the icon
//! in its own process, so the app is never told the icon's on-screen rectangle
//! and never receives the click coordinate; Wayland additionally forbids a client
//! from positioning its own toplevel. A tray-*anchored* styled popup is therefore
//! architecturally impossible on Linux/Wayland. muri does not pretend otherwise:
//! [`Tray::run`] still works there (it installs an SNI/AppIndicator **native**
//! menu), but the tray *anchor rect* is unavailable — the backend reports
//! [`Error::Unsupported`]`(`[`Unsupported::TrayAnchor`]`)` — so a styled
//! tray-anchored popup is not offered; render the same [`Menu`] through that
//! native menu, or show it as a pointer-anchored [`ContextMenu`]. See
//! [`Unsupported`].

// The OS backends (NSStatusItem via objc2, plus the native popup surface —
// NSPanel/CALayer on macOS, layered HWND/UpdateLayeredWindow on Windows,
// X11 override-redirect on Linux) genuinely require `unsafe`; the portable
// scene drawer and data model do not. `unsafe` is therefore denied crate-wide
// and re-allowed only inside the per-OS `src/platform/{mac,windows}.rs` modules
// (via a module-level `#![allow(unsafe_code)]`), so no `target_os` gate is
// needed at the crate root — the platform seam already localizes the `unsafe`.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod a11y;
pub mod anchor;
// The muda-compatibility facade (spec 02/60), behind the `muda-compat` feature.
#[cfg(feature = "muda-compat")]
pub mod compat;
pub mod error;
// The process-global `MenuEvent` channel (spec 03 §3). Part of the native crate
// surface (always compiled); the muda-compat facade re-exports it.
pub mod event;
pub mod flyout;
pub mod geometry;
pub mod keynav;
pub mod layout;
pub mod menu;
pub mod platform;
pub mod render;
pub mod style;
pub mod theme;

pub use a11y::{announcement, build_tree, focused_id, locate, AxId, AxNode, AxRole, AxTree};
pub use anchor::place_popup;
pub use error::{Error, Result, Unsupported};
pub use event::MenuEventReceiver;
pub use flyout::{next_flyout, place_flyout, FlyoutPlacement, FlyoutSide, HoverTarget};
pub use geometry::{Edge, Insets, LogicalPoint, LogicalRect, LogicalSize};
pub use keynav::{handle_key, FlyoutFocus, MenuFocus, NavAction, NavKey};
pub use menu::{
    Align, Axis, ClickHandler, Content, Flex, Icon, Item, Menu, MenuEvent, MenuId, Row, Segment,
    Stack, StyleRun, TextContent,
};
pub use platform::{Appearance, Platform, PlatformEvent};
pub use render::{render_menu_to_png, render_menu_to_rgba};
pub use style::{Color, Font, FontFamily, Rgba, Weight};
pub use theme::{GutterPolicy, MenuOptions, OsFamily, Preset, Theme, ThemeMode, ThemeSource};

use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

// =============================================================================
// MainThreadMarker
// =============================================================================

/// A zero-cost, `!Send + !Sync` proof that the calling code is running on the
/// thread that obtained it — required by [`Tray::run`] and [`Tray::spawn`]
/// because installing an `NSStatusItem` is only ever safe from AppKit's main
/// thread on macOS (issue #46). Windows and Linux have no such restriction,
/// but both entry points take the same proof so there is one uniform,
/// compile-time-flagged contract across all three backends.
///
/// `PhantomData<*const ()>` is what makes this `!Send + !Sync` for free (a raw
/// pointer is neither): a `MainThreadMarker` obtained on one thread cannot be
/// moved to another thread and used there, so it cannot be smuggled across a
/// channel or into a spawned closure. The only way to have one on a given
/// thread is to call [`MainThreadMarker::new`] *on* that thread.
///
/// ```compile_fail
/// fn is_send<T: Send>() {}
/// is_send::<muri::MainThreadMarker>(); // fails: MainThreadMarker is !Send
/// ```
#[derive(Clone, Copy, Debug)]
pub struct MainThreadMarker(PhantomData<*const ()>);

impl MainThreadMarker {
    /// Obtain a proof that the caller is on the main thread.
    ///
    /// muri has no portable, safe way to *verify* "is this the main thread"
    /// without OS-specific FFI — and this crate root denies `unsafe_code`
    /// (see the crate-level `#![deny(unsafe_code)]`), so a real runtime check
    /// would have to live behind the per-OS [`platform`] seam, not here. This
    /// constructor is therefore intentionally **not** a runtime check: it
    /// always returns `Some`. Its value is entirely at compile time — see the
    /// type-level doc on [`MainThreadMarker`] — so it is the caller's
    /// responsibility to actually call this from the real main thread
    /// (typically the first thing `fn main()` does), exactly as documented.
    /// The per-OS backend (e.g. macOS's `objc2::MainThreadMarker`, used
    /// internally by the macOS `platform` module) still performs its own real
    /// runtime check before touching AppKit, so a caller that gets this wrong is
    /// caught there, not silently accepted.
    #[allow(clippy::unnecessary_wraps)]
    pub fn new() -> Option<Self> {
        Some(MainThreadMarker(PhantomData))
    }
}

// =============================================================================
// SurfaceId
// =============================================================================

/// A process-global, monotonically increasing identifier for one *surface*
/// instance — a [`Tray`], [`ContextMenu`], or [`Popup`] — so a [`MenuEvent`]
/// consumer can tell which surface an activation came from (issue #51).
///
/// Assigned once, in the surface's constructor, from a process-wide
/// [`AtomicU64`] counter; every live surface has a distinct id, and ids are
/// issued in creation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(u64);

impl SurfaceId {
    /// Issue the next process-global id.
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        SurfaceId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

// =============================================================================
// Tray
// =============================================================================

/// A live tray icon with an attached styled menu.
///
/// Construct with [`Tray::new`], configure via the builder methods, then call
/// [`Tray::run`] to install the icon and enter the platform event loop. Content
/// can be swapped at runtime with [`Tray::set_menu`] (or from another thread via
/// a [`TrayHandle`]) — usagio rebuilds its menu on a ~0.75s tick.
///
/// ## Anchoring, per OS
///
/// - **macOS:** anchored to the `NSStatusItem` button; AppKit computes the
///   on-screen rect and which display the menu bar is on.
/// - **Windows:** anchored via `Shell_NotifyIconGetRect`, positioning a
///   `WS_EX_NOACTIVATE` layered window toward screen center.
/// - **Linux:** [`Tray::run`] installs an SNI/AppIndicator **native** menu (it
///   does not block on anchoring); a styled *tray-anchored* popup is not offered
///   (the anchor rect is unavailable — the backend reports
///   [`Error::Unsupported`]`(`[`Unsupported::TrayAnchor`]`)`). Use that native
///   menu or a pointer-anchored [`ContextMenu`].
///
/// ## Shutdown
///
/// Dropping a [`Tray`] itself does **not** post `TrayCommand::Shutdown` — a
/// `Tray` is normally consumed by [`Tray::run`]/[`Tray::spawn`] long before it
/// would go out of scope. What *does* auto-shut-down the tray is dropping the
/// **last** outstanding [`TrayHandle`] obtained from a given [`Tray::handle`]
/// call (a handle and every clone of it): `TrayHandle`'s own `Drop`
/// (issue #47) posts `Shutdown` exactly once, when its internal clone count
/// reaches zero, so simply letting a handle family go out of scope removes
/// the tray — matching `tray-icon`'s drop-removes contract without an
/// explicit [`TrayHandle::shutdown`] call.
pub struct Tray {
    icon: Icon,
    menu: Menu,
    tooltip: Option<String>,
    /// Optional text shown *beside/instead of* the icon in the status item —
    /// the macOS menu-bar title (e.g. a live "45%"). Rendered on the
    /// `NSStatusItem` button; on Windows/Linux the notification area has no text
    /// label, so it is retained but not drawn.
    title: Option<String>,
    options: MenuOptions,
    on_click: Option<ClickHandler>,
    /// Commands posted by a [`TrayHandle`] from any thread, drained on the
    /// platform run loop. Shared with every handle handed out via
    /// [`Tray::handle`]; the backend installs the wake mechanism in
    /// [`Tray::run`] so posts made before `run` simply buffer here.
    commands: std::sync::Arc<std::sync::Mutex<Vec<TrayCommand>>>,
    /// The main-thread wake, installed by the backend once its run loop is
    /// live. Posting a command calls this (if present) to schedule a drain.
    waker: std::sync::Arc<std::sync::Mutex<Option<WakeFn>>>,
    /// This surface's process-global identity (issue #51). Exposed via
    /// [`Tray::surface_id`].
    surface_id: SurfaceId,
}

/// A thread-safe wake callback the backend installs to poke its native run
/// loop when a [`TrayHandle`] posts a command.
type WakeFn = Box<dyn Fn() + Send + Sync + 'static>;

/// A live command from a [`TrayHandle`] to a running [`Tray`], applied on the
/// platform's UI thread. Deliberately OS-neutral (carries only data model
/// types) so the same vocabulary drives every backend.
///
/// Every variant's payload is consumed by all three backends' `apply_command`
/// (macOS, Windows, Linux — see `src/platform/{mac,windows,linux}.rs`).
#[derive(Debug)]
pub(crate) enum TrayCommand {
    /// Replace the menu content and repaint/refresh any open popup.
    SetMenu(Menu),
    /// Replace the tray icon.
    SetIcon(Icon),
    /// Replace the tooltip / accessible name.
    SetTooltip(Option<String>),
    /// Replace the status-item text title (macOS menu-bar text).
    SetTitle(Option<String>),
    /// Show or hide the tray status item.
    SetVisible(bool),
    /// Programmatically open the popup anchored to the tray icon.
    Open,
    /// Dismiss the popup if shown.
    Close,
    /// Stop the tray: remove the OS status item and end the backend's run loop
    /// (and, for a spawned tray, its background thread). Posted by the compat
    /// facade's `Drop`, by an explicit [`TrayHandle::shutdown`] call, and
    /// **automatically by [`TrayHandle`]'s own `Drop`** when the last
    /// outstanding handle from a given [`Tray::handle`] call (including every
    /// clone of it) goes out of scope (issue #47) — so dropping every handle
    /// in a family removes the tray with no explicit call needed, matching
    /// `tray-icon`'s drop-removes contract. Note that `Tray` itself has no
    /// `Drop` impl; only `TrayHandle` auto-posts this. Best-effort and
    /// asynchronous, like every other command; the process-exit path also
    /// reclaims the OS registration on all three backends.
    Shutdown,
    /// Swap the live theme source. Applied on the backend's UI thread: the next
    /// popup open uses it, and any currently-open popup is repainted with it —
    /// so a consumer can offer an in-menu "Preview theme" switcher (issue #45).
    /// On Linux the persistent tray is a native `dbusmenu` the host renders, so
    /// this only affects muri's own styled context-menu popups, not the SNI tray.
    SetTheme(ThemeSource),
    /// Swap the live [`MenuOptions`] wholesale (theme + width bounds + gutter
    /// policy). Applied exactly like [`SetTheme`](TrayCommand::SetTheme) (#45).
    SetOptions(MenuOptions),
    /// Best-effort cross-thread request for the tray icon's current on-screen
    /// anchor rectangle: the backend replies on its UI thread with the live rect
    /// (or `None` when unavailable / unsupported, e.g. the Linux SNI tray, whose
    /// host never exposes icon geometry) (issue #48).
    QueryAnchorRect(std::sync::mpsc::Sender<Option<LogicalRect>>),
}

/// A cheap, `Clone + Send` remote control for a running [`Tray`].
///
/// [`Tray::run`] consumes the tray, so the consumer cannot mutate it afterward
/// directly. `TrayHandle` closes that gap: obtain one with [`Tray::handle`]
/// *before* `run`, move it to any thread, and post commands that the backend
/// applies on its UI thread. This is load-bearing for live menus — usagio
/// rewrites its menu roughly every 0.75s.
///
/// Commands posted before the run loop is live simply buffer and apply once it
/// starts.
#[derive(Clone)]
pub struct TrayHandle {
    queue: std::sync::Arc<std::sync::Mutex<Vec<TrayCommand>>>,
    waker: std::sync::Arc<std::sync::Mutex<Option<WakeFn>>>,
    /// Shared across every clone of *this handle family* — a fresh one per
    /// [`Tray::handle`] call, `.clone()` shares it. Its [`Drop`] posts
    /// `TrayCommand::Shutdown` **exactly once**, when the last clone releases the
    /// final `Arc` reference (issue #47). Deliberately its own `Arc`, independent
    /// of `queue`/`waker` (which the running backend also holds), so the backend's
    /// reference never factors in.
    ///
    /// Held purely for its `Drop` (an RAII shutdown guard); never read directly,
    /// hence the `allow(dead_code)` — cloning it (via `derive(Clone)`) is what
    /// shares the family across handle clones.
    #[allow(dead_code)]
    family: std::sync::Arc<HandleFamily>,
}

/// The shared drop-guard for a [`TrayHandle`] family. Posting `Shutdown` from
/// *this* type's `Drop` (rather than from `TrayHandle::drop` gated on
/// `Arc::strong_count == 1`) makes the "last clone gone" signal race-free: the
/// `Arc` runtime guarantees `HandleFamily::drop` runs exactly once, when the
/// final clone is released, even if two final clones on different threads drop
/// concurrently. A `strong_count == 1` check in `TrayHandle::drop` could let both
/// such drops read `count > 1` (each still counts itself, and neither field
/// decrement has happened yet) and neither post — leaking the tray + its thread.
struct HandleFamily {
    queue: std::sync::Arc<std::sync::Mutex<Vec<TrayCommand>>>,
    waker: std::sync::Arc<std::sync::Mutex<Option<WakeFn>>>,
}

impl Drop for HandleFamily {
    fn drop(&mut self) {
        if let Ok(mut q) = self.queue.lock() {
            q.push(TrayCommand::Shutdown);
        }
        if let Ok(waker) = self.waker.lock() {
            if let Some(wake) = waker.as_ref() {
                wake();
            }
        }
    }
}

impl std::fmt::Debug for TrayHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrayHandle").finish_non_exhaustive()
    }
}

impl TrayHandle {
    /// Test-only: drain and return the commands posted so far, so a unit test can
    /// assert that a setter posted the *right* `TrayCommand` with the right
    /// payload (the facade setters go through this path but the OS backend that
    /// would otherwise consume it is not installed headlessly).
    #[cfg(test)]
    pub(crate) fn take_posted(&self) -> Vec<TrayCommand> {
        self.queue
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    fn post(&self, command: TrayCommand) {
        if let Ok(mut q) = self.queue.lock() {
            q.push(command);
        }
        if let Ok(waker) = self.waker.lock() {
            if let Some(wake) = waker.as_ref() {
                wake();
            }
        }
    }

    /// Replace the menu shown on the next open (and repaint + refresh the
    /// accessibility tree live if the popup is already open).
    pub fn set_menu(&self, menu: Menu) {
        self.post(TrayCommand::SetMenu(menu));
    }

    /// Replace the tray icon.
    pub fn set_icon(&self, icon: Icon) {
        self.post(TrayCommand::SetIcon(icon));
    }

    /// Replace the tooltip / accessible name.
    pub fn set_tooltip(&self, tooltip: Option<impl Into<String>>) {
        self.post(TrayCommand::SetTooltip(tooltip.map(Into::into)));
    }

    /// Replace the status-item text title (macOS menu-bar text, e.g. a live
    /// "45%"). A no-op on the drawn item on Windows/Linux.
    pub fn set_title(&self, title: Option<impl Into<String>>) {
        self.post(TrayCommand::SetTitle(title.map(Into::into)));
    }

    /// Show or hide the tray status item.
    pub fn set_visible(&self, visible: bool) {
        self.post(TrayCommand::SetVisible(visible));
    }

    /// Programmatically open the popup anchored to the tray icon.
    pub fn open(&self) {
        self.post(TrayCommand::Open);
    }

    /// Dismiss the popup if shown.
    pub fn close(&self) {
        self.post(TrayCommand::Close);
    }

    /// Stop the tray: remove the OS status item and end its backend run loop (and
    /// background thread, for a spawned tray). Best-effort and asynchronous — the
    /// removal is applied on the backend's UI thread. Used by the compat facade
    /// to remove the icon when its `TrayIcon` is dropped, matching `tray-icon`.
    pub fn shutdown(&self) {
        self.post(TrayCommand::Shutdown);
    }

    /// Swap the live theme source (issue #45). Applied asynchronously on the
    /// backend's UI thread: the next popup open uses it, and any currently-open
    /// popup is repainted — so a consumer can wire an in-menu "Preview theme"
    /// submenu. See `TrayCommand::SetTheme` for the Linux SNI-tray caveat.
    pub fn set_theme(&self, theme: ThemeSource) {
        self.post(TrayCommand::SetTheme(theme));
    }

    /// Swap the live [`MenuOptions`] wholesale — theme, width bounds, gutter
    /// policy (issue #45). Applied like [`set_theme`](TrayHandle::set_theme).
    pub fn set_options(&self, options: MenuOptions) {
        self.post(TrayCommand::SetOptions(options));
    }

    /// Best-effort cross-thread query for the tray icon's current on-screen
    /// rectangle (issue #48). The native, main-thread counterpart is
    /// [`Tray::anchor_rect`].
    ///
    /// Posts a `TrayCommand::QueryAnchorRect` and waits briefly for the
    /// backend to reply on its UI thread. Returns `None` if the run loop is not
    /// yet live, the query times out, or the platform can't report geometry
    /// (the Linux SNI tray never can — its host owns the icon).
    pub fn anchor_rect(&self) -> Option<LogicalRect> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.post(TrayCommand::QueryAnchorRect(tx));
        rx.recv_timeout(std::time::Duration::from_millis(200))
            .ok()
            .flatten()
    }
}

impl Tray {
    /// Create a tray with the given status-bar icon.
    pub fn new(icon: Icon) -> Self {
        Tray {
            icon,
            menu: Menu::new(),
            tooltip: None,
            title: None,
            options: MenuOptions::default(),
            on_click: None,
            commands: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            waker: std::sync::Arc::new(std::sync::Mutex::new(None)),
            surface_id: SurfaceId::next(),
        }
    }

    /// This tray's process-global [`SurfaceId`] (issue #51), assigned once in
    /// [`Tray::new`]. Lets a [`MenuEvent`] consumer correlate an activation
    /// back to the surface it came from.
    pub fn surface_id(&self) -> SurfaceId {
        self.surface_id
    }

    /// A cheap, `Clone + Send` [`TrayHandle`] that can drive this tray from any
    /// thread once [`Tray::run`] is live (posts made earlier buffer). Obtain it
    /// before `run` consumes the tray.
    ///
    /// Each call starts a fresh, independently-tracked handle *family*: drop
    /// every handle and clone in the family returned by one `handle()` call
    /// and the tray auto-shuts-down (see [`TrayHandle`]'s `Drop`, issue #47).
    /// Calling `handle()` more than once creates separate families that don't
    /// share that shutdown-on-last-drop tracking with each other — prefer
    /// calling it once and fanning out with `.clone()`.
    pub fn handle(&self) -> TrayHandle {
        TrayHandle {
            queue: std::sync::Arc::clone(&self.commands),
            waker: std::sync::Arc::clone(&self.waker),
            family: std::sync::Arc::new(HandleFamily {
                queue: std::sync::Arc::clone(&self.commands),
                waker: std::sync::Arc::clone(&self.waker),
            }),
        }
    }

    /// The tray icon's current on-screen rectangle in logical coordinates
    /// (issue #48) — the native, main-thread counterpart of
    /// [`TrayHandle::anchor_rect`].
    ///
    /// **Honest limitation:** [`Tray::run`]/[`Tray::spawn`] *consume* the
    /// `Tray` to install the real, live status item, so by the time an icon
    /// exists to have an anchor rect, there is no `&self` left to call this
    /// on. This method queries a **fresh** [`platform::current()`] instance
    /// instead, which has never had `install_tray` called on it — so on
    /// macOS/Windows (which require an installed status item to compute the
    /// rect) it will reliably return `Err` until the engine grows a way to
    /// query the *live* platform state a running `Tray`/backend owns. It is
    /// provided now for API completeness/symmetry with
    /// [`TrayHandle::anchor_rect`] and because Linux's answer
    /// ([`Unsupported::TrayAnchor`]) does not depend on installation state.
    pub fn anchor_rect(&self) -> Result<LogicalRect> {
        platform::current().tray_anchor_rect()
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

    /// Set the status-item text title — the macOS menu-bar text shown beside (or
    /// instead of) the icon, e.g. a live "45%". Rendered on the `NSStatusItem`
    /// button; on Windows/Linux the notification area has no text label, so this
    /// is retained but not drawn.
    pub fn title(mut self, text: impl Into<String>) -> Self {
        self.title = Some(text.into());
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

    /// Replace the status-bar icon at runtime (re-published on next backend
    /// update; on Linux this re-registers the SNI `icon_pixmap`).
    pub fn set_icon(&mut self, icon: Icon) {
        self.icon = icon;
    }

    /// Replace the status-item text title at runtime (macOS menu-bar text).
    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    /// The status-item text title, if set.
    pub fn title_text(&self) -> Option<&str> {
        self.title.as_deref()
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
    ///
    /// Order (spec 03 §3): the per-surface `on_click` closure runs first
    /// (synchronously), then the same activation is projected onto the global
    /// [`MenuEvent`] channel and any `set_event_handler`. An inert
    /// [`MenuId::none`] fires neither.
    pub fn dispatch(&self, id: &MenuId) {
        if id.is_none() {
            return;
        }
        if let Some(handler) = &self.on_click {
            handler(id);
        }
        event::emit(id.clone(), self.surface_id);
    }

    /// Install the tray icon and run the platform event loop, dispatching row
    /// activations to the registered handler and to the global [`MenuEvent`]
    /// channel. This consumes the [`Tray`] and blocks for the lifetime of the
    /// tray; obtain a [`TrayHandle`] with [`Tray::handle`] *before* calling this
    /// to drive it (swap the menu/icon, show/hide the popup) from any thread.
    ///
    /// Per-OS backend, selected once in [`platform::current`]: macOS installs an
    /// `NSStatusItem` and runs the native `NSApplication` loop; Windows installs
    /// the `Shell_NotifyIcon` icon and runs the Win32 message pump; Linux
    /// installs an SNI/AppIndicator native menu and runs its worker loop.
    ///
    /// Takes a [`MainThreadMarker`] (issue #46): installing the `NSStatusItem`
    /// is only ever safe from AppKit's main thread on macOS, and requiring the
    /// proof here makes that a compile-time-visible contract for every
    /// backend, not just a documentation note.
    pub fn run(self, _m: MainThreadMarker) -> Result<()> {
        // One seam: the per-OS backend selected once in `platform::current()`.
        platform::current().run_tray(self)
    }

    /// Install the tray icon and begin driving it **without blocking**, returning
    /// a [`TrayHandle`] to mutate it (swap the menu/icon, show/hide the popup)
    /// from any thread. The non-blocking counterpart to [`Tray::run`], for hosts
    /// that own their own event loop — notably the `tray-icon` compatibility
    /// facade.
    ///
    /// On Windows and Linux the tray's native UI pump runs on a dedicated
    /// background thread. On macOS this is **best-effort**: AppKit's status item
    /// must live on the main thread, so `spawn` must be called from the main
    /// thread and relies on the host's existing `NSApplication` run loop to
    /// service the icon (see [`Platform::spawn_tray`]).
    ///
    /// Takes a [`MainThreadMarker`] (issue #46) for the same reason as
    /// [`Tray::run`].
    pub fn spawn(self, _m: MainThreadMarker) -> Result<TrayHandle> {
        let handle = self.handle();
        platform::current().spawn_tray(self)?;
        Ok(handle)
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
    /// This surface's process-global identity (issue #51).
    surface_id: SurfaceId,
}

impl ContextMenu {
    /// Create a context menu from a [`Menu`].
    pub fn new(menu: Menu) -> Self {
        ContextMenu {
            menu,
            options: MenuOptions::default(),
            on_click: None,
            surface_id: SurfaceId::next(),
        }
    }

    /// This surface's process-global [`SurfaceId`] (issue #51), assigned once
    /// in [`ContextMenu::new`].
    pub fn surface_id(&self) -> SurfaceId {
        self.surface_id
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

    /// Dispatch a click to the registered handler, if any. Fires the `on_click`
    /// closure first, then projects the activation onto the global
    /// [`MenuEvent`] channel (spec 03 §3); an inert [`MenuId::none`] fires
    /// neither.
    pub fn dispatch(&self, id: &MenuId) {
        if id.is_none() {
            return;
        }
        if let Some(handler) = &self.on_click {
            handler(id);
        }
        event::emit(id.clone(), self.surface_id);
    }

    /// Show the menu at the given screen point, growing from `edge`, and block
    /// until it is dismissed.
    ///
    /// The point is treated as a zero-size anchor rectangle and funnelled through
    /// the shared `PopupSession` (the same `place_popup` + scene drawer + dismiss
    /// machinery the tray uses — spec 20 §3). Row activation is dispatched to the
    /// [`on_click`](ContextMenu::on_click) handler. On platforms whose styled
    /// popup loop is not implemented yet this returns [`Error::Platform`].
    pub fn open_at(&self, point: LogicalPoint, edge: Edge) -> Result<()> {
        let anchor = LogicalRect::new(point, LogicalSize::new(0.0, 0.0));
        let handler = |id: &MenuId| self.dispatch(id);
        platform::current().open_popup_session(
            self.menu.clone(),
            self.options.clone(),
            &handler,
            anchor,
            edge,
        )
    }
}

// =============================================================================
// Popup / Dropdown
// =============================================================================

/// A styled dropdown popup anchored to an **arbitrary caller rectangle** (e.g. a
/// toolbar button), rather than the tray icon or a bare point. It reuses the exact
/// `place_popup` math the tray uses; [`Tray`], [`ContextMenu`], and `Popup` differ
/// only in how the anchor rectangle is obtained, and all funnel into one shared
/// `PopupSession` (spec 01 §5.3, spec 20 §3).
pub struct Popup {
    menu: Menu,
    options: MenuOptions,
    on_click: Option<ClickHandler>,
    /// This surface's process-global identity (issue #51).
    surface_id: SurfaceId,
}

impl Popup {
    /// Create a dropdown popup from a [`Menu`].
    pub fn new(menu: Menu) -> Self {
        Popup {
            menu,
            options: MenuOptions::default(),
            on_click: None,
            surface_id: SurfaceId::next(),
        }
    }

    /// This surface's process-global [`SurfaceId`] (issue #51), assigned once
    /// in [`Popup::new`].
    pub fn surface_id(&self) -> SurfaceId {
        self.surface_id
    }

    /// Set popup options (width bounds, theme source).
    pub fn options(mut self, options: MenuOptions) -> Self {
        self.options = options;
        self
    }

    /// Register a click handler invoked with the activated row's [`MenuId`].
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

    /// Dispatch a click to the registered handler, if any. Fires the `on_click`
    /// closure first, then projects the activation onto the global
    /// [`MenuEvent`] channel (spec 03 §3) — so a `Popup` is a first-class event
    /// source alongside [`Tray`] and [`ContextMenu`], as
    /// [`MenuEvent::receiver`](crate::event) promises. An inert
    /// [`MenuId::none`] fires neither.
    pub fn dispatch(&self, id: &MenuId) {
        if id.is_none() {
            return;
        }
        if let Some(handler) = &self.on_click {
            handler(id);
        }
        event::emit(id.clone(), self.surface_id);
    }

    /// Show the popup anchored to `anchor` (a caller rectangle in screen logical
    /// coordinates), growing from `edge`, and block until it is dismissed.
    ///
    /// This is the tray's session anchored to an arbitrary rect instead of the
    /// tray icon (spec 20 §3). On platforms whose styled popup loop is not
    /// implemented yet this returns [`Error::Platform`].
    pub fn anchored_to(&self, anchor: LogicalRect, edge: Edge) -> Result<()> {
        let handler = |id: &MenuId| self.dispatch(id);
        platform::current().open_popup_session(
            self.menu.clone(),
            self.options.clone(),
            &handler,
            anchor,
            edge,
        )
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
            .title("45%")
            .menu(Menu::new().row(Row::new("quit").label("Quit")))
            .theme(ThemeSource::System(ThemeMode::Dark));
        assert_eq!(tray.tooltip_text(), Some("usagio"));
        assert_eq!(tray.title_text(), Some("45%"));
        assert_eq!(tray.current_menu().len(), 1);
        assert!(matches!(
            tray.menu_options().theme,
            ThemeSource::System(ThemeMode::Dark)
        ));
    }

    #[test]
    fn tray_handle_setters_post_the_matching_command() {
        // Guards the mechanism every facade setter relies on: a TrayHandle setter
        // must post the *right* TrayCommand with the right payload. A swap (e.g.
        // set_title posting SetTooltip) would fail here — the facade unit tests
        // can't catch that headlessly because no OS backend drains the queue.
        let tray = Tray::new(Icon::Checkmark);
        let handle = tray.handle();
        handle.set_title(Some("45%"));
        handle.set_tooltip(Some("tip"));
        handle.set_visible(false);
        let posted = handle.take_posted();
        assert!(
            matches!(&posted[0], TrayCommand::SetTitle(Some(s)) if s == "45%"),
            "got {:?}",
            posted.first()
        );
        assert!(
            matches!(&posted[1], TrayCommand::SetTooltip(Some(s)) if s == "tip"),
            "got {:?}",
            posted.get(1)
        );
        assert!(matches!(&posted[2], TrayCommand::SetVisible(false)));
    }

    #[test]
    fn tray_title_defaults_none_and_set_title_replaces_it() {
        // A tray with no title (the macOS menu-bar text) reports None; the
        // runtime setter replaces and clears it (issue #8 part 3).
        let mut tray = Tray::new(Icon::Checkmark);
        assert_eq!(tray.title_text(), None);
        tray.set_title(Some("12%".to_owned()));
        assert_eq!(tray.title_text(), Some("12%"));
        tray.set_title(None);
        assert_eq!(tray.title_text(), None);
    }

    #[test]
    fn tray_dispatch_invokes_handler_with_id() {
        let _guard = crate::event::test_lock();
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
    fn dispatch_fires_closure_before_channel_and_skips_inert() {
        let _guard = crate::event::test_lock();
        use std::sync::atomic::AtomicBool;
        use std::sync::Mutex;

        // Drain any stragglers so the in-closure peek below sees only our event.
        while MenuEvent::receiver().try_recv().is_ok() {}

        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        // Whether, *at the moment the closure runs*, the activation is already
        // on the global channel. Under the correct order (closure first, then
        // `event::emit`) it must NOT be — the channel is still empty while the
        // closure executes. A reversed `emit`-then-closure implementation would
        // leave the event waiting here, flipping this flag.
        let seen_on_channel_in_closure = Arc::new(AtomicBool::new(false));
        let log2 = Arc::clone(&log);
        let flag2 = Arc::clone(&seen_on_channel_in_closure);
        let tray = Tray::new(Icon::Checkmark).on_click(move |id| {
            if let Ok(ev) = MenuEvent::receiver().try_recv() {
                if ev.id == MenuId::from("m3_order_probe") {
                    flag2.store(true, Ordering::SeqCst);
                }
            }
            log2.lock()
                .unwrap()
                .push(format!("closure:{}", id.as_str()))
        });

        // An inert `MenuId::none()` fires neither the closure nor the channel.
        tray.dispatch(&MenuId::none());
        assert!(
            log.lock().unwrap().is_empty(),
            "inert id must not fire the closure"
        );

        tray.dispatch(&MenuId::from("m3_order_probe"));
        log.lock().unwrap().push("after_dispatch".to_string());

        assert!(
            !seen_on_channel_in_closure.load(Ordering::SeqCst),
            "channel must still be empty while the closure runs (closure-first order)"
        );

        let mut found = false;
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == MenuId::from("m3_order_probe") {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "activation must be projected onto the global channel after the closure"
        );

        let log = log.lock().unwrap();
        assert_eq!(log[0], "closure:m3_order_probe");
        assert_eq!(log[1], "after_dispatch");
    }

    #[test]
    fn context_menu_dispatch_works() {
        let _guard = crate::event::test_lock();
        let hit = Arc::new(AtomicUsize::new(0));
        let hit2 = Arc::clone(&hit);
        let cm = ContextMenu::new(Menu::new()).on_click(move |_| {
            hit2.fetch_add(1, Ordering::SeqCst);
        });
        cm.dispatch(&MenuId::from("x"));
        assert_eq!(hit.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn main_thread_marker_new_always_returns_some() {
        // Documents the intentional "no portable safe runtime check" contract
        // (issue #46): see the type-level compile_fail doctest for the
        // !Send/!Sync half of the guarantee.
        assert!(MainThreadMarker::new().is_some());
    }

    #[test]
    fn surface_id_is_unique_and_monotonic_across_surface_kinds() {
        // issue #51: every Tray/ContextMenu/Popup gets its own id, in
        // creation order, from the same process-global counter.
        let a = Tray::new(Icon::Checkmark).surface_id();
        let b = ContextMenu::new(Menu::new()).surface_id();
        let c = Popup::new(Menu::new()).surface_id();
        let d = Tray::new(Icon::Checkmark).surface_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, d);
        assert!(b.0 > a.0);
        assert!(c.0 > b.0);
        assert!(d.0 > c.0);
    }

    #[test]
    fn tray_handle_drop_posts_shutdown_once_on_last_clone_only() {
        // issue #47: an intermediate clone dropping must NOT shut the tray
        // down; only the last outstanding clone of a handle family does, and
        // exactly once. `inspector` is a *separate* handle family (its own
        // `handle()` call) sharing the same underlying command queue, used
        // purely to observe what got posted without itself counting toward
        // `family`'s clone count.
        let tray = Tray::new(Icon::Checkmark);
        let family = tray.handle();
        let clone = family.clone();
        let inspector = tray.handle();

        drop(clone);
        assert!(
            inspector.take_posted().is_empty(),
            "an intermediate clone dropping must not post Shutdown"
        );

        drop(family);
        let posted = inspector.take_posted();
        assert!(
            matches!(posted.as_slice(), [TrayCommand::Shutdown]),
            "the last clone dropping must post exactly one Shutdown, got {:?}",
            posted
        );
    }

    #[test]
    fn concurrent_last_clone_drops_post_shutdown_exactly_once() {
        // Race regression: the last two clones of a family dropped concurrently on
        // two threads must still post exactly ONE Shutdown. The old
        // `Arc::strong_count == 1` check in `TrayHandle::drop` could let both drops
        // read count > 1 and neither post, leaking the tray; the `Arc<HandleFamily>`
        // drop-guard makes it exactly-once regardless of interleaving (issue #47).
        let tray = Tray::new(Icon::Checkmark);
        let inspector = tray.handle(); // separate family; only observes the queue
        let h1 = tray.handle();
        let h2 = h1.clone();
        let t1 = std::thread::spawn(move || drop(h1));
        let t2 = std::thread::spawn(move || drop(h2));
        t1.join().unwrap();
        t2.join().unwrap();
        let posted = inspector.take_posted();
        let shutdowns = posted
            .iter()
            .filter(|c| matches!(c, TrayCommand::Shutdown))
            .count();
        assert_eq!(
            shutdowns, 1,
            "the family's final drop must post exactly one Shutdown, got {posted:?}"
        );
    }

    #[test]
    fn set_theme_and_options_post_live_swap_commands() {
        // The in-menu "Preview theme" path (#45): a handle posts SetTheme /
        // SetOptions, which the backend applies on its UI thread.
        let tray = Tray::new(Icon::Checkmark);
        let inspector = tray.handle();
        let control = tray.handle();
        control.set_theme(ThemeSource::MacOs(ThemeMode::Dark));
        control.set_options(MenuOptions::default().min_width(120.0));
        let posted = inspector.take_posted();
        assert!(
            matches!(
                posted.as_slice(),
                [TrayCommand::SetTheme(_), TrayCommand::SetOptions(_)]
            ),
            "set_theme/set_options must post the matching commands in order, got {:?}",
            posted
        );
    }

    #[test]
    fn anchor_rect_query_times_out_to_none_with_no_live_backend() {
        // With no run loop draining the queue, the QueryAnchorRect reply never
        // arrives, so the best-effort cross-thread query returns None (#48).
        let tray = Tray::new(Icon::Checkmark);
        let handle = tray.handle();
        assert!(handle.anchor_rect().is_none());
        // Prove it actually *posted* the query (not merely returned a stubbed
        // None): a QueryAnchorRect command must be sitting in the queue.
        let posted = handle.take_posted();
        assert!(
            posted
                .iter()
                .any(|c| matches!(c, TrayCommand::QueryAnchorRect(_))),
            "anchor_rect must post a QueryAnchorRect command, got {posted:?}"
        );
    }
}
