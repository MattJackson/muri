//! Pure keyboard-navigation state machine for the menu and its N-level flyout stack.
//!
//! muri owns keyboard navigation directly (independent of any accessibility
//! backend): arrow keys move the highlight, Right/Left open and close the
//! flyout, Enter/Space activate, Esc pops one level, and a typed character
//! jumps to the next matching row (type-ahead). Kept free of any window, event
//! loop, or platform call so it's exhaustively unit-testable; the live backend
//! ([`crate::platform`]) only translates key events into [`NavKey`]s and
//! applies the returned [`NavAction`].
//!
//! ## Focus model
//!
//! Navigation state is a [`MenuFocus`]: the selected **top-level** item index
//! and a **stack** of open flyout levels ([`FlyoutFocus`]), one frame per open
//! submenu (decision #8, N-level nested submenus — spec 40 §5), mirroring the
//! live backend's flyout **window** stack so keyboard and mouse selection stay
//! consistent ([`crate::flyout::next_flyout`]). `Right`/`Activate` on a submenu
//! row pushes a deeper level; `Left`/`Escape` pops one, closing the whole stack
//! once popped past the top.
//!
//! Selection wraps, and skips non-focusable items (everything
//! [`Item::is_interactive`] marks `false`).

use crate::menu::{Item, Menu, MenuId};

/// A key press, normalized from the platform key event into the small set that
/// menu navigation cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    /// Move the highlight to the next focusable row (wraps).
    Down,
    /// Move the highlight to the previous focusable row (wraps).
    Up,
    /// Open the flyout of the selected submenu row (no-op elsewhere).
    Right,
    /// Close the open flyout, returning focus to its parent (no-op at top level).
    Left,
    /// Activate the selected row (Enter/Space): fire its click, or open a
    /// submenu.
    Activate,
    /// Pop one level: close the flyout if open, else dismiss the whole menu.
    Escape,
    /// Jump to the first focusable row.
    Home,
    /// Jump to the last focusable row.
    End,
    /// Type-ahead: jump to the next focusable row whose name starts with this
    /// character (case-insensitive).
    Char(char),
}

/// One open flyout level on the [`MenuFocus`] stack: the submenu `parent` row it
/// opened from (an index into the menu **one level up** — [`Menu::items`] at the
/// top level, or the enclosing submenu's items when nested) and the selected
/// `child` within this level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlyoutFocus {
    /// Item index (into the parent level's [`Menu::items`]) of the open submenu.
    pub parent: usize,
    /// Selected child item index within this submenu level, if any.
    pub child: Option<usize>,
}

/// The current keyboard-navigation selection over the menu and its open flyout
/// **stack** (decision #8): the selected top-level item plus zero or more open
/// flyout levels, deepest last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuFocus {
    /// Selected top-level item index (into [`Menu::items`]), if any.
    pub top: Option<usize>,
    /// The stack of open flyout levels, one frame per open submenu (empty when no
    /// flyout is open); the last frame is the deepest, currently-navigated level.
    pub flyout: Vec<FlyoutFocus>,
}

/// What the backend should do after a key press. The pure [`handle_key`] mutates
/// the [`MenuFocus`] in place and returns one of these for the live loop to act on
/// (open/close a flyout window, dispatch a click, dismiss, or just repaint).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavAction {
    /// Nothing changed; no repaint required.
    None,
    /// Selection moved within the current level; repaint the affected window.
    Redraw,
    /// Push a flyout level: open (and focus into) the submenu at this row index of
    /// the currently deepest open level (the top level when the stack is empty).
    OpenFlyout(usize),
    /// Pop one flyout level, returning focus to the submenu row it opened from.
    CloseFlyout,
    /// Activate the row with this id: the backend dispatches it and closes the
    /// whole stack.
    Activate(MenuId),
    /// Dismiss the entire menu stack.
    CloseAll,
}

/// Advance the navigation state for one key press, returning the action the
/// backend should perform. Pure: no I/O, no platform calls.
pub fn handle_key(menu: &Menu, focus: &mut MenuFocus, key: NavKey) -> NavAction {
    if focus.flyout.is_empty() {
        handle_in_top(menu, focus, key)
    } else {
        handle_in_flyout(menu, focus, key)
    }
}

/// Resolve the [`Menu`] whose items the deepest open flyout level selects among,
/// by descending `submenu_child` through every frame's `parent`. Returns `None`
/// (with the length of the still-valid prefix) if some parent up the stack is no
/// longer a submenu — e.g. the menu was swapped underneath an open flyout.
fn deepest_level<'a>(
    menu: &'a Menu,
    stack: &[FlyoutFocus],
) -> std::result::Result<&'a Menu, usize> {
    let mut level = menu;
    for (depth, frame) in stack.iter().enumerate() {
        match submenu_child(level, frame.parent) {
            Some(child) => level = child,
            None => return Err(depth),
        }
    }
    Ok(level)
}

fn handle_in_top(menu: &Menu, focus: &mut MenuFocus, key: NavKey) -> NavAction {
    match key {
        NavKey::Down => {
            move_selection(menu, &mut focus.top, Dir::Next);
            NavAction::Redraw
        }
        NavKey::Up => {
            move_selection(menu, &mut focus.top, Dir::Prev);
            NavAction::Redraw
        }
        NavKey::Home => {
            focus.top = first_focusable(menu);
            NavAction::Redraw
        }
        NavKey::End => {
            focus.top = last_focusable(menu);
            NavAction::Redraw
        }
        NavKey::Char(c) => match type_ahead(menu, focus.top, c) {
            Some(i) => {
                focus.top = Some(i);
                NavAction::Redraw
            }
            None => NavAction::None,
        },
        NavKey::Right => match focus.top {
            Some(i) if is_submenu_at(menu, i) => open_flyout(menu, focus, i),
            _ => NavAction::None,
        },
        NavKey::Activate => match focus.top {
            Some(i) if is_submenu_at(menu, i) => open_flyout(menu, focus, i),
            Some(i) => match interactive_id_at(menu, i) {
                Some(id) => NavAction::Activate(id),
                None => NavAction::None,
            },
            None => NavAction::None,
        },
        NavKey::Left => NavAction::None,
        NavKey::Escape => NavAction::CloseAll,
    }
}

fn handle_in_flyout(menu: &Menu, focus: &mut MenuFocus, key: NavKey) -> NavAction {
    // The deepest open level's menu (the one whose items the top stack frame's
    // `child` selects among).
    let level = match deepest_level(menu, &focus.flyout) {
        Ok(level) => level,
        Err(valid) => {
            // A parent up the stack is no longer a submenu (menu swapped
            // underneath us): drop the stale sub-stack to the deepest live level,
            // restoring top-level focus if that empties the stack.
            let top_parent = focus.flyout.first().map(|f| f.parent);
            focus.flyout.truncate(valid);
            if focus.flyout.is_empty() {
                focus.top = top_parent;
            }
            return NavAction::CloseFlyout;
        }
    };
    let fly = *focus.flyout.last().expect("stack is non-empty here");
    let mut sel = fly.child;
    match key {
        NavKey::Down => {
            move_selection(level, &mut sel, Dir::Next);
            commit_deepest(focus, sel);
            NavAction::Redraw
        }
        NavKey::Up => {
            move_selection(level, &mut sel, Dir::Prev);
            commit_deepest(focus, sel);
            NavAction::Redraw
        }
        NavKey::Home => {
            commit_deepest(focus, first_focusable(level));
            NavAction::Redraw
        }
        NavKey::End => {
            commit_deepest(focus, last_focusable(level));
            NavAction::Redraw
        }
        NavKey::Char(c) => match type_ahead(level, sel, c) {
            Some(i) => {
                commit_deepest(focus, Some(i));
                NavAction::Redraw
            }
            None => NavAction::None,
        },
        NavKey::Left | NavKey::Escape => {
            let popped = focus.flyout.pop();
            if focus.flyout.is_empty() {
                focus.top = popped.map(|f| f.parent);
            }
            NavAction::CloseFlyout
        }
        // Right / Activate on a submenu child descends into a nested flyout.
        NavKey::Right => match sel {
            Some(ci) if is_submenu_at(level, ci) => descend(level, focus, ci),
            _ => NavAction::None,
        },
        NavKey::Activate => match sel {
            Some(ci) if is_submenu_at(level, ci) => descend(level, focus, ci),
            Some(ci) => match interactive_id_at(level, ci) {
                Some(id) => NavAction::Activate(id),
                None => NavAction::None,
            },
            None => NavAction::None,
        },
    }
}

/// Push a flyout level for `parent` (an index into the top-level `menu`), its
/// first focusable child selected. Used from the top level.
fn open_flyout(menu: &Menu, focus: &mut MenuFocus, parent: usize) -> NavAction {
    let child = submenu_child(menu, parent).and_then(first_focusable);
    focus.top = Some(parent);
    focus.flyout = vec![FlyoutFocus { parent, child }];
    NavAction::OpenFlyout(parent)
}

/// Push a deeper flyout level for submenu row `child_index` of `level` (the
/// currently deepest open level), its first focusable grandchild selected.
fn descend(level: &Menu, focus: &mut MenuFocus, child_index: usize) -> NavAction {
    let grand = submenu_child(level, child_index).and_then(first_focusable);
    focus.flyout.push(FlyoutFocus {
        parent: child_index,
        child: grand,
    });
    NavAction::OpenFlyout(child_index)
}

/// Set the deepest open level's selected child.
fn commit_deepest(focus: &mut MenuFocus, child: Option<usize>) {
    if let Some(frame) = focus.flyout.last_mut() {
        frame.child = child;
    }
}

#[derive(Clone, Copy)]
enum Dir {
    Next,
    Prev,
}

/// Move `sel` to the next/previous focusable item in `menu`, wrapping. A `None`
/// or now-unfocusable selection jumps to the first (Next) or last (Prev).
fn move_selection(menu: &Menu, sel: &mut Option<usize>, dir: Dir) {
    let f = focusable_indices(menu);
    if f.is_empty() {
        *sel = None;
        return;
    }
    let new = match sel.and_then(|cur| f.iter().position(|&x| x == cur)) {
        Some(pos) => match dir {
            Dir::Next => f[(pos + 1) % f.len()],
            Dir::Prev => f[(pos + f.len() - 1) % f.len()],
        },
        None => match dir {
            Dir::Next => f[0],
            Dir::Prev => f[f.len() - 1],
        },
    };
    *sel = Some(new);
}

fn first_focusable(menu: &Menu) -> Option<usize> {
    focusable_indices(menu).first().copied()
}

fn last_focusable(menu: &Menu) -> Option<usize> {
    focusable_indices(menu).last().copied()
}

/// Type-ahead: the next focusable item after `current` (wrapping) whose accessible
/// name starts with `c`, case-insensitively.
fn type_ahead(menu: &Menu, current: Option<usize>, c: char) -> Option<usize> {
    let needle = c.to_ascii_lowercase();
    let f = focusable_indices(menu);
    if f.is_empty() {
        return None;
    }
    let start = current
        .and_then(|cur| f.iter().position(|&x| x == cur))
        .map(|p| p + 1)
        .unwrap_or(0);
    for k in 0..f.len() {
        let idx = f[(start + k) % f.len()];
        if item_name(menu, idx)
            .chars()
            .next()
            .map(|ch| ch.to_ascii_lowercase() == needle)
            .unwrap_or(false)
        {
            return Some(idx);
        }
    }
    None
}

fn focusable_indices(menu: &Menu) -> Vec<usize> {
    menu.items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.is_interactive())
        .map(|(i, _)| i)
        .collect()
}

fn submenu_child(menu: &Menu, i: usize) -> Option<&Menu> {
    match menu.items.get(i) {
        Some(Item::Submenu { menu, .. }) => Some(menu),
        _ => None,
    }
}

fn is_submenu_at(menu: &Menu, i: usize) -> bool {
    matches!(menu.items.get(i), Some(Item::Submenu { .. }))
}

fn interactive_id_at(menu: &Menu, i: usize) -> Option<MenuId> {
    match menu.items.get(i) {
        Some(Item::Row(r)) if r.enabled && !r.id.is_none() => Some(r.id.clone()),
        _ => None,
    }
}

fn item_name(menu: &Menu, i: usize) -> String {
    match menu.items.get(i) {
        Some(Item::Row(r)) | Some(Item::SectionHeader(r)) => r.accessible_name(),
        Some(Item::Submenu { label, .. }) => label.accessible_name(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Row;

    // Header (0), account rows (1,2), separator (3), submenu (4), quit (5).
    // Focusable: 1, 2, 4, 5.
    fn menu() -> Menu {
        Menu::new()
            .section_header(Row::label_only("Claude"))
            .row(Row::new("a").label("Apple"))
            .row(Row::new("b").label("Banana"))
            .separator()
            .submenu(
                Row::new("settings").label("Settings"),
                Menu::new()
                    .row(Row::new("s1").label("One"))
                    .row(Row::new("s2").label("Two")),
            )
            .row(Row::new("quit").label("Quit"))
    }

    fn press(m: &Menu, f: &mut MenuFocus, k: NavKey) -> NavAction {
        handle_key(m, f, k)
    }

    #[test]
    fn down_from_nothing_selects_first_focusable_skipping_header() {
        let m = menu();
        let mut f = MenuFocus::default();
        assert_eq!(press(&m, &mut f, NavKey::Down), NavAction::Redraw);
        assert_eq!(f.top, Some(1)); // skips the section header at 0
    }

    #[test]
    fn up_from_nothing_selects_last_focusable() {
        let m = menu();
        let mut f = MenuFocus::default();
        press(&m, &mut f, NavKey::Up);
        assert_eq!(f.top, Some(5));
    }

    #[test]
    fn down_skips_separator_and_wraps() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: Vec::new(),
        };
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(2));
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(4)); // skips separator at 3
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(5));
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(1)); // wraps past the end
    }

    #[test]
    fn up_wraps_to_last() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: Vec::new(),
        };
        press(&m, &mut f, NavKey::Up);
        assert_eq!(f.top, Some(5));
    }

    #[test]
    fn home_and_end_jump_to_bounds() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: Vec::new(),
        };
        press(&m, &mut f, NavKey::Home);
        assert_eq!(f.top, Some(1));
        press(&m, &mut f, NavKey::End);
        assert_eq!(f.top, Some(5));
    }

    #[test]
    fn right_on_submenu_opens_flyout_and_focuses_first_child() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: Vec::new(),
        };
        assert_eq!(press(&m, &mut f, NavKey::Right), NavAction::OpenFlyout(4));
        assert_eq!(
            f.flyout,
            vec![FlyoutFocus {
                parent: 4,
                child: Some(0),
            }]
        );
    }

    #[test]
    fn right_on_leaf_does_nothing() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: Vec::new(),
        };
        assert_eq!(press(&m, &mut f, NavKey::Right), NavAction::None);
        assert!(f.flyout.is_empty());
    }

    #[test]
    fn activate_leaf_returns_its_id() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(5),
            flyout: Vec::new(),
        };
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::Activate(MenuId::from("quit"))
        );
    }

    #[test]
    fn activate_submenu_opens_it() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: Vec::new(),
        };
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::OpenFlyout(4)
        );
    }

    #[test]
    fn navigation_within_flyout_then_left_closes() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: vec![FlyoutFocus {
                parent: 4,
                child: Some(0),
            }],
        };
        // Down moves within the child menu.
        assert_eq!(press(&m, &mut f, NavKey::Down), NavAction::Redraw);
        assert_eq!(f.flyout.last().unwrap().child, Some(1));
        // Down wraps back to the first child.
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.flyout.last().unwrap().child, Some(0));
        // Left closes the flyout, focus back on the parent.
        assert_eq!(press(&m, &mut f, NavKey::Left), NavAction::CloseFlyout);
        assert!(f.flyout.is_empty());
        assert_eq!(f.top, Some(4));
    }

    #[test]
    fn activate_in_flyout_returns_child_id() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: vec![FlyoutFocus {
                parent: 4,
                child: Some(1),
            }],
        };
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::Activate(MenuId::from("s2"))
        );
    }

    #[test]
    fn escape_pops_one_level_then_closes_all() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: vec![FlyoutFocus {
                parent: 4,
                child: Some(0),
            }],
        };
        assert_eq!(press(&m, &mut f, NavKey::Escape), NavAction::CloseFlyout);
        assert!(f.flyout.is_empty());
        assert_eq!(f.top, Some(4));
        assert_eq!(press(&m, &mut f, NavKey::Escape), NavAction::CloseAll);
    }

    #[test]
    fn type_ahead_jumps_and_wraps() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: Vec::new(),
        };
        // From "Apple" (1), 'q' jumps to "Quit" (5).
        assert_eq!(press(&m, &mut f, NavKey::Char('q')), NavAction::Redraw);
        assert_eq!(f.top, Some(5));
        // From "Quit", 'b' wraps forward to "Banana" (2).
        press(&m, &mut f, NavKey::Char('b'));
        assert_eq!(f.top, Some(2));
        // A character no row starts with does nothing.
        assert_eq!(press(&m, &mut f, NavKey::Char('z')), NavAction::None);
        assert_eq!(f.top, Some(2));
    }

    #[test]
    fn empty_menu_has_no_selection() {
        let m = Menu::new();
        let mut f = MenuFocus::default();
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, None);
    }

    // -- N-level nested submenus (decision #8) -------------------------------

    // Top-level submenu "More" (index 0) whose child "Deep" (index 0) is itself
    // a submenu of "Leaf1"/"Leaf2"; sibling leaf "End" at index 1.
    fn nested_menu() -> Menu {
        Menu::new()
            .submenu(
                Row::new("more").label("More"),
                Menu::new()
                    .submenu(
                        Row::new("deep").label("Deep"),
                        Menu::new()
                            .row(Row::new("leaf1").label("Leaf1"))
                            .row(Row::new("leaf2").label("Leaf2")),
                    )
                    .row(Row::new("end").label("End")),
            )
            .row(Row::new("quit").label("Quit"))
    }

    #[test]
    fn right_within_flyout_descends_into_nested_submenu() {
        let m = nested_menu();
        let mut f = MenuFocus {
            top: Some(0),
            flyout: Vec::new(),
        };
        // Open the first-level flyout (child 0 = "Deep", a submenu).
        assert_eq!(press(&m, &mut f, NavKey::Right), NavAction::OpenFlyout(0));
        assert_eq!(
            f.flyout,
            vec![FlyoutFocus {
                parent: 0,
                child: Some(0)
            }]
        );
        // Right on "Deep" descends into the nested flyout, focusing "Leaf1".
        assert_eq!(press(&m, &mut f, NavKey::Right), NavAction::OpenFlyout(0));
        assert_eq!(
            f.flyout,
            vec![
                FlyoutFocus {
                    parent: 0,
                    child: Some(0)
                },
                FlyoutFocus {
                    parent: 0,
                    child: Some(0)
                },
            ]
        );
        // Down moves within the deepest level only.
        assert_eq!(press(&m, &mut f, NavKey::Down), NavAction::Redraw);
        assert_eq!(f.flyout.last().unwrap().child, Some(1)); // "Leaf2"
        assert_eq!(f.flyout[0].child, Some(0)); // level-1 selection unchanged
    }

    #[test]
    fn activate_deep_leaf_returns_its_id() {
        let m = nested_menu();
        let mut f = MenuFocus {
            top: Some(0),
            flyout: vec![
                FlyoutFocus {
                    parent: 0,
                    child: Some(0),
                },
                FlyoutFocus {
                    parent: 0,
                    child: Some(1),
                },
            ],
        };
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::Activate(MenuId::from("leaf2"))
        );
    }

    #[test]
    fn left_and_escape_pop_one_level_at_a_time() {
        let m = nested_menu();
        let mut f = MenuFocus {
            top: Some(0),
            flyout: vec![
                FlyoutFocus {
                    parent: 0,
                    child: Some(0),
                },
                FlyoutFocus {
                    parent: 0,
                    child: Some(0),
                },
            ],
        };
        // Left pops only the deepest level.
        assert_eq!(press(&m, &mut f, NavKey::Left), NavAction::CloseFlyout);
        assert_eq!(
            f.flyout,
            vec![FlyoutFocus {
                parent: 0,
                child: Some(0)
            }]
        );
        assert_eq!(f.top, Some(0));
        // Escape pops the last level; focus returns to the top-level parent.
        assert_eq!(press(&m, &mut f, NavKey::Escape), NavAction::CloseFlyout);
        assert!(f.flyout.is_empty());
        assert_eq!(f.top, Some(0));
        // Escape at the top closes everything.
        assert_eq!(press(&m, &mut f, NavKey::Escape), NavAction::CloseAll);
    }

    #[test]
    fn activate_on_nested_submenu_child_descends() {
        let m = nested_menu();
        let mut f = MenuFocus {
            top: Some(0),
            flyout: vec![FlyoutFocus {
                parent: 0,
                child: Some(0),
            }],
        };
        // "Deep" (child 0 of level 1) is a submenu: Activate descends.
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::OpenFlyout(0)
        );
        assert_eq!(f.flyout.len(), 2);
        assert_eq!(f.flyout.last().unwrap().child, Some(0));
    }

    #[test]
    fn stale_nested_stack_collapses_to_live_parent() {
        // A two-deep stack whose deepest parent no longer resolves (menu shape
        // changed): the stale level is dropped and focus returns to the live one.
        let m = menu(); // single-level submenu at index 4; no nesting
        let mut f = MenuFocus {
            top: Some(4),
            flyout: vec![
                FlyoutFocus {
                    parent: 4,
                    child: Some(0),
                },
                FlyoutFocus {
                    parent: 0,
                    child: Some(0),
                }, // stale: child 0 isn't a submenu
            ],
        };
        assert_eq!(press(&m, &mut f, NavKey::Down), NavAction::CloseFlyout);
        assert_eq!(
            f.flyout,
            vec![FlyoutFocus {
                parent: 4,
                child: Some(0)
            }]
        );
    }
}
