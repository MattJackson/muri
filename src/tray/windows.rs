//! Windows tray anchoring via `Shell_NotifyIconGetRect`.
//!
//! `Shell_NotifyIconGetRect` returns the screen rectangle of a notification icon
//! given a `NOTIFYICONIDENTIFIER`; muri feeds that rect to a `WS_EX_NOACTIVATE |
//! WS_EX_TOOLWINDOW` layered window, choosing the edge so the popup opens toward
//! screen center. Outside-click dismiss is backed by a `WH_MOUSE_LL` hook plus
//! `WM_ACTIVATEAPP`.
//!
//! The real implementation uses the `windows`/`windows-sys` crates; the bodies
//! are `todo!()` until the Windows backend phase lands.

use crate::error::Result;
use crate::geometry::LogicalRect;
use crate::tray::TrayAnchor;

/// The Windows notification-area anchor.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct WindowsAnchor;

impl WindowsAnchor {
    /// Create the (not-yet-installed) Windows anchor.
    pub fn new() -> Self {
        WindowsAnchor
    }
}

impl TrayAnchor for WindowsAnchor {
    fn install(&mut self, _tooltip: Option<&str>) -> Result<()> {
        todo!("Shell_NotifyIcon registration — see the muri design doc roadmap")
    }

    fn anchor_rect(&self) -> Result<LogicalRect> {
        todo!("Shell_NotifyIconGetRect")
    }

    fn supports_tray_anchor(&self) -> bool {
        true
    }
}
