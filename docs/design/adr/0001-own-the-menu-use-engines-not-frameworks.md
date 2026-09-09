# ADR-0001 — Own the menu, use engines not frameworks

**Status:** Accepted — 2026-09-09.

Supersedes the "locked rendering stack" framing in
[`../spec/00-overview.md` §5](../spec/00-overview.md) (winit + softbuffer +
tiny-skia + cosmic-text, plus `accesskit_winit`). That list is now historical;
this ADR is the current direction. Cross-referenced from
[`../spec/10-rendering-layout.md`](../spec/10-rendering-layout.md) and
[`../spec/03-threading-events-versioning.md` §4](../spec/03-threading-events-versioning.md).

---

## Context

muri's north star includes being **lean, low-MSRV, and few-dependency** — a tray +
popup-menu crate should not drag a general-purpose GUI toolkit's worth of transitive
crates behind it. The original spec treated the rendering/windowing stack as a
**locked decision**: `winit` (windowing / event loop) + `softbuffer` (CPU
framebuffer) + `tiny-skia` (2D raster) + `cosmic-text` (font lookup / shaping /
layout), with `accesskit` + `accesskit_winit` for the a11y bridge. Measuring that
stack surfaced facts that motivate a change:

- **MSRV floor 1.85 comes from `cosmic-text`.** `cosmic-text` (~43 transitive
  crates) pulls a **non-optional `unicode-segmentation 1.13.3`**, which sets the MSRV
  floor at **1.85** (muri's `Cargo.toml` currently pins `rust-version = 1.86`). muri
  cannot go below 1.85 while `cosmic-text` is a dependency.
- **~94 crates on macOS.** The macOS dependency graph is roughly **94 crates**, the
  bulk of which are the general-purpose *frameworks* wrapped around a few genuinely
  deep engines — and `winit` additionally drags a **duplicate `objc2` 0.5 stack**
  alongside muri's own `objc2` 0.6.
- **The framework-vs-engine distinction is the key insight.** The *engines* are the
  hard, valuable parts: a HarfBuzz-class text shaper, a glyph rasterizer, the
  accessibility platform bridges. The *weight* is the general-purpose frameworks
  wrapped around those engines, of which muri uses only a sliver (a borderless,
  non-activating popup window; a handful of raster primitives; a single UI font
  family). muri pays for the whole framework to use a fraction of it.

## Decision

**Principle: own the menu system and the thin platform glue; *use* — do not reinvent
— the deep engines.** Reinventing a text shaper (`rustybuzz`, a HarfBuzz port), a
glyph rasterizer (`swash`), or the accessibility platform bridges (`accesskit`) would
be multi-year, buggier, slower work that undermines the goal. The weight problem is
the general-purpose *frameworks* wrapped around those engines, where muri uses only a
sliver. So: shed the frameworks, keep the engines, and own everything in between —
all menu logic, the rendering glue, and the windowing.

**Target end-state (by 1.0):** depend on a handful of **engine** crates + raw OS FFI,
owning all menu logic + rendering glue + windowing.

Per-layer plan:

- **Text.** Replace `cosmic-text` with `rustybuzz` + `swash` + `fontdb` used
  **directly**, plus a **muri-owned shaping-glue + font-fallback layer (~300 lines)**
  to preserve cross-script fallback + color-emoji — **zero menu-relevant feature
  loss**. Result: **MSRV 1.85 → ~1.73** and roughly **14 fewer crates**. The
  `resolve_ui_family` bold-fallback invariant
  ([`../spec/10-rendering-layout.md` §7.1](../spec/10-rendering-layout.md)) ports
  cleanly — its only `cosmic-text` touchpoint is `fs.db().face().families`, which maps
  directly onto `fontdb`.
- **Raster.** Replace `tiny-skia` with a **small in-house blitter** implementing only
  the primitives muri actually uses: fill rect, anti-aliased rounded-rect, and alpha
  blit. Rework icon PNG handling to shed the ~9-crate png/zlib subtree. Guard the
  in-house raster with **golden-image tests**.
- **Windowing.** Go **native per-OS** and remove `winit` (~18 exclusive crates, plus
  it drags a duplicate `objc2` 0.5 stack). Fold this **into** the already-mandatory
  non-activating-`NSPanel` + vibrancy rewrite on macOS, so windowing is rewritten
  **once, not twice**. Drop `softbuffer` + `raw-window-handle` (present via
  CALayer / CoreGraphics). Rebuild the a11y platform bridge directly on
  `accesskit_macos` / `accesskit_windows` / `accesskit_unix` (**drop
  `accesskit_winit`**). Windows uses a native Win32 message pump; Linux uses
  `xdg_popup` with the documented Wayland tray-anchor carve-out.
- **KEEP — do not reinvent.** Engines: `swash`, `rustybuzz`, `fontdb`, `accesskit`.
  Raw OS FFI: `objc2` / `objc2-foundation` / `objc2-app-kit`, `windows-sys`.
  (`tiny-skia` is the one engine that *is* replaced — the in-house raster is small and
  MSRV-neutral, so keeping tiny-skia buys nothing.)

**Sequencing — a multi-release journey.** The diet lands across the milestone ladder,
not in one commit. **M1** does the macOS native-windowing + text/raster swap; **M4/M5**
carry it to Windows/Linux. Every step ships **usable + green**, and **macOS stays
green throughout**. **1.0 = diet complete + all three platforms + all four screen
readers verified.**

**Consumer note.** usagio is the early embedder and real-world tester of muri's macOS
native surface. The **Tier-1 data model** and **Tier-2 surface API**
([`../spec/03-threading-events-versioning.md` §5](../spec/03-threading-events-versioning.md))
stay **stable through the diet**, so the diet is **internal-only churn** — embedders
build against the same API before and after.

## Consequences

**Pros:**

- **MSRV drops to ~1.73** (off the `cosmic-text` → `unicode-segmentation` 1.85 floor).
- **Roughly half the dependencies** on macOS (shed the winit/softbuffer/cosmic-text
  framework subtrees + the duplicate `objc2` 0.5 stack + the png/zlib subtree).
- **Full control** over windowing, compositing, and raster — which is what makes the
  required non-activating `NSPanel` + real translucent **vibrancy** (spec decision #6)
  achievable, rather than fighting a general-purpose windowing crate for it.
- Windowing is rewritten **once** (folded into the vibrancy/NSPanel rewrite already
  required), not twice.

**Cons / risks:**

- muri must now **own font fallback** (the ~300-line shaping-glue + fallback layer)
  where `cosmic-text` provided it — cross-script fallback and color-emoji are on us.
- muri must **own an anti-aliased raster** and prove it with **golden-image tests**;
  a subtle AA/gamma/rounding bug is now muri's to catch.
- **Native windowing is high-effort and high-risk.** The non-activating
  `NSPanel` ↔ keyboard-input problem (a panel that does not steal key focus yet must
  still route keyboard menu navigation) is genuinely hard, and going **per-OS
  multiplies the windowing effort** across macOS / Windows / Linux instead of
  amortizing it through one cross-platform crate.

## Alternatives considered

- **Keep `cosmic-text` / `winit` as-is (the previously-locked stack).** Rejected: it
  hard-pins MSRV ≥ 1.85, carries ~94 crates on macOS with a duplicate `objc2` stack,
  and still cannot deliver the required non-activating vibrant `NSPanel` without a
  windowing rewrite — so the "safe" option does not even avoid the hard windowing
  work.
- **Reinvent the engines** (write our own shaper / rasterizer / a11y bridges).
  Rejected as **multi-year, buggier, and slower** work that directly undermines the
  lean-and-correct goal. `rustybuzz` (HarfBuzz port), `swash`, and `accesskit` are
  exactly the deep, hard-won components worth depending on; the value is in *using*
  them directly rather than through a framework, not in replacing them.
