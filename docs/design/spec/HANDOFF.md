# HANDOFF — muri 1.0 build kickoff, decision log, current state

This is the entry point for the team implementing muri 1.0. Read it first, then the
spec ([`README.md`](README.md) index → [`00-overview.md`](00-overview.md) → the rest).

---

## Part A — Build kickoff

**Repo:** `/Users/matthew/Developer/muri` (standalone, `github.com/MattJackson/muri`,
MIT). Author commits as **Matthew Jackson `<1085847+MattJackson@users.noreply.github.com>`**;
**never** add `Co-Authored-By`. Run `git pull --rebase origin main` first — origin has
production-presentation commits and the spec commits are local, so reconcile once.

**Read first (authoritative):** `docs/design/spec/` — the README index, then
`00-overview` (mission, locked decisions, milestone ladder), `01-api-contract` (the
native API — the north star), `02-muda-compat`, `03-threading-events-versioning`,
then `10` rendering, `20`/`21`/`22` platforms, `30` a11y, `40` input, `50` testing,
`60` migration. Then read current `src/` — Phases 1–4 shipped a working macOS backend
(tray, popup, flyouts, keynav, a11y adapter), Windows groundwork, and a Linux
skeleton. **Build on what works.**

**Locked product decisions:**

- **(a) Native API primary** — a fluent builder
  `Menu::new().add(Text::new("Quit").color(Color::Red).align(Align::Right).on(quit))`,
  per-item styling, a retained `TrayHandle` for live runtime updates, and
  muri-native events (callbacks / stream, **not** muda's global channel).
- **(b) `.add(impl Into<Item>)` is the one stable, extensible door.** 1.0 built-ins:
  `Text` / `Image` / `Separator` / `Submenu` / `Check` / `Predefined`, plus a
  `MenuNode` extension trait (measure / draw / hit-test / a11y). Video and rich
  content are additive **1.x**, built on that trait; `Item` is `#[non_exhaustive]`.
- **(c) muda-compat is the simple, faithful drop-in on-ramp** — `use
  muri::compat::muda as muda;` compiles, runs, and looks native; it **preserves
  muda's passive loop + global channel** and maps 1:1 onto the native builder. It
  rides on top of the native API and never distorts it.
- **(d)** N-level nested submenus; `Theme::native()` with **real vibrancy** (macOS
  `NSVisualEffectView`, Windows acrylic); the `a11y` feature **on by default**; all
  three platforms; all screen readers verified (a **hard release gate**).

**How to work:** incremental by the milestone ladder ([`00` §8](00-overview.md)),
each milestone usable and green. Order:

1. **M1** — solidify the macOS native API (builder + `TrayHandle` + events + N-level
   + vibrancy) so usagio adopts it behind its `custom-popup` flag.
2. **M2** — context menus + `ContextMenu::open_at`.
3. **M3** — muda-compat + `Theme::native()` fidelity.
4. **M4** — Windows.
5. **M5** — Linux + all-screen-reader verification.
6. **1.0.**

Honor the early-embedder stability contract in [`03` §5](03-threading-events-versioning.md).

**Tackle the flagged hard risks deliberately:** the non-activating `NSPanel` vs
must-receive-keyboard bind (macOS); cross-OS-window flyout-**stack** screen-reader
focus (the riskiest a11y claim); the Windows `WH_MOUSE_LL` outside-click dismiss;
Wayland client-positioning limits; bidi/RTL styled runs.

**Fix the shipped bugs the spec notes:** the hard-coded accent color; dark-mode
run-color resolution (`style_pieces` uses `Theme::light()`); the per-frame redraw
busy-loop; the multi-monitor Y-flip against the primary screen.

**Gates before every push (all must pass):** `cargo build --all-targets`;
`cargo fmt --all --check`; `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo test`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`. Keep
`#![deny(missing_docs)]`. Unit-test all pure logic; golden-image snapshots per
[`50`](50-testing-verification.md); screen-reader verification is manual and gates
1.0. Commit in logical chunks, update `CHANGELOG` `[Unreleased]`, keep macOS green at
every step. Report per milestone: real vs `todo!()`/device-unverified, gate results,
and the remainder for the next milestone.

---

## Part B — Decision log (the "why")

- **Pivot.** The RIGHT native API for muri's *own* use — a live app (usagio) that
  rewrites its menu ~every 0.75s — is the north star; the muda mapping is a layer on
  top, not a constraint that shapes the core. *Rationale:* forcing muda's shape
  (passive loop, global channel, `run(self)` consumes) onto muri distorted it; the
  native builder maps 1:1 to muda anyway, so both can be first-class.
- **Adoption strategy.** The only realistic path for existing muda users is a
  **simple, faithful drop-in**, *then* progressively adding muri features. So
  muda-compat is first-class (not optional/bonus) **and** preserves muda's passive
  integration model so `s/muda/muri/` truly compiles and runs. Two doors into one
  engine: compat = the simple on-ramp; the native API = the feature layer they grow
  into.
- **API ergonomics (owner's sketch).** Fluent `.add()` builder; per-item chained
  modifiers (`.color`/`.align`/`.icon`/`.bold`/`.enabled`/`.checked`/`.accelerator`/
  `.on`); free-function constructors (`text()`/`submenu()`/`separator()`/`image()`)
  as an alternative to `::new`; idiomatic Rust casing.
- **Extensibility.** `.add()` is where capability grows over time; 1.0 ships the core
  nodes **plus** the `MenuNode` trait seam; Video / rich content is **1.x**,
  additive, no breaking change (`#[non_exhaustive]` + the trait).
- **The four newly-locked decisions** (events re-implemented natively / vibrancy
  required / a11y on by default / N-level nesting), and the honest scope note: N-level
  + all-screen-readers + vibrancy **together raise 1.0 effort and risk** (see below).

**Honest scope/risk read.** The three together are the bulk of the incremental 1.0
cost: (1) **N-level submenus** turn a single flyout into a *stack* of OS windows and
multiply the hardest a11y problem (cross-OS-window focus) across arbitrary depth;
(2) **all-screen-readers verified** is a manual, non-automatable gate on four ATs and
is already the riskiest claim; (3) **vibrancy** forces a transparency/compositing
rework (macOS `NSVisualEffectView` hosting + alpha-preserving present) that couples
with the non-activating-`NSPanel` work. None is individually blocking, but stacked
they materially raise the 1.0 schedule and risk versus the pre-pivot scope —
plan M1/M2 to prove the macOS combination (N-level stack + vibrancy + VoiceOver across
the flyout boundary) before generalizing to Windows/Linux.

---

## Current state

- **macOS: usable (M1-level).** `NSStatusItem` tray, borderless styled popup,
  live flyout submenu window, mouse + keyboard nav, dark/light follow, and an
  AccessKit adapter are shipped and tested; VoiceOver is device-unverified. The
  native `add(…)` builder, `TrayHandle`, N-level flyout stack, and vibrancy are
  1.0-new on top of this.
- **Windows: groundwork.** `Shell_NotifyIcon` install + anchor rect + shared
  `place_popup` are compile-verified; the popup event loop, `WH_MOUSE_LL` dismiss,
  PNG→`HICON`, and acrylic are net-new (M4).
- **Linux: skeleton.** `LinuxAnchor` correctly returns `Unsupported::TrayAnchor`;
  SNI/dbusmenu install and the styled `xdg_popup` path are net-new (M5). A
  tray-anchored styled popup is a permanent, honest non-goal.
- **Pure core: dense and green.** `menu` / `theme` / `layout` / `anchor` / `flyout` /
  `keynav` / `a11y` are exhaustively unit-tested and portable; a headless PNG
  snapshot test exists (to be upgraded to golden images per [`50`](50-testing-verification.md)).

## Where things stand

The spec in this directory is **complete and adversarially reviewed**, reconciled to
the pivot and the eight locked decisions above. It supersedes the narrative roadmap
as the contract the implementation is measured against. **usagio** is the first
adopter, integrating muri's native macOS surface behind its own `custom-popup`
feature flag under the pre-1.0 stability contract ([`03` §5](03-threading-events-versioning.md)).
Begin at M1.
