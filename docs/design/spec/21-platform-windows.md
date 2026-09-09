# 21 — Platform backend: Windows (groundwork)

Status: **section spec.** Grounded in the compile-verified `src/tray/windows.rs`
(`WindowsAnchor`), the shared `src/anchor.rs`, and the macOS reference loop that
Windows must mirror (`src/tray/macos.rs`). Windows is **groundwork**: the anchor
installs a real `Shell_NotifyIcon` and reports a real rect, but there is **no live
popup event loop yet** — that, and PNG→`HICON` decode, are the net-new work
(milestone M4, doc 00 §8).

Cross-references: surfaces & `TrayHandle`
[`01-api-contract.md`](01-api-contract.md); facade `init_for_hwnd` passthrough +
`tray-icon` parity [`02-muda-compat.md`](02-muda-compat.md) §6, §8; the unified
`UserEvent` loop shape [`03-threading-events-versioning.md`](03-threading-events-versioning.md)
§2; the scene drawer [`10-rendering-layout.md`](10-rendering-layout.md); NVDA +
Narrator [`30-accessibility.md`](30-accessibility.md); input
[`40-input-interaction.md`](40-input-interaction.md).

**Feasibility read: AMBER.** The hard geometry (rect → logical → `place_popup`) is
done and the APIs are all well-trodden Win32. The risk is concentrated in **two
places that do not exist yet**: the popup event loop wiring winit↔Win32 message
handling, and the `WH_MOUSE_LL` outside-click dismiss (a global hook with real
correctness and lifetime hazards, §2). Everything else is mechanical.

---

## 1. Tray icon — `Shell_NotifyIcon` (groundwork)

### The message-only owner window

A notification-area icon must belong to an `HWND`. `WindowsAnchor::install`
(`src/tray/windows.rs`) creates a **message-only window** parented to
`HWND_MESSAGE`:

```
RegisterClassW(WNDCLASSW { lpfnWndProc: wnd_proc, hInstance, lpszClassName: "muri_tray_msgwnd", .. })  // once, guarded by CLASS_REGISTERED
CreateWindowExW(0, class, null, 0, 0,0,0,0, HWND_MESSAGE, null, hinstance, null)
```

A `HWND_MESSAGE` window is invisible, never painted, and exists solely to own the
icon and receive its callback messages — the correct idiom for a headless tray
owner. Class registration is guarded by a process-global
`CLASS_REGISTERED: AtomicBool` (`swap(true)`), so a second tray in-process does not
double-register.

### Creation

```
NOTIFYICONDATAW {
    cbSize, hWnd: self.hwnd, uID: TRAY_ICON_UID (0x0001),
    uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
    uCallbackMessage: WM_TRAY_CALLBACK (WM_APP + 1),
    hIcon: <see decode below>, szTip: <tooltip UTF-16>,
}
Shell_NotifyIconW(NIM_ADD, &nid)   // 0 ⇒ Error::Platform, and DestroyWindow the owner
```

`uCallbackMessage = WM_APP + 1` means every tray interaction is posted to
`wnd_proc` as `WM_TRAY_CALLBACK`, with the low word of `lParam` carrying the actual
mouse message. Tooltip text is copied into `szTip` as NUL-terminated UTF-16 via the
`wide()` helper, clamped to the array length.

### Icon format + decoding (`todo!()`)

Today `nid.hIcon = LoadIconW(null, IDI_APPLICATION)` — the **stock application
icon** as a visible placeholder. 1.0 must decode a muri `Icon::Png` into an
`HICON`:

- Decode the PNG to straight-alpha RGBA (the same decode the drawer's `draw_image`
  already does — reuse it, do not add an image crate twice).
- Build a 32-bit `HBITMAP` (`CreateDIBSection`, top-down BGRA) for the color plane
  plus a monochrome mask `HBITMAP`, then `CreateIconIndirect(&ICONINFO{ fIcon: 1,
  hbmColor, hbmMask })`. Delete both bitmaps after; the `HICON` owns its own copy.
- Cache by icon identity (`Arc<[u8]>` pointer) so `set_icon` on a tick does not
  re-decode.

**DPI note:** the taskbar wants a `GetSystemMetrics(SM_CXSMICON)`-sized icon (16px
at 96 dpi, scaled by the shell DPI). Decode/scale to that, and provide the larger
`GetSystemMetrics(SM_CXICON)` variant so high-DPI overflow flyouts stay crisp. Bad
bytes → `Error::BadIcon` (doc 03 §7). `Icon::Svg`/`Symbol` are not drawn (doc 01
§3, divergence D6); a `NativeIcon` (doc 02 §4.7) has no Win32 stock-icon path in a
custom surface and renders text-only until `Symbol` ships.

### Click routing (net-new)

`wnd_proc` today just calls `DefWindowProcW` — the comment notes "the tray callback
is polled elsewhere once the event loop lands." 1.0's `wnd_proc` must decode
`WM_TRAY_CALLBACK`: `LOWORD(lParam)` is the mouse message
(`WM_LBUTTONUP` / `WM_RBUTTONUP` / `WM_LBUTTONDBLCLK` / `WM_MOUSEMOVE` /
`NIN_POPUPOPEN`…). A left-up posts the equivalent of macOS's
`UserEvent::ToggleTray` into the winit loop; a right-up on modern Windows is the
gesture that opens the context menu. This is the direct analogue of macOS's
`trayClicked:` target/action bridge (doc 20 §1) — and gives the `tray-icon`
`TrayIconEvent` parity the facade needs (doc 02 §8) essentially for free, since
Win32 hands us the discrete mouse messages that macOS makes us reconstruct.

**`WM_TASKBARCREATED` (must-add for 1.0).** If Explorer restarts, all icons are
lost. A robust tray registers for the `RegisterWindowMessageW("TaskbarCreated")`
broadcast and re-adds its icon on receipt. The shipped groundwork does not; 1.0
must, or the icon vanishes permanently after an Explorer crash.

### Teardown (shipped)

`Drop for WindowsAnchor` calls `Shell_NotifyIconW(NIM_DELETE, …)` then
`DestroyWindow(hwnd)` — this is done correctly today, and is the pattern macOS still
lacks (doc 20 §1).

## 2. Popup / anchored window (net-new)

### Anchor rect (shipped)

`WindowsAnchor::anchor_rect`:

```
Shell_NotifyIconGetRect(&NOTIFYICONIDENTIFIER{ cbSize, hWnd, uID }, &mut rect)  // physical px
let scale = GetDpiForWindow(hwnd) as f32 / 96.0;   // 96 dpi == 1.0
LogicalRect { origin: (rect.left/scale, rect.top/scale), size: (w/scale, h/scale) }
```

This converts the physical-pixel shell rect to muri logical coordinates. It is
already routed through the shared placement math via `WindowsAnchor::popup_origin`:

```
place_popup(anchor, popup, work_area, Edge::Top, 2.0)
```

`Edge::Top` is correct because Windows taskbars sit at the **bottom** by default,
so the popup grows **up** from the icon (flipping down if the taskbar is at the
top). `place_popup` already handles left/right taskbars via `Edge::Left`/`Right`
(anchor.rs tests cover the flips). The work area comes from
`SystemParametersInfoW(SPI_GETWORKAREA)` on the monitor the icon sits on
(`MonitorFromRect(&rect)` + `GetMonitorInfoW`) — **not** the primary monitor; the
overflow tray can live on a secondary display.

**`Shell_NotifyIconGetRect` gotcha:** it returns the rect of the icon **only when
the icon is visible in the tray**. If the user has moved it into the *overflow*
flyout (the hidden-icons chevron), the call returns the chevron's rect (or
`E_FAIL`). 1.0 must handle the non-`S_OK` return by anchoring to the notification
overflow button or falling back to a cursor-anchored open. Concrete failure mode
to test.

### Window flags

The popup is the same borderless winit + `softbuffer` surface as macOS, but with
Windows-specific extended styles applied to the `HWND` (via `objc2`-equivalent raw
handle manipulation with `SetWindowLongPtrW`, or winit's platform extension):

- **`WS_EX_NOACTIVATE`** — the window does not take foreground activation when
  shown or clicked. This is the Windows equivalent of macOS's non-activating panel
  (doc 20 §2): the popup receives clicks without stealing focus from the user's
  foreground app, so `PredefinedMenuItem` edit actions still target the previously
  focused window (doc 02 §4.6).
- **`WS_EX_TOOLWINDOW`** — keeps the popup out of the Alt-Tab list and the taskbar.
- **`WS_EX_LAYERED`** — enables per-pixel alpha so the rounded-corner panel and
  transparency work (the drawer produces premultiplied output; use
  `UpdateLayeredWindow` or `SetLayeredWindowAttributes`). Without layering, the
  rectangle's corners are opaque black. **The per-pixel-alpha path must now
  *preserve* alpha** (not composite over an opaque theme background) so the acrylic
  backdrop below shows through — see the vibrancy requirement.
- `WS_POPUP` (not `WS_OVERLAPPED`); top-most via `SetWindowPos(HWND_TOPMOST)` or
  `WS_EX_TOPMOST`.

**Vibrancy — the Windows equivalent is ACRYLIC (1.0 requirement, decision #6).**
`Theme::native()` requires real translucent blur, not an opaque panel (macOS uses
`NSVisualEffectView`, [`20`](20-platform-macos.md); Windows uses acrylic). The
popup must request an acrylic/system backdrop so the panel reads as native menu
material: on Windows 11, `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE,
DWMSBT_TRANSIENTWINDOW)` (the transient-window backdrop is exactly the menu/flyout
material); on Windows 10, `SetWindowCompositionAttribute` with
`ACCENT_ENABLE_ACRYLICBLURBEHIND` (undocumented but the established path, with a
graceful fallback to a translucent tinted fill where unavailable). The drawer then
paints the menu content over the blurred backdrop: the panel `fill_round_rect`
background is drawn at **reduced alpha or skipped** so the acrylic shows, and the
rounded-corner region (`SetWindowRgn` / an alpha corner mask) clips the backdrop.
This means the layered-window blit must carry straight/premultiplied alpha end to
end — the opaque-composite shortcut the shipped drawer uses on macOS is not
acceptable here. Where the OS cannot provide acrylic (very old Windows, remote
sessions, transparency disabled in system settings), muri degrades to an opaque
`Theme::native()` panel and documents it as an environment limitation, not a muri
divergence.

**Keyboard-input tension (mirror of macOS §2).** `WS_EX_NOACTIVATE` means the popup
does not become the foreground window, so it does **not** receive `WM_KEYDOWN` by
default. Menu keyboard nav (`keynav`, doc 40) therefore needs one of: a thread-local
keyboard hook (`WH_KEYBOARD`), or explicitly calling `SetFocus`/`SetForegroundWindow`
on the popup while suppressing the foreground-app deactivation, or the Win32 menu
idiom of a modal message pump that reads the keyboard directly. This is the same
"non-activating but must read keys" bind macOS hits; spec it as a shared constraint
in [`40-input-interaction.md`](40-input-interaction.md).

### Outside-click dismiss — `WH_MOUSE_LL` + `WM_ACTIVATEAPP` (net-new, the hazard)

macOS dismisses on `WindowEvent::Focused(false)`. On Windows a `WS_EX_NOACTIVATE`
popup **never had focus**, so winit's `Focused(false)` is unreliable as a dismiss
signal (the module docstring says exactly this). The specified mechanism:

- **`WH_MOUSE_LL`** — a low-level mouse hook (`SetWindowsHookExW(WH_MOUSE_LL, …)`)
  observes every mouse-down system-wide while the popup is open; a click whose point
  is **outside** the popup (and outside any open flyout) dismisses the stack. This
  is the Win32 analogue of muri's outside-click logic and how native tracking menus
  detect dismissal.
- **`WM_ACTIVATEAPP`** — when the *application* loses activation (user Alt-Tabs or
  clicks another app), dismiss. This catches focus-driven dismissal that the
  low-level mouse hook alone would miss (e.g. keyboard app-switch).

**Adversarial hazards with `WH_MOUSE_LL` (the sharpest Windows risk):**

1. **It is a global, system-wide hook** running the callback on mouse events for
   *every* app. It must be **installed only while the popup is open** and removed
   (`UnhookWindowsHookEx`) the instant it closes — a leaked hook degrades
   system-wide input latency and is a classic resource leak.
2. **The callback runs on the thread that installed the hook, which must have a
   running message pump.** With winit owning the pump, the hook callback and the
   winit loop share a thread; the callback must do near-zero work (post a
   `UserEvent`, return) or it stalls global input.
3. **`WH_MOUSE_LL` callbacks have a timeout** (`LowLevelHooksTimeout`); a slow
   callback is silently *skipped*, so dismiss can be missed under load — do not do
   hit-testing in the hook, just marshal the point to the UI loop.
4. Coordinates are physical screen pixels — convert with the popup's DPI before
   hit-testing against the logical `LaidMenu`.

An alternative worth documenting: the classic Win32 tracking-menu trick of
`SetCapture` on the popup and dismissing on any click that `WindowFromPoint` places
outside — lighter than a global hook, but it fights winit's own capture handling.
1.0 should prototype both; `WH_MOUSE_LL` is the spec default because it matches
native menu behavior and does not require capture ownership.

### Flyout dismiss / focus set

macOS uses `focused: HashSet<WindowId>` and dismisses when it empties (doc 20 §2).
On Windows, with `WS_EX_NOACTIVATE` popups that never hold focus, the focus-set
signal is inverted: the equivalent is "is the last mouse-down inside *any* muri
window." 1.0 reuses the same `HashSet<WindowId>` structure but feeds it from the
`WH_MOUSE_LL` hit-test (point ∈ any open popup or flyout) plus `WM_ACTIVATEAPP`,
not from `Focused` events. This is the concrete divergence from the macOS dismiss
path and must be unit-tested at the geometry level (point-in-any-window) even
though the hook itself cannot be.

**N-level flyout stack (decision #8).** muri ships **N-level nested submenus** in
1.0, not a single flyout level. On Windows that means the popup machinery owns a
**stack of flyout windows** (`Vec<Flyout>`), each a `WS_EX_NOACTIVATE | TOOLWINDOW
| LAYERED` window placed by `place_flyout` relative to the **previous** level's
panel (right by default, flipping left on spill), each with its own acrylic
backdrop and its own `LaidMenu` hit-test map. The `HashSet<WindowId>` /
point-in-any-window rule already generalizes to N windows unchanged: the stack
dismisses only when a `WH_MOUSE_LL` mouse-down lands outside **every** open popup
and flyout (or on `WM_ACTIVATEAPP` deactivation). Opening a deeper level pushes a
window; `Left`/`Esc` (doc 40) pops the deepest. This mirrors the macOS backend's
stack (doc 20 §2) — same shared `keynav`/`flyout` logic, only the window flags and
the hook-driven dismiss differ.

## 3. `ContextMenu::open_at` and dropdown surfaces

Same story as macOS (doc 20 §3): reuse the popup machinery with the anchor rect
sourced from a point instead of `Shell_NotifyIconGetRect`. The facade's
`unsafe fn show_context_menu_for_hwnd(hwnd, position)` (doc 02 §6) maps muda's
`dpi::Position` (or `GetCursorPos` when `None`) to a `LogicalPoint`, using the
`hwnd`'s monitor for work-area context. `Popup::anchored_to(rect, edge)` (dropdown)
anchors to a caller rect. All three funnel through the shared `PopupSession`
(defined in doc 20 §3 / doc 01 §5.3) — Windows implements the *same* session, only
the window-flags and dismiss shim differ.

## 4. App-menu-bar native passthrough — `init_for_hwnd`

Per decision #2, muri does not draw the Windows window menu bar. The compat layer's
`unsafe fn init_for_hwnd(hwnd)` (doc 02 §6) builds an `HMENU` **itself** via
windows-sys (decision #5 — not the muda crate) and installs it via
`SetMenu(hwnd, hmenu)`; that native menu owns rendering, mnemonics (`&File`), and
accelerator registration, and feeds the same muri dispatch/global-channel path
(doc 03 §3). muri draws nothing here. Unlike macOS there is **no activation-policy conflict** on Windows — a tray
and a window menu bar coexist freely (the tray owner is a separate message-only
window; the menu bar belongs to the app's real top-level window). So divergence D9
(doc 02 §9) is macOS-only; Windows just installs both.

The one subtlety: muda's `init_for_hwnd` is `unsafe` and takes a raw `HWND`. The
facade preserves the `unsafe fn` signature verbatim so callers compile unchanged
(doc 02 §6).

## 5. Event loop integration (winit) and threading

Windows must adopt the **exact `UserEvent` loop shape macOS established** (doc 03
§2): one `EventLoop<UserEvent>`, an `EventLoopProxy` posted to from (a) the
`wnd_proc` tray-callback, (b) the `WH_MOUSE_LL` hook, (c) the `TrayHandle` command
channel, and (d) the AccessKit adapter. The `ApplicationHandler` is the *same*
`App`/`PopupSession` state as macOS — this is the payoff of keeping the engine
portable (doc 00 §10): the Windows backend is "new window-flags + new dismiss shim,"
not a new application.

- **Threading:** winit's Windows loop is also main-thread-oriented; `Tray::run`
  keeps the same UI-thread contract. `Shell_NotifyIcon` and the message-only window
  must be created/destroyed on the thread that pumps their messages.
- **`TrayHandle`:** the `UserEvent::Command(TrayCommand)` variant (doc 03 §2)
  applies `set_menu`/`set_icon`/`set_tooltip`(→`Shell_NotifyIconW(NIM_MODIFY)`)/
  `set_visible`/`open`/`close` on the UI thread. `set_icon` triggers the
  PNG→`HICON` decode (§1) and a `NIM_MODIFY`.
- **The still-`todo!()` popup event loop** is the single largest Windows deliverable:
  wiring `wnd_proc` messages and the hook into the winit loop, creating the
  layered/no-activate window, and driving `render_menu` → `softbuffer` present
  (the blit code is shared with macOS; only the window creation differs).

## 6. Accessibility (Windows) — pointer to doc 30

1.0 bridges the a11y tree to **UI Automation** via `accesskit_windows` (doc 03 §4),
verified against **NVDA and Narrator** (decision #4). The adapter attaches to the
popup `HWND` the same create-hidden-then-reveal way macOS attaches
`accesskit_winit::Adapter` (doc 20 §6). The **separate-OS-window flyout** problem
(doc 00 §7) is identical on Windows — a second top-level `HWND` whose items are
nested nodes in the parent's UIA tree — and is owned by
[`30-accessibility.md`](30-accessibility.md), not here. Note UIA has a native
`Menu`/`MenuItem` control-type vocabulary AccessKit maps onto; whether NVDA follows
focus across the two `HWND`s is the Windows-specific unknown for the a11y gate.

## 7. Status vs 1.0 gap (Windows)

| Capability | Status |
|---|---|
| Message-only owner window + class registration | **Groundwork** |
| `Shell_NotifyIcon(NIM_ADD)` install, `NIM_DELETE` on drop | **Groundwork** |
| `Shell_NotifyIconGetRect` → logical via `GetDpiForWindow` | **Groundwork** |
| `popup_origin` via shared `place_popup(Edge::Top)` | **Groundwork** |
| PNG → `HICON` decode | **Skeleton** (`todo!()`, stock icon placeholder) |
| `wnd_proc` decoding `WM_TRAY_CALLBACK` → click routing | **1.0-new** |
| `WM_TASKBARCREATED` re-add on Explorer restart | **1.0-new** |
| Borderless `WS_EX_NOACTIVATE|TOOLWINDOW|LAYERED` popup + blit | **1.0-new** |
| Acrylic vibrancy backdrop (`DWMWA_SYSTEMBACKDROP_TYPE` / `ACCENT_ENABLE_ACRYLICBLURBEHIND`), alpha-preserving blit | **1.0-new** (§2, decision #6) |
| N-level flyout **stack** (`Vec<Flyout>` windows) | **1.0-new** (§2, decision #8) |
| `WH_MOUSE_LL` + `WM_ACTIVATEAPP` outside-click dismiss (point-in-any-window over the stack) | **1.0-new** |
| winit `UserEvent` loop wiring (shared with macOS `App`) | **1.0-new** |
| Keyboard input to a no-activate popup | **1.0-new** (§2, shared with macOS) |
| `ContextMenu::open_at` / `Popup::anchored_to` via `PopupSession` | **1.0-new** |
| `TrayHandle` (`NIM_MODIFY` for icon/tooltip) | **1.0-new** |
| `init_for_hwnd` native passthrough | **1.0-new** (facade) |
| UIA via `accesskit_windows`; NVDA + Narrator passes | **1.0-new** (gate) |

### Concrete risks / unknowns

1. **The `WH_MOUSE_LL` dismiss** (§2) — a global hook with lifetime, latency, and
   timeout hazards. The subtlest Windows correctness/perf risk; must be installed
   only while open, do zero work in the callback, and be unit-tested at the
   geometry (point-in-any-window) level.
2. **Popup event loop from scratch** (§5) — the only backend where the loop itself
   is unbuilt. Risk is integration (winit ↔ `wnd_proc` ↔ hook), not algorithm.
3. **Keyboard into a `WS_EX_NOACTIVATE` popup** (§2) — same non-activating/needs-keys
   bind as macOS; resolution is a shared input decision (doc 40).
4. **`Shell_NotifyIconGetRect` under the overflow flyout** (§2) — returns the
   chevron rect or fails; needs a fallback anchor.
5. **DPI correctness end-to-end** — physical shell rects, per-monitor DPI, and the
   layered-window blit must all agree on scale, or the popup is mis-sized on
   mixed-DPI multi-monitor setups.
6. **Acrylic backdrop + alpha-preserving layered blit** (§2, decision #6) — the
   `SetWindowCompositionAttribute` acrylic path is undocumented and fragile across
   Windows builds and can be disabled by system transparency settings; muri must
   detect the capability and fall back to an opaque `Theme::native()` panel cleanly
   rather than rendering a black or fully-transparent rectangle. A wrong
   premultiplied-alpha blit shows as a dark halo around the rounded corners.
7. **N-level flyout stack** (§2, decision #8) — the geometry generalizes cleanly
   (`place_flyout` relative to the previous level), but the deep stack multiplies
   the cross-OS-window screen-reader traversal problem owned by
   [`30-accessibility.md`](30-accessibility.md), and the `WH_MOUSE_LL` hit-test must
   test point-in-**any** of N windows on every mouse-down without exceeding the
   low-level-hook timeout.
