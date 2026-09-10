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

use crate::error::Result;
use crate::geometry::{Edge, LogicalRect};
use crate::keynav::NavKey;
use crate::menu::{Icon, Menu, MenuId};
use crate::theme::MenuOptions;
use crate::Tray;

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

    /// The host's current light/dark appearance for the anchor's own monitor.
    fn appearance(&self) -> Appearance;

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
