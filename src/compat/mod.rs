//! muda + `tray-icon` **compatibility facade** (spec `02`, `60`) — behind the
//! `muda-compat` feature.
//!
//! This module mirrors muda's and `tray-icon`'s module layout and type *names* so
//! an unchanged app can change only its imports
//! (`use muda::…` → `use muri::compat::muda::…`,
//! `use tray_icon::…` → `use muri::compat::tray_icon::…`) and compile. Every
//! facade type maps **onto muri's native model** ([`crate::menu`],
//! [`crate::Tray`], [`crate::ContextMenu`]); the facade does **not** depend on the
//! real `muda` / `tray-icon` crates (locked decision #5). Events are emitted by
//! muri itself and projected onto the process-global channel in [`crate::event`],
//! which both the native and facade doors read.
//!
//! ## Scope of this (M3) implementation
//!
//! The routing decision, the item-type translation onto muri's `Item`/`Row`
//! tree, muda's `MenuId` auto-generation counter semantics, and the unified
//! `MenuEvent` channel are all implemented and unit-tested here (spec `50` §2.6).
//! Actual native menu-bar materialization (`NSMenu`/`HMENU`/GTK) and live custom
//! surface display route through muri's platform backends, which land per the
//! milestone ladder; the facade tags the surface mode and builds the muri
//! surface, and is honest about the documented divergences (D1–D9, spec `02` §9).

pub mod muda;
pub mod tray_icon;
