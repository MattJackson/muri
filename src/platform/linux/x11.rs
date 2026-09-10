//! X11 styled popup backend for `ContextMenu::open_at` (spec 22 §2 Path 2 / §3).
//!
//! X11 — unlike Wayland — lets a client position its own toplevel at absolute
//! screen coordinates, so muri's portable `place_popup`/`place_flyout` math
//! applies directly here (spec 22 §2, "X11 *does* allow client positioning").
//! This module opens an **override-redirect** `_NET_WM_WINDOW_TYPE_POPUP_MENU`
//! window (the X11 equivalent of "no WM decoration, no focus steal") at the
//! caller's pointer coordinate, paints the same [`RasterDrawer`] framebuffer every
//! backend uses via `PutImage`, grabs the pointer + keyboard, and runs a local
//! event loop: pointer motion drives hover + the N-level flyout **stack**
//! (decision #8) through the shared pure [`next_flyout`]/[`place_flyout`] logic, a
//! click on a row dispatches (or opens a submenu), and Esc / a click outside every
//! panel / `FocusOut` dismisses without dispatching.
//!
//! It uses x11rb's **pure-Rust** `RustConnection` (no `libxcb`), so muri gains no
//! system X11 link dependency — only a reachable X server (including XWayland) at
//! runtime.
//!
//! ## Verification status
//!
//! The protocol flow (connect, override-redirect create, `PutImage` present,
//! pointer/keyboard grab, event pump) is complete and compiles cleanly for the
//! Linux target, but the live pixel/grab/keymap behavior needs a real X server:
//! every such spot is marked `DEVICE-VERIFY(0.9.0)`. The window is an **opaque**
//! panel drawn on the screen's default (typically 24-bit) visual — spec 22 §6
//! makes the Linux styled surface opaque anyway (no client-controllable vibrancy),
//! and a real ARGB visual for rounded-corner alpha is deferred (see
//! [`encode_framebuffer`]).

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, GrabMode, GrabStatus,
    ImageFormat, ImageOrder, PropMode, Screen, Visualtype, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use crate::error::{Error, Result, Unsupported};
use crate::flyout::{next_flyout, place_flyout, HoverTarget};
use crate::geometry::{Edge, LogicalPoint, LogicalRect, LogicalSize};
use crate::keynav::{handle_key, FlyoutFocus, MenuFocus, NavAction, NavKey};
use crate::menu::{Item, Menu, MenuId};
use crate::render::paint::{render_menu, LaidMenu};
use crate::render::RasterDrawer;
use crate::style::{Color, Rgba};
use crate::theme::{MenuOptions, OsFamily, Theme};

/// Device-scale factor the X11 popup rasterizes at.
///
/// DEVICE-VERIFY(0.9.0): HiDPI. A real per-monitor scale needs an `Xft.dpi` /
/// RANDR query; 1.0 is correct for the common 96-DPI case and never mis-sizes the
/// window relative to what it paints (the framebuffer is rendered at this same
/// scale).
const SCALE: f32 = 1.0;

/// The gap (logical px) between the anchor point and the popup, matching the other
/// backends' `place_popup` gap.
const POPUP_GAP: f32 = 2.0;

/// Open a styled, pointer-anchored popup session on X11 and block until it
/// dismisses (spec 22 §2 Path 2). Returns `Ok(())` once the menu closes; a
/// connection failure is surfaced as [`Error::Platform`].
pub(super) fn open_popup_session(
    menu: Menu,
    options: MenuOptions,
    on_click: &(dyn Fn(&MenuId) + '_),
    anchor: LogicalRect,
    edge: Edge,
    dark: bool,
) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None)
        .map_err(|e| Error::Platform(format!("X11 connect failed: {e}")))?;
    let mut session = Session::new(&conn, screen_num, menu, options, on_click, edge, dark)?;
    session.run(anchor)
}

/// The immutable per-connection X11 facts the present + placement paths need.
struct X11Env {
    root: u32,
    /// The screen's default visual + depth (an opaque panel is drawn on it).
    depth: u8,
    visual: Visualtype,
    /// Bits-per-pixel for `depth` from the server's pixmap formats (usually 32).
    bits_per_pixel: u8,
    byte_order: ImageOrder,
    /// The whole-screen work area in logical pixels (origin at 0,0).
    ///
    /// DEVICE-VERIFY(0.9.0): real multi-monitor bounds + `_NET_WORKAREA` struts;
    /// this is the primary screen's full extent.
    work_area: LogicalRect,
    net_wm_window_type: u32,
    net_wm_window_type_popup_menu: u32,
}

/// A live popup or flyout panel: its X11 window, the drawer that paints it, its
/// current layout + hover, and its top-left origin in logical screen coordinates.
struct Panel {
    window: u32,
    gc: u32,
    drawer: RasterDrawer,
    laid: LaidMenu,
    /// Top-left origin in logical screen coordinates.
    origin: LogicalPoint,
    hovered: Option<usize>,
}

/// One open flyout level: the parent row (within its parent level's menu) it
/// opened from, and its panel. Level `k` of [`Session::flyouts`] is menu level
/// `k + 1`.
struct Flyout {
    parent: usize,
    panel: Panel,
}

/// The X11 popup session: everything needed to render, anchor, and drive the
/// popup + its flyout stack to dismissal.
struct Session<'conn, 'cb> {
    conn: &'conn RustConnection,
    env: X11Env,
    menu: Menu,
    options: MenuOptions,
    theme: Theme,
    edge: Edge,
    dispatch: &'cb (dyn Fn(&MenuId) + 'cb),
    popup: Option<Panel>,
    flyouts: Vec<Flyout>,
    /// Keycode -> primary keysym map (indexed by `keycode - min_keycode`).
    keymap: Vec<u32>,
    min_keycode: u8,
    keysyms_per_keycode: u8,
    done: bool,
}

impl<'conn, 'cb> Session<'conn, 'cb> {
    fn new(
        conn: &'conn RustConnection,
        screen_num: usize,
        menu: Menu,
        options: MenuOptions,
        dispatch: &'cb (dyn Fn(&MenuId) + 'cb),
        edge: Edge,
        dark: bool,
    ) -> Result<Self> {
        let setup = conn.setup();
        let screen: &Screen = setup
            .roots
            .get(screen_num)
            .ok_or_else(|| Error::Platform("X11 screen out of range".into()))?;

        let (visual, depth) = default_visual(screen)
            .ok_or_else(|| Error::Platform("no usable TrueColor X11 visual".into()))?;
        // `encode_framebuffer`/`pack_rgb` only know how to pack straight 8-bit-
        // per-channel RGB into a ZPixmap byte, which is only correct for 24-
        // and 32-bit-per-pixel TrueColor visuals (see the comment on
        // `pack_rgb`). A 16-bit (565) or other narrower default visual would
        // either garble the popup or trip a `BadLength` from `PutImage`, so
        // fail fast here instead of drawing corrupted pixels.
        if depth != 24 && depth != 32 {
            return Err(Error::Platform(format!(
                "unsupported X11 visual depth {depth} (the styled popup needs a \
                 24- or 32-bit TrueColor visual)"
            )));
        }
        let bits_per_pixel = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .map(|f| f.bits_per_pixel)
            .unwrap_or(32);

        let net_wm_window_type = intern(conn, b"_NET_WM_WINDOW_TYPE")?;
        let net_wm_window_type_popup_menu = intern(conn, b"_NET_WM_WINDOW_TYPE_POPUP_MENU")?;

        let work_area = LogicalRect::new(
            LogicalPoint::new(0.0, 0.0),
            LogicalSize::new(
                screen.width_in_pixels as f32 / SCALE,
                screen.height_in_pixels as f32 / SCALE,
            ),
        );

        let env = X11Env {
            root: screen.root,
            depth,
            visual,
            bits_per_pixel,
            byte_order: setup.image_byte_order,
            work_area,
            net_wm_window_type,
            net_wm_window_type_popup_menu,
        };

        let (keymap, min_keycode, keysyms_per_keycode) = load_keymap(conn)?;

        let theme = resolve_theme(&options, dark);

        Ok(Session {
            conn,
            env,
            menu,
            options,
            theme,
            edge,
            dispatch,
            popup: None,
            flyouts: Vec::new(),
            keymap,
            min_keycode,
            keysyms_per_keycode,
            done: false,
        })
    }

    /// Open the popup at `anchor`, grab input, and pump events to dismissal.
    fn run(&mut self, anchor: LogicalRect) -> Result<()> {
        self.open_popup(anchor)?;
        let Some(popup) = self.popup.as_ref() else {
            return Err(Error::Platform(
                "failed to open the X11 popup window".into(),
            ));
        };
        let grab_window = popup.window;

        // Grab pointer + keyboard so motion/clicks/keys outside our windows still
        // reach us (for outside-click dismiss and keyboard nav).
        //
        // DEVICE-VERIFY(0.9.0): grab acquisition can fail transiently (another
        // client holds a grab); we treat a non-success status as a soft failure
        // and still run, relying on FocusOut/Esc for dismissal.
        let _ = self.conn.grab_pointer(
            true,
            grab_window,
            EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION
                | EventMask::LEAVE_WINDOW,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            NONE,
            NONE,
            CURRENT_TIME,
        );
        if let Ok(cookie) = self.conn.grab_keyboard(
            true,
            grab_window,
            CURRENT_TIME,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        ) {
            if let Ok(reply) = cookie.reply() {
                let _ = reply.status == GrabStatus::SUCCESS;
            }
        }
        self.conn
            .flush()
            .map_err(|e| Error::Platform(format!("X11 flush failed: {e}")))?;

        while !self.done {
            let event = self
                .conn
                .wait_for_event()
                .map_err(|e| Error::Platform(format!("X11 event wait failed: {e}")))?;
            self.handle_event(event)?;
            self.conn
                .flush()
                .map_err(|e| Error::Platform(format!("X11 flush failed: {e}")))?;
        }
        self.close_all();
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
        let _ = self.conn.flush();
        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Expose(_) => {
                // Repaint every live panel (menus are tiny; a full repaint is cheap).
                self.redraw_all()?;
            }
            Event::MotionNotify(e) => {
                let pt = self.to_logical(e.root_x, e.root_y);
                self.on_motion(pt)?;
            }
            Event::ButtonPress(e) => {
                if e.detail == 1 || e.detail == 3 {
                    let pt = self.to_logical(e.root_x, e.root_y);
                    self.on_click(pt)?;
                }
            }
            Event::KeyPress(e) => {
                if let Some(key) = self.translate_key(e.detail) {
                    self.on_key(key)?;
                }
            }
            Event::FocusOut(_) => {
                // Focus genuinely left our stack (not a within-stack transfer, which
                // an override-redirect grab keeps internal): dismiss without dispatch.
                //
                // DEVICE-VERIFY(0.9.0): FocusOut vs. grab interactions differ across
                // window managers; outside-click + Esc are the primary dismiss paths.
                self.done = true;
            }
            _ => {}
        }
        Ok(())
    }

    // -- placement / open ----------------------------------------------------

    fn open_popup(&mut self, anchor: LogicalRect) -> Result<()> {
        // Forced-OS theme -> target OS font; else host-native (#54).
        let mut drawer = RasterDrawer::for_menu_options(SCALE, &self.options);
        let laid = render_menu(&mut drawer, &self.menu, &self.theme, &self.options, None);
        let origin =
            crate::anchor::place_popup(anchor, laid.size, self.env.work_area, self.edge, POPUP_GAP);
        let panel = self.create_panel(origin, laid, drawer)?;
        self.popup = Some(panel);
        self.redraw_all()?;
        Ok(())
    }

    /// Create + map an override-redirect popup window sized to `laid`, at `origin`.
    fn create_panel(
        &self,
        origin: LogicalPoint,
        laid: LaidMenu,
        drawer: RasterDrawer,
    ) -> Result<Panel> {
        let conn = self.conn;
        let window = conn
            .generate_id()
            .map_err(|e| Error::Platform(format!("X11 id alloc failed: {e}")))?;
        let (dx, dy) = self.to_device(origin);
        let (w, h) = (
            (laid.size.width * SCALE).round().max(1.0) as u16,
            (laid.size.height * SCALE).round().max(1.0) as u16,
        );

        let bg = self.theme.resolve(self.theme.background);
        let aux = CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(self.pack_pixel(bg))
            .border_pixel(0)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::KEY_PRESS
                    | EventMask::LEAVE_WINDOW
                    | EventMask::FOCUS_CHANGE
                    | EventMask::STRUCTURE_NOTIFY,
            );

        conn.create_window(
            self.env.depth,
            window,
            self.env.root,
            dx,
            dy,
            w,
            h,
            0,
            WindowClass::INPUT_OUTPUT,
            self.env.visual.visual_id,
            &aux,
        )
        .map_err(|e| Error::Platform(format!("X11 create_window failed: {e}")))?;

        // The window now exists on the server; any later fallible step must
        // destroy it (and free any GC created since) before propagating so we
        // don't leak an unmapped window or a dangling GC XID.
        let mut created_gc: Option<u32> = None;
        let build = (|| -> Result<Panel> {
            // Mark it a popup menu so a compositor treats it correctly (no shadow
            // decoration, no taskbar entry). Override-redirect already bypasses the WM.
            conn.change_property32(
                PropMode::REPLACE,
                window,
                self.env.net_wm_window_type,
                AtomEnum::ATOM,
                &[self.env.net_wm_window_type_popup_menu],
            )
            .map_err(|e| Error::Platform(format!("X11 set window type failed: {e}")))?;

            let gc = conn
                .generate_id()
                .map_err(|e| Error::Platform(format!("X11 id alloc failed: {e}")))?;
            conn.create_gc(gc, window, &CreateGCAux::new())
                .map_err(|e| Error::Platform(format!("X11 create_gc failed: {e}")))?;
            created_gc = Some(gc);

            conn.map_window(window)
                .map_err(|e| Error::Platform(format!("X11 map_window failed: {e}")))?;

            Ok(Panel {
                window,
                gc,
                drawer,
                laid,
                origin,
                hovered: None,
            })
        })();

        build.inspect_err(|_| {
            if let Some(gc) = created_gc {
                let _ = conn.free_gc(gc);
            }
            let _ = conn.destroy_window(window);
        })
    }

    // -- present -------------------------------------------------------------

    fn redraw_all(&mut self) -> Result<()> {
        self.redraw(0)?;
        for d in 0..self.flyouts.len() {
            self.redraw(d + 1)?;
        }
        Ok(())
    }

    /// Re-render + present menu level `level` (0 = popup) from its hovered row.
    fn redraw(&mut self, level: usize) -> Result<()> {
        // Borrow the level's menu from `self.menu`; the panel below is taken from
        // the disjoint `self.popup`/`self.flyouts` fields (not via `panel_mut`,
        // which would borrow all of `self`), so no clone is needed.
        let Some(menu) = crate::menu::descend(
            &self.menu,
            self.flyouts.iter().take(level).map(|f| f.parent),
        ) else {
            return Ok(());
        };
        let theme = self.theme.clone();
        let options = self.options.clone();
        let (env_depth, env_bpp, env_byte_order) =
            (self.env.depth, self.env.bits_per_pixel, self.env.byte_order);
        let visual = self.env.visual;
        let bg = theme.resolve(theme.background);

        let panel = if level == 0 {
            self.popup.as_mut()
        } else {
            self.flyouts.get_mut(level - 1).map(|f| &mut f.panel)
        };
        let Some(panel) = panel else {
            return Ok(());
        };
        let laid = render_menu(&mut panel.drawer, menu, &theme, &options, panel.hovered);
        panel.laid = laid;
        let fb = panel.drawer.framebuffer();
        let (w, h) = (fb.width(), fb.height());
        let data = encode_framebuffer(fb.pixels(), w, h, &visual, env_byte_order, env_bpp, bg);
        let window = panel.window;
        let gc = panel.gc;

        present(
            self.conn, window, gc, w as u16, h as u16, env_depth, env_bpp, &data,
        )
    }

    // -- pointer -------------------------------------------------------------

    fn on_motion(&mut self, pt: LogicalPoint) -> Result<()> {
        // Find which panel (topmost/deepest first) the pointer is over.
        let target = self.hover_target(pt);
        // Update the hovered row on whichever panel the pointer is in.
        let mut dirty: Option<usize> = None;
        if let Some((level, local)) = self.panel_at(pt) {
            if let Some(panel) = self.panel_mut(level) {
                let h = panel.laid.hit(local);
                if h != panel.hovered {
                    panel.hovered = h;
                    dirty = Some(level);
                }
            }
        }
        let next = next_flyout(&self.flyout_stack(), target);
        self.apply_flyout_stack(&next)?;
        if let Some(level) = dirty {
            // The panel may have been truncated away; guard in redraw.
            self.redraw(level)?;
        }
        Ok(())
    }

    fn on_click(&mut self, pt: LogicalPoint) -> Result<()> {
        let Some((level, local)) = self.panel_at(pt) else {
            // Click outside every panel: dismiss without dispatch.
            self.done = true;
            return Ok(());
        };
        let Some(menu) = self.menu_at_level(level) else {
            return Ok(());
        };
        let (hit, id) = {
            let Some(panel) = self.panel_ref(level) else {
                return Ok(());
            };
            (panel.laid.hit(local), panel.laid.id_at(local))
        };
        if let Some(i) = hit {
            if matches!(menu.items.get(i), Some(Item::Submenu { .. })) {
                self.truncate_flyouts(level)?;
                self.push_flyout(i)?;
                return Ok(());
            }
        }
        if let Some(id) = id {
            if !id.is_none() {
                (self.dispatch)(&id);
            }
            self.done = true;
        }
        Ok(())
    }

    // -- keyboard ------------------------------------------------------------

    fn on_key(&mut self, key: NavKey) -> Result<()> {
        let mut focus = self.current_focus();
        let action = handle_key(&self.menu, &mut focus, key);
        if let Some(popup) = self.popup.as_mut() {
            popup.hovered = focus.top;
        }
        match action {
            NavAction::None => return Ok(()),
            NavAction::Redraw => {}
            NavAction::OpenFlyout(i) => self.push_flyout(i)?,
            NavAction::CloseFlyout => {
                let keep = self.flyouts.len().saturating_sub(1);
                self.truncate_flyouts(keep)?;
            }
            NavAction::Activate(id) => {
                if !id.is_none() {
                    (self.dispatch)(&id);
                }
                self.done = true;
                return Ok(());
            }
            NavAction::CloseAll => {
                self.done = true;
                return Ok(());
            }
        }
        for (k, f) in self.flyouts.iter_mut().enumerate() {
            if let Some(ff) = focus.flyout.get(k) {
                f.panel.hovered = ff.child;
            }
        }
        self.redraw_all()
    }

    // -- flyout stack --------------------------------------------------------

    /// Reconcile the open flyout stack to `target` (per-level parent indices):
    /// keep the common prefix, close deeper, then push the remaining levels.
    fn apply_flyout_stack(&mut self, target: &[usize]) -> Result<()> {
        let mut common = 0;
        while common < target.len()
            && common < self.flyouts.len()
            && self.flyouts[common].parent == target[common]
        {
            common += 1;
        }
        self.truncate_flyouts(common)?;
        for &parent in &target[common..] {
            self.push_flyout(parent)?;
        }
        Ok(())
    }

    /// Open a flyout for row `parent_index` of the currently deepest open level.
    fn push_flyout(&mut self, parent_index: usize) -> Result<()> {
        let depth = self.flyouts.len();
        let Some(parent_menu) = self.menu_at_level(depth) else {
            return Ok(());
        };
        let Some(child) = (match parent_menu.items.get(parent_index) {
            Some(Item::Submenu { menu, .. }) => Some(menu.clone()),
            _ => None,
        }) else {
            return Ok(());
        };

        let (parent_origin, parent_size, row_rect) = {
            let Some(pp) = self.panel_ref(depth) else {
                return Ok(());
            };
            let Some(rect) = pp
                .laid
                .rows
                .iter()
                .find(|r| r.index == parent_index)
                .map(|r| r.rect)
            else {
                return Ok(());
            };
            (pp.origin, pp.laid.size, rect)
        };

        // Forced-OS theme -> target OS font; else host-native (#54).
        let mut drawer = RasterDrawer::for_menu_options(SCALE, &self.options);
        let child_laid = render_menu(&mut drawer, &child, &self.theme, &self.options, None);
        let parent_rect = LogicalRect::new(parent_origin, parent_size);
        let placement = place_flyout(parent_rect, row_rect, child_laid.size, self.env.work_area);
        let panel = self.create_panel(placement.origin, child_laid, drawer)?;
        self.flyouts.push(Flyout {
            parent: parent_index,
            panel,
        });
        self.redraw(depth + 1)?;
        Ok(())
    }

    /// Close every flyout deeper than `len`, destroying their windows.
    fn truncate_flyouts(&mut self, len: usize) -> Result<()> {
        while self.flyouts.len() > len {
            if let Some(f) = self.flyouts.pop() {
                self.destroy_panel(&f.panel)?;
            }
        }
        Ok(())
    }

    fn close_all(&mut self) {
        let _ = self.truncate_flyouts(0);
        if let Some(popup) = self.popup.take() {
            let _ = self.destroy_panel(&popup);
        }
    }

    fn destroy_panel(&self, panel: &Panel) -> Result<()> {
        let _ = self.conn.free_gc(panel.gc);
        self.conn
            .destroy_window(panel.window)
            .map_err(|e| Error::Platform(format!("X11 destroy_window failed: {e}")))?;
        Ok(())
    }

    // -- helpers -------------------------------------------------------------

    fn flyout_stack(&self) -> Vec<usize> {
        self.flyouts.iter().map(|f| f.parent).collect()
    }

    fn current_focus(&self) -> MenuFocus {
        MenuFocus {
            top: self.popup.as_ref().and_then(|p| p.hovered),
            flyout: self
                .flyouts
                .iter()
                .map(|f| FlyoutFocus {
                    parent: f.parent,
                    child: f.panel.hovered,
                })
                .collect(),
        }
    }

    /// The menu shown at `level` (0 = top-level), descending the open flyouts'
    /// parents. Borrows from `self.menu` — no per-call clone (this runs on every
    /// redraw, i.e. every hover change and keynav).
    fn menu_at_level(&self, level: usize) -> Option<&Menu> {
        crate::menu::descend(
            &self.menu,
            self.flyouts.iter().take(level).map(|f| f.parent),
        )
    }

    fn panel_ref(&self, level: usize) -> Option<&Panel> {
        if level == 0 {
            self.popup.as_ref()
        } else {
            self.flyouts.get(level - 1).map(|f| &f.panel)
        }
    }

    fn panel_mut(&mut self, level: usize) -> Option<&mut Panel> {
        if level == 0 {
            self.popup.as_mut()
        } else {
            self.flyouts.get_mut(level - 1).map(|f| &mut f.panel)
        }
    }

    /// The deepest panel containing `pt` (logical screen coords) and the
    /// window-local point within it.
    fn panel_at(&self, pt: LogicalPoint) -> Option<(usize, LogicalPoint)> {
        for level in (0..=self.flyouts.len()).rev() {
            if let Some(panel) = self.panel_ref(level) {
                let rect = LogicalRect::new(panel.origin, panel.laid.size);
                if rect.contains(pt) {
                    let local = LogicalPoint::new(pt.x - panel.origin.x, pt.y - panel.origin.y);
                    return Some((level, local));
                }
            }
        }
        None
    }

    /// Classify the pointer position for [`next_flyout`].
    fn hover_target(&self, pt: LogicalPoint) -> HoverTarget {
        let Some((level, local)) = self.panel_at(pt) else {
            return HoverTarget::Outside;
        };
        let Some(panel) = self.panel_ref(level) else {
            return HoverTarget::Outside;
        };
        match panel.laid.hit(local) {
            Some(i) => {
                let is_submenu = self
                    .menu_at_level(level)
                    .is_some_and(|m| matches!(m.items.get(i), Some(Item::Submenu { .. })));
                if is_submenu {
                    HoverTarget::ParentRow {
                        panel: level,
                        index: i,
                    }
                } else {
                    HoverTarget::OtherRow { panel: level }
                }
            }
            // Over a panel body but not a row: keep the stack open if it's a flyout.
            None if level > 0 => HoverTarget::Flyout,
            None => HoverTarget::OtherRow { panel: level },
        }
    }

    /// Logical screen point from an X11 root-relative device coordinate.
    fn to_logical(&self, x: i16, y: i16) -> LogicalPoint {
        LogicalPoint::new(x as f32 / SCALE, y as f32 / SCALE)
    }

    /// X11 device coordinates from a logical screen origin.
    fn to_device(&self, p: LogicalPoint) -> (i16, i16) {
        // X11 window position is protocol INT16, so a very wide multi-monitor
        // virtual desktop (device x > 32767) would silently mis-cast with a bare
        // `as i16`. Saturate (NaN-safe) so the popup pins to the reachable edge
        // instead of wrapping to a wrong/negative position (#38).
        fn to_i16(v: f32) -> i16 {
            v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
        }
        (to_i16(p.x * SCALE), to_i16(p.y * SCALE))
    }

    /// Pack a straight-alpha color into the visual's pixel value (opaque).
    fn pack_pixel(&self, c: Rgba) -> u32 {
        pack_rgb(&self.env.visual, c.r, c.g, c.b)
    }

    /// Translate an X11 keycode to a muri [`NavKey`], if it maps to one.
    fn translate_key(&self, keycode: u8) -> Option<NavKey> {
        let idx =
            (keycode.checked_sub(self.min_keycode)? as usize) * self.keysyms_per_keycode as usize;
        let keysym = *self.keymap.get(idx)?;
        keysym_to_navkey(keysym)
    }
}

// =============================================================================
// Free helpers (pure X11 glue, no session state)
// =============================================================================

/// Intern an atom by name.
fn intern(conn: &RustConnection, name: &[u8]) -> Result<u32> {
    conn.intern_atom(false, name)
        .map_err(|e| Error::Platform(format!("X11 intern_atom failed: {e}")))?
        .reply()
        .map(|r| r.atom)
        .map_err(|e| Error::Platform(format!("X11 intern_atom reply failed: {e}")))
}

/// The screen's default TrueColor visual + depth, if it is a usable RGB visual.
fn default_visual(screen: &Screen) -> Option<(Visualtype, u8)> {
    for depth in &screen.allowed_depths {
        for visual in &depth.visuals {
            if visual.visual_id == screen.root_visual {
                if visual.red_mask == 0 || visual.green_mask == 0 || visual.blue_mask == 0 {
                    return None;
                }
                return Some((*visual, depth.depth));
            }
        }
    }
    None
}

/// The `count` argument for `GetKeyboardMapping` given a server-reported
/// `min`/`max` keycode pair, or `None` if the pair is invalid (`max < min`)
/// or its span doesn't fit in the protocol's single-byte `count` field.
fn keycode_range_count(min: u8, max: u8) -> Option<u8> {
    u16::from(max)
        .checked_sub(u16::from(min))
        .and_then(|span| span.checked_add(1))
        .and_then(|span| u8::try_from(span).ok())
}

/// Load the keycode -> keysym table once for the session.
fn load_keymap(conn: &RustConnection) -> Result<(Vec<u32>, u8, u8)> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let max = setup.max_keycode;
    // `GetKeyboardMapping`'s `count` is a single byte, so the server-reported
    // range has to fit within it. `min`/`max` come from a possibly hostile or
    // buggy `$DISPLAY` server (untrusted): a `min > max` pair would underflow
    // the naive `max - min + 1`, and a 256-keycode span (e.g. min=0, max=255)
    // would overflow it. Reject anything that doesn't fit rather than panic
    // or misindex `keymap` later in `translate_key`.
    let count = keycode_range_count(min, max).ok_or_else(|| {
        Error::Platform(format!(
            "X11 server reported an invalid keycode range (min={min}, max={max})"
        ))
    })?;
    let reply = conn
        .get_keyboard_mapping(min, count)
        .map_err(|e| Error::Platform(format!("X11 get_keyboard_mapping failed: {e}")))?
        .reply()
        .map_err(|e| Error::Platform(format!("X11 keyboard mapping reply failed: {e}")))?;
    Ok((reply.keysyms, min, reply.keysyms_per_keycode))
}

/// Map an X11 keysym to a menu [`NavKey`]. Covers the arrows, activate, escape,
/// home/end, and printable ASCII for type-ahead.
///
/// DEVICE-VERIFY(0.9.0): live keymap coverage (layouts, keypad) against a real
/// keyboard; the core navigation keysyms are standard X11 constants.
fn keysym_to_navkey(keysym: u32) -> Option<NavKey> {
    match keysym {
        0xff52 => Some(NavKey::Up),                // XK_Up
        0xff54 => Some(NavKey::Down),              // XK_Down
        0xff51 => Some(NavKey::Left),              // XK_Left
        0xff53 => Some(NavKey::Right),             // XK_Right
        0xff0d | 0xff8d => Some(NavKey::Activate), // Return / KP_Enter
        0x0020 => Some(NavKey::Activate),          // space
        0xff1b => Some(NavKey::Escape),            // XK_Escape
        0xff50 => Some(NavKey::Home),              // XK_Home
        0xff57 => Some(NavKey::End),               // XK_End
        // Printable ASCII (Latin-1 keysyms equal their codepoint) for type-ahead.
        0x0021..=0x007e => char::from_u32(keysym).map(NavKey::Char),
        _ => None,
    }
}

/// The bit shift for a channel mask (its least-significant set bit).
fn mask_shift(mask: u32) -> u32 {
    if mask == 0 {
        0
    } else {
        mask.trailing_zeros()
    }
}

/// Pack straight 8-bit RGB into a visual's pixel value. Assumes 8-bit-per-
/// channel packing, which holds for 24- and 32-bit-per-pixel TrueColor
/// visuals (the only depths `Session::new` accepts); it is not correct for
/// narrower visuals such as 16-bit (565) TrueColor.
fn pack_rgb(visual: &Visualtype, r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << mask_shift(visual.red_mask))
        | ((g as u32) << mask_shift(visual.green_mask))
        | ((b as u32) << mask_shift(visual.blue_mask))
}

/// Convert a premultiplied-RGBA framebuffer into `PutImage` ZPixmap bytes for the
/// server's visual + byte order, compositing over the opaque theme background so
/// the result is opaque (spec 22 §6: the Linux styled panel is opaque).
fn encode_framebuffer(
    pixels: &[u8],
    width: u32,
    height: u32,
    visual: &Visualtype,
    byte_order: ImageOrder,
    bits_per_pixel: u8,
    bg: Rgba,
) -> Vec<u8> {
    let bytes_per_pixel = (bits_per_pixel as usize / 8).max(4);
    let count = (width as usize) * (height as usize);
    let mut out = vec![0u8; count * bytes_per_pixel];
    let msb = byte_order == ImageOrder::MSB_FIRST;
    for (i, src) in pixels.chunks_exact(4).enumerate() {
        // `src` is premultiplied RGBA; composite over the opaque background:
        // out_c = src_c + bg_c * (255 - a) / 255.
        let a = src[3] as u32;
        let inv = 255 - a;
        let r = (src[0] as u32 + bg.r as u32 * inv / 255).min(255) as u8;
        let g = (src[1] as u32 + bg.g as u32 * inv / 255).min(255) as u8;
        let b = (src[2] as u32 + bg.b as u32 * inv / 255).min(255) as u8;
        let value = pack_rgb(visual, r, g, b);
        let o = i * bytes_per_pixel;
        if msb {
            out[o] = (value >> 24) as u8;
            out[o + 1] = (value >> 16) as u8;
            out[o + 2] = (value >> 8) as u8;
            out[o + 3] = value as u8;
        } else {
            out[o] = value as u8;
            out[o + 1] = (value >> 8) as u8;
            out[o + 2] = (value >> 16) as u8;
            out[o + 3] = (value >> 24) as u8;
        }
    }
    out
}

/// Push a whole framebuffer to a window with `PutImage`, splitting into row bands
/// so no single request exceeds the server's maximum request length.
#[allow(clippy::too_many_arguments)]
fn present(
    conn: &RustConnection,
    window: u32,
    gc: u32,
    width: u16,
    height: u16,
    depth: u8,
    bits_per_pixel: u8,
    data: &[u8],
) -> Result<()> {
    if width == 0 || height == 0 {
        return Ok(());
    }
    let bytes_per_pixel = (bits_per_pixel as usize / 8).max(4);
    let row_bytes = width as usize * bytes_per_pixel;
    // Leave generous headroom under the max request for the PutImage header.
    let max_bytes = conn
        .maximum_request_bytes()
        .saturating_sub(1024)
        .max(row_bytes);
    let rows_per_chunk = (max_bytes / row_bytes.max(1)).max(1);

    let mut y = 0u16;
    while y < height {
        let rows = rows_per_chunk.min((height - y) as usize) as u16;
        let start = y as usize * row_bytes;
        let end = start + rows as usize * row_bytes;
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            window,
            gc,
            width,
            rows,
            0,
            y as i16,
            0,
            depth,
            &data[start..end],
        )
        .map_err(|e| Error::Platform(format!("X11 put_image failed: {e}")))?;
        y += rows;
    }
    Ok(())
}

/// Resolve the popup theme for the current appearance (mirrors the other
/// backends' resolution; the Linux styled panel is opaque per spec 22 §6).
fn resolve_theme(options: &MenuOptions, dark: bool) -> Theme {
    // Resolve against the host family (GNOME/Adwaita) + live appearance. Only a
    // `System(..)` source gets the live GNOME accent injected; explicit family /
    // preset / custom themes render as authored. The Adwaita base is flat/opaque
    // (no Linux vibrancy), so there is no transparency toggle here.
    let mut theme = options.theme.resolve(OsFamily::Gnome, dark);
    if options.theme.injects_system() {
        if let Some((r, g, b, a)) = super::system_accent() {
            theme.accent = Color::Rgba(r, g, b, a);
        }
    }
    theme
}

/// Whether an X11 popup is possible right now: an X server (incl. XWayland) is
/// reachable via `$DISPLAY`. Wayland-only sessions (no `$DISPLAY`) return `false`,
/// which the caller surfaces as [`Unsupported::ClientPositioning`].
pub(super) fn is_available() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
}

/// The honest Wayland answer when no X11 display is reachable (spec 22 §2, §3): a
/// Wayland client cannot self-position a popup without a parent surface + input
/// serial, which this API does not carry.
pub(super) fn wayland_unsupported() -> Error {
    Error::Unsupported(Unsupported::ClientPositioning)
}

#[cfg(test)]
mod descend_tests {
    use crate::menu::{Item, Menu, Row};

    fn first_row_id(menu: &Menu) -> &str {
        match &menu.items[0] {
            Item::Row(row) => row.id.as_str(),
            other => panic!("expected a row, got {other:?}"),
        }
    }

    /// `menu_at_level`/`descend` must borrow nested submenus straight out of the
    /// source `Menu` (it runs on every redraw) rather than cloning the tree.
    #[test]
    fn descend_borrows_nested_submenus_without_cloning() {
        let grandchild = Menu::new().row(Row::new("c").label("C"));
        let child = Menu::new()
            .row(Row::new("b").label("B"))
            .submenu(Row::new("sub2").label("Sub2"), grandchild);
        let top = Menu::new()
            .row(Row::new("a").label("A"))
            .submenu(Row::new("sub1").label("Sub1"), child);

        assert_eq!(
            first_row_id(crate::menu::descend(&top, [0usize; 0]).unwrap()),
            "a"
        );
        assert_eq!(first_row_id(crate::menu::descend(&top, [1]).unwrap()), "b");
        assert_eq!(
            first_row_id(crate::menu::descend(&top, [1, 1]).unwrap()),
            "c"
        );
        // A non-submenu row on the path -> None.
        assert!(crate::menu::descend(&top, [0]).is_none());
        // An out-of-range index -> None.
        assert!(crate::menu::descend(&top, [9]).is_none());

        // The returned reference points *into* `top` (no clone).
        let borrowed = crate::menu::descend(&top, [1]).unwrap();
        let Item::Submenu { menu, .. } = &top.items[1] else {
            panic!("index 1 should be a submenu");
        };
        assert!(std::ptr::eq(borrowed, menu));
    }
}

#[cfg(test)]
mod keycode_range_tests {
    use super::keycode_range_count;

    /// A normal X11 keymap (min=8, max=255 is the common real-world case).
    #[test]
    fn typical_range_is_accepted() {
        assert_eq!(keycode_range_count(8, 255), Some(248));
        assert_eq!(keycode_range_count(0, 0), Some(1));
        assert_eq!(keycode_range_count(5, 5), Some(1));
    }

    /// The widest range that still fits in the protocol's single-byte count.
    #[test]
    fn max_representable_span_is_accepted() {
        assert_eq!(keycode_range_count(0, 254), Some(255));
    }

    /// A hostile/buggy server reporting `min > max` must not underflow.
    #[test]
    fn inverted_range_is_rejected() {
        assert_eq!(keycode_range_count(255, 0), None);
        assert_eq!(keycode_range_count(10, 9), None);
    }

    /// A 256-keycode span (e.g. min=0, max=255) overflows the single-byte
    /// `count` field and must be rejected rather than wrapping.
    #[test]
    fn full_span_overflow_is_rejected() {
        assert_eq!(keycode_range_count(0, 255), None);
    }
}
