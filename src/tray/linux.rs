//! Linux tray anchoring — the honest carve-out.
//!
//! The modern Linux tray is StatusNotifierItem / AppIndicator over D-Bus: the
//! *host* owns and draws the icon in its own process and renders the menu from a
//! `com.canonical.dbusmenu` description. The application is never told the
//! icon's on-screen rectangle and never receives the click coordinate, and
//! Wayland additionally forbids a client from positioning its own toplevel. A
//! tray-*anchored* styled popup is therefore architecturally impossible here, so
//! [`anchor_rect`](TrayAnchor::anchor_rect) returns
//! [`Unsupported::TrayAnchor`](crate::Unsupported::TrayAnchor).
//!
//! Consumers use the native-menu fallback (render the same `Menu` through a
//! `dbusmenu` tree) or a pointer-anchored [`ContextMenu`](crate::ContextMenu).

use crate::error::{Error, Result, Unsupported};
use crate::geometry::LogicalRect;
use crate::tray::TrayAnchor;

/// The Linux SNI/AppIndicator anchor stub. It can register an icon (future
/// work), but cannot report an anchor rectangle.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct LinuxAnchor;

impl LinuxAnchor {
    /// Create the Linux anchor.
    pub fn new() -> Self {
        LinuxAnchor
    }
}

impl TrayAnchor for LinuxAnchor {
    fn install(&mut self, _tooltip: Option<&str>) -> Result<()> {
        todo!("SNI/AppIndicator registration + dbusmenu fallback — see the muri design doc roadmap")
    }

    fn anchor_rect(&self) -> Result<LogicalRect> {
        Err(Error::Unsupported(Unsupported::TrayAnchor))
    }

    fn supports_tray_anchor(&self) -> bool {
        false
    }
}
