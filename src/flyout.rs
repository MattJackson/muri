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
//! The live backends ([`crate::platform`]) drive their second popup window from these
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

/// Where the pointer currently is, relative to an open menu with a **stack** of
/// flyout panels (decision #8), for the hover-stack decision in [`next_flyout`].
///
/// `panel` is the depth of the panel the pointer is over: `0` is the top-level
/// popup, `1` its first flyout, `2` the flyout of that flyout, and so on. Hovering
/// a row in a shallower panel closes every flyout deeper than it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverTarget {
    /// Over a submenu parent row (its item index) in the panel at depth `panel`.
    ParentRow {
        /// Depth of the panel the row lives in (`0` = top-level popup).
        panel: usize,
        /// Item index of the hovered submenu row within that panel's menu.
        index: usize,
    },
    /// Over a row that is *not* a submenu parent in the panel at depth `panel`.
    OtherRow {
        /// Depth of the panel the row lives in (`0` = top-level popup).
        panel: usize,
    },
    /// Over an open flyout panel body away from any row, or the gap the pointer
    /// crosses between adjacent panels — keeps the whole stack open.
    Flyout,
    /// Not over the menu or any of its flyouts at all.
    Outside,
}

/// Decide the open-flyout **stack** after the pointer moves to `target`, given the
/// currently open stack (`current`): `current[k]` is the submenu-row index, within
/// panel `k`'s menu, whose flyout is open as panel `k + 1`. The returned stack has
/// the same shape.
///
/// Rules (matching native menu behavior, generalized across the stack):
/// - hovering a submenu parent in panel `p` opens/switches its flyout and closes
///   everything deeper (`current[..p]` then the newly hovered index) — but
///   re-hovering the *already-open* parent keeps its deeper levels intact so
///   grandchildren don't collapse;
/// - hovering a non-submenu row in panel `p` closes panel `p`'s flyout and deeper;
/// - hovering a flyout body or crossing an inter-panel gap keeps the stack;
/// - drifting outside leaves the stack as-is (dismissal is a click-outside / Esc
///   concern handled by the caller).
pub fn next_flyout(current: &[usize], target: HoverTarget) -> Vec<usize> {
    match target {
        HoverTarget::ParentRow { panel, index } => {
            if current.get(panel) == Some(&index) {
                // Already open here: keep the whole stack (don't collapse deeper
                // levels the pointer just travelled back up through).
                current.to_vec()
            } else {
                let keep = panel.min(current.len());
                let mut next = current[..keep].to_vec();
                next.push(index);
                next
            }
        }
        HoverTarget::OtherRow { panel } => {
            let keep = panel.min(current.len());
            current[..keep].to_vec()
        }
        HoverTarget::Flyout | HoverTarget::Outside => current.to_vec(),
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
        assert_eq!(
            next_flyout(&[], HoverTarget::ParentRow { panel: 0, index: 2 }),
            vec![2]
        );
        // Switch directly from one top-level parent's flyout to another's.
        assert_eq!(
            next_flyout(&[2], HoverTarget::ParentRow { panel: 0, index: 5 }),
            vec![5]
        );
    }

    #[test]
    fn hover_other_row_closes_the_flyout() {
        assert_eq!(
            next_flyout(&[2], HoverTarget::OtherRow { panel: 0 }),
            Vec::<usize>::new()
        );
        assert_eq!(
            next_flyout(&[], HoverTarget::OtherRow { panel: 0 }),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn moving_into_flyout_keeps_it_open() {
        assert_eq!(next_flyout(&[2], HoverTarget::Flyout), vec![2]);
    }

    #[test]
    fn drifting_outside_keeps_current() {
        assert_eq!(next_flyout(&[2], HoverTarget::Outside), vec![2]);
        assert_eq!(next_flyout(&[], HoverTarget::Outside), Vec::<usize>::new());
    }

    #[test]
    fn hover_submenu_row_in_flyout_opens_deeper_level() {
        // Panel 1 (the first flyout) is open via top-level parent 2; hovering a
        // submenu row (index 4) inside it opens a second flyout level.
        assert_eq!(
            next_flyout(&[2], HoverTarget::ParentRow { panel: 1, index: 4 }),
            vec![2, 4]
        );
    }

    #[test]
    fn rehovering_open_parent_keeps_deeper_levels() {
        // A 3-deep stack; moving back onto the already-open panel-1 parent (index
        // 4) must not collapse the grandchild flyout.
        assert_eq!(
            next_flyout(&[2, 4, 1], HoverTarget::ParentRow { panel: 1, index: 4 }),
            vec![2, 4, 1]
        );
    }

    #[test]
    fn hover_shallower_panel_closes_deeper_flyouts() {
        // With flyouts open at panels 1 and 2, hovering a non-submenu row back in
        // the top-level popup closes both.
        assert_eq!(
            next_flyout(&[2, 4], HoverTarget::OtherRow { panel: 0 }),
            Vec::<usize>::new()
        );
        // Switching to a different submenu row in panel 1 truncates level 2.
        assert_eq!(
            next_flyout(&[2, 4], HoverTarget::ParentRow { panel: 1, index: 6 }),
            vec![2, 6]
        );
    }

    #[test]
    fn place_flyout_chains_relative_to_the_previous_panel() {
        // Level-2 placement uses the level-1 panel as its `parent` rect: the same
        // right-flush, row-aligned rule applies at every depth.
        let popup = rect(100.0, 100.0, 200.0, 300.0);
        let row1 = rect(0.0, 40.0, 200.0, 22.0);
        let l1 = place_flyout(popup, row1, LogicalSize::new(180.0, 150.0), screen());
        assert_eq!(l1.side, FlyoutSide::Right);
        let l1_rect = LogicalRect::new(l1.origin, LogicalSize::new(180.0, 150.0));
        let row2 = rect(0.0, 20.0, 180.0, 22.0);
        let l2 = place_flyout(l1_rect, row2, LogicalSize::new(160.0, 120.0), screen());
        assert_eq!(l2.side, FlyoutSide::Right);
        assert_eq!(l2.origin.x, l1_rect.max_x());
        assert_eq!(l2.origin.y, l1.origin.y + 20.0);
    }

    #[test]
    fn keyboard_and_mouse_drive_the_same_open_flyout_state() {
        // Cross-module: prove keynav::handle_key and flyout::next_flyout — the
        // two independent drivers of "which parent's flyout is open" — converge
        // on the same parent index for the same submenu row.
        use crate::keynav::{handle_key, MenuFocus, NavKey};
        use crate::menu::{Menu, Row};

        let menu = Menu::new()
            .row(Row::new("a").label("Apple"))
            .submenu(
                Row::new("settings").label("Settings"),
                Menu::new().row(Row::new("s1").label("One")),
            )
            .row(Row::new("quit").label("Quit"));
        let submenu_index = 1;

        // Keyboard: select the submenu row, then Right opens its flyout.
        let mut focus = MenuFocus {
            top: Some(submenu_index),
            flyout: Vec::new(),
        };
        handle_key(&menu, &mut focus, NavKey::Right);
        let keyboard_open: Vec<usize> = focus.flyout.iter().map(|fly| fly.parent).collect();

        // Mouse: hover the same submenu row.
        let mouse_open = next_flyout(
            &[],
            HoverTarget::ParentRow {
                panel: 0,
                index: submenu_index,
            },
        );

        assert_eq!(keyboard_open, vec![submenu_index]);
        assert_eq!(mouse_open, vec![submenu_index]);
        assert_eq!(keyboard_open, mouse_open);
    }
}
