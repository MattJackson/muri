//! Wayland `wlr-layer-shell` styled-popup presenter (research doc §3, §12; ADR-0003).
//!
//! This is the third [`LinuxMenuPresenter`](super::LinuxMenuPresenter) impl: the
//! self-drawn, pixel-identical styled popup on the compositors that expose
//! `zwlr_layer_shell_v1` — every wlroots compositor (sway, Hyprland, river,
//! Wayfire, labwc, cosmic-comp) **and** KWin/Plasma. GNOME/Mutter refuses
//! layer-shell as an architectural stance (research doc §6, mutter#973), so it
//! stays on the native dbusmenu presenter — see [`super::detect_linux_presenter`].
//!
//! ## The technique (research doc §3 "the pointer-accurate recipe")
//!
//! muri opens a single **full-output overlay** layer surface — anchored to all four
//! edges, `exclusive_zone(-1)`, `KeyboardInteractivity::OnDemand` — and draws the
//! whole menu (popup + every open flyout) into *one* framebuffer the size of the
//! output, transparent everywhere except under the panels. Because the surface
//! covers the output, `wl_pointer` motion coordinates **are** output coordinates
//! (the only pointer-position channel Wayland gives a client — research doc §5), so
//! hover, the N-level flyout stack, and outside-click dismissal all fall out of the
//! same shared [`crate::flyout`]/[`crate::keynav`] state machines the X11 backend
//! uses. Only the *transport* (blit into a `wl_shm` `Argb8888` buffer via
//! attach/damage/commit) and *positioning* (overlay + muri's own
//! [`place_popup`](crate::anchor::place_popup) math) are Wayland-specific; unlike
//! the opaque X11 path this keeps **true alpha**, so rounded corners / shadow
//! survive (the compositor composites the ARGB surface).
//!
//! Compared with the child-`xdg_popup` variant sketched in the research doc, the
//! single-overlay compositing approach is equivalent in fidelity (muri already does
//! its own on-screen flip/slide in `place_popup`/`place_flyout`, so the positioner's
//! constraint-adjustment isn't needed) and is simpler + more robust to drive: one
//! surface, one input stream in output coordinates, natural outside-click dismiss.
//!
//! ## Keyboard without libxkbcommon
//!
//! muri drives `wl_keyboard` directly and maps **raw evdev keycodes** for menu
//! navigation (arrows / enter / escape / home / end are layout-independent), so it
//! does not pull `smithay-client-toolkit`'s `xkbcommon` feature and the
//! `libxkbcommon` system-library build probe it carries. Printable type-ahead uses
//! a best-effort US-QWERTY evdev map (`DEVICE-VERIFY(0.11.1)`: non-US layouts).
//!
//! ## Verification status
//!
//! The protocol flow (bind globals, overlay layer surface, `wl_shm` blit, seat
//! input, teardown) is complete and type-checked against `smithay-client-toolkit`
//! 0.19 on the Linux target, but the live surface/loop needs a real compositor:
//! such spots are marked `DEVICE-VERIFY(0.11.1)`. HiDPI is deferred like the X11
//! backend (`SCALE == 1.0`).

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, QueueHandle, WEnum,
};

use crate::error::{Error, Result};
use crate::flyout::{next_flyout, place_flyout, HoverTarget};
use crate::geometry::{Edge, LogicalPoint, LogicalRect, LogicalSize};
use crate::keynav::{handle_key, FlyoutFocus, MenuFocus, NavAction, NavKey};
use crate::menu::{Item, Menu, MenuId};
use crate::render::paint::{render_menu, LaidMenu};
use crate::render::RasterDrawer;
use crate::style::Color;
use crate::theme::{MenuOptions, OsFamily, Theme};

/// Device-scale factor the Wayland popup rasterizes at (see the X11 backend's note;
/// `DEVICE-VERIFY(0.11.1)` HiDPI via `wl_output` scale + `set_buffer_scale`).
const SCALE: f32 = 1.0;

/// The gap (logical px) between the anchor point and the popup, matching the other
/// backends' `place_popup` gap.
const POPUP_GAP: f32 = 2.0;

/// evdev button code for the primary (left) mouse button (`BTN_LEFT`).
const BTN_LEFT: u32 = 0x110;
/// evdev button code for the secondary (right) mouse button (`BTN_RIGHT`).
const BTN_RIGHT: u32 = 0x111;

/// Whether the current Wayland compositor advertises `zwlr_layer_shell_v1` — the
/// authoritative test for whether the styled layer-shell presenter is usable
/// (research doc §10, detection step 2). [`super::detect_linux_presenter`] uses a
/// cheap env heuristic to *pre-select* the presenter; this is the live confirmation
/// a caller can run before actually opening a surface.
///
/// Connects to `$WAYLAND_DISPLAY`, roundtrips the registry, and returns whether the
/// `zwlr_layer_shell_v1` global appears. `false` on any connection error (headless /
/// no compositor) — never panics.
pub(super) fn layer_shell_available() -> bool {
    let Ok(conn) = Connection::connect_to_env() else {
        return false;
    };
    // `PopupState` is only used here as the queue's phantom state type (it provides
    // the required `Dispatch<wl_registry, GlobalListContents>` via `delegate_registry`);
    // no instance is created and the queue is never dispatched.
    let Ok((globals, _queue)) = registry_queue_init::<PopupState>(&conn) else {
        return false;
    };
    globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == "zwlr_layer_shell_v1"))
}

/// The global pointer position — on Wayland this is only knowable *inside* a
/// muri-owned surface the pointer is over (research doc §5), so outside a live popup
/// session there is nothing to report. The tray-anchored path instead uses the
/// out-of-band SNI `Activate`/`ContextMenu` coordinate (Plasma/Waybar).
pub(super) fn cursor_position() -> Option<LogicalPoint> {
    None
}

/// Open a styled, self-drawn popup on a layer-shell compositor and block until it
/// dismisses — the Wayland analogue of [`super::x11::open_popup_session`].
///
/// `anchor` is the pointer (or SNI-reported) point the popup grows from relative to
/// `edge`; `on_click` receives the activated row's id. The click is collected during
/// the event loop and dispatched here *after* it returns (the sctk event state must
/// be `'static`, so the borrowed `on_click` closure cannot live inside it).
pub(super) fn open_popup_session(
    menu: Menu,
    options: MenuOptions,
    on_click: &(dyn Fn(&MenuId) + '_),
    anchor: LogicalRect,
    edge: Edge,
    dark: bool,
) -> Result<()> {
    let activated = run_popup(menu, options, anchor, edge, dark)?;
    if let Some(id) = activated {
        if !id.is_none() {
            on_click(&id);
        }
    }
    Ok(())
}

/// Drive one popup session to dismissal, returning the activated row id (if any).
fn run_popup(
    menu: Menu,
    options: MenuOptions,
    anchor: LogicalRect,
    edge: Edge,
    dark: bool,
) -> Result<Option<MenuId>> {
    let conn = Connection::connect_to_env()
        .map_err(|e| Error::Platform(format!("Wayland connect failed: {e}")))?;
    let (globals, mut event_queue) = registry_queue_init::<PopupState>(&conn)
        .map_err(|e| Error::Platform(format!("Wayland registry init failed: {e}")))?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|e| Error::Platform(format!("wl_compositor unavailable: {e}")))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|e| Error::Platform(format!("zwlr_layer_shell_v1 unavailable: {e}")))?;
    let shm = Shm::bind(&globals, &qh)
        .map_err(|e| Error::Platform(format!("wl_shm unavailable: {e}")))?;

    // A single full-output overlay: anchor all four edges (size 0,0 => the
    // compositor reports the output size in the first configure), do-not-move
    // exclusive zone, and on-demand keyboard so menu nav can take focus without
    // permanently grabbing the keyboard. The default (whole-surface) input region
    // means clicks land on us anywhere on the output — that is how outside-click
    // dismissal works.
    let surface = compositor.create_surface(&qh);
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("muri-menu"), None);
    layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::BOTTOM | Anchor::RIGHT);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
    layer.set_size(0, 0);
    layer.commit();

    let theme = resolve_theme(&options, dark);
    let a11y = super::a11y::PopupA11y::attach(&menu);

    let mut state = PopupState {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool: None,
        layer,
        keyboard: None,
        pointer: None,
        width: 0,
        height: 0,
        configured: false,
        menu,
        options,
        theme,
        edge,
        anchor,
        popup: None,
        flyouts: Vec::new(),
        keyboard_focus: false,
        had_focus: false,
        activated: None,
        done: false,
        needs_redraw: false,
        a11y,
    };

    // DEVICE-VERIFY(0.11.1): the live surface/input loop on a real compositor.
    while !state.done {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|e| Error::Platform(format!("Wayland dispatch failed: {e}")))?;
        if state.needs_redraw {
            state.needs_redraw = false;
            state.draw(&qh)?;
        }
    }
    Ok(state.activated)
}

/// One rendered panel ready to composite into the output buffer: its device-pixel
/// top-left, size, and premultiplied-RGBA pixels (copied out so the composite step
/// doesn't hold a borrow on the panel while it also borrows the `wl_shm` canvas).
struct Rendered {
    ox: i32,
    oy: i32,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// A live popup or flyout panel: the drawer that paints it, its current layout +
/// hover, and its top-left origin in logical screen coordinates. Unlike the X11
/// [`Panel`](super::x11), it owns no window — every panel is composited into the one
/// full-output overlay surface.
struct Panel {
    drawer: RasterDrawer,
    laid: LaidMenu,
    origin: LogicalPoint,
    hovered: Option<usize>,
}

/// One open flyout level: the parent row it opened from (within its parent level's
/// menu) and its panel.
struct Flyout {
    parent: usize,
    panel: Panel,
}

/// The sctk event-loop state for one popup session. Owns everything the handlers
/// touch; must be `'static` (sctk stores dispatch state behind type-erased,
/// `'static` object data), which is why the activated id is collected here and
/// dispatched by the caller after the loop.
struct PopupState {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    /// Allocated on the first configure, once the output size is known.
    pool: Option<SlotPool>,
    layer: LayerSurface,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Output size in device pixels (from the layer-surface configure).
    width: u32,
    height: u32,
    configured: bool,
    menu: Menu,
    options: MenuOptions,
    theme: Theme,
    edge: Edge,
    anchor: LogicalRect,
    popup: Option<Panel>,
    flyouts: Vec<Flyout>,
    keyboard_focus: bool,
    /// Whether keyboard focus was ever held (so a `leave` before any `enter` — e.g.
    /// during map — doesn't dismiss the popup).
    had_focus: bool,
    /// The row id activated by a click / Enter; dispatched after the loop.
    activated: Option<MenuId>,
    done: bool,
    needs_redraw: bool,
    a11y: Option<super::a11y::PopupA11y>,
}

impl PopupState {
    /// The whole-output work area in logical pixels (origin at 0,0).
    fn work_area(&self) -> LogicalRect {
        LogicalRect::new(
            LogicalPoint::new(0.0, 0.0),
            LogicalSize::new(self.width as f32 / SCALE, self.height as f32 / SCALE),
        )
    }

    // -- open / placement ----------------------------------------------------

    /// Render + place the top-level popup at [`Self::anchor`]. Called once the
    /// output size is known (first configure).
    fn open_popup(&mut self) {
        let mut drawer = RasterDrawer::for_menu_options(SCALE, &self.options);
        let laid = render_menu(&mut drawer, &self.menu, &self.theme, &self.options, None);
        let origin = crate::anchor::place_popup(
            self.anchor,
            laid.size,
            self.work_area(),
            self.edge,
            POPUP_GAP,
        );
        self.popup = Some(Panel {
            drawer,
            laid,
            origin,
            hovered: None,
        });
        self.needs_redraw = true;
    }

    // -- present -------------------------------------------------------------

    /// Composite every live panel into a fresh `wl_shm` `Argb8888` buffer (transparent
    /// elsewhere) and present it.
    fn draw(&mut self, _qh: &QueueHandle<Self>) -> Result<()> {
        if !self.configured {
            return Ok(());
        }
        let (ow, oh) = (self.width, self.height);
        if ow == 0 || oh == 0 {
            return Ok(());
        }

        // Render all panels first (each borrows `self` mutably in turn); collect the
        // pixels so the composite step below can borrow the `wl_shm` canvas freely.
        let mut rendered: Vec<Rendered> = Vec::with_capacity(1 + self.flyouts.len());
        for level in 0..=self.flyouts.len() {
            if let Some(r) = self.render_level(level) {
                rendered.push(r);
            }
        }

        let stride = ow as i32 * 4;
        let pool = self
            .pool
            .as_mut()
            .ok_or_else(|| Error::Platform("wl_shm pool missing".into()))?;
        let (buffer, canvas) = pool
            .create_buffer(ow as i32, oh as i32, stride, wl_shm::Format::Argb8888)
            .map_err(|e| Error::Platform(format!("wl_shm create_buffer failed: {e}")))?;
        // Transparent everywhere; the compositor composites the ARGB overlay.
        canvas.fill(0);
        for r in &rendered {
            composite_panel(canvas, ow, oh, r);
        }

        let surface = self.layer.wl_surface();
        surface.damage_buffer(0, 0, ow as i32, oh as i32);
        buffer
            .attach_to(surface)
            .map_err(|e| Error::Platform(format!("wl_shm attach failed: {e}")))?;
        self.layer.commit();
        Ok(())
    }

    /// Re-render menu level `level` (0 = popup) from its hovered row and copy its
    /// pixels + device origin out for compositing.
    fn render_level(&mut self, level: usize) -> Option<Rendered> {
        // Borrow the level's menu out of `self.menu` (the `descend` iterator borrows
        // `self.flyouts` only until it returns), then take the panel from the
        // disjoint `self.popup`/`self.flyouts` field.
        let menu = crate::menu::descend(
            &self.menu,
            self.flyouts.iter().take(level).map(|f| f.parent),
        )?;
        let theme = self.theme.clone();
        let options = self.options.clone();
        let panel = if level == 0 {
            self.popup.as_mut()?
        } else {
            &mut self.flyouts.get_mut(level - 1)?.panel
        };
        let laid = render_menu(&mut panel.drawer, menu, &theme, &options, panel.hovered);
        panel.laid = laid;
        let origin = panel.origin;
        let fb = panel.drawer.framebuffer();
        let (w, h) = (fb.width(), fb.height());
        let rgba = fb.pixels().to_vec();
        let (ox, oy) = to_device(origin);
        Some(Rendered { ox, oy, w, h, rgba })
    }

    // -- pointer -------------------------------------------------------------

    fn on_motion(&mut self, pt: LogicalPoint) {
        let target = self.hover_target(pt);
        let mut dirty = false;
        if let Some((level, local)) = self.panel_at(pt) {
            if let Some(panel) = self.panel_mut(level) {
                let h = panel.laid.hit(local);
                if h != panel.hovered {
                    panel.hovered = h;
                    dirty = true;
                }
            }
        }
        let next = next_flyout(&self.flyout_stack(), target);
        let changed = self.apply_flyout_stack(&next);
        if dirty || changed {
            self.needs_redraw = true;
            self.sync_a11y_focus();
        }
    }

    fn on_click(&mut self, pt: LogicalPoint) {
        let Some((level, local)) = self.panel_at(pt) else {
            // Click outside every panel: dismiss without dispatch.
            self.done = true;
            return;
        };
        let Some(menu) = self.menu_at_level(level) else {
            return;
        };
        let (hit, id) = {
            let Some(panel) = self.panel_ref(level) else {
                return;
            };
            (panel.laid.hit(local), panel.laid.id_at(local))
        };
        if let Some(i) = hit {
            if matches!(menu.items.get(i), Some(Item::Submenu { .. })) {
                self.truncate_flyouts(level);
                self.push_flyout(i);
                return;
            }
        }
        if let Some(id) = id {
            if !id.is_none() {
                self.activated = Some(id);
            }
            self.done = true;
        }
    }

    // -- keyboard ------------------------------------------------------------

    fn on_key(&mut self, key: NavKey) {
        let mut focus = self.current_focus();
        let action = handle_key(&self.menu, &mut focus, key);
        if let Some(popup) = self.popup.as_mut() {
            popup.hovered = focus.top;
        }
        match action {
            NavAction::None => return,
            NavAction::Redraw => {}
            NavAction::OpenFlyout(i) => self.push_flyout(i),
            NavAction::CloseFlyout => {
                let keep = self.flyouts.len().saturating_sub(1);
                self.truncate_flyouts(keep);
            }
            NavAction::Activate(id) => {
                if !id.is_none() {
                    self.activated = Some(id);
                }
                self.done = true;
                return;
            }
            NavAction::CloseAll => {
                self.done = true;
                return;
            }
        }
        for (k, f) in self.flyouts.iter_mut().enumerate() {
            if let Some(ff) = focus.flyout.get(k) {
                f.panel.hovered = ff.child;
            }
        }
        self.needs_redraw = true;
        self.sync_a11y_focus();
    }

    // -- flyout stack --------------------------------------------------------

    /// Reconcile the open flyout stack to `target`; returns whether it changed.
    fn apply_flyout_stack(&mut self, target: &[usize]) -> bool {
        let mut common = 0;
        while common < target.len()
            && common < self.flyouts.len()
            && self.flyouts[common].parent == target[common]
        {
            common += 1;
        }
        let mut changed = self.flyouts.len() != common;
        self.truncate_flyouts(common);
        for &parent in &target[common..] {
            self.push_flyout(parent);
            changed = true;
        }
        changed
    }

    /// Open a flyout for row `parent_index` of the currently deepest open level.
    fn push_flyout(&mut self, parent_index: usize) {
        let depth = self.flyouts.len();
        let Some(parent_menu) = self.menu_at_level(depth) else {
            return;
        };
        let Some(child) = (match parent_menu.items.get(parent_index) {
            Some(Item::Submenu { menu, .. }) => Some(menu.clone()),
            _ => None,
        }) else {
            return;
        };

        let (parent_origin, parent_size, row_rect) = {
            let Some(pp) = self.panel_ref(depth) else {
                return;
            };
            let Some(rect) = pp
                .laid
                .rows
                .iter()
                .find(|r| r.index == parent_index)
                .map(|r| r.rect)
            else {
                return;
            };
            (pp.origin, pp.laid.size, rect)
        };

        let mut drawer = RasterDrawer::for_menu_options(SCALE, &self.options);
        let child_laid = render_menu(&mut drawer, &child, &self.theme, &self.options, None);
        let parent_rect = LogicalRect::new(parent_origin, parent_size);
        let placement = place_flyout(parent_rect, row_rect, child_laid.size, self.work_area());
        self.flyouts.push(Flyout {
            parent: parent_index,
            panel: Panel {
                drawer,
                laid: child_laid,
                origin: placement.origin,
                hovered: None,
            },
        });
        self.needs_redraw = true;
    }

    /// Close every flyout deeper than `len`.
    fn truncate_flyouts(&mut self, len: usize) {
        while self.flyouts.len() > len {
            self.flyouts.pop();
            self.needs_redraw = true;
        }
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

    /// The deepest panel containing `pt` (logical output coords) and the
    /// panel-local point within it.
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
            None if level > 0 => HoverTarget::Flyout,
            None => HoverTarget::OtherRow { panel: level },
        }
    }

    /// Push the current top-level focus to the AT (if any adapter is attached).
    fn sync_a11y_focus(&mut self) {
        let top = self.popup.as_ref().and_then(|p| p.hovered);
        if let Some(a) = self.a11y.as_mut() {
            a.focus_row(top);
        }
    }
}

// =============================================================================
// Free helpers (pure byte work, no Wayland handle)
// =============================================================================

/// Logical→device pixel origin (`SCALE == 1.0`, so a straight round).
fn to_device(p: LogicalPoint) -> (i32, i32) {
    ((p.x * SCALE).round() as i32, (p.y * SCALE).round() as i32)
}

/// Composite one rendered panel into the full-output canvas at its device origin,
/// converting muri's premultiplied **RGBA** to `wl_shm` `Argb8888` (BGRA in memory,
/// little-endian, alpha unchanged) via [`blit_argb8888`] per fully-in-bounds row and
/// a per-pixel path for the clipped edges.
fn composite_panel(canvas: &mut [u8], out_w: u32, out_h: u32, r: &Rendered) {
    let row_bytes = r.w as usize * 4;
    for row in 0..r.h {
        let dy = r.oy + row as i32;
        if dy < 0 || dy as u32 >= out_h {
            continue;
        }
        let src_start = (row as usize) * row_bytes;
        let src_row = &r.rgba[src_start..src_start + row_bytes];
        // Fast path: the whole row lands inside the output horizontally.
        if r.ox >= 0 && (r.ox as u32 + r.w) <= out_w {
            let dst_start = ((dy as u32 * out_w + r.ox as u32) * 4) as usize;
            let dst_row = &mut canvas[dst_start..dst_start + row_bytes];
            let _ = blit_argb8888(src_row, dst_row);
            continue;
        }
        // Clipped path: place pixel-by-pixel, skipping columns off either edge.
        for col in 0..r.w {
            let dx = r.ox + col as i32;
            if dx < 0 || dx as u32 >= out_w {
                continue;
            }
            let si = (col as usize) * 4;
            let di = ((dy as u32 * out_w + dx as u32) * 4) as usize;
            canvas[di] = src_row[si + 2]; // B
            canvas[di + 1] = src_row[si + 1]; // G
            canvas[di + 2] = src_row[si]; // R
            canvas[di + 3] = src_row[si + 3]; // A
        }
    }
}

/// Blit muri's premultiplied-RGBA framebuffer into a `wl_shm` `Argb8888`
/// (BGRA-in-memory, premultiplied) destination: a per-pixel R↔B swap, alpha copied
/// through unchanged (already premultiplied). Pure byte-shuffling — no Wayland
/// handle — so it is host-unit-testable; used by [`composite_panel`] for the common
/// unclipped-row case.
///
/// `src` is muri's framebuffer (`RasterDrawer::framebuffer().pixels()`, RGBA
/// premultiplied); `dst` is the mapped `wl_shm` `Argb8888` canvas slice. Both must be
/// `4 * width * height` bytes.
fn blit_argb8888(src: &[u8], dst: &mut [u8]) -> Result<()> {
    if src.len() != dst.len() {
        return Err(Error::Platform(format!(
            "wl_shm blit size mismatch: src {} bytes, dst {} bytes",
            src.len(),
            dst.len()
        )));
    }
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        d[0] = s[2]; // B
        d[1] = s[1]; // G
        d[2] = s[0]; // R
        d[3] = s[3]; // A (already premultiplied — no change)
    }
    Ok(())
}

/// Map a raw evdev keycode (as delivered verbatim by `wl_keyboard`) to a menu
/// [`NavKey`]. Navigation keys are layout-independent; printable type-ahead uses a
/// best-effort US-QWERTY map.
///
/// DEVICE-VERIFY(0.11.1): non-US keyboard layouts (the character map assumes
/// US-QWERTY; the navigation keys are position-stable across layouts).
fn evdev_to_navkey(code: u32) -> Option<NavKey> {
    // Linux input-event-codes.h.
    match code {
        1 => Some(NavKey::Escape),         // KEY_ESC
        28 | 96 => Some(NavKey::Activate), // KEY_ENTER / KEY_KPENTER
        57 => Some(NavKey::Activate),      // KEY_SPACE
        103 => Some(NavKey::Up),           // KEY_UP
        108 => Some(NavKey::Down),         // KEY_DOWN
        105 => Some(NavKey::Left),         // KEY_LEFT
        106 => Some(NavKey::Right),        // KEY_RIGHT
        102 => Some(NavKey::Home),         // KEY_HOME
        107 => Some(NavKey::End),          // KEY_END
        other => evdev_us_char(other).map(NavKey::Char),
    }
}

/// Best-effort US-QWERTY evdev keycode → lowercase character, for type-ahead.
fn evdev_us_char(code: u32) -> Option<char> {
    let c = match code {
        // Letters (KEY_A..), by evdev position.
        16 => 'q',
        17 => 'w',
        18 => 'e',
        19 => 'r',
        20 => 't',
        21 => 'y',
        22 => 'u',
        23 => 'i',
        24 => 'o',
        25 => 'p',
        30 => 'a',
        31 => 's',
        32 => 'd',
        33 => 'f',
        34 => 'g',
        35 => 'h',
        36 => 'j',
        37 => 'k',
        38 => 'l',
        44 => 'z',
        45 => 'x',
        46 => 'c',
        47 => 'v',
        48 => 'b',
        49 => 'n',
        50 => 'm',
        // Number row (KEY_1..KEY_0).
        2 => '1',
        3 => '2',
        4 => '3',
        5 => '4',
        6 => '5',
        7 => '6',
        8 => '7',
        9 => '8',
        10 => '9',
        11 => '0',
        _ => return None,
    };
    Some(c)
}

/// Resolve the popup theme for the current appearance (mirrors the X11 backend and
/// the other platforms; the Adwaita base is flat, but layer-shell keeps true alpha).
fn resolve_theme(options: &MenuOptions, dark: bool) -> Theme {
    let mut theme = options.theme.resolve(OsFamily::Gnome, dark);
    if options.theme.injects_system() {
        if let Some((r, g, b, a)) = super::system_accent() {
            theme.accent = Color::Rgba(r, g, b, a);
        }
    }
    theme
}

// =============================================================================
// sctk / wayland-client handler impls
// =============================================================================

impl CompositorHandler for PopupState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // DEVICE-VERIFY(0.11.1): honor per-output scale (set_buffer_scale + render at
        // that scale). SCALE is pinned to 1.0 for now (see the module note).
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for PopupState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for PopupState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.done = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // An all-edges-anchored overlay is sized to the whole output; a zero axis
        // means "you choose" — impossible here, so ignore it and wait for a real size.
        let (w, h) = configure.new_size;
        if w == 0 || h == 0 {
            return;
        }
        self.width = w;
        self.height = h;
        if self.pool.is_none() {
            let len = (w as usize * h as usize * 4).max(4);
            self.pool = SlotPool::new(len, &self.shm).ok();
        }
        let first = !self.configured;
        self.configured = true;
        if first {
            self.open_popup();
        }
    }
}

impl SeatHandler for PopupState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            // Bind the keyboard directly (not via sctk's xkbcommon-backed helper) so
            // no libxkbcommon build probe is pulled; we read raw evdev keycodes.
            self.keyboard = Some(seat.get_keyboard(qh, ()));
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            if let Ok(pointer) = self.seat_state.get_pointer(qh, &seat) {
                self.pointer = Some(pointer);
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(kbd) = self.keyboard.take() {
                kbd.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(ptr) = self.pointer.take() {
                ptr.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for PopupState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            // Only our overlay surface matters (it covers the whole output).
            if &event.surface != self.layer.wl_surface() {
                continue;
            }
            let pt = LogicalPoint::new(
                event.position.0 as f32 / SCALE,
                event.position.1 as f32 / SCALE,
            );
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.on_motion(pt);
                }
                PointerEventKind::Press { button, .. } => {
                    if button == BTN_LEFT || button == BTN_RIGHT {
                        self.on_click(pt);
                    }
                }
                PointerEventKind::Leave { .. }
                | PointerEventKind::Release { .. }
                | PointerEventKind::Axis { .. } => {}
            }
        }
    }
}

impl ShmHandler for PopupState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

// The keyboard is bound directly on the seat (bypassing sctk's xkbcommon helper),
// so muri handles `wl_keyboard` events itself: focus enter/leave (for dismiss on
// focus loss + AT focus reporting) and key presses mapped from raw evdev codes.
impl Dispatch<wl_keyboard::WlKeyboard, ()> for PopupState {
    fn event(
        this: &mut Self,
        _kbd: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => {
                if &surface == this.layer.wl_surface() {
                    this.keyboard_focus = true;
                    this.had_focus = true;
                    if let Some(a) = this.a11y.as_mut() {
                        a.set_focused(true);
                    }
                }
            }
            wl_keyboard::Event::Leave { surface, .. } => {
                if &surface == this.layer.wl_surface() {
                    this.keyboard_focus = false;
                    if let Some(a) = this.a11y.as_mut() {
                        a.set_focused(false);
                    }
                    // Focus genuinely left the popup (e.g. the user clicked another
                    // surface): dismiss without dispatch. DEVICE-VERIFY(0.11.1):
                    // enter/leave timing vs. the overlay grab differs per compositor.
                    if this.had_focus {
                        this.done = true;
                    }
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                if let Some(nav) = evdev_to_navkey(key) {
                    this.on_key(nav);
                }
            }
            _ => {}
        }
    }
}

delegate_compositor!(PopupState);
delegate_output!(PopupState);
delegate_shm!(PopupState);
delegate_seat!(PopupState);
delegate_pointer!(PopupState);
delegate_layer!(PopupState);
delegate_registry!(PopupState);

impl ProvidesRegistryState for PopupState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

#[cfg(test)]
mod tests {
    use super::{blit_argb8888, composite_panel, evdev_to_navkey, Rendered};
    use crate::keynav::NavKey;

    /// The RGBA→BGRA (Argb8888 LE) channel swap with alpha passed through.
    #[test]
    fn blit_swaps_r_and_b_and_keeps_alpha() {
        // One pixel: R=10, G=20, B=30, A=40 (premultiplied) -> B,G,R,A.
        let src = [10u8, 20, 30, 40];
        let mut dst = [0u8; 4];
        blit_argb8888(&src, &mut dst).unwrap();
        assert_eq!(dst, [30, 20, 10, 40]);
    }

    #[test]
    fn blit_rejects_size_mismatch() {
        let src = [0u8; 8];
        let mut dst = [0u8; 4];
        assert!(blit_argb8888(&src, &mut dst).is_err());
    }

    /// A 1×1 RGBA panel composited at an offset lands at the right output pixel with
    /// the channels swapped to BGRA.
    #[test]
    fn composite_places_panel_at_device_origin_in_bgra() {
        // 2×2 transparent output canvas.
        let mut canvas = vec![0u8; 2 * 2 * 4];
        let panel = Rendered {
            ox: 1,
            oy: 1,
            w: 1,
            h: 1,
            rgba: vec![10, 20, 30, 255], // R,G,B,A
        };
        composite_panel(&mut canvas, 2, 2, &panel);
        // Only the bottom-right pixel (index 3) is written, as BGRA.
        assert_eq!(&canvas[0..12], &[0u8; 12]);
        assert_eq!(&canvas[12..16], &[30, 20, 10, 255]);
    }

    /// A panel partly off the right/bottom edge is clipped, not out-of-bounds.
    #[test]
    fn composite_clips_panels_that_overflow_the_output() {
        let mut canvas = vec![0u8; 2 * 2 * 4];
        // 2×2 panel whose origin is at (1,1): only its top-left pixel is on-screen.
        let panel = Rendered {
            ox: 1,
            oy: 1,
            w: 2,
            h: 2,
            rgba: vec![
                1, 2, 3, 255, 4, 5, 6, 255, // row 0
                7, 8, 9, 255, 10, 11, 12, 255, // row 1
            ],
        };
        composite_panel(&mut canvas, 2, 2, &panel);
        // Only output pixel (1,1) is written (from the panel's (0,0) pixel).
        assert_eq!(&canvas[12..16], &[3, 2, 1, 255]);
        assert_eq!(&canvas[0..12], &[0u8; 12]);
    }

    #[test]
    fn evdev_navigation_keys_map_layout_independently() {
        assert_eq!(evdev_to_navkey(1), Some(NavKey::Escape));
        assert_eq!(evdev_to_navkey(103), Some(NavKey::Up));
        assert_eq!(evdev_to_navkey(108), Some(NavKey::Down));
        assert_eq!(evdev_to_navkey(28), Some(NavKey::Activate));
        assert_eq!(evdev_to_navkey(96), Some(NavKey::Activate));
        assert_eq!(evdev_to_navkey(30), Some(NavKey::Char('a')));
        assert_eq!(evdev_to_navkey(0), None);
    }
}
