//! Pure, platform-independent flyout-submenu geometry and hover-stack logic.
//!
//! A [`Item::Submenu`](crate::Item::Submenu) row opens a second panel beside it.
//! Two decisions are pure and testable, and live here free of any window or OS
//! dependency:
//!
//! - **Placement** ([`place_flyout`]): where the flyout panel sits relative to
//!   its parent popup — to the parent's right by default, flipped to the left
//!   when it would spill off the monitor, and clamped vertically into the work
//!   area.
//! - **Hover-stack transitions** ([`next_flyout`]): which parent row's flyout is
//!   open as the pointer moves between parent rows, the open flyout, and empty
//!   space. Hovering a submenu parent opens (or switches to) its flyout; hovering
//!   a different, non-submenu row closes it; moving into the flyout — or across
//!   the gap between the two panels — keeps it open.
//!
//! The live backends ([`crate::tray`]) drive their second popup window from these
//! two functions; the snapshot test composits a parent + child using
//! [`place_flyout`] directly.

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};

/// Which side of the parent popup a flyout opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlyoutSide {
    /// The flyout opened to the right of its parent (the default).
    Right,
    /// The flyout was flipped to the left of its parent to stay on-screen.
    Left,
}

/// The resolved placement of a flyout panel: its top-left screen origin and the
/// side of the parent it opened on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlyoutPlacement {
    /// Top-left corner of the flyout panel, in the same (screen) space as the
    /// `parent` and `work_area` passed to [`place_flyout`].
    pub origin: LogicalPoint,
    /// The side the flyout opened on.
    pub side: FlyoutSide,
}

/// Place a flyout panel beside its parent row.
///
/// - `parent` is the parent popup's rectangle in screen coordinates.
/// - `row` is the hovered submenu row's rectangle, relative to the parent
///   popup's top-left (i.e. window/content coordinates, as produced by
///   [`render_menu`](crate::render::paint::render_menu)).
/// - `flyout` is the child panel's size.
/// - `work_area` is the target monitor's usable rectangle in screen coordinates.
///
/// The flyout is placed flush against the parent's right edge and vertically
/// aligned so its top lines up with the hovered row. If it would overflow the
/// right edge of `work_area` it flips to the parent's left; if that still
/// overflows the left edge it is clamped inside the work area. Finally it is
/// clamped vertically so it never spills off the top or bottom.
pub fn place_flyout(
    parent: LogicalRect,
    row: LogicalRect,
    flyout: LogicalSize,
    work_area: LogicalRect,
) -> FlyoutPlacement {
    // Preferred side: to the right, flush against the parent's edge.
    let right_x = parent.max_x();
    let fits_right = right_x + flyout.width <= work_area.max_x();

    let (side, mut x) = if fits_right {
        (FlyoutSide::Right, right_x)
    } else {
        // Flip to the left of the parent.
        (FlyoutSide::Left, parent.origin.x - flyout.width)
    };

    // Clamp horizontally into the work area (covers a flip-left that still
    // overflows the left edge, or an unusually wide panel).
    let max_x = (work_area.max_x() - flyout.width).max(work_area.origin.x);
    x = x.clamp(work_area.origin.x, max_x);

    // Align the flyout's top with the hovered row's top (in screen space), then
    // clamp vertically so it stays fully on-screen.
    let mut y = parent.origin.y + row.origin.y;
    let max_y = (work_area.max_y() - flyout.height).max(work_area.origin.y);
    y = y.clamp(work_area.origin.y, max_y);

    FlyoutPlacement {
        origin: LogicalPoint::new(x, y),
        side,
    }
}

/// Where the pointer currently is, relative to an open menu with (at most) one
/// flyout, for the hover-stack decision in [`next_flyout`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverTarget {
    /// Over a top-level row that is a submenu parent (with its item index).
    ParentRow(usize),
    /// Over a top-level row that is *not* a submenu parent.
    OtherRow,
    /// Over the currently open flyout panel (or the gap the pointer crosses to
    /// reach it).
    Flyout,
    /// Not over the menu or its flyout at all.
    Outside,
}

/// Decide which parent row's flyout should be open after the pointer moves to
/// `target`, given the currently open parent index (`current`).
///
/// Rules (matching native menu behavior):
/// - hovering a submenu parent opens its flyout, switching away from any other;
/// - hovering a non-submenu row closes the open flyout;
/// - hovering the flyout itself (or crossing the gap to it) keeps it open;
/// - drifting outside the menu leaves the current flyout as-is (dismissal of the
///   whole stack is a click-outside / Esc concern, handled by the caller).
pub fn next_flyout(current: Option<usize>, target: HoverTarget) -> Option<usize> {
    match target {
        HoverTarget::ParentRow(i) => Some(i),
        HoverTarget::OtherRow => None,
        HoverTarget::Flyout | HoverTarget::Outside => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> LogicalRect {
        LogicalRect::new(LogicalPoint::new(x, y), LogicalSize::new(w, h))
    }

    // A generous monitor: 0,0 .. 1440,900.
    fn screen() -> LogicalRect {
        rect(0.0, 0.0, 1440.0, 900.0)
    }

    #[test]
    fn opens_to_the_right_flush_and_row_aligned() {
        let parent = rect(100.0, 100.0, 200.0, 300.0);
        let row = rect(0.0, 40.0, 200.0, 22.0); // second-ish row
        let p = place_flyout(parent, row, LogicalSize::new(180.0, 150.0), screen());
        assert_eq!(p.side, FlyoutSide::Right);
        // Flush against the parent's right edge.
        assert_eq!(p.origin.x, 300.0);
        // Top aligned with the row (parent.y + row.y).
        assert_eq!(p.origin.y, 140.0);
    }

    #[test]
    fn flips_left_when_no_room_on_the_right() {
        // Parent hugs the right edge; a right-opening flyout would overflow.
        let parent = rect(1300.0, 100.0, 130.0, 300.0);
        let row = rect(0.0, 0.0, 130.0, 22.0);
        let p = place_flyout(parent, row, LogicalSize::new(180.0, 150.0), screen());
        assert_eq!(p.side, FlyoutSide::Left);
        // Placed to the left of the parent: 1300 - 180 = 1120.
        assert_eq!(p.origin.x, 1120.0);
    }

    #[test]
    fn flip_left_is_clamped_into_the_work_area() {
        // Narrow screen, wide flyout: even flipped left it can't fully fit, so it
        // clamps to the work-area's left edge rather than going negative.
        let work = rect(0.0, 0.0, 300.0, 900.0);
        let parent = rect(250.0, 0.0, 50.0, 300.0);
        let row = rect(0.0, 0.0, 50.0, 22.0);
        let p = place_flyout(parent, row, LogicalSize::new(280.0, 150.0), work);
        assert_eq!(p.side, FlyoutSide::Left);
        assert_eq!(p.origin.x, 0.0); // clamped, not -30
    }

    #[test]
    fn clamps_to_the_bottom_of_the_work_area() {
        let parent = rect(100.0, 800.0, 200.0, 80.0);
        let row = rect(0.0, 40.0, 200.0, 22.0); // screen y would be 840
        let p = place_flyout(parent, row, LogicalSize::new(180.0, 200.0), screen());
        // 840 + 200 = 1040 > 900, so clamp to 900 - 200 = 700.
        assert_eq!(p.origin.y, 700.0);
    }

    #[test]
    fn clamps_to_the_top_of_the_work_area() {
        // Work area starts at y = 25 (a menu bar); a row above it clamps down.
        let work = rect(0.0, 25.0, 1440.0, 875.0);
        let parent = rect(100.0, 10.0, 200.0, 300.0);
        let row = rect(0.0, 0.0, 200.0, 22.0); // screen y = 10
        let p = place_flyout(parent, row, LogicalSize::new(180.0, 150.0), work);
        assert_eq!(p.origin.y, 25.0);
    }

    #[test]
    fn hover_parent_opens_and_switches() {
        assert_eq!(next_flyout(None, HoverTarget::ParentRow(2)), Some(2));
        // Switch directly from one parent's flyout to another's.
        assert_eq!(next_flyout(Some(2), HoverTarget::ParentRow(5)), Some(5));
    }

    #[test]
    fn hover_other_row_closes_the_flyout() {
        assert_eq!(next_flyout(Some(2), HoverTarget::OtherRow), None);
        assert_eq!(next_flyout(None, HoverTarget::OtherRow), None);
    }

    #[test]
    fn moving_into_flyout_keeps_it_open() {
        assert_eq!(next_flyout(Some(2), HoverTarget::Flyout), Some(2));
    }

    #[test]
    fn drifting_outside_keeps_current() {
        assert_eq!(next_flyout(Some(2), HoverTarget::Outside), Some(2));
        assert_eq!(next_flyout(None, HoverTarget::Outside), None);
    }
}
