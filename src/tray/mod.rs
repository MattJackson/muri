//! Tray anchoring: the one genuinely per-OS problem. Anchoring decides *where on
//! screen* the popup appears relative to the tray icon, and it is the same
//! problem regardless of who draws the pixels — which is exactly why a single
//! cross-platform toolkit wouldn't have saved the work.
//!
//! Each platform implements [`TrayAnchor`]: install the status-bar/notification
//! icon, report the icon's on-screen rectangle so the shared renderer can place
//! the popup, and say honestly whether tray-anchoring is possible at all.
//!
//! - **macOS** — [`macos::MacosAnchor`]: `NSStatusItem.button` rect, positioned
//!   against a non-activating `NSPanel`.
//! - **Windows** — `WindowsAnchor`: `Shell_NotifyIconGetRect` +
//!   `WS_EX_NOACTIVATE` layered window.
//! - **Linux** — `LinuxAnchor`: *no* tray anchor (SNI/AppIndicator gives no
//!   geometry and no click coordinate, and Wayland forbids client positioning),
//!   so [`TrayAnchor::anchor_rect`] returns
//!   [`Unsupported::TrayAnchor`](crate::Unsupported::TrayAnchor).

use crate::error::Result;
use crate::geometry::LogicalRect;

/// A per-OS tray-icon anchor. The shared renderer queries [`anchor_rect`] to
/// place the popup; [`supports_tray_anchor`] lets a consumer branch to a
/// fallback (a native menu or a pointer-anchored context menu) up front.
///
/// [`anchor_rect`]: TrayAnchor::anchor_rect
/// [`supports_tray_anchor`]: TrayAnchor::supports_tray_anchor
pub trait TrayAnchor {
    /// Install the tray/status icon with an optional tooltip / accessible name.
    fn install(&mut self, tooltip: Option<&str>) -> Result<()>;

    /// The icon's current on-screen rectangle in logical coordinates, used to
    /// anchor the popup. Returns [`Unsupported::TrayAnchor`] where the platform
    /// never exposes it (Linux).
    ///
    /// [`Unsupported::TrayAnchor`]: crate::Unsupported::TrayAnchor
    fn anchor_rect(&self) -> Result<LogicalRect>;

    /// Whether this platform can anchor a styled popup to the tray icon at all.
    fn supports_tray_anchor(&self) -> bool;
}

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::MacosAnchor as PlatformAnchor;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::WindowsAnchor as PlatformAnchor;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
pub use linux::LinuxAnchor as PlatformAnchor;
