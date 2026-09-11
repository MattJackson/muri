# ADR-0003 — Linux styled tray menu: presenter seam, X11 custom popup, Wayland layer-shell scaffold

**Status:** Accepted (0.11.0). Partially implemented on the macOS host; the Wayland
renderer and the accessibility wiring are **device-side** tasks (see the phased
checklist at the end).

**Context.** Through 0.10.x the Linux tray menu was *always* the native, host-drawn
`com.canonical.dbusmenu` (SNI), and the only self-drawn styled surface was the X11
override-redirect `ContextMenu::open_at` popup. The feasibility study
(`docs/research/wayland-styled-menu.md`) establishes that a **pixel-identical,
self-drawn** tray/context menu is reachable on X11, on wlroots compositors, and on
KWin/Plasma — but is **impossible for a client on GNOME/Mutter** (layer-shell
refused as policy, mutter#973) and requires no companion artifact only for the
X11/wlroots/KWin cases. 0.11.0's scope decision: *"all but GNOME working custom."*

This ADR records the architecture that lets the tray backend pick, at runtime, the
best presenter for the current session while keeping ADR-0002 intact (all
`cfg(target_os)` stays under `src/platform/`) and keeping the macOS
`--all-features` gate green (no Linux-only Wayland deps yet).

## Decision

### 1. A presenter seam (`LinuxMenuPresenter`)

`src/platform/linux.rs` defines an enum with one variant per strategy:

| Variant | Surface | Where | Accessibility |
|---|---|---|---|
| `NativeDbusMenu` | host-drawn `com.canonical.dbusmenu` (`sni` module) | GNOME + universal fallback | free (AT-SPI, host draws it) |
| `X11Popup` | muri's override-redirect styled popup at the pointer (`x11` module) | real X11 session | via `accesskit_unix` (device-side) |
| `WaylandLayerShell` | muri's `wlr-layer-shell` styled popup (`wayland` module) | wlroots + KWin/Plasma | via `accesskit_unix` (device-side) |

The SNI adapter (`MuriSni`) carries the selected presenter and routes accordingly:
`menu()` returns an empty dbusmenu when `X11Popup` is active (so muri's own popup is
the only surface), and the native tree otherwise; `activate()` opens muri's own
styled popup for `X11Popup` and is a no-op (host draws the menu) otherwise.

### 2. Runtime detection (`detect_linux_presenter`)

Cheap, non-blocking, never-panicking **env logic** (implemented for real):

1. `$WAYLAND_DISPLAY` set → Wayland.
   - Desktop is GNOME/Mutter (`$XDG_CURRENT_DESKTOP` / `$XDG_SESSION_DESKTOP` /
     `$DESKTOP_SESSION` contains `gnome`, case-insensitive) → `NativeDbusMenu`
     (Mutter refuses layer-shell — research §6).
   - Else, when the `wayland-styled` scaffold is compiled **and** the env names a
     known layer-shell desktop (sway/Hyprland/river/Wayfire/labwc/cosmic/KDE/Plasma)
     → `WaylandLayerShell`. Without the feature (today's default) → `NativeDbusMenu`.
2. Else `$DISPLAY` set → real X11 → `X11Popup` when `x11-popup` is compiled, else
   `NativeDbusMenu`.
3. Else (headless/CI) → `NativeDbusMenu`.

The **authoritative** layer-shell test is binding `zwlr_layer_shell_v1` from the
Wayland registry (`wayland::layer_shell_available`). That needs a live `wl_display`
that the macOS gate host cannot open, so its body is `todo!("DEVICE-VERIFY: …")`
and it is **not** called from detection — detection uses the env heuristic to
*pre-select*, and the registry bind is the device-side confirmation performed
before a surface is actually created.

### 3. X11 custom popup wiring (implemented as far as safe)

`open_x11_popup_at_cursor(&Tray)` (feature `x11-popup`): ignores any host-supplied
SNI `(x,y)` (an unreliable hint, often `0,0` off KDE/Waybar — research §5), reads
the live pointer via `x11::cursor_position()` (`XQueryPointer`), builds a zero-size
anchor rect there, and routes to the existing `x11::open_popup_session` with the
tray's own menu/options and a dispatch closure over `Tray::dispatch`. This reuses
the entire existing X11 backend (no new deps) and is the same styled path
`ContextMenu::open_at` funnels into. Gated behind presenter selection, so default
behavior only changes when `X11Popup` is chosen.

**ksni 0.3.6 constraint (the load-bearing device-side caveat).** ksni derives the
SNI `ItemIsMenu` property from the *type-level* `const MENU_ON_ACTIVATE`, and:

- when `ItemIsMenu == true`, ksni answers `Activate` with `UnknownMethod` so the
  host shows the exported dbusmenu instead of calling our `activate()`;
- ksni's `ContextMenu(x,y)` D-Bus method (right-click) is a hard `UnknownMethod`
  ("please use `menu`") — muri cannot intercept it at all.

So to actually *receive* the activate for the `X11Popup` presenter, a Linux session
must select a `MENU_ON_ACTIVATE = false` adapter for that presenter. Because
`MENU_ON_ACTIVATE` is a `const`, that means either (a) a const-generic split of
`MuriSni<const MENU_ACTIVATE: bool>` threaded through the `run_sni_loop` seam (note
`spawn_tray_thread` takes a non-generic `fn` pointer, so the split must be resolved
before that boundary), or (b) upstreaming a per-instance `ItemIsMenu` + a
`context_menu` callback to ksni, or (c) dropping to a raw-`zbus` SNI implementation.
The routing itself is wired and correct the moment `activate` fires; only this
type-level switch (and the unreachable right-click) is deferred.

### 4. Wayland layer-shell (scaffold only)

`src/platform/linux/wayland.rs`, gated behind `all(unix, not(target_os = "macos"))`
(via its parent) **and** the off-by-default `wayland-styled` feature. It fixes the
function signatures + lifecycle; every function touching a live Wayland connection
has a `todo!("DEVICE-VERIFY: …")` body. The one piece that is real today is
`blit_argb8888` (the RGBA→BGRA channel swap, unit-tested), because it is pure byte
work with no Wayland handle.

**Crucially, `wayland-styled` pulls no external crates yet** — the heavy Wayland
client stack is Linux-only and would break `cargo build --all-features` on macOS.
The exact dep additions and implementation steps are the **first device task**
below and are mirrored in the research doc.

### 5. Accessibility (scaffold + plan)

The self-drawn popups lose the dbusmenu's free AT-SPI. `src/platform/linux/a11y.rs`
defines the `PopupA11y` seam (attach on open, `focus_row`, `set_focused`) as inert
no-ops. The plan (research §13): drive `accesskit_unix`, which implements AT-SPI2
over D-Bus (display-server-agnostic; never touches the compositor), feeding a
`TreeUpdate` built from the popup's laid-out rows — the same tree muri already
builds for macOS/Windows behind the `a11y` feature. Caveat: `set_root_window_bounds`
is X11-only, so absolute-screen AT hit-testing won't be exact on Wayland (roles,
text, focus, actions all work).

## Consequences

- Default behavior is unchanged unless a custom presenter is selected. GNOME-Wayland
  and every host without layer-shell keep the native dbusmenu (accessible, correct).
- No new default dependencies; `wayland-styled` is off and dep-free.
- ADR-0002 preserved: all new code is under `src/platform/`, gated by cargo
  features (not `cfg(target_os)` outside the platform module). `strict_cfg` passes.
- macOS `--all-features` build/test/clippy/doc gate stays green.

## Per-compositor behavior matrix (target end state)

| Environment | Presenter | Tray menu | `ContextMenu::open_at` |
|---|---|---|---|
| X11 (any WM) | `X11Popup` | muri styled popup at pointer | muri styled popup |
| sway/Hyprland/river/Wayfire/labwc/cosmic | `WaylandLayerShell` | muri styled popup (layer-shell + xdg-popup) | muri styled popup |
| KDE Plasma (KWin) | `WaylandLayerShell` | muri styled popup; SNI coord anchors it | muri styled popup |
| GNOME (Mutter) | `NativeDbusMenu` | host-drawn dbusmenu | `Unsupported::ClientPositioning` |
| any host w/o layer-shell | `NativeDbusMenu` | host-drawn dbusmenu | X11 popup if `$DISPLAY`, else unsupported |

## Wayland dep additions + blit sketch (first device task)

Add to the Linux target table in `Cargo.toml` and gate the deps on the feature:

```toml
[target.'cfg(all(unix, not(target_os = "macos")))'.dependencies]
wayland-client            = { version = "0.31", optional = true }
smithay-client-toolkit    = { version = "0.19", optional = true, default-features = false }
# (bump to the latest 0.21+ verified on the pinned MSRV/toolchain during device work)

[features]
wayland-styled = ["dep:wayland-client", "dep:smithay-client-toolkit"]
```

Recipe (research §3, §12): bind `wl_compositor` / `wl_shm` / `wl_seat` /
`zwlr_layer_shell_v1`; create a **full-output overlay** layer surface (all-edges
anchor, `exclusive_zone(-1)`, `KeyboardInteractivity::OnDemand`, empty input region
except under the menu) so `wl_pointer` motion == output coordinates; `get_popup` a
child `xdg_popup` positioned at the pointer/SNI point; blit muri's premultiplied
RGBA framebuffer into a `wl_shm` `Argb8888` slot via `blit_argb8888` (BGRA in
memory, alpha already premultiplied — keep true alpha for rounded corners/shadow,
unlike the opaque X11 path); drive input from the seat handlers, reusing the shared
`flyout` + `keynav` state machines. Only transport + positioning are Wayland-specific.

## Phased device-verify checklist

**Implemented here (macOS host), gate-green:**
- [x] `LinuxMenuPresenter` enum + `detect_linux_presenter` env logic.
- [x] `MuriSni` carries the presenter; `menu()` empties on `X11Popup`; `activate()`
      routes `X11Popup` to the custom popup.
- [x] `open_x11_popup_at_cursor` (cursor-anchored, reuses `x11::open_popup_session`).
- [x] `wayland` scaffold module (signatures + lifecycle + real `blit_argb8888`,
      unit-tested), behind the dep-free `wayland-styled` feature.
- [x] `a11y` (`PopupA11y`) seam (inert no-ops).
- [x] macOS `build` / `build --all-features` / `test --all-features` /
      `clippy --all-targets --all-features -D warnings` / `fmt` / `doc -D warnings`.
- [x] Linux cross-check: `cargo check`/`clippy --target x86_64-unknown-linux-gnu`
      for default, `--all-features`, `--no-default-features`, and
      `--features wayland-styled` all clean.

**Device-side (a real Linux session must finish/verify):**
- [ ] **X11:** confirm SNI-click → `XQueryPointer` → styled popup end-to-end; resolve
      the ksni `MENU_ON_ACTIVATE`/`ItemIsMenu` switch (const-generic split or ksni
      upstream / raw-zbus) so `activate` fires for `X11Popup`; decide right-click
      handling (unreachable in ksni 0.3.6).
- [ ] **Wayland deps:** add `wayland-client` + `smithay-client-toolkit` behind
      `wayland-styled` (per the toml above); confirm the macOS gate is unaffected
      (deps are Linux-target-gated).
- [ ] **Wayland renderer:** implement `wayland::layer_shell_available` (registry
      bind), `open_popup_session`, `cursor_position`; verify on sway/Hyprland + KWin
      at 100%/150% scale, single + dual monitor.
- [ ] **KDE SNI coord:** use the real `ContextMenu`/`Activate` coordinate to anchor
      the layer-shell popup on Plasma/Waybar.
- [ ] **Accessibility:** wire `accesskit_unix` in `PopupA11y` (feature `a11y`, Linux
      target); feed the row `TreeUpdate`; verify with Orca; note the Wayland
      window-bounds caveat.
- [ ] **HiDPI / work area:** replace the `x11.rs` `SCALE = 1.0` and default work-area
      with real `Xft.dpi`/RANDR and output geometry.
