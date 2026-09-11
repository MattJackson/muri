# Styled, self-drawn tray/context menus on Wayland — feasibility for muri

**Status:** research report (no code changes). **Date:** 2026-09-10. **Author:** research pass for muri 0.9.x.

**Scope.** muri draws its own CPU-raster menu (a premultiplied-RGBA framebuffer blitted into a borderless popup) on macOS, Windows, and X11. On X11 it positions an override-redirect `_NET_WM_WINDOW_TYPE_POPUP_MENU` window at the absolute pointer via `XQueryPointer`. This report asks: **how close can muri get a pixel-identical, self-drawn, styled tray/context menu on Wayland**, and where are the hard protocol walls. The SNI/AppIndicator *tray icon* itself (via `ksni`) already works identically on X11 and Wayland; only the *styled menu presentation* is the gap.

Throughout, "spec allows" is distinguished from "compositor actually implements in 2025/2026." Primary sources are cited inline; a consolidated list is at the end.

---

## 1. Executive answer

**Can Wayland reach "99% identical at first glance"? Only on some compositors, and never as a plug-and-play library feature.**

The blunt summary:

- **There is no portable Wayland path to a styled popup anchored to the tray icon.** Three independent, deliberate protocol facts make it impossible for a normal client: (a) no global pointer query, (b) no absolute/self positioning of surfaces, (c) no cross-process surface parenting (so you cannot make an `xdg_popup` hang off a tray icon another process owns). These are design decisions repeatedly reaffirmed upstream, not missing features.
- **The one real coordinate channel is out-of-band: the SNI `ContextMenu(x,y)` / `Activate(x,y)` D-Bus hint.** It carries the tray icon's real global screen coordinate **on KDE Plasma and on Waybar** (both compute it via `mapToGlobal` / `x_root + bar_global`), is a meaningless hint (often `0,0`) elsewhere, and is absent on GNOME.
- **Even with a good coordinate, you still need a way to put a surface there.** On **wlroots compositors and KWin**, `zwlr_layer_shell_v1` lets you place a full-output overlay surface and then spawn an `xdg_popup` at the pointer/coordinate — that is the realistic "styled menu on Wayland" path, and it can be made pixel-identical to muri's other backends. On **GNOME/Mutter there is no such protocol at all** and none is planned; the only in-process route is a GNOME Shell extension.
- **The XWayland fallback does *not* rescue this.** muri's existing override-redirect + `XQueryPointer` path is **fundamentally unreliable under XWayland** (frozen pointer when the cursor is over any non-XWayland surface; compositor-remapped/clamped/denied override-redirect placement). It is not a dependable Tier-1 on any Wayland compositor.

**The hard floor:**

| Environment | Best achievable styled-menu fidelity |
|---|---|
| **wlroots (sway, Hyprland, river, Wayfire, labwc, cosmic)** | **~99% identical** — layer-shell overlay + xdg-popup, muri blits its own framebuffer, pointer-accurate. |
| **KDE Plasma (KWin)** | **~99% identical is reachable** — KWin has layer-shell *and* real SNI coordinates; but requires the consuming app to open a muri layer surface. Native dbusmenu remains the zero-effort baseline. |
| **GNOME (Mutter)** | **Native host-rendered dbusmenu only** (Tier 3). A styled menu is impossible for a third-party client; the *only* escape is shipping a GNOME Shell extension (a separate JS product), which is not viable for a drop-in library. |

So "99% identical at first glance" is real **on wlroots and KDE**, and **unreachable on GNOME-Wayland** short of a Shell extension. That is the honest ceiling.

**Crucial framing (library, not application).** The two product-architecture articles that prompted this ("Muri Core + desktop-integration adapters", `org.muri.App` D-Bus seam, Tier-1 Shell-extension/plasmoid) model muri as an *application that owns its whole desktop integration*. muri is a **library** other apps `cargo add`. That distinction changes every tier's verdict — see §9 and §10. In short: a library can ship the **layer-shell renderer** (pure Rust, no external artifact) and the **SNI/dbusmenu fallback** it already has; it **cannot** realistically ship a GNOME Shell extension or a KDE plasmoid, because those are separately-packaged, separately-installed, per-desktop artifacts on foreign release cadences and review gates.

---

## 2. Per-compositor capability matrix

| Compositor | Base | `zwlr_layer_shell_v1`? | Usable SNI coords? | XWayland OR-positioning reliable? | Best achievable styled menu |
|---|---|---|---|---|---|
| **sway** | wlroots | ✅ reference impl | via host (waybar: yes) | ❌ documented top-left / wrong-output bugs | **99% — layer-shell + xdg-popup** |
| **Hyprland** | wlroots | ✅ | via host | ❌ weakest XWayland WM | **99% — layer-shell + xdg-popup** |
| **river** | wlroots | ✅ | via host | ❌ | 99% — layer-shell |
| **Wayfire** | wlroots | ✅ | via host | ❌ | 99% — layer-shell |
| **labwc** | wlroots | ✅ | via host | ❌ | 99% — layer-shell |
| **cosmic-comp** | Smithay | ✅ | via host | ❌ (XWayland popup loss under scaling) | 99% — layer-shell |
| **KWin / Plasma 6** | own | ✅ (mature, ~since 5.20/5.21) | ✅ **real coords** (`mapToGlobal`) | ⚠️ better than wlroots, still frozen-pointer + scaling bugs | **99% — layer-shell + real SNI coord**, else dbusmenu |
| **GNOME / Mutter** | own | ❌ **refused** (mutter#973) | ❌ (no meaningful coord) | ⚠️ "mostly works" but stale-pointer for global triggers | **Native dbusmenu only** (or Shell extension) |
| **Mir** | own | ⚠️ opt-in (`--add-wayland-extension`) | via host | — | layer-shell if enabled, else dbusmenu |

Sources for this matrix are in the per-section detail below and the source list.

---

## 3. wlr-layer-shell (`zwlr_layer_shell_v1`) — capabilities and limits (Q1)

**Exact capabilities** (protocol XML rendered at <https://wayland.app/protocols/wlr-layer-shell-unstable-v1>; wlroots header <https://emersion.pages.freedesktop.org/wlroots/wlr/types/wlr_layer_shell_v1.h.html>):

- **Four layers** (`zwlr_layer_shell_v1.layer`, z-order bottom→top): `background`(0), `bottom`(1), `top`(2), `overlay`(3). Fixed at `get_layer_surface`, changeable via `set_layer` (v2+).
- **Anchoring** (`zwlr_layer_surface_v1.anchor`, a **bitfield**): `top`(1) / `bottom`(2) / `left`(4) / `right`(8). Corners = OR of two perpendicular edges; opposite edges stretch; all four fill the output. **There is no anchor value for an arbitrary interior point.**
- **Margins**: `set_margin(top,right,bottom,left)` px. A margin on a non-anchored edge has no effect; the exclusive zone includes the margin.
- **Exclusive zone**: positive N reserves N px from the anchored edge (panels/bars); `0` = willing to be moved aside; `-1` = do not move me (fullscreen overlays/backgrounds).
- **Keyboard interactivity** (`set_keyboard_interactivity`): `none`(0) default, no focus possible; `exclusive`(1) grab-all (lock screens/launchers); `on_demand`(2, v4+) normal click-to-focus. A menu wants `on_demand` (or `exclusive` while open) so it can take keyboard nav without permanently stealing the keyboard.
- **Size**: `set_size(w,h)`; 0 in an axis = compositor chooses, but then you must anchor both edges in that axis.

**Can it position a menu at the pointer or a tray icon?**

- **Layer-shell alone: NO.** Only edge/corner anchoring + margins. There is **no request that takes an (x,y) point and no pointer-coordinate input** in the interface.
- **Indirectly: YES** — `zwlr_layer_surface_v1.get_popup(xdg_popup)` makes a layer surface the **parent of an `xdg_popup`**, positioned by `xdg_positioner` (`set_anchor_rect`, `set_anchor`, `set_gravity`, `set_constraint_adjustment`) relative to the parent layer surface (xdg-shell: <https://wayland.app/protocols/xdg-shell>; wlroots example <https://github.com/swaywm/wlroots/blob/master/examples/layer-shell.c>; gtk-layer-shell popup path <https://github.com/wmww/gtk-layer-shell/blob/master/src/xdg-popup-surface.c>).
- **The pointer-accurate recipe** (the load-bearing technique): create an `overlay`/`top` layer surface **anchored to all four edges** (covering the whole output), exclusive zone `-1`, `on_demand`/`exclusive` keyboard, with an input region only where you want clicks. Because the surface covers the output, its `wl_pointer` motion coordinates *are* output coordinates — so you now know where the pointer is. Then `get_popup` with a 1×1 anchor rect at that point → the menu appears at the cursor, with automatic on-screen flip/slide from the positioner's constraint adjustment.

  **But note the critical limit:** there is **no hook to a tray icon's location**. This recipe only knows where the pointer is *within a surface muri owns*. To place the menu at the tray icon, muri must be handed the icon coordinate out of band (the SNI hint, §5). A fire-and-forget menu with no surface under the cursor cannot discover where anything is.

**How real menu/launcher apps use it.** fuzzel, wofi, tofi, rofi-wayland, wlogout are **launchers**, not pointer-anchored context menus: they create one layer surface, center/edge-anchor it, and grab the keyboard (wofi: `location` + `xoffset/yoffset`, or `normal_window=true` to fall back to an xdg-toplevel — <https://man.archlinux.org/man/wofi.5.en>; fuzzel is a layer-shell app on the `top` layer — <https://codeberg.org/dnkl/fuzzel>). **None of the standalone launchers pop at the pointer.** Pointer-accurate menus in the wlroots world come from *panels/bars* (waybar, nwg-shell, quickshell) that own a layer surface and spawn `get_popup` context menus via the positioner — exactly the recipe above. Takeaway: a menu can be pointer-accurate **only if it is (or embeds) the layer surface that received the pointer event**.

**Compositor support (2025/2026):** sway (reference impl, <https://github.com/swaywm/wlr-protocols/pull/7>), Hyprland, river, Wayfire, labwc (<https://labwc.github.io/integration.html>), cosmic-comp (Smithay, <https://docs.rs/smithay/latest/smithay/wayland/shell/wlr_layer/index.html>) — all **yes**. **KWin — yes** (mature in Plasma 6; landed ~Plasma 5.20/5.21 late-2020/early-2021 — exact commit not pinned; Qt binding layer-shell-qt <https://invent.kde.org/plasma/layer-shell-qt>; remaining gaps tracked at <https://invent.kde.org/plasma/kwin/-/issues/229>). **Mir — opt-in** (`--add-wayland-extension zwlr_layer_shell_v1`). **Mutter/GNOME — refused** (§6). The ecosystem shorthand "everyone but GNOME" is accurate (gtk4-layer-shell README, <https://github.com/wmww/gtk4-layer-shell>).

---

## 4. xdg-shell popups (`xdg_popup` + `xdg_positioner`) (Q2)

**Can a client create a grab popup anchored to its own surface? Yes.** `xdg_surface.get_popup` + `xdg_positioner` (anchor rect, gravity, constraint adjustment) + `xdg_popup.grab(seat, serial)` is the standard menu mechanism, fully supported when the client owns both surfaces (<https://wayland.app/protocols/xdg-shell>, <https://wayland-book.com/xdg-shell-in-depth/popups.html>).

**Does it help a tray menu when muri does NOT own the tray-icon surface? No — structurally impossible.** Two spec facts combine:

1. A grab popup's parent **must be an `xdg_toplevel` or another grabbing `xdg_popup`**, and positioning is always *relative to the parent surface geometry* (<https://wayland.app/protocols/xdg-shell>). muri would first need its own mapped toplevel/layer surface; the popup is then placed relative to *that*, which has no defined spatial relationship to the tray icon.
2. **Wayland object IDs are per-connection**: "the object id is only meaningful in the context of one connection, so you cannot share an ID between processes" (<https://wayland.freedesktop.org/docs/html/ch04.html>). The tray icon's surface belongs to the panel/SNI-host process on *its* connection. The `parent` argument can only name a surface muri created. The spec text doesn't say "same client" because the object model already makes cross-process parenting unrepresentable.

**Any way to parent to the tray across processes? No.** There is no cross-process surface-parenting or handle-passing mechanism. `xdg-foreign` (`zxdg_exported`) exports a *toplevel* so another client can use it as a **parent for a transient toplevel** — not for an `xdg_popup`, and SNI panels don't export tray icons anyway. `xwayland-shell-v1` is an XWayland↔compositor-internal association, not a general adoption facility.

**Conclusion:** xdg-shell alone cannot solve tray-menu positioning. It is useful only as the *child* of a muri-owned layer surface (§3 recipe).

---

## 5. Getting a coordinate / anchor at all on Wayland (Q3)

**No Wayland input protocol exposes a global pointer position.** Core `wl_pointer` `enter`/`motion` deliver **surface-local** coordinates, and only for surfaces the client owns and the pointer is over (<https://wayland-book.com/seat/pointer.html>). `relative-pointer-unstable-v1` gives deltas, explicitly "not an absolute position" (<https://wayland.app/protocols/relative-pointer-unstable-v1>). `pointer-constraints-unstable-v1` locks/confines the pointer to your own surface; no global read (<https://wayland.app/protocols/pointer-constraints-unstable-v1>). input-method concerns text fields. This is deliberate ("Wayland blocks global cursor queries for security", per the RemoteDesktop portal docs).

**The SNI `ContextMenu(x,y)` / `Activate(x,y)` D-Bus hint is the one real coordinate channel — and it works *outside* Wayland.** The StatusNotifierItem spec defines the args as screen-coordinate *hints* (<https://specifications.freedesktop.org/status-notifier-item/latest/status-notifier-item.html>). They originate in the **host** (the panel, which knows where its own tray icon is) and arrive over D-Bus, sidestepping Wayland client isolation entirely. Per host:

- **KDE Plasma system tray — real global coords.** plasma-workspace computes the icon position with `mapToGlobal` (`popupPosition` → `mapToScene` → `mapToGlobal`) and forwards it verbatim into the D-Bus call (<https://github.com/KDE/plasma-workspace/blob/master/applets/systemtray/systemtray.cpp>, <https://github.com/KDE/plasma-workspace/blob/master/applets/systemtray/statusnotifieritemsource.cpp>). **This is the strongest positioning signal muri can get on Wayland**, and it's Plasma-specific. (Note: Plasma prefers the exported dbusmenu and only falls back to the `ContextMenu(x,y)` call when no dbusmenu is exported.)
- **Waybar — real global coords** (`ev->x_root + bar_.x_global`, etc.: <https://github.com/Alexays/Waybar/blob/master/src/modules/sni/item.cpp>), though with known off-screen/dismissal quirks (Waybar #750, #1494) because GTK-on-Wayland root coords and the bar's own global offset are themselves unreliable.
- **ksni (the crate muri uses for the item side)** passes the host's x,y through verbatim to `Tray::activate`/`context_menu` with no synthesis (<https://docs.rs/ksni/latest/ksni/trait.Tray.html>). Garbage in, garbage out — quality is 100% the host's.
- **GNOME Shell** (AppIndicator/KStatusNotifierItem extension) fetches and draws the dbusmenu itself and does not deliver a meaningful per-icon coordinate. Treat GNOME as "no usable coord".

**Is there any path to know where the tray icon is on screen? Only the SNI D-Bus hint** — real on Plasma/Waybar, absent/meaningless elsewhere. There is no Wayland-protocol path.

---

## 6. KDE Plasma vs GNOME specifics (Q4, Q5)

**KDE Plasma / KWin.**
- Plasma exposes `org_kde_plasma_shell` / `org_kde_plasma_surface` with `set_position` (global coords) and roles (panel, tooltip, appletpopup…), **but the protocol header states: "Warning! The protocol described in this file is a desktop environment implementation detail. Regular clients must not use this protocol."** (<https://wayland.app/protocols/kde-plasma-shell>). KWin honors it only for Plasma's own shell components — **not usable by muri.** `kde-plasma-window-management` reports window geometry in absolute coords but is an inspection/taskbar protocol, not a self-positioning API (<https://wayland.app/protocols/kde-plasma-window-management>).
- KWin **does** implement `zwlr_layer_shell_v1` (mature in Plasma 6), and its SNI host **does** pass real coordinates. So Plasma is meaningfully friendlier than GNOME: you can open a layer-shell overlay and anchor an xdg-popup, optionally using the real SNI coordinate. None of this grants a normal client free absolute positioning — the Plasma-only `set_position` stays off-limits.

**GNOME / Mutter.**
- Mutter **refuses layer-shell and third-party client positioning as an architectural stance**, not a backlog item. mutter#973 ("implement layer_shell protocol") is **Closed**, labeled a protocol "which may not be implemented by Mutter or exposed to all clients" (<https://gitlab.gnome.org/GNOME/mutter/-/issues/973>). Maintainer Jonas Ådahl, verbatim: *"we don't want to support arbitrary third party panels etc, as that is not how GNOME is designed to work"* and *"a GNOME Shell using a Wayland client to implement the panel etc, would not use layer shell, it'd use a private protocol that it has full control over… not intended to be a shell client that could be run on other compositors."* The parallel gnome-shell#1141 concurs.
- **Is there ANY GNOME-blessed path for a third-party styled positioned popup?** No standard-protocol path. A normal client gets only compositor-placed `xdg_toplevel`s and self-relative `xdg_popup`s. Portals do not fill the gap (no "place popup at point" / "styled anchored panel" portal; §8). Tray icons themselves require a Shell extension, and even then GNOME Shell draws the menu. **The only route to custom positioned UI on GNOME-Wayland is an in-process GNOME Shell extension** (§9). "Impossible without an extension" is accurate.

---

## 7. XWayland fallback — the important one (Q6)

**Verdict: muri's X11 override-redirect + `XQueryPointer` popup path is fundamentally unreliable under XWayland and must not be treated as a dependable Wayland tier.** Two failure modes stack:

**(a) `XQueryPointer` does not return a trustworthy global pointer position.** XWayland is itself a Wayland client, so it only learns the live pointer position while the cursor is over an XWayland-backed surface. When the pointer is over any Wayland-native surface (a panel, another toplevel, the desktop), XWayland reports a **frozen last-known** coordinate — plausible-looking but stale (confirmed on KDE/Plasma: <https://github.com/WarmUpTill/SceneSwitcher/issues/512>; Hyprland: <https://github.com/hyprwm/Hyprland/issues/5152>). For a tray menu the trigger comes from the panel (a Wayland surface muri doesn't own), so the coordinate is exactly the unreliable case. (`XWarpPointer`, the inverse, is likewise largely non-functional under XWayland — <https://bugs.freedesktop.org/show_bug.cgi?id=104426>.)

**(b) Override-redirect absolute placement is compositor-controlled and semantically remapped.** In rootless XWayland the compositor *is* the X window manager and "fakes" the global coordinate space; override-redirect windows are mapped to Wayland surfaces (often `xdg_popup`, which requires a parent). Requested root x,y is frequently clamped, re-parented to another toplevel, dropped to (0,0), or dismissed — worst under fractional scaling and multi-monitor (xwayland-satellite architecture + issue #429: <https://github.com/Supreeeme/xwayland-satellite/issues/429>; general model: <https://www.collabora.com/news-and-blog/news-and-events/a-wayland-driver-for-wine.html>).

**Per-compositor reliability:** GNOME/Mutter is *most likely to mostly work* (no layer-shell, so it exercises the XWayland tooltip/menu path heavily) but still hits the frozen-pointer problem for global triggers. KWin is generally functional but shows scaling artifacts. **sway has documented override-redirect placement bugs** (menus at top-left, wrong output — <https://github.com/swaywm/sway/issues/5247>, <https://github.com/swaywm/sway/issues/7608>). **Hyprland is weakest** for XWayland window management. COSMIC loses XWayland popups under fractional scaling entirely (<https://github.com/pop-os/cosmic-comp/issues/2349>).

**Legacy-app proxy** confirms this is broken in the wild, not theoretical: Java Swing `JPopupMenu`, Wine menus, and even Firefox's XWayland popups all ship positioning/dismissal bugs on this exact path across compositors (<https://bugzilla.mozilla.org/show_bug.cgi?id=1701877>, <https://bugzilla.mozilla.org/show_bug.cgi?id=1718507>). A hand-rolled override-redirect popup will fare no better than well-funded toolkits that fail here.

**Consequence for the "truly-unsupported set."** The hope that "XWayland shrinks unsupported to pure-Wayland-no-XWayland" does **not** hold. XWayland reuse is at best an opportunistic, per-compositor, sometimes-right behavior — acceptable only when the popup is triggered *inside muri's own X11 window* (where the local coordinate is valid). For a tray menu triggered from the panel, it is unreliable everywhere. **Do not promote it to Tier 1.**

**Test plan (per compositor: GNOME, KDE, sway, Hyprland; at 100% and 150% scale; single + dual monitor; force X path via `GDK_BACKEND=x11`/`QT_QPA_PLATFORM=xcb`):**
- *Test A — pointer accuracy:* a tiny X11 program logs `XQueryPointer(root)` on a timer; move the cursor (i) over its own window, (ii) over a Wayland panel, (iii) over another app, (iv) over empty desktop. Expected failure: coordinates track only in (i), freeze elsewhere. Compare against the true compositor cursor.
- *Test B — override-redirect placement:* create `override_redirect=True` + `_NET_WM_WINDOW_TYPE_POPUP_MENU`, `XMoveResizeWindow` to a chosen root coord, `XMapRaised`; screenshot and check whether it lands at the requested coord, at (0,0), attached elsewhere, on the wrong monitor, or is dismissed. Repeat with/without a pre-existing X toplevel, at fractional scale, and on the secondary output.
- *Test C — real-app proxy:* right-click menus of an old GTK2/Qt4 app, a Swing app, and a Wine app near screen edges and on the secondary monitor; record where menus land / whether they auto-dismiss.

---

## 8. xdg-desktop-portal (Q7)

**No portal — current or accepted-proposed as of Sept 2026 — gives an app the global pointer position or a way to draw a styled popup at an absolute point.**

- **RemoteDesktop portal**: injection only (`NotifyPointerMotion`/`NotifyPointerMotionAbsolute` *send* synthetic input); **no method reads the current pointer**, and "absolute" is relative to a screencast stream, not the desktop (<https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html>, XML: <https://github.com/flatpak/xdg-desktop-portal/blob/main/data/org.freedesktop.portal.RemoteDesktop.xml>). It also needs an interactive consent session.
- **Global cursor position portal**: requested (e.g. OBS) but **unfulfilled** — documents the capability is intentionally missing.
- **Window position portal (#980)**: proposed self-positioning; **closed as not planned**, reaffirming Wayland's refusal to let clients pick global position (<https://github.com/flatpak/xdg-desktop-portal/issues/980>).
- **Global menu portal (#946)**: about exporting a *menubar model* to a panel (macOS/Plasma global-menu style), not coordinates or popups; still "needs discussion" (<https://github.com/flatpak/xdg-desktop-portal/issues/946>).
- **Global Shortcuts / Screenshot / Inhibit**: Global Shortcuts registers hotkeys but delivers **no coordinate** on activation; Screenshot returns an image, not a live pointer or a drawable surface.

Portals are a dead end for both the coordinate and the surface-placement problem.

---

## 9. Architecture C — desktop-specific integration adapters

The user's articles propose an escalation tier *above* passive clients: per-desktop adapters that run **inside** the shell (which owns the panel button and thus the icon geometry), talking to the core over D-Bus (`org.muri.App`). Assessed honestly, through the **library** lens:

### 9a. GNOME Shell extension adapter

An extension runs as **GJS/JavaScript inside gnome-shell**, drawing **St (Shell Toolkit)** actors on Clutter/Cogl (not GTK, no surface of its own). It owns a panel button, knows the icon geometry, and can create a `PopupMenu` itself — **the only way to get a truly shell-attached custom popup on GNOME-Wayland.**

- **Can it blit muri's RGBA framebuffer, or must UI be rebuilt in St widgets?** It *can* blit a raster. The current path (GNOME 45+, since `Clutter.Image` was removed) is **`St.ImageContent`**, which implements `Clutter.Content` and has `set_bytes()` accepting raw pixel data — in GNOME 48: `set_bytes(cogl_context, GLib.Bytes, Cogl.PixelFormat.RGBA_8888, width, height, rowstride)`, with `cogl_context` from `global.stage.context.get_backend().get_cogl_context()` (<https://gjs.guide/extensions/upgrading/gnome-shell-48.html>, <https://gnome.pages.gitlab.gnome.org/gnome-shell/st/class.ImageContent.html>). Alternatively `St.DrawingArea` gives a Cairo context per repaint. So muri's framebuffer *can* be shown as Clutter content — **but** you then lose native theming, HiDPI scaling, AT-SPI a11y, and St's input hit-testing, and must re-implement pointer hit-testing over the raster plus manage Cogl/Cairo memory. Normal extensions instead build menus from St widgets (`PopupMenu.PopupMenuItem`, `PopupSwitchMenuItem`, …) — <https://gjs.guide/extensions/topics/popup-menu.html>.
- **Distribution / version friction (the real cost):** `metadata.json` `"shell-version"` is an array of supported GNOME majors; the shell **refuses to load** an extension whose list omits the running version (<https://gjs.guide/extensions/overview/anatomy.html>), and the GJS API **breaks roughly every 6-month release**, forcing per-release porting. extensions.gnome.org is **manual human review** of the JS on every submitted version, and as of 2026 explicitly rejects AI-generated "slop" code (<https://gjs.guide/extensions/review-guidelines/review-guidelines.html>, <https://blogs.gnome.org/jrahmatzadeh/2026/07/27/ego-ai-reference/>).
- **Realistic for a library? No.** The extension is unavoidably a **separate JavaScript product** that talks to muri over D-Bus; a Rust crate cannot embed it. muri would have to maintain and independently version a JS codebase, re-port it every 6 months (or be silently disabled by the shell-version gate), and pass manual EGO review on each update — a human-in-the-loop cadence a library can't automate. It also inverts the value proposition: GNOME users would have to separately install, enable, and version-match a Shell extension instead of just `cargo add muri`.

### 9b. KDE / Plasma adapter (plasmoid)

- A **plasmoid** is a QML/Kirigami package placed in the system-tray containment (`compactRepresentation` icon + `fullRepresentation` popup). It **can** host custom UI, but **only authored in QML/Kirigami** — you cannot embed a foreign `wl_surface`. An app-provided raster can only be shown by handing it to QML as an `Image` source (file/URL/data), not as a native rendered surface (<https://develop.kde.org/docs/plasma/widget/setup/>, <https://github.com/KDE/plasma-workspace/blob/master/applets/systemtray/package/contents/ui/main.qml>).
- **Runtime injection: no.** Plasmoids are **statically installed** packages discovered by plasmashell via metadata keys and loaded at startup. There is no D-Bus/Wayland API for an external process to inject a plasmoid or attach a styled popup to its SNI entry at runtime (<https://develop.kde.org/docs/plasma/widget/properties/>).
- **KStatusNotifierItem gives no extra presentation control:** it serializes its `QMenu` over `com.canonical.dbusmenu`; the app controls structure/labels/icons/enabled-state only — not geometry, styling, or custom widgets (<https://api.kde.org/kstatusnotifieritem.html>). (This dbusmenu-is-not-expressive-enough limit is exactly why compositor-specific rich-tray proposals like `jay_tray_v1` exist — <https://github.com/swaywm/sway/issues/8489>.)
- **Realistic for a library? No** — same reasoning as the extension: a plasmoid is a separately-packaged, separately-installed, Plasma-only QML component tied to the applet lifecycle; it can't be emitted by a linked crate, and its UI must be reimplemented in QML with state marshalled across a process boundary.

### 9c. wlroots adapter (layer-shell)

This is the one adapter a library *can* ship, because it needs no external artifact — just Wayland client code linked into muri. **But it explicitly cannot attach to a foreign tray icon's geometry** (§3–§5): only self-owned surfaces are positionable, and there is no query for another process's icon location. Quantified fidelity:

- **Pointer-anchored context menus (`ContextMenu::open_at`): yes, ~99% identical** via the full-output overlay + xdg-popup recipe — muri owns the surface that receives the pointer, so it can place the menu exactly.
- **Tray-icon-anchored menu: only approximate.** Without a coordinate you can anchor to the screen corner nearest the tray (a heuristic overlay). *With* the SNI coordinate (Plasma/Waybar), you can anchor a full-output overlay and place the popup at the reported point — good, but a heuristic, not a real parented popup, and it depends on the host supplying a real coordinate.

---

## 10. Tiered recommendation for muri

Ordered by fidelity, filtered through "what a **library** can actually ship."

**Tier 0 — SNI + native dbusmenu (today's fallback). Keep as the universal baseline.** Zero external artifacts, works on every SNI host including GNOME, free AT-SPI accessibility. This is `muri`'s current Wayland path and it is *correct*. For the **menu case** (rows/checks/radios/submenus) this is genuinely fine — see §11.

**Tier 1 (new, library-shippable) — wlr-layer-shell styled popup on layer-shell compositors.** On wlroots + KWin, open a muri-owned `overlay` layer surface (all-edges anchor, `exclusive_zone(-1)`, `on_demand` keyboard, empty input region except the menu), receive `wl_pointer` motion to get output coordinates, and spawn an `xdg_popup` at the pointer (for `ContextMenu::open_at`) or at the SNI-reported coordinate (for a tray-anchored menu on Plasma/Waybar). Blit muri's existing `RasterDrawer` framebuffer via `wl_shm` (§12). **This is the path to 99% identity on wlroots and KDE.** Pure Rust, no companion package.

**Tier 2 — do NOT rely on XWayland override-redirect reuse.** Per §7 it is unreliable for panel-triggered menus on every compositor. Optionally keep it *only* for muri-owned-window-triggered popups where the local pointer is valid, gated behind explicit detection, and never as the primary tray path. Realistically, prefer Tier 1 where layer-shell exists and Tier 0 otherwise.

**Tier 3 — native dbusmenu fallback on GNOME (and anywhere without layer-shell).** Unavoidable on GNOME-Wayland. A styled menu there requires a Shell extension, which is **out of scope for a library** (§9a).

**Architecture C (Shell extension / plasmoid) — document as "app-owned, not library-owned."** Not something muri ships; if a consuming *application* wants pixel-perfect GNOME/KDE integration, *it* provides and installs the extension/plasmoid and talks to muri's model. muri can facilitate by exposing a clean model + D-Bus seam (below), but must not promise to ship these artifacts.

**Runtime detection strategy:**
1. If `$WAYLAND_DISPLAY` is unset and `$DISPLAY` is set → real X11 → existing override-redirect path (reliable). 
2. If `$WAYLAND_DISPLAY` is set → Wayland. Detect compositor via `$XDG_CURRENT_DESKTOP` / `$XDG_SESSION_DESKTOP` and, more robustly, by **binding `zwlr_layer_shell_v1` from the registry**: if the global is advertised → Tier 1 (layer-shell) is available; if not (GNOME) → Tier 0/3 dbusmenu. 
3. For tray-anchored placement, use the SNI `context_menu`/`activate` x,y only when non-zero and sane; otherwise fall back to corner-anchor or a centered surface.
4. Never fabricate a coordinate; when none is available and layer-shell is absent, honestly return the existing `Unsupported::ClientPositioning` and let the dbusmenu tray carry the interaction.

**Is the `org.muri.App` D-Bus IPC split worth adopting as muri's internal seam?** Partially. muri does **not** need a cross-process D-Bus seam for its own rendering — Tier 1 is in-process Wayland client code. The valuable idea to borrow is the **model/presenter separation**: keep the menu model + `RasterDrawer` decoupled from a "presenter" that knows how to place pixels (X11 override-redirect, Wayland layer-shell, or — for an app that chose Architecture C — a D-Bus bridge to its own extension). That seam is worth having internally so a consuming app *can* plug in a desktop adapter, without muri committing to ship one. Adopting a literal D-Bus protocol now (when only Tier 0/1 ship) would be premature; adopting the **trait boundary** is not.

### What a library can ship vs. what the consuming app must provide

| Capability | GNOME-Wayland | KDE Plasma-Wayland | wlroots |
|---|---|---|---|
| **SNI tray icon + native dbusmenu** | ✅ library (ksni) | ✅ library | ✅ library |
| **Styled pointer-anchored popup (`open_at`)** | ❌ impossible for a client | ✅ **library** (layer-shell + xdg-popup) | ✅ **library** (layer-shell + xdg-popup) |
| **Styled *tray-anchored* menu (at the icon)** | ❌ impossible | ⚠️ **library**, using real SNI coord (approx via overlay) | ⚠️ **library**, only if a coord is available (else corner heuristic) |
| **Pixel-perfect shell-attached tray popup** | app-provided **GNOME Shell extension** (JS + D-Bus) | app-provided **plasmoid** (QML, installed) | ✅ library (layer-shell is native) |
| **Accessibility of a self-drawn menu** | ⚠️ library via `accesskit_unix` (no window bounds) | ⚠️ same | ⚠️ same |

**One-line synthesis:** a library can ship (1) the dbusmenu tray everywhere and (2) the layer-shell styled renderer on wlroots+KDE. Anything shell-attached on GNOME/KDE is an **app-owned** artifact. muri should build (2), keep (1), and expose a presenter seam for apps that want Architecture C — not attempt to ship extensions/plasmoids itself.

---

## 11. Is chasing custom Wayland popups even in scope for a *menu* library?

Distinguish two cases the articles conflate:

- **(a) The menu case** — rows, checks, radios, submenus, accelerators. Here **native dbusmenu on Wayland is genuinely fine**: it renders with the desktop's own fonts/theme, is accessible for free over AT-SPI, and every comparable project (below) uses it. The only thing lost is *muri's* specific theme/font styling on the tray menu — a cosmetic delta, not a functional one. For a menu library, Tier 0 is a defensible, honest default and arguably the *right* one on GNOME/KDE tray menus.
- **(b) The arbitrary-custom-UI case** — graphs, navigation, async content, sliders. This is where the hard anchoring/positioning wall actually bites, and it is *not* what a menu library does. Volume-slider-style richness is exactly what dbusmenu can't express and what drives compositor-specific proposals like `jay_tray_v1` (<https://github.com/swaywm/sway/issues/8489>).

**Recommendation:** muri should pursue the **styled pointer-anchored `ContextMenu::open_at` popup** on Wayland (Tier 1 layer-shell) because that is a genuine menu use case where muri already owns the surface and can be pixel-identical. It should **not** try to force a styled *tray*-anchored menu on GNOME, nor chase arbitrary custom popup UI — those are out of scope for a menu library and blocked by protocol walls. Treating the tray menu as "native dbusmenu, host-styled" is the correct, honest posture; the styled path is for the pointer popup.

### What comparable projects do (Q10)

**None self-draw a styled tray menu on Wayland — they all fall back to host-rendered `com.canonical.dbusmenu` via SNI.** This is structural: Wayland removed XEmbed (the only mechanism that ever let an app reparent its drawn window into the tray), and SNI+dbusmenu is deliberately a *data* protocol so the shell can draw menus consistently regardless of toolkit (dbusmenu co-author, <https://agateau.com/2009/statusnotifieritem-and-dbusmenu/>).

- **Electron** — Tray uses SNI + dbusmenu model; menu is host-drawn; frequently invisible on GNOME without the AppIndicator extension; 2026 regression broke SNI trays on GNOME 50/Wayland (electron#52674).
- **Qt `QSystemTrayIcon`** — since Qt 5.2 serializes the `QMenu` over dbusmenu "instead of being rendered directly in an XEmbed window"; the QMenu's QStyle/stylesheet is **discarded** on the SNI path (<https://code.qt.io/cgit/qt/qtbase.git/commit/?h=v5.2.1&id=38abd653774aa0b3c5cdfd9a8b78619605230726>).
- **libayatana-appindicator** — self-described "DBusMenu-based, OBSOLETE"; panel owns rendering.
- **Tauri / `tray-icon` / `muda`** — menus are item models; two Linux backends (libappindicator or `ksni`), both host-rendered; no menu-styling API.
- **ksni** — implements `org.kde.StatusNotifierItem` + `com.canonical.dbusmenu`; you build a `Vec<MenuItem>` model; **no drawing callback, no color/font/CSS**.

The only "self-drawn" escape hatch anyone uses is **non-tray**: an app-drawn wlr-layer-shell popup — which is exactly muri's Tier 1, and which does not work on GNOME.

---

## 12. Rust crate choices + implementation sketch (Q8)

**Recommended stack for Tier 1: `wayland-client 0.31` + `smithay-client-toolkit 0.21` only.** No GTK, no winit, no iced, no raw-window-handle. This mirrors sctk's own `layer_window.rs` example and is the lightest way to get a layer surface and blit an RGBA framebuffer.

| Crate | Latest | Released | Role / status |
|---|---|---|---|
| smithay-client-toolkit | 0.21.1 | 2026-07-23 | **Ergonomic layer-shell + shm.** `shell::wlr_layer` (`LayerShell`, `LayerSurface`, `Anchor`, `KeyboardInteractivity`) + `shm::slot::SlotPool`. Active. |
| wayland-client | 0.31.15 | 2026-07-22 | Core. `wl_shm::Format::Argb8888`. Active. |
| wayland-protocols-wlr | 0.3.12 | 2026-03-31 | Raw `zwlr_layer_shell_v1` bindings (if you skip sctk). Active. |
| gtk4-layer-shell | 0.8.1 | 2026-08-10 | Works but pulls the whole GTK4 stack — **avoid** for a blit. |
| gtk-layer-shell (GTK3) | 0.8.2 | 2024-12-09 | Stale. |
| iced_layershell | 0.19.1 | 2026-07-12 | iced-on-layer-shell (waycrate), built on sctk. Overkill for a blit. |
| winit | 0.30.13 | 2026-03-02 | **No layer-shell** (feature request #2582 still open) — not usable here. |
| raw-window-handle | 0.6.2 | 2024-05-17 | Only needed to hand a surface to a GPU renderer; **not needed for wl_shm blit.** |

**Pixel format — confirmed match to muri's framebuffer.** `wl_shm::Format::Argb8888` is "32-bit ARGB, [31:0] A:R:G:B, little endian" → **in-memory byte order `[B,G,R,A]`, premultiplied alpha** (<https://docs.rs/wayland-client/latest/wayland_client/protocol/wl_shm/enum.Format.html>). muri's `RasterDrawer::framebuffer()` returns **premultiplied RGBA device pixels** (`src/render/mod.rs:389`). So the only per-pixel work is an **R↔B swap** (RGBA→BGRA); alpha is already premultiplied and needs no change — simpler than the existing X11 `encode_framebuffer` path (`src/platform/linux/x11.rs`), which composites over an opaque background and packs to the server visual. On layer-shell muri can keep true alpha (rounded corners / shadow) since the compositor composites the ARGB surface.

**Blit sketch** (sctk; mirrors `src/platform/linux/x11.rs::present` but into a `wl_shm` slot):

```rust
// Cargo: wayland-client = "0.31", smithay-client-toolkit = "0.21"
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerSurface, Layer, Anchor, KeyboardInteractivity,
    LayerSurfaceConfigure, LayerShellHandler,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler, slot::SlotPool};
use wayland_client::protocol::wl_shm;

let shm = Shm::bind(&globals, &qh).expect("wl_shm");
let layer_shell = LayerShell::bind(&globals, &qh).expect("zwlr_layer_shell_v1"); // absent on GNOME -> Tier 0

// Full-output overlay so wl_pointer motion == output coords (pointer-accurate recipe):
let surface = compositor.create_surface(&qh);
let layer = layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("muri-menu"), None);
layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::BOTTOM | Anchor::RIGHT);
layer.set_exclusive_zone(-1);
layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
layer.commit();

impl LayerShellHandler for App {
    fn configure(&mut self, _c, _qh, layer: &LayerSurface, cfg: LayerSurfaceConfigure, _serial) {
        // Render muri's menu framebuffer at the size we want (menu panel, not full output),
        // then draw it into a child xdg_popup created via layer.get_popup(...) anchored at the
        // pointer/SNI coordinate. For the panel buffer:
        let (w, h) = (self.menu_w as i32, self.menu_h as i32);
        let stride = w * 4;
        let mut pool = SlotPool::new((w * h * 4) as usize, &self.shm).unwrap();
        let (buffer, canvas) = pool.create_buffer(w, h, stride, wl_shm::Format::Argb8888).unwrap();

        // muri framebuffer is premultiplied RGBA; wl_shm Argb8888 LE == BGRA in memory.
        for (dst, src) in canvas.chunks_exact_mut(4).zip(self.framebuffer.chunks_exact(4)) {
            dst[0] = src[2]; // B
            dst[1] = src[1]; // G
            dst[2] = src[0]; // R
            dst[3] = src[3]; // A (already premultiplied)
        }
        let s = /* the popup's wl_surface */;
        buffer.attach_to(s).unwrap();
        s.damage_buffer(0, 0, w, h);
        s.commit();
    }
    fn closed(&mut self, _c, _qh, _l: &LayerSurface) { /* dismiss */ }
}
impl ShmHandler for App { fn shm_state(&mut self) -> &mut Shm { &mut self.shm } }
```

Reuse from muri's existing code: the whole `RasterDrawer`/`render_menu` layout and hover/flyout state machine in `src/platform/linux/x11.rs` is display-server-agnostic; only the *transport* (X11 `PutImage` → `wl_shm` attach/damage/commit) and *positioning* (override-redirect at absolute coord → layer-surface + xdg-popup) change. Input (pointer enter/motion/button, keyboard) comes from sctk's seat handlers instead of the X11 event loop.

---

## 13. Accessibility (Q9)

**`accesskit_unix` works under Wayland and is display-server-agnostic.** It implements the AT-SPI2 D-Bus interfaces via `zbus`; **AT-SPI2 rides D-Bus, not the display protocol**, so the transport is identical under X11 or Wayland and never touches the compositor (<https://accesskit.dev/how-it-works/>, <https://wiki.linuxfoundation.org/accessibility/d-bus>). The Unix adapter depends only on `atspi`/`zbus` — no X11 or Wayland client library.

**Wayland caveats:**
1. **Window bounds are X11-only.** `Adapter::set_root_window_bounds()` is documented "only makes sense under X11" because a Wayland client can't get its window position — so absolute-screen hit-testing isn't reliable, but roles, text, focus, and actions all work (<https://github.com/AccessKit/accesskit/blob/main/adapters/unix/src/adapter.rs>).
2. Needs a running a11y D-Bus bus + an active AT (Orca); lazy init via `request_initial_tree`, no-op when nothing is listening.
3. The app must report focus (`update_window_focus_state`).
4. **Works with any surface, including a layer-shell overlay** — AccessKit never talks to the compositor; it only needs a logical `Window`-role node in its tree. Wayland is now AccessKit's reference test environment (PR #625).
5. AT-detection can regress with Orca/AT-SPI churn (e.g. #713, GNOME 49; fixed 2026-05).

Coordinated release **0.25.0 / accesskit_unix 0.23.0 (2026-08-29)**; pre-1.0 but mature (~monthly releases, adopted by egui, Slint, Vizia, Godot, GTK).

**Implication:** if muri draws its own Wayland menu (Tier 1), it *does* lose the free AT-SPI that the native dbusmenu gives for nothing, but it can regain accessibility by feeding a `TreeUpdate` (menu items as `Role::MenuItem`, etc.) to `accesskit_unix` — the same bridge it would use for the X11/macOS/Windows self-drawn menus. Note the coordinate caveat means screen-relative AT hit-testing won't be exact on Wayland. This is a real but bounded cost, and a reason to keep Tier 0 (native dbusmenu, free a11y) as the default where styling isn't essential.

---

## 14. Hard protocol walls (no engineering removes these)

1. **No global pointer position** on Wayland — surface-local only, for surfaces you own. (Deliberate, security-motivated.)
2. **No absolute/self positioning** of a client's own surfaces — the compositor places toplevels; layer-shell offers only edge/corner+margin; Plasma's `set_position` is forbidden to regular clients.
3. **No cross-process surface parenting** — object IDs are per-connection, so you cannot `xdg_popup`-parent to a tray icon another process owns. No `xdg-foreign`-style export exists for popups or tray icons.
4. **No portal** for global cursor position or positioned/styled popups — proposals closed as not-planned.
5. **GNOME/Mutter will not implement layer-shell or any public client-positioning protocol** — architectural stance, not a backlog item. On GNOME-Wayland a third-party styled positioned popup is impossible without an in-process Shell extension.
6. **dbusmenu is a data protocol** — the app hands over a menu *description*; the shell owns pixels/fonts/colors/animation. No styling passes through.
7. **XWayland does not restore absolute positioning** — the compositor fakes the X global coordinate space and remaps/denies override-redirect placement; `XQueryPointer` freezes when the cursor leaves XWayland surfaces.

Everything else (blitting a framebuffer, keyboard nav, flyouts, theming *within a muri-owned surface*, accessibility over D-Bus) is solvable engineering.

---

## Sources

Protocols & specs:
- wlr-layer-shell: <https://wayland.app/protocols/wlr-layer-shell-unstable-v1> · wlroots header <https://emersion.pages.freedesktop.org/wlroots/wlr/types/wlr_layer_shell_v1.h.html>
- xdg-shell (popups/positioner): <https://wayland.app/protocols/xdg-shell> · <https://wayland-book.com/xdg-shell-in-depth/popups.html>
- wl_pointer (surface-local): <https://wayland-book.com/seat/pointer.html> · object-id per-connection: <https://wayland.freedesktop.org/docs/html/ch04.html>
- relative-pointer: <https://wayland.app/protocols/relative-pointer-unstable-v1> · pointer-constraints: <https://wayland.app/protocols/pointer-constraints-unstable-v1>
- StatusNotifierItem: <https://specifications.freedesktop.org/status-notifier-item/latest/status-notifier-item.html> · dbusmenu rationale: <https://agateau.com/2009/statusnotifieritem-and-dbusmenu/>
- kde-plasma-shell (regular clients must not use): <https://wayland.app/protocols/kde-plasma-shell> · kde-plasma-window-management: <https://wayland.app/protocols/kde-plasma-window-management>
- xwayland-shell: <https://wayland.app/protocols/xwayland-shell-v1>

Compositor stances & support:
- Mutter refusal (#973, Ådahl quotes): <https://gitlab.gnome.org/GNOME/mutter/-/issues/973> · gnome-shell#1141: <https://gitlab.gnome.org/GNOME/gnome-shell/-/issues/1141> · GNOME protocol guidance: <https://discourse.gnome.org/t/which-protocols-to-use-supported-by-mutter/37216>
- KWin layer-shell binding: <https://invent.kde.org/plasma/layer-shell-qt> · gaps: <https://invent.kde.org/plasma/kwin/-/issues/229> · Plasma 5.20: <https://kde.org/announcements/plasma/5/5.20.0/>
- Plasma SNI coords: <https://github.com/KDE/plasma-workspace/blob/master/applets/systemtray/systemtray.cpp> · <https://github.com/KDE/plasma-workspace/blob/master/applets/systemtray/statusnotifieritemsource.cpp>
- Waybar SNI coords: <https://github.com/Alexays/Waybar/blob/master/src/modules/sni/item.cpp> · quirks <https://github.com/Alexays/Waybar/issues/750> · <https://github.com/Alexays/Waybar/issues/1494>
- "everyone but GNOME": <https://github.com/wmww/gtk4-layer-shell> · labwc: <https://labwc.github.io/integration.html> · cosmic layer-shell: <https://docs.rs/smithay/latest/smithay/wayland/shell/wlr_layer/index.html>
- Launchers: wofi <https://man.archlinux.org/man/wofi.5.en> · fuzzel <https://codeberg.org/dnkl/fuzzel>

XWayland:
- Xwayland rootless: <https://man.archlinux.org/man/Xwayland.1.en> · xwayland-satellite arch + OR bug: <https://github.com/Supreeeme/xwayland-satellite/blob/main/ARCHITECTURE.md> · <https://github.com/Supreeeme/xwayland-satellite/issues/429>
- Frozen pointer: <https://github.com/WarmUpTill/SceneSwitcher/issues/512> · <https://github.com/hyprwm/Hyprland/issues/5152> · XWarpPointer limits: <https://bugs.freedesktop.org/show_bug.cgi?id=104426>
- sway OR placement bugs: <https://github.com/swaywm/sway/issues/5247> · <https://github.com/swaywm/sway/issues/7608>
- COSMIC XWayland popup loss: <https://github.com/pop-os/cosmic-comp/issues/2349>
- Firefox popup/scaling: <https://bugzilla.mozilla.org/show_bug.cgi?id=1701877> · <https://bugzilla.mozilla.org/show_bug.cgi?id=1718507>
- Wine Wayland driver: <https://www.collabora.com/news-and-blog/news-and-events/a-wayland-driver-for-wine.html>

Portals:
- RemoteDesktop: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html> · XML <https://github.com/flatpak/xdg-desktop-portal/blob/main/data/org.freedesktop.portal.RemoteDesktop.xml>
- Window position #980 (not planned): <https://github.com/flatpak/xdg-desktop-portal/issues/980> · Global menu #946: <https://github.com/flatpak/xdg-desktop-portal/issues/946>

Architecture C:
- GNOME extension St widgets/popup: <https://gjs.guide/extensions/topics/popup-menu.html> · <https://gjs.guide/extensions/topics/st-widgets.html> · St.ImageContent raster: <https://gnome.pages.gitlab.gnome.org/gnome-shell/st/class.ImageContent.html> · GNOME 48 set_bytes: <https://gjs.guide/extensions/upgrading/gnome-shell-48.html> · St.DrawingArea: <https://gnome.pages.gitlab.gnome.org/gnome-shell/st/class.DrawingArea.html>
- metadata.json shell-version: <https://gjs.guide/extensions/overview/anatomy.html> · EGO review: <https://gjs.guide/extensions/review-guidelines/review-guidelines.html> · EGO AI ban: <https://blogs.gnome.org/jrahmatzadeh/2026/07/27/ego-ai-reference/>
- Plasmoid: <https://develop.kde.org/docs/plasma/widget/setup/> · <https://develop.kde.org/docs/plasma/widget/properties/> · systray main.qml <https://github.com/KDE/plasma-workspace/blob/master/applets/systemtray/package/contents/ui/main.qml> · KStatusNotifierItem <https://api.kde.org/kstatusnotifieritem.html> · jay_tray_v1 rationale <https://github.com/swaywm/sway/issues/8489>

Rust ecosystem & a11y:
- smithay-client-toolkit: <https://crates.io/crates/smithay-client-toolkit> · layer_shell docs <https://docs.rs/smithay-client-toolkit/latest/smithay_client_toolkit/shell/wlr_layer/index.html> · SlotPool <https://docs.rs/smithay-client-toolkit/latest/smithay_client_toolkit/shm/slot/struct.SlotPool.html>
- wl_shm Argb8888 format: <https://docs.rs/wayland-client/latest/wayland_client/protocol/wl_shm/enum.Format.html>
- winit no layer-shell (#2582): <https://github.com/rust-windowing/winit/issues/2582> · iced_layershell: <https://crates.io/crates/iced_layershell>
- ksni: <https://docs.rs/ksni/latest/ksni/trait.Tray.html> · <https://github.com/iovxw/ksni>
- AccessKit how-it-works: <https://accesskit.dev/how-it-works/> · Unix adapter (window-bounds caveat): <https://github.com/AccessKit/accesskit/blob/main/adapters/unix/src/adapter.rs> · AT-SPI over D-Bus: <https://wiki.linuxfoundation.org/accessibility/d-bus>

Comparable projects:
- Qt SNI/dbusmenu commit: <https://code.qt.io/cgit/qt/qtbase.git/commit/?h=v5.2.1&id=38abd653774aa0b3c5cdfd9a8b78619605230726> · QSystemTrayIcon: <https://doc.qt.io/qt-6/qsystemtrayicon.html>
- Electron Tray: <https://www.electronjs.org/docs/latest/api/tray> · GNOME/Wayland regression: <https://github.com/electron/electron/issues/52674>
- libayatana-appindicator (obsolete/dbusmenu): <https://github.com/AyatanaIndicators/libayatana-appindicator>
- tray-icon: <https://github.com/tauri-apps/tray-icon> · Tauri ksni feature: <https://github.com/tauri-apps/tauri/issues/11293>

*Sourcing note:* several confirmations (KDE/Waybar source lines, wayland.app protocol renderings, some GitHub issues) were read via fetch-summaries of primary pages; the load-bearing facts (Plasma `mapToGlobal`→SNI chain, Mutter #973 refusal quotes, XWayland frozen-pointer + OR-remap, wl_shm Argb8888 byte order, layer-shell edge-only anchoring) are drawn from primary sources and cross-checked. A precise KWin layer-shell landing version (~5.20/5.21) could not be pinned to a commit and is marked approximate.
