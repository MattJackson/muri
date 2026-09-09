//! macOS tray anchoring via `NSStatusItem`.
//!
//! The status item's `button` is both the click target and the anchor rect;
//! AppKit also reports which display the menu bar is on. The shared raster
//! surface is hosted in a non-activating `NSPanel` positioned relative to that
//! rect, and transient dismiss uses a global `NSEvent` monitor. AppKit is the
//! *anchor*, not the renderer.
//!
//! The real implementation uses `objc2` / `objc2-app-kit`; the bodies are
//! `todo!()` until the macOS backend phase lands.

use crate::error::Result;
use crate::geometry::LogicalRect;
use crate::tray::TrayAnchor;

/// The macOS `NSStatusItem`-based anchor.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct MacosAnchor;

impl MacosAnchor {
    /// Create the (not-yet-installed) macOS anchor.
    pub fn new() -> Self {
        MacosAnchor
    }
}

impl TrayAnchor for MacosAnchor {
    fn install(&mut self, _tooltip: Option<&str>) -> Result<()> {
        todo!("NSStatusItem creation via objc2-app-kit — see the muri design doc roadmap")
    }

    fn anchor_rect(&self) -> Result<LogicalRect> {
        todo!("NSStatusItem.button screen frame")
    }

    fn supports_tray_anchor(&self) -> bool {
        true
    }
}
