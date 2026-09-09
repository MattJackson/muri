//! Pure accessibility-tree model: the parallel structure a screen reader walks.
//!
//! A `tiny-skia` pixmap is an opaque rectangle to an assistive technology — there
//! are no `NSView`/`HWND`/widget objects to enumerate. So muri publishes a
//! **parallel accessibility tree** that mirrors the menu's logical structure, and
//! keeps it in sync with what is drawn and focused. The design calls this the
//! hardest part of the project; the declarative [`Menu`] tree *is* that
//! accessibility tree in disguise, and this module performs the mapping purely and
//! testably.
//!
//! The tree here is backend-neutral. On macOS/Windows the live backend feeds it to
//! the platform AT through **AccessKit** (`accesskit_macos` → NSAccessibility,
//! `accesskit_windows` → UIA); the [`accesskit`](crate::a11y::accesskit) adapter,
//! enabled by the `a11y` feature, converts an [`AxTree`] straight into an
//! AccessKit `TreeUpdate`. The mapping mirrors the design:
//!
//! - the popup → an [`AxRole::Menu`] container (`AXMenu` / UIA `Menu`);
//! - an interactive row → an [`AxRole::MenuItem`], or [`AxRole::MenuItemCheckbox`]
//!   when it carries a checked state, with its accessible **name** from the
//!   concatenated segment text, **enabled** from `Row.enabled`, and set-position
//!   info among its focusable siblings;
//! - a section header → a non-focusable [`AxRole::GroupLabel`];
//! - a submenu → a menu item with `has_popup` and an `expanded` state plus a child
//!   [`AxRole::Menu`]'s worth of items.
//!
//! Keyboard navigation ([`crate::keynav`]) owns the focus; [`focused_id`] maps a
//! [`MenuFocus`](crate::keynav::MenuFocus) onto the tree node the backend should
//! announce, and [`announcement`] renders the human-readable string a screen
//! reader speaks for a node.

use crate::keynav::MenuFocus;
use crate::menu::{Item, Menu};

/// A stable identifier for an [`AxNode`] within one built tree. Assigned
/// depth-first from `0` (the root), so it maps directly onto an AccessKit
/// `NodeId(u64)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AxId(pub u64);

/// The accessible role of a node, mapped to the platform AT role by the backend
/// (via AccessKit): NSAccessibility on macOS, UIA on Windows, AT-SPI on Linux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxRole {
    /// The popup container (`AXMenu` / UIA `Menu` / AT-SPI `menu`).
    Menu,
    /// An activatable row (`AXMenuItem` / `menuitem`).
    MenuItem,
    /// A row carrying a checked/unchecked state (a checkable menu item).
    MenuItemCheckbox,
    /// A non-interactive section heading.
    GroupLabel,
    /// A divider (exposed for structure; never focusable).
    Separator,
}

/// One node in the accessibility tree.
#[derive(Clone, Debug, PartialEq)]
pub struct AxNode {
    /// Stable id within this tree.
    pub id: AxId,
    /// The accessible role.
    pub role: AxRole,
    /// The accessible name a screen reader announces.
    pub name: String,
    /// Whether the node is enabled (dimmed rows and headers are not).
    pub enabled: bool,
    /// `Some(checked)` for a checkable item; `None` if it has no checked state.
    pub checked: Option<bool>,
    /// Whether activating this node opens a submenu (`AXMenuItem` `haspopup`).
    pub has_popup: bool,
    /// For a submenu item, whether its flyout is currently expanded.
    pub expanded: Option<bool>,
    /// 1-based position among focusable siblings (for "3 of 7" announcements).
    pub pos_in_set: Option<usize>,
    /// Count of focusable siblings at this level.
    pub set_size: Option<usize>,
    /// Index into the owning menu level's [`Menu::items`], for backend lookup.
    pub item_index: Option<usize>,
    /// Child nodes (a submenu's items; empty otherwise).
    pub children: Vec<AxNode>,
}

impl AxNode {
    /// Find a descendant (or self) by id.
    pub fn find(&self, id: AxId) -> Option<&AxNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
}

/// A built accessibility tree rooted at an [`AxRole::Menu`] container.
#[derive(Clone, Debug, PartialEq)]
pub struct AxTree {
    /// The root menu-container node.
    pub root: AxNode,
}

impl AxTree {
    /// Find a node anywhere in the tree by id.
    pub fn find(&self, id: AxId) -> Option<&AxNode> {
        self.root.find(id)
    }

    /// Total number of nodes (including the root container).
    pub fn node_count(&self) -> usize {
        fn count(n: &AxNode) -> usize {
            1 + n.children.iter().map(count).sum::<usize>()
        }
        count(&self.root)
    }
}

/// Build the accessibility tree for `menu`. Submenus recurse into nested
/// [`AxRole::Menu`]-worth of child items; ids are assigned depth-first from `0`.
pub fn build_tree(menu: &Menu) -> AxTree {
    let mut next: u64 = 0;
    let root_id = alloc(&mut next);
    let children = build_level(menu, &mut next);
    AxTree {
        root: AxNode {
            id: root_id,
            role: AxRole::Menu,
            name: String::new(),
            enabled: true,
            checked: None,
            has_popup: false,
            expanded: None,
            pos_in_set: None,
            set_size: None,
            item_index: None,
            children,
        },
    }
}

fn alloc(next: &mut u64) -> AxId {
    let id = AxId(*next);
    *next += 1;
    id
}

fn build_level(menu: &Menu, next: &mut u64) -> Vec<AxNode> {
    let set_size = menu.interactive_count();
    let mut pos = 0usize;
    let mut out = Vec::with_capacity(menu.items.len());
    for (i, item) in menu.items.iter().enumerate() {
        let id = alloc(next);
        let interactive = item.is_interactive();
        let (pos_in_set, set) = if interactive {
            pos += 1;
            (Some(pos), Some(set_size))
        } else {
            (None, None)
        };
        let node = match item {
            Item::Separator => AxNode {
                id,
                role: AxRole::Separator,
                name: String::new(),
                enabled: false,
                checked: None,
                has_popup: false,
                expanded: None,
                pos_in_set: None,
                set_size: None,
                item_index: Some(i),
                children: Vec::new(),
            },
            Item::SectionHeader(row) => AxNode {
                id,
                role: AxRole::GroupLabel,
                name: row.accessible_name(),
                enabled: false,
                checked: None,
                has_popup: false,
                expanded: None,
                pos_in_set: None,
                set_size: None,
                item_index: Some(i),
                children: Vec::new(),
            },
            Item::Row(row) => AxNode {
                id,
                role: if row.checked.is_some() {
                    AxRole::MenuItemCheckbox
                } else {
                    AxRole::MenuItem
                },
                name: row.accessible_name(),
                enabled: row.enabled,
                checked: row.checked,
                has_popup: false,
                expanded: None,
                pos_in_set,
                set_size: set,
                item_index: Some(i),
                children: Vec::new(),
            },
            Item::Submenu { label, menu: child } => AxNode {
                id,
                role: if label.checked.is_some() {
                    AxRole::MenuItemCheckbox
                } else {
                    AxRole::MenuItem
                },
                name: label.accessible_name(),
                enabled: label.enabled,
                checked: label.checked,
                has_popup: true,
                expanded: Some(false),
                pos_in_set,
                set_size: set,
                item_index: Some(i),
                children: build_level(child, next),
            },
        };
        out.push(node);
    }
    out
}

/// The id of the node a screen reader should focus for the given keyboard-nav
/// selection: the selected child inside an open flyout, else the open flyout's
/// parent, else the selected top-level row, else `None`.
pub fn focused_id(tree: &AxTree, focus: &MenuFocus) -> Option<AxId> {
    if let Some(fly) = focus.flyout {
        let parent = tree
            .root
            .children
            .iter()
            .find(|n| n.item_index == Some(fly.parent))?;
        return match fly.child {
            Some(ci) => parent
                .children
                .iter()
                .find(|n| n.item_index == Some(ci))
                .map(|n| n.id)
                .or(Some(parent.id)),
            None => Some(parent.id),
        };
    }
    let top = focus.top?;
    tree.root
        .children
        .iter()
        .find(|n| n.item_index == Some(top))
        .map(|n| n.id)
}

/// Mark the submenu node whose `item_index` matches `parent` as expanded (or not)
/// in place. The backend calls this when it opens/closes a flyout so the tree's
/// `expanded` state — and any resulting AT notification — stays truthful.
pub fn set_expanded(tree: &mut AxTree, parent: usize, expanded: bool) {
    if let Some(node) = tree
        .root
        .children
        .iter_mut()
        .find(|n| n.item_index == Some(parent) && n.has_popup)
    {
        node.expanded = Some(expanded);
    }
}

/// The human-readable string a screen reader speaks for a node: its name, then
/// its checked/submenu/dimmed state, its role, and its set position. Assembled
/// deterministically so it can be asserted in tests (the live AT ultimately
/// composes its own phrasing from the same structured fields).
pub fn announcement(node: &AxNode) -> String {
    let mut parts = Vec::new();
    if !node.name.is_empty() {
        parts.push(node.name.clone());
    }
    match node.checked {
        Some(true) => parts.push("checked".to_string()),
        Some(false) => parts.push("unchecked".to_string()),
        None => {}
    }
    if node.has_popup {
        parts.push("submenu".to_string());
        match node.expanded {
            Some(true) => parts.push("expanded".to_string()),
            Some(false) => parts.push("collapsed".to_string()),
            None => {}
        }
    }
    if !node.enabled && node.role != AxRole::GroupLabel && node.role != AxRole::Separator {
        parts.push("dimmed".to_string());
    }
    match node.role {
        AxRole::MenuItem | AxRole::MenuItemCheckbox => parts.push("menu item".to_string()),
        AxRole::GroupLabel => parts.push("heading".to_string()),
        AxRole::Menu => parts.push("menu".to_string()),
        AxRole::Separator => parts.push("separator".to_string()),
    }
    if let (Some(p), Some(s)) = (node.pos_in_set, node.set_size) {
        parts.push(format!("{p} of {s}"));
    }
    parts.join(", ")
}

/// AccessKit adapter: convert a muri [`AxTree`] into an AccessKit `TreeUpdate`.
///
/// Enabled by the `a11y` feature. The live macOS/Windows backends feed the
/// resulting update to an `accesskit_macos` / `accesskit_windows` adapter, which
/// bridges to NSAccessibility / UIA respectively. Kept behind a feature flag so
/// the default build carries no AccessKit dependency.
#[cfg(feature = "a11y")]
pub mod accesskit {
    use super::{AxNode, AxRole, AxTree};
    use accesskit::{Node, NodeId, Role, Toggled, Tree, TreeUpdate};

    fn role_of(node: &AxNode) -> Role {
        match node.role {
            AxRole::Menu => Role::Menu,
            AxRole::MenuItem => Role::MenuItem,
            AxRole::MenuItemCheckbox => Role::MenuItemCheckBox,
            AxRole::GroupLabel => Role::Label,
            AxRole::Separator => Role::Splitter,
        }
    }

    fn push(node: &AxNode, out: &mut Vec<(NodeId, Node)>) {
        let mut n = Node::new(role_of(node));
        if !node.name.is_empty() {
            n.set_label(node.name.clone());
        }
        if !node.enabled {
            n.set_disabled();
        }
        match node.checked {
            Some(true) => n.set_toggled(Toggled::True),
            Some(false) => n.set_toggled(Toggled::False),
            None => {}
        }
        if node.has_popup {
            n.set_expanded(node.expanded.unwrap_or(false));
        }
        if let (Some(p), Some(s)) = (node.pos_in_set, node.set_size) {
            n.set_position_in_set(p);
            n.set_size_of_set(s);
        }
        n.set_children(
            node.children
                .iter()
                .map(|c| NodeId(c.id.0))
                .collect::<Vec<_>>(),
        );
        out.push((NodeId(node.id.0), n));
        for c in &node.children {
            push(c, out);
        }
    }

    /// Build an AccessKit `TreeUpdate` for the whole menu, focused on `focus`
    /// (falling back to the root container when nothing is selected).
    pub fn tree_update(tree: &AxTree, focus: Option<super::AxId>) -> TreeUpdate {
        let mut nodes = Vec::with_capacity(tree.node_count());
        push(&tree.root, &mut nodes);
        let root = NodeId(tree.root.id.0);
        TreeUpdate {
            nodes,
            tree: Some(Tree::new(root)),
            focus: focus.map(|f| NodeId(f.0)).unwrap_or(root),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keynav::FlyoutFocus;
    use crate::menu::{Icon, Row};

    // Header(0), active account(1, checked), plain account(2), separator(3),
    // disabled row(4), submenu(5 -> child One/Two), quit(6).
    fn menu() -> Menu {
        Menu::new()
            .section_header(Row::info().label("Claude"))
            .row(
                Row::new("switch:me")
                    .label("me@example.com")
                    .leading(Icon::Checkmark)
                    .checked(true),
            )
            .row(Row::new("switch:you").label("you@example.com"))
            .separator()
            .row(Row::new("locked").label("Locked").enabled(false))
            .submenu(
                Row::new("settings").label("Settings"),
                Menu::new()
                    .row(Row::new("s1").label("One"))
                    .row(Row::new("s2").label("Two")),
            )
            .row(Row::new("quit").label("Quit"))
    }

    #[test]
    fn root_is_a_menu_with_one_node_per_item() {
        let tree = build_tree(&menu());
        assert_eq!(tree.root.role, AxRole::Menu);
        assert_eq!(tree.root.children.len(), 7);
    }

    #[test]
    fn roles_and_states_map_from_the_menu() {
        let tree = build_tree(&menu());
        let c = &tree.root.children;
        assert_eq!(c[0].role, AxRole::GroupLabel); // header
        assert_eq!(c[0].name, "Claude");
        assert_eq!(c[1].role, AxRole::MenuItemCheckbox); // checked account
        assert_eq!(c[1].checked, Some(true));
        assert_eq!(c[2].role, AxRole::MenuItem);
        assert_eq!(c[3].role, AxRole::Separator);
        assert!(!c[4].enabled); // disabled row
        assert!(c[5].has_popup);
        assert_eq!(c[5].expanded, Some(false));
        assert_eq!(c[5].children.len(), 2); // submenu children
        assert_eq!(c[5].children[0].name, "One");
    }

    #[test]
    fn set_position_counts_only_focusable_siblings() {
        let tree = build_tree(&menu());
        let c = &tree.root.children;
        // Focusable top-level items: active(1), plain(2), submenu(5), quit(6) = 4.
        // Header, separator, and the disabled row are excluded from the set.
        assert_eq!(c[1].pos_in_set, Some(1));
        assert_eq!(c[1].set_size, Some(4));
        assert_eq!(c[2].pos_in_set, Some(2));
        assert_eq!(c[4].pos_in_set, None); // disabled
        assert_eq!(c[5].pos_in_set, Some(3));
        assert_eq!(c[6].pos_in_set, Some(4));
    }

    #[test]
    fn ids_are_unique_and_findable() {
        let tree = build_tree(&menu());
        let n = tree.node_count();
        // root + 7 top-level + 2 submenu children = 10.
        assert_eq!(n, 10);
        for id in 0..n as u64 {
            assert!(tree.find(AxId(id)).is_some(), "id {id} should exist");
        }
    }

    #[test]
    fn focused_id_tracks_top_level_selection() {
        let tree = build_tree(&menu());
        let focus = MenuFocus {
            top: Some(2),
            flyout: None,
        };
        let id = focused_id(&tree, &focus).unwrap();
        assert_eq!(tree.find(id).unwrap().name, "you@example.com");
    }

    #[test]
    fn focused_id_tracks_flyout_child() {
        let tree = build_tree(&menu());
        let focus = MenuFocus {
            top: Some(5),
            flyout: Some(FlyoutFocus {
                parent: 5,
                child: Some(1),
            }),
        };
        let id = focused_id(&tree, &focus).unwrap();
        assert_eq!(tree.find(id).unwrap().name, "Two");
    }

    #[test]
    fn focused_id_on_open_flyout_without_child_is_the_parent() {
        let tree = build_tree(&menu());
        let focus = MenuFocus {
            top: Some(5),
            flyout: Some(FlyoutFocus {
                parent: 5,
                child: None,
            }),
        };
        let id = focused_id(&tree, &focus).unwrap();
        assert_eq!(tree.find(id).unwrap().name, "Settings");
    }

    #[test]
    fn set_expanded_flips_the_submenu_state() {
        let mut tree = build_tree(&menu());
        set_expanded(&mut tree, 5, true);
        assert_eq!(tree.root.children[5].expanded, Some(true));
        set_expanded(&mut tree, 5, false);
        assert_eq!(tree.root.children[5].expanded, Some(false));
    }

    #[test]
    fn announcement_reads_name_state_role_and_position() {
        let tree = build_tree(&menu());
        let c = &tree.root.children;
        assert_eq!(
            announcement(&c[1]),
            "me@example.com, checked, menu item, 1 of 4"
        );
        assert_eq!(announcement(&c[0]), "Claude, heading");
        assert_eq!(
            announcement(&c[4]),
            "Locked, dimmed, menu item" // disabled row, not in the focusable set
        );
        assert_eq!(
            announcement(&c[5]),
            "Settings, submenu, collapsed, menu item, 3 of 4"
        );
    }
}
