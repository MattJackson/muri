//! Pure popup-placement math shared by every OS backend.
//!
//! Anchoring is the one genuinely per-OS problem, but the *arithmetic* — given an
//! icon rectangle, a popup size, the screen work area, and which edge to grow
//! from, where does the popup's top-left corner go? — is identical everywhere.
//! Each backend obtains the anchor rect natively (`NSStatusItem.button` on macOS,
//! `Shell_NotifyIconGetRect` on Windows) and the work area from its screen APIs,
//! then calls [`place_popup`]; keeping the geometry here makes it exhaustively
//! unit-testable without a window server.
//!
//! Placement rules:
//! - **[`Edge::Bottom`]** opens the popup below the anchor; if it would spill off
//!   the bottom of the work area it flips to open *above* instead.
//! - **[`Edge::Top`]** opens above, flipping below on spill.
//! - **[`Edge::Right`]/[`Edge::Left`]** open to that side of the anchor, flipping
//!   to the opposite side on spill (used for taskbar-edge tray layouts).
//! - The cross axis is aligned to the anchor's leading edge (top for
//!   left/right), then the whole rect is clamped into the work area so it is
//!   always fully on-screen.

use crate::geometry::{Edge, LogicalPoint, LogicalRect, LogicalSize};

/// Resolve the top-left corner of a popup of size `popup` anchored to `anchor`,
/// growing from `edge`, separated by `gap` logical pixels, clamped so it stays
/// entirely inside `work_area`.
pub fn place_popup(
    anchor: LogicalRect,
    popup: LogicalSize,
    work_area: LogicalRect,
    edge: Edge,
    gap: f32,
) -> LogicalPoint {
    let (mut x, mut y) = match edge {
        Edge::Bottom => {
            let below = anchor.max_y() + gap;
            let above = anchor.origin.y - gap - popup.height;
            let y = if below + popup.height <= work_area.max_y() || above < work_area.origin.y {
                below
            } else {
                above
            };
            (anchor.origin.x, y)
        }
        Edge::Top => {
            let above = anchor.origin.y - gap - popup.height;
            let below = anchor.max_y() + gap;
            let y = if above >= work_area.origin.y || below + popup.height > work_area.max_y() {
                above
            } else {
                below
            };
            (anchor.origin.x, y)
        }
        Edge::Right => {
            let right = anchor.max_x() + gap;
            let left = anchor.origin.x - gap - popup.width;
            let x = if right + popup.width <= work_area.max_x() || left < work_area.origin.x {
                right
            } else {
                left
            };
            (x, anchor.origin.y)
        }
        Edge::Left => {
            let left = anchor.origin.x - gap - popup.width;
            let right = anchor.max_x() + gap;
            let x = if left >= work_area.origin.x || right + popup.width > work_area.max_x() {
                left
            } else {
                right
            };
            (x, anchor.origin.y)
        }
    };

    // Clamp fully into the work area (the lower bound wins if the popup is wider
    // or taller than the work area itself).
    let max_x = (work_area.max_x() - popup.width).max(work_area.origin.x);
    let max_y = (work_area.max_y() - popup.height).max(work_area.origin.y);
    x = x.clamp(work_area.origin.x, max_x);
    y = y.clamp(work_area.origin.y, max_y);
    LogicalPoint::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> LogicalRect {
        LogicalRect::new(LogicalPoint::new(0.0, 0.0), LogicalSize::new(1440.0, 900.0))
    }

    #[test]
    fn bottom_edge_opens_below_and_left_aligned() {
        // Menu-bar icon near the top-left.
        let anchor = LogicalRect::new(LogicalPoint::new(100.0, 0.0), LogicalSize::new(24.0, 24.0));
        let p = place_popup(
            anchor,
            LogicalSize::new(200.0, 300.0),
            work(),
            Edge::Bottom,
            2.0,
        );
        assert_eq!(p.x, 100.0);
        assert_eq!(p.y, 26.0); // 24 + 2 gap
    }

    #[test]
    fn bottom_edge_flips_above_when_it_would_spill_off_the_bottom() {
        // A taskbar-style icon near the bottom of the work area.
        let anchor = LogicalRect::new(
            LogicalPoint::new(100.0, 850.0),
            LogicalSize::new(24.0, 24.0),
        );
        let p = place_popup(
            anchor,
            LogicalSize::new(200.0, 300.0),
            work(),
            Edge::Bottom,
            2.0,
        );
        // 850 + 24 + 300 = 1174 > 900, so flip above: 850 - 2 - 300 = 548.
        assert_eq!(p.y, 548.0);
    }

    #[test]
    fn x_is_clamped_so_a_right_edge_icon_stays_on_screen() {
        let anchor = LogicalRect::new(LogicalPoint::new(1430.0, 0.0), LogicalSize::new(24.0, 24.0));
        let p = place_popup(
            anchor,
            LogicalSize::new(200.0, 300.0),
            work(),
            Edge::Bottom,
            2.0,
        );
        assert_eq!(p.x, 1240.0); // 1440 - 200
    }

    #[test]
    fn top_edge_opens_above_the_anchor() {
        let anchor = LogicalRect::new(
            LogicalPoint::new(100.0, 860.0),
            LogicalSize::new(24.0, 24.0),
        );
        let p = place_popup(
            anchor,
            LogicalSize::new(200.0, 300.0),
            work(),
            Edge::Top,
            2.0,
        );
        assert_eq!(p.y, 558.0); // 860 - 2 - 300
    }

    #[test]
    fn right_edge_opens_to_the_right_then_flips_left_on_spill() {
        let near = LogicalRect::new(LogicalPoint::new(10.0, 100.0), LogicalSize::new(24.0, 24.0));
        let p = place_popup(
            near,
            LogicalSize::new(200.0, 300.0),
            work(),
            Edge::Right,
            2.0,
        );
        assert_eq!(p.x, 36.0); // 10 + 24 + 2

        let far = LogicalRect::new(
            LogicalPoint::new(1400.0, 100.0),
            LogicalSize::new(24.0, 24.0),
        );
        let p = place_popup(far, LogicalSize::new(200.0, 300.0), work(), Edge::Left, 2.0);
        assert_eq!(p.x, 1198.0); // 1400 - 2 - 200
    }

    #[test]
    fn an_oversized_popup_is_pinned_to_the_work_area_origin() {
        let anchor = LogicalRect::new(LogicalPoint::new(700.0, 0.0), LogicalSize::new(24.0, 24.0));
        let p = place_popup(
            anchor,
            LogicalSize::new(2000.0, 2000.0),
            work(),
            Edge::Bottom,
            2.0,
        );
        assert_eq!(p.x, 0.0);
        assert_eq!(p.y, 0.0);
    }

    // Multi-monitor cases: a secondary monitor whose work area does NOT start at
    // (0, 0) — here one placed up and to the left of the primary, at
    // (-1920, -1080), 1920x1080. If `place_popup` ever implicitly assumed a
    // primary-origin work area, these would place the popup relative to (0, 0)
    // instead of the given `work_area`'s own bounds.
    fn secondary_monitor_up_left() -> LogicalRect {
        LogicalRect::new(
            LogicalPoint::new(-1920.0, -1080.0),
            LogicalSize::new(1920.0, 1080.0),
        )
    }

    #[test]
    fn bottom_edge_places_correctly_on_an_offset_monitor() {
        let work = secondary_monitor_up_left();
        // Anchor near the top-left of the secondary monitor.
        let anchor = LogicalRect::new(
            LogicalPoint::new(-1820.0, -1080.0),
            LogicalSize::new(24.0, 24.0),
        );
        let p = place_popup(
            anchor,
            LogicalSize::new(200.0, 300.0),
            work,
            Edge::Bottom,
            2.0,
        );
        // Left-aligned to the anchor, opens below it — both relative to the
        // secondary monitor's own coordinates, not the primary's.
        assert_eq!(p.x, -1820.0);
        assert_eq!(p.y, -1054.0); // -1080 + 24 + 2 gap
    }

    #[test]
    fn bottom_edge_flips_above_when_spilling_off_an_offset_monitor() {
        let work = secondary_monitor_up_left();
        // Anchor near the bottom of the secondary monitor (whose bottom edge is
        // at y = -1080 + 1080 = 0).
        let anchor = LogicalRect::new(
            LogicalPoint::new(-1820.0, -230.0),
            LogicalSize::new(24.0, 24.0),
        );
        let p = place_popup(
            anchor,
            LogicalSize::new(200.0, 300.0),
            work,
            Edge::Bottom,
            2.0,
        );
        // Below would be -230 + 24 + 2 + 300 = 96, past the monitor's max_y (0),
        // so it flips above: -230 - 2 - 300 = -532.
        assert_eq!(p.y, -532.0);
    }

    #[test]
    fn x_is_clamped_against_an_offset_monitors_far_edge() {
        let work = secondary_monitor_up_left();
        // Anchor near the right edge of the secondary monitor (max_x = 0).
        let anchor = LogicalRect::new(
            LogicalPoint::new(-20.0, -1080.0),
            LogicalSize::new(24.0, 24.0),
        );
        let p = place_popup(
            anchor,
            LogicalSize::new(200.0, 300.0),
            work,
            Edge::Bottom,
            2.0,
        );
        // Clamped to the secondary monitor's own far edge (0 - 200 = -200), not
        // to the primary monitor's edge (which would clamp to 1440 - 200).
        assert_eq!(p.x, -200.0);
    }
}
