//! The single platform seam: one [`Platform`] trait, one implementation module
//! per OS behind it, selected exactly once here.
//!
//! This module is the **only** place in the crate allowed to branch on
//! `#[cfg(target_os = ...)]` (see [ADR-0002] and the `strict_cfg` test). Every
//! other module — the menu data model, layout, rendering glue, the `Tray` /
//! `ContextMenu` surface in the crate root — is OS-agnostic and reaches the host
//! only through the [`Platform`] trait and the unified types below
//! ([`PlatformEvent`], [`Appearance`]). No OS- or toolkit-specific handle
//! (`NSWindow`, `HWND`, a winit `Window`, …) is ever named in this trait or in
//! the types it exchanges with the engine.
//!
//! The concrete per-OS type is re-exported as [`PlatformImpl`], and
//! [`current()`] returns a fresh instance of it. The engine only ever writes
//! `platform::current()` — it never names `MacPlatform` / `WindowsPlatform` /
//! `LinuxPlatform` directly.
//!
//! - **macOS** — `mac`: `NSStatusItem` anchor + a native non-activating
//!   `NSPanel` run-loop popup (implemented).
//! - **Windows** — `windows`: `Shell_NotifyIcon` tray + anchor-rect, plus a
//!   native `WS_EX_NOACTIVATE` layered-window message-pump popup
//!   (implemented).
//! - **Linux** — `linux`: an SNI/AppIndicator native tray menu, plus an X11
//!   override-redirect `open_at` for pointer-anchored popups; tray-anchored
//!   popups remain the honest carve-out (SNI/AppIndicator gives no geometry),
//!   so [`Platform::run_tray`] reports
//!   [`Unsupported::TrayAnchor`](crate::Unsupported::TrayAnchor).
//!
//! [ADR-0002]: https://github.com/MattJackson/muri/blob/main/docs/design/adr/0002-single-platform-module-per-os-behind-one-trait.md

use std::path::PathBuf;

use crate::error::Result;
use crate::geometry::{Edge, LogicalRect};
use crate::keynav::NavKey;
use crate::menu::{Icon, Menu, MenuId};
use crate::theme::MenuOptions;
use crate::Tray;

/// Where the host's native menu-font face data comes from, so the render layer
/// can register it in its `fontdb` database and resolve
/// [`FontFamily::System`](crate::FontFamily::System) to the real OS UI face — no
/// `cfg(target_os)` and no `NSFont`/`HFONT`/CoreText handle crossing the seam.
#[derive(Debug, Clone)]
pub enum SystemFontSource {
    /// Raw font-file bytes to register directly into the database.
    Data(Vec<u8>),
    /// A filesystem path to a font file to register.
    Path(PathBuf),
    /// A family name already present in the host font database (Windows'
    /// `Segoe UI`, a Linux desktop's configured UI family) — pinned by name with
    /// no byte loading.
    Family(String),
}

/// The host's native menu font: where to get its face (or family name) and the
/// point size the OS draws menus at. Acquired OS-specifically by the per-OS
/// backend ([`Platform::system_menu_font`]) and consumed OS-agnostically by the
/// renderer.
#[derive(Debug, Clone)]
pub struct SystemFont {
    /// Where the face data (or family name) comes from.
    pub source: SystemFontSource,
    /// The OS menu point size (logical points), e.g. ~13.5 on macOS, 9.0 on
    /// Windows. A non-positive value means "use the theme default".
    pub point_size: f32,
}

impl SystemFont {
    /// Apply the OS menu point size to a theme's row + header fonts (preserving
    /// their weights). A non-positive size is ignored (keeps the theme default).
    pub fn apply_size_to(&self, theme: &mut crate::theme::Theme) {
        if self.point_size > 0.0 {
            theme.row_font.size = self.point_size;
            theme.header_font.size = self.point_size;
        }
    }
}

/// The host's live menu colors, each an optional straight-alpha `(r, g, b, a)`.
/// Acquired OS-specifically ([`Platform::system_palette`]) and applied over a
/// theme's per-OS base; an unset field leaves the base value in place. Kept as
/// tuples (not [`Color`](crate::style::Color)) so the seam carries no OS handle.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemPalette {
    /// Primary menu text color (e.g. macOS `labelColor`, Win32 `COLOR_MENUTEXT`).
    pub label: Option<(u8, u8, u8, u8)>,
    /// De-emphasized text (macOS `secondaryLabelColor`, Win32 `COLOR_GRAYTEXT`).
    pub secondary_label: Option<(u8, u8, u8, u8)>,
    /// Separator hairline color (macOS `separatorColor`, Win32 `COLOR_3DSHADOW`).
    pub separator: Option<(u8, u8, u8, u8)>,
    /// Opaque menu background, for backends without a vibrancy backdrop. Left
    /// `None` on macOS/Windows so the translucent theme background is preserved.
    pub background: Option<(u8, u8, u8, u8)>,
}

impl SystemPalette {
    /// Apply the set fields of this palette onto `theme` (each unset field leaves
    /// the theme's base value). Shared by every backend's `theme()`.
    pub fn apply_to(&self, theme: &mut crate::theme::Theme) {
        use crate::style::Color;
        if let Some((r, g, b, a)) = self.label {
            theme.label = Color::Rgba(r, g, b, a);
        }
        if let Some((r, g, b, a)) = self.secondary_label {
            theme.secondary_label = Color::Rgba(r, g, b, a);
        }
        if let Some((r, g, b, a)) = self.separator {
            theme.separator = Color::Rgba(r, g, b, a);
        }
        if let Some((r, g, b, a)) = self.background {
            theme.background = Color::Rgba(r, g, b, a);
        }
    }
}

/// The host's current light/dark appearance, used by the engine to resolve the
/// [`Theme`](crate::Theme) without ever touching an OS appearance API directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    /// A light system appearance.
    Light,
    /// A dark system appearance.
    Dark,
}

impl Appearance {
    /// Whether this appearance is dark.
    pub fn is_dark(self) -> bool {
        matches!(self, Appearance::Dark)
    }

    /// Map a raw dark/light boolean (as reported by a platform query) onto the
    /// unified enum.
    pub fn from_is_dark(is_dark: bool) -> Self {
        if is_dark {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    }
}

/// A platform UI event surfaced by the per-OS event loop, normalized so the
/// engine never sees a toolkit- or OS-specific event type.
///
/// The per-OS backend translates native events (an AppKit status-item click, a
/// Win32 tray callback, …) into this vocabulary on the UI thread. Keyboard input
/// is carried as an already-translated [`NavKey`] so the engine's pure
/// [`keynav`](crate::keynav) state machine can consume it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlatformEvent {
    /// The tray icon was activated (clicked); the engine toggles the popup.
    TrayActivated,
    /// A navigation key was pressed while a muri popup held focus.
    Key(NavKey),
    /// Every muri window lost focus; the engine dismisses the popup stack.
    Dismissed,
}

/// The one seam between muri's OS-agnostic engine and the host windowing /
/// tray / accessibility APIs.
///
/// Exactly one implementation is compiled per target (see [`PlatformImpl`]).
/// All OS-specific behavior — installing the tray icon, reporting the anchor
/// rectangle, creating the non-activating popup + flyout windows, presenting the
/// rendered pixmap, pumping the native event loop into [`PlatformEvent`]s,
/// tracking focus/dismiss, driving the accessibility adapter (behind the `a11y`
/// feature), and querying the environment (appearance, work area, scale) — lives
/// behind these methods, inside the single per-OS module that implements them.
///
/// Only unified, OS-neutral types cross this boundary: [`LogicalRect`],
/// [`Appearance`], [`PlatformEvent`], the [`Icon`] / [`Menu`] data
/// model, and [`Result`]. No `NSWindow` / `HWND` / winit handle is ever exposed.
pub trait Platform {
    /// Install the tray/status icon with an optional tooltip / accessible name.
    ///
    /// Where a platform cannot host a tray-anchored icon (Linux), this reports
    /// the failure rather than papering over it.
    fn install_tray(&mut self, icon: &Icon, tooltip: Option<&str>) -> Result<()>;

    /// The tray icon's current on-screen rectangle in logical coordinates, used
    /// to anchor the popup. Returns
    /// [`Unsupported::TrayAnchor`](crate::Unsupported::TrayAnchor) where the
    /// platform never exposes it (Linux).
    fn tray_anchor_rect(&self) -> Result<LogicalRect>;

    /// Whether this platform can anchor a styled popup to the tray icon at all;
    /// lets a consumer branch to a fallback (native menu / pointer-anchored
    /// [`ContextMenu`](crate::ContextMenu)) up front.
    fn supports_tray_anchor(&self) -> bool;

    /// The current mouse-cursor position in **screen logical coordinates**
    /// (top-left origin, the same space [`ContextMenu::open_at`](crate::ContextMenu::open_at) takes), or `None`
    /// when the platform cannot report it (Wayland forbids a client from querying
    /// the global pointer). Used by the compat context-menu facade to place a
    /// `None`-position menu at the cursor like muda does. Default `None`; each
    /// backend overrides it (#1).
    fn cursor_position(&self) -> Option<crate::geometry::LogicalPoint> {
        None
    }

    /// The host's current light/dark appearance for the anchor's own monitor.
    fn appearance(&self) -> Appearance;

    /// The host's native menu font (the real OS UI face and its menu point size),
    /// acquired OS-specifically so the render layer can pin
    /// [`FontFamily::System`](crate::FontFamily::System) to it — SF Pro on macOS,
    /// Segoe UI on Windows, the configured UI family on Linux (#10).
    ///
    /// Must never panic: a platform that cannot resolve its menu font
    /// (headless/CI, a sandbox, an acquisition failure) returns `None`, and the
    /// renderer falls back to its installed-font discovery. The default returns
    /// `None` for any platform that does not override it.
    fn system_menu_font(&self) -> Option<SystemFont> {
        None
    }

    /// The host's live menu palette — label / secondary-label / separator (and,
    /// where meaningful, background) colors read from the OS at draw time, so the
    /// custom surface tracks the exact native menu colors, not just hardcoded
    /// defaults. Each field is optional: a platform fills what it can read and
    /// leaves the rest `None`, and the theme keeps its per-OS base value for any
    /// unread field. The default returns an empty palette (no overrides).
    ///
    /// Must never panic: an acquisition failure returns `None` per field.
    fn system_palette(&self) -> SystemPalette {
        SystemPalette::default()
    }

    /// The logical work area (screen minus reserved bars) of the monitor the
    /// tray anchor lives on, used to clamp/flip popup placement so it never
    /// spills off-screen.
    fn work_area(&self) -> LogicalRect;

    /// Install the tray and run the platform UI event loop to completion,
    /// opening the styled popup on activation and dispatching row clicks to the
    /// tray's handler. Consumes both the platform and the [`Tray`]; returns when
    /// the loop exits.
    ///
    /// On Linux this returns
    /// [`Unsupported::TrayAnchor`](crate::Unsupported::TrayAnchor) without
    /// entering a loop (tray-anchored popups are not offered there); macOS and
    /// Windows both run a native popup event loop.
    fn run_tray(self, tray: Tray) -> Result<()>
    where
        Self: Sized;

    /// Install the tray and begin driving it **without blocking the caller**,
    /// returning once the icon is (best-effort) live. The non-blocking
    /// counterpart to [`run_tray`](Platform::run_tray), for hosts that own their
    /// own event loop or want only a passive handle — notably the `tray-icon`
    /// compatibility facade, whose `TrayIconBuilder::build()` must return
    /// immediately.
    ///
    /// - **Windows / Linux** run the tray's native UI pump on a dedicated
    ///   background thread; a [`TrayHandle`](crate::TrayHandle) obtained before
    ///   the call drives it cross-thread (the existing command/waker path).
    /// - **macOS** is best-effort: AppKit's `NSStatusItem` must live on the main
    ///   thread, so this must be called from the main thread and relies on the
    ///   host's existing `NSApplication` run loop to service the item — it does
    ///   **not** call `app.run()` and does not change the app's activation policy.
    ///
    /// The default reports the capability as unavailable; every per-OS backend
    /// overrides it.
    fn spawn_tray(self, tray: Tray) -> Result<()>
    where
        Self: Sized,
    {
        let _ = tray;
        Err(crate::error::Error::Platform(
            "spawning a non-blocking tray is not implemented on this platform".into(),
        ))
    }

    /// Open a pointer/rect-anchored styled popup session — the shared backend for
    /// [`ContextMenu::open_at`](crate::ContextMenu::open_at) and
    /// [`Popup::anchored_to`](crate::Popup::anchored_to) — and block until it
    /// dismisses. `anchor` is the rectangle the popup grows from relative to
    /// `edge` (a zero-size rect at a point for `open_at`); `on_click` is dispatched
    /// with the activated row's id and is borrowed only for the duration of the
    /// blocking call.
    ///
    /// The default reports the capability as not-yet-implemented; all three
    /// per-OS backends override it (macOS: spec 20 §3 `NSPanel`; Windows: the
    /// layered-window popup; Linux: X11 override-redirect).
    fn open_popup_session(
        &mut self,
        menu: Menu,
        options: MenuOptions,
        on_click: &(dyn Fn(&MenuId) + '_),
        anchor: LogicalRect,
        edge: Edge,
    ) -> Result<()> {
        let _ = (menu, options, on_click, anchor, edge);
        Err(crate::error::Error::Platform(
            "styled popup sessions are not implemented on this platform yet".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(target_os = "macos")]
pub use mac::MacPlatform as PlatformImpl;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::WindowsPlatform as PlatformImpl;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
pub use linux::LinuxPlatform as PlatformImpl;

/// Construct the [`Platform`] implementation for the target OS. This is the one
/// place the engine crosses into OS-specific code; the returned [`PlatformImpl`]
/// is the single per-OS type selected by the `cfg` above.
pub fn current() -> PlatformImpl {
    PlatformImpl::new()
}

/// Launch the `muri-tray` background UI thread that drives a tray to completion
/// via `run`, returning once the thread is spawned. Shared by the Windows and
/// Linux [`Platform::spawn_tray`] implementations (macOS installs on the main
/// thread instead, so it does not use this). An install/run failure is reported
/// on stderr, since the caller — the compat facade's `build()` — has already
/// returned by the time this thread installs.
#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
pub(crate) fn spawn_tray_thread(tray: Tray, run: fn(Tray) -> Result<()>) -> Result<()> {
    std::thread::Builder::new()
        .name("muri-tray".to_owned())
        .spawn(move || {
            if let Err(e) = run(tray) {
                eprintln!("muri: tray thread exited with error: {e}");
            }
        })
        .map(|_| ())
        .map_err(|e| {
            crate::error::Error::Platform(format!("failed to spawn muri tray thread: {e}"))
        })
}
