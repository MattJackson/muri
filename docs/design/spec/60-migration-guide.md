# 60 — muda → muri migration guide

Status: **foundational spec.** A worked, end-to-end migration of a real
`muda` + `tray-icon` app onto muri via the compatibility facade, then the
progressive-restyle path that unlocks muri's native styling. Every divergence a
migrating app will hit is surfaced inline as an actionable note.

This document is the *reader-facing* companion to the *contract* in
[`02-muda-compat.md`](02-muda-compat.md): 02 enumerates the facade per item and
owns the authoritative divergence register (D1–D9); 60 shows the diffs and tells a
consumer what to do about each divergence. Threading / event-loop and the
pre-1.0 stability tiers are [`03-threading-events-versioning.md`](03-threading-events-versioning.md);
the native styling API is [`01-api-contract.md`](01-api-contract.md).

---

## 1. The migration in one sentence

Change your `muda` and `tray-icon` imports to `muri::compat::muda` and
`muri::compat::tray_icon`, **keep your own event loop and your `MenuEvent::receiver()`
polling** (the compat layer preserves muda's passive model — [`02` §2.1](02-muda-compat.md)),
and your app compiles, runs, and **looks native by default** via `Theme::native()`
(with real vibrancy) — after which you can restyle every pixel at your own pace. The
guarantee, precisely scoped (and what it does *not* promise), is
[`02` §1](02-muda-compat.md).

## 2. The BEFORE app (muda + tray-icon)

A representative status-bar app: a tray icon whose menu has an app-name header, a
couple of action items, a checkable toggle, a separator, and a native Quit — plus
a native macOS application menu bar. This is deliberately close to what usagio (the
first real adopter, §7) and countless tao/tauri tray apps do.

```rust
// Cargo.toml
// muda = "0.15"
// tray-icon = "0.19"

use muda::{
    accelerator::{Accelerator, Code, Modifiers},
    CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

fn build_tray_menu() -> Menu {
    let menu = Menu::new();

    // App-name header emulated as a disabled item (a common muda idiom).
    let header = MenuItem::new("MyApp", false, None);
    let open = MenuItem::with_id("open", "Open Window", true, None);
    let notify = CheckMenuItem::with_id("notify", "Notifications", true, true, None);

    // A per-item detail submenu.
    let account = Submenu::with_id("account", "Account", true);
    account
        .append(&MenuItem::with_id("switch", "Switch account", true, None))
        .unwrap();

    let sep = PredefinedMenuItem::separator();
    // A predefined OS action with an accelerator.
    let quit = PredefinedMenuItem::quit(Some("Quit MyApp"));

    menu.append_items(&[&header, &open, &notify, &account, &sep, &quit])
        .unwrap();
    menu
}

fn build_menu_bar() -> Menu {
    let bar = Menu::new();
    let file = Submenu::with_id("file", "File", true);
    let save = MenuItem::with_id(
        "save",
        "Save",
        true,
        Some(Accelerator::new(Some(Modifiers::SUPER), Code::KeyS)),
    );
    file.append_items(&[&save, &PredefinedMenuItem::separator(),
                        &PredefinedMenuItem::close_window(None)]).unwrap();
    bar.append(&file).unwrap();
    bar
}

fn main() {
    // The app owns its own event loop (tao/winit); muda/tray-icon do not.
    let event_loop = /* tao or winit EventLoop */;

    let bar = build_menu_bar();
    #[cfg(target_os = "macos")]
    bar.init_for_nsapp();            // app menu bar → native

    let tray_menu = build_tray_menu();
    let _tray = TrayIconBuilder::new()
        .with_tooltip("MyApp")
        .with_icon(Icon::from_rgba(icon_rgba(), 32, 32).unwrap())
        .with_menu(Box::new(tray_menu))
        .build()
        .unwrap();

    // The app polls the two global channels from inside its own loop.
    event_loop.run(move |_event, _elwt| {
        if let Ok(ev) = MenuEvent::receiver().try_recv() {
            match ev.id.0.as_str() {
                "open"   => open_window(),
                "notify" => toggle_notifications(),
                "switch" => switch_account(),
                "save"   => save_document(),
                _ => {}
            }
        }
        if let Ok(_ev) = TrayIconEvent::receiver().try_recv() {
            // react to tray clicks (macOS/Windows)
        }
    });
}
```

## 3. The AFTER app (muri facade) — `s/muda/muri/`

### 3.1 The import change

The mechanical change is the imports (and the dependency swap). Enabling the
`muda-compat` feature ([`03` §4](03-threading-events-versioning.md)) re-exports the
facade under muda's own paths, so even the `use` lines can stay byte-identical; the
explicit form below makes the routing visible.

```diff
  // Cargo.toml
- muda = "0.15"
- tray-icon = "0.19"
+ muri = { version = "1", features = ["muda-compat"] }   # a11y is on by default

- use muda::{
+ use muri::compat::muda::{
      accelerator::{Accelerator, Code, Modifiers},
      CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
  };
- use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
+ use muri::compat::tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
```

Everything in `build_tray_menu` and `build_menu_bar` **compiles unchanged**: the
facade mirrors muda's type names, builders, `with_id`, `append_items`,
`Submenu`, `CheckMenuItem`, `PredefinedMenuItem`, `Accelerator`, and the `Icon`
`from_rgba` constructor ([`02` §4, §5, §8](02-muda-compat.md)). `ev.id.0` still
works because `muri::compat::muda::MenuId` **is** `muri::MenuId`, structurally
`pub struct MenuId(pub String)` — identical to muda's ([`02` §3](02-muda-compat.md)).

### 3.2 What each surface does now

- `bar.init_for_nsapp()` → **native passthrough** ([`02` §2, §6](02-muda-compat.md)).
  The compat layer builds a native `NSMenu` itself (via objc2 — decision #5, not the
  muda crate) and installs it as the macOS app menu bar. It is byte-for-byte native
  because it *is* native: the `File` menu, the `Cmd-S` accelerator, and
  `close_window` all behave exactly as before. muri draws nothing here.
- `TrayIconBuilder…with_menu(tray_menu)` → **muri custom surface**. The tray menu
  is now **drawn by muri** with `Theme::native()`, resolving to live OS colors and
  the system font — visually near-native ([`02` §1](02-muda-compat.md)), with the
  divergences in §4.
- `MenuEvent::receiver()` → **one unified channel** ([`03` §3](03-threading-events-versioning.md)).
  Both the native menu-bar `save` click and the muri tray `open`/`notify`/`switch`
  clicks arrive on the **same** `muri::MenuEvent::receiver()`, in order — muri emits
  for both the native menu bar it installs and its custom surfaces through one
  `dispatch` (decision #5; no wrapped-muda channel, no forwarder). Your `match` on
  `ev.id.0.as_str()` is unchanged.

### 3.3 The event loop — your loop stays yours (passive model preserved)

The compat layer **keeps muda's and tray-icon's passive model**: your app still owns
its event loop and still polls `MenuEvent::receiver()`. `TrayIconBuilder::build()`
**returns immediately** (as tray-icon's does) — it does **not** block on a loop and
does **not** force you onto muri's native `Tray::run(self)` / `TrayHandle`
([`02` §2.1](02-muda-compat.md)). The one wrinkle: muri's custom-drawn popup needs a
`winit` loop to draw and dismiss, so the compat tray **attaches to your loop** rather
than replacing it.

**(a) You already run a `winit` loop** (most tray apps do): keep your loop and keep
your `MenuEvent::receiver()` polling exactly as in §2 — the mechanical import change
is *all* the code that changes. muri's backend posts through the same
`EventLoopProxy`/`UserEvent` mechanism its macOS backend already uses
([`03` §2](03-threading-events-versioning.md)); the compat layer exposes an
integration entry you call from your loop.

```rust
let _tray = TrayIconBuilder::new()
    .with_tooltip("MyApp")
    .with_icon(Icon::from_rgba(icon_rgba(), 32, 32).unwrap())
    .with_menu(Box::new(tray_menu))
    .build()
    .unwrap();                       // returns immediately — no blocking, no run()

// Your loop is unchanged; keep polling the same global channel:
event_loop.run(move |_event, _elwt| {
    if let Ok(ev) = MenuEvent::receiver().try_recv() {
        match ev.id.0.as_str() {
            "open" => open_window(),
            "notify" => toggle_notifications(),
            "switch" => switch_account(),
            "save" => save_document(),
            _ => {}
        }
    }
});
```

**(b) You have *no* loop of your own** (a headless daemon that was fully passive):
opt into the convenience where muri owns the loop — `tray.run()` blocks on the
event loop after you spawn your reaction logic on a channel reader. This is a
convenience, **not** required for a drop-in.

**Runtime menu updates (passive):** rebuild and call the tray's own
`set_menu`/`set_icon`/`set_tooltip` (mirroring tray-icon's post-construction API),
which internally posts to the loop — you never hold a native `TrayHandle`. Only if
you *graduate to the native API* ([`01` §5.1](01-api-contract.md)) do you use
`TrayHandle` directly. Either way the ~0.75s live-rebuild path is supported.

### 3.4 The tray-icon crate is subsumed, not a second dependency

muri's `Tray` replaces `tray-icon` entirely ([`02` §8](02-muda-compat.md)): the
facade's `TrayIconBuilder`, `TrayIcon::set_icon/set_tooltip/set_title/set_visible`,
`TrayIconId`, and `TrayIconEvent` + its `receiver()` are all mirrored, so you drop
the `tray-icon` dependency and gain the styled popup. The important behavior change:
raw tray-icon's click opened the *OS's* native menu; **muri's click opens the
custom-drawn popup** — that is the entire point of migrating.

## 4. Divergence / caveat table (actionable migration notes)

Every place muri is **not** a faithful drop-in, restated from the authoritative
register in [`02` §9](02-muda-compat.md) (ids D1–D9) as "what you will see, and
what to do." A migration guide that hides these is worse than none.

| ID | Divergence | Where it bites | What to do |
|---|---|---|---|
| **D1** | **Vibrancy — RESOLVED.** `Theme::native()` renders with real vibrancy (macOS `NSVisualEffectView`, Windows acrylic — locked decision #6, [`01` §4](01-api-contract.md)); your tray/context menu is frosted, not solid. | No longer a divergence on macOS/Windows. | Nothing to do. (The Linux *styled* surface stays opaque — compositor-owned blur isn't client-controllable; the Linux native fallback is host-drawn.) |
| **D2** | **Metrics differ subtly.** Row height, padding, corner radius, font metrics are muri's, tuned to look native but not pixel-identical. | Side-by-side with a native menu, spacing differs slightly. | If exactness matters, override the metrics via `Theme` (`row_height`, `corner_radius`, `padding`, `column_gap`) — this is already the restyle path (§5). |
| **D3** | **A `Menu` used as both a menu bar and a context menu** renders native in the bar, custom in the context menu. | Only if you reuse one `Menu` value for `init_for_*` **and** `show_context_menu_for_*`. Rare. | Build two menus, or accept the two looks. The facade never silently breaks the common case ([`02` §2](02-muda-compat.md)). |
| **D4** | **`PredefinedMenuItem` OS actions in a custom surface** are best-effort emulated; `services()` and off-macOS `about()` are **menu-bar-only**. | Your tray/context menu's `copy`/`paste`/`quit`/`about`/`services` items. `separator` is exact. Edit/window/app actions need a focusable target window/responder. `services` renders as a **disabled placeholder**. | Keep OS actions in the **native menu bar** where they work perfectly. In a custom surface, prefer routing to your own logic via a `MenuEvent` id over relying on emulated `quit`/`about`. Inemulable items render as a **visible disabled row** so nothing silently vanishes ([`02` §4.6](02-muda-compat.md)). |
| **D5** | **Accelerators in a custom surface** are *displayed* (right-aligned, dogfooding flush-right) and handled **only while the menu is open** — no global/app-wide firing when closed. | A tray/context item whose accelerator the user expects to fire from anywhere in the app. | In the **menu bar** (passthrough), accelerators fire natively — leave them there. For a true global hotkey, use the `global-hotkey` crate (a stated non-goal, [`00` §3](00-overview.md)). |
| **D6** | **`NativeIcon` / `Icon::Symbol` not yet drawn** in a custom surface until 1.0 ships symbol rendering. | An `IconMenuItem` using a `NativeIcon`, or a muri `Icon::Symbol`, in the tray/context menu. | Use `Icon::from_rgba`/`from_png_bytes` (drawn today) for custom surfaces until symbol rendering lands; `NativeIcon` in the native menu bar is unaffected ([`02` §4.7](02-muda-compat.md)). |
| **D7** | **Linux tray has no styled anchored popup.** `Tray::run` returns `Unsupported::TrayAnchor`; the tray falls back to a native SNI/dbusmenu menu or a pointer `ContextMenu`. `TrayIconEvent` is **not emitted on Linux** (matching tray-icon). | Any tray-anchored styled popup on Linux/Wayland — architecturally impossible ([`00` §6](00-overview.md)). | Use the **native-menu fallback** (same `Menu` spec, native look, AT-SPI for free) for the Linux tray, and use `ContextMenu::open_at` for styled right-click menus where a pointer coordinate exists ([`22`](22-platform-linux.md)). |
| **D8** | **Submenu depth — RESOLVED.** N-level nested submenus ship in 1.0 (locked decision #8, [`02` §4.2](02-muda-compat.md), [`40` §5](40-input-interaction.md)). | A muda menu of any submenu depth maps directly. | Nothing to do — no flattening; nesting is faithful at every level. |
| **D9** | **macOS activation policy** shifts `Accessory` → `Regular` when a native menu bar is installed alongside a tray. | A tray-only app is `Accessory` (no Dock icon); adding `init_for_nsapp` forces `Regular` (Dock presence). | Expected. The facade decides the policy from the *combination* of surfaces you install ([`02` §2](02-muda-compat.md), [`20`](20-platform-macos.md)); nothing to do unless you specifically wanted no Dock icon *and* a menu bar (incompatible on macOS). |

The interesting migrations — muri's actual value — are apps with **tray/context
menus**, which move from OS-drawn to muri-drawn and gain the full styling API the
moment they want it ([`02` §10](02-muda-compat.md)). An app entirely on the native
menu bar migrates transparently (everything is passthrough) but gains little.

## 5. The progressive-restyle story (the unlock)

Once compiled and native-looking on the facade, a consumer restyles in three
widening steps with **no cliff** between "muda-like" and "fully custom"
([`01` §1](01-api-contract.md)). Each step reaches a little further past the
facade into muri's native `Theme` / `Style` / `Menu` API.

### Step 1 — Swap the theme (global look, zero data-model change)

The facade defaults to `Theme::native()`. To take control of the palette, spacing,
and fonts, drop from the facade tray to muri's native `Tray` (or set options the
facade forwards) and hand it a custom `Theme`:

```rust
use muri::{Theme, ThemeSource, MenuOptions, Color, Font, Insets};

// BEFORE: implicit Theme::native() via the facade.
// AFTER: one custom theme, applied globally to the styled surface.
let theme = Theme {
    background: Color::rgb(28, 28, 34),
    label: Color::rgb(235, 235, 240),
    accent: Color::rgb(120, 170, 255),
    row_highlight: Color::rgb(48, 52, 66),
    corner_radius: 10.0,
    row_height: 26.0,
    padding: Insets::symmetric(6.0, 8.0),
    ..Theme::native()
};

let tray = muri::Tray::new(icon)
    .menu(menu)
    .options(MenuOptions { theme: ThemeSource::Custom(theme), ..Default::default() });
```

(Custom-theme dark/light following is a flagged 1.0 addition —
`ThemeSource::CustomFollowSystem { light, dark }` — see
[`01` §4](01-api-contract.md); `ThemeSource::Custom` alone is a single fixed look.)

### Step 2 — Restyle per row with `Segment` / `Flex` / `Align` / `Color`

This is the payoff a native menu **cannot** give: true multi-column rows and a
flush-right value with **no reserved chevron column** ([`01` §2](01-api-contract.md)).
A plain muda item becomes an expressive muri `Row`:

```rust
// BEFORE (facade / muda): a single flat label string.
let acct = MenuItem::with_id("switch:me", "me@example.com — 47% / 89%", true, None);

// AFTER (muri native Row): the label Grows to eat leftover width, so the value
// sits flush at the true right edge; the "89%" span is colored red.
use muri::{Row, Segment, Align, Flex, StyleRun, Color, Weight, Font, Icon};

let acct = Row::new("switch:me")
    .leading(Icon::Checkmark)          // check column only when you want it
    .checked(true)
    .segments(vec![
        Segment::new("me@example.com")
            .flex(Flex::Grow)          // <-- eats the slack
            .font(Font::system(13.0, Weight::Bold)),
        Segment::new("47% / 89%")
            .align(Align::Right)       // <-- flush right, no chevron column
            .runs(vec![StyleRun::new(6, 3, Color::SystemRed)]), // "89%" in red
    ]);
```

The id (`"switch:me"`) is unchanged, so your existing `MenuEvent` handler still
matches — muri hands `MenuId` strings back verbatim and never interprets them
([`01` §4, §6](01-api-contract.md)). This is exactly the menu the `usagio_menu`
example and the snapshot suite ([`50` §3](50-testing-verification.md)) build.

### Step 3 — Section headers, logos, submenus, per-row background

The full data model: bold logo'd group headers (`section_header` + `Icon`),
per-account detail flyouts (`submenu`), leading/trailing icons, and a greyed
version tail — all expressible now (the `examples/usagio_menu.rs` and
`examples/demo_tray.rs` build precisely this). Nothing here is reachable through a
native menu; every step is additive over the app you already migrated.

## 6. Migration checklist

1. Swap `muda` + `tray-icon` deps for `muri` with `features = ["muda-compat"]`.
2. Redirect imports (`muri::compat::muda`, `muri::compat::tray_icon`) — or enable
   the path re-export and leave them.
3. Adopt muri's event loop for the tray (`Tray::run`, or integrate with your
   `winit` loop); move runtime menu updates onto a `TrayHandle` (§3.3).
4. Keep OS-action `PredefinedMenuItem`s and accelerators in the **native menu bar**
   (D4, D5); audit any that live in the tray/context menu.
5. On Linux, choose the native-menu fallback for the tray and `ContextMenu` for
   styled right-click menus (D7).
6. Verify against the divergence table (§4); accept or work around each.
7. Restyle progressively (§5) once it compiles and looks native.

## 7. The early-embedder path (usagio, pre-1.0)

usagio is muri's first real adopter and does **not** wait for 1.0 or the facade
([`00` §8](00-overview.md)). It adopts muri's **native macOS surface** behind its
existing `custom-popup` feature flag, governed by the pre-1.0 stability contract in
[`03` §5](03-threading-events-versioning.md). The concrete path:

1. **Native API, not the facade.** An early macOS-only embedder builds directly
   against muri's Tier-1 data model (`Menu` / `Row` / `Segment` / `Theme`) and
   Tier-2 surfaces (`Tray` / `TrayHandle`) — **not** `compat::muda`, which is
   Tier-3 "churning" and doesn't land until M3. usagio already builds its tray this
   way in `examples/usagio_menu.rs`.
2. **Behind a feature flag.** All muri calls sit behind usagio's `custom-popup`
   flag so the build can fall back to native muda instantly. Keep native muda as a
   **kill-switch** one release past cutover — a muri regression becomes a flag flip,
   not an outage ([`03` §5](03-threading-events-versioning.md)).
3. **Pin an exact version.** During 0.x, depend on `muri = "=0.x.y"` (or a git
   rev): pre-1.0 semver-minor may carry breaking changes, and Tier-2 surface
   signatures (notably the `run` vs `handle`/`TrayHandle` split and
   `ContextMenu::open_at`'s Wayland surface argument) are still refining.
4. **Isolate the surface calls.** Keep every Tier-2 call (`Tray::new`,
   `.menu`, `.run`, `TrayHandle`) behind one thin adapter module, so a signature
   refinement is a one-file change — usagio's `custom-popup` seam already does this.
5. **Use muri-native events, not the global channel.** The global `MenuEvent`
   channel is the compat door (Tier-3); pre-1.0, use muri's native events — per-item
   `.on()` handlers or the surface event stream ([`01` §6](01-api-contract.md)).
   usagio reports the menu it builds and muri hands `MenuId` strings back verbatim,
   so usagio's id grammar (`switch:…`, `capture:…`) is unaffected by any muri change
   (Tier-1 guarantee).

When the facade ships (M3), a *new* muda-based adopter takes the §2–§3 path; usagio,
already on the native API, simply gains the facade as an option it doesn't need —
it is already past the unlock (§5) that the facade exists to lead others to.
