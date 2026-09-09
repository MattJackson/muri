# Contributing to muri

Thanks for your interest in improving muri! This document explains how to build,
test, and submit changes.

muri is an early, macOS-first work in progress. The **macOS** backend is
functional today (real `NSStatusItem` tray, custom-drawn popup, flyout submenus,
dark mode, keyboard navigation, and an AccessKit adapter behind the `a11y`
feature), on top of a fully pure, unit-tested cross-platform core (menu model,
layout, flyout placement, key-nav, theme, a11y tree). The **Windows** backend
installs the tray icon and reports its anchor rect but still needs its popup
event loop; the **Linux** tray path is a deliberate native-menu fallback.
Contributions that flesh out the Windows/Linux backends, the pure core, tests,
and docs are all welcome.

## Prerequisites

- A recent stable Rust toolchain (the crate pins a minimum via `rust-version`
  in `Cargo.toml`). Install with [rustup](https://rustup.rs).
- `rustfmt` and `clippy` components:

  ```sh
  rustup component add rustfmt clippy
  ```

## Build & test

```sh
cargo build --all-targets          # build the lib, example, and tests
cargo run --example usagio_menu     # print usagio's menu tree via the API
cargo test                          # run the data-model / layout / theme tests
cargo doc --no-deps --open          # render the API docs
```

## The quality gate (must pass before a PR merges)

CI runs exactly these three checks; run them locally first:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

- **`cargo fmt --all --check`** — formatting must be clean. Run `cargo fmt --all`
  to fix.
- **`cargo clippy ... -D warnings`** — clippy must be warning-free. No `#[allow]`
  escape hatches without a comment justifying them.
- **`cargo test`** — all tests (including doctests) must pass.
- **`#![deny(missing_docs)]`** is on: every public item needs a doc comment, so
  `cargo doc` stays clean.

The crate must **build and pass fmt + clippy at every commit**, not just at the
tip of a branch — keep each commit green.

## Commit conventions

Commits use a short `area: summary` subject line, lower-case, imperative mood,
e.g.:

```
menu: add Row::min_height builder
layout: resolve Flex::Grow leftover across multiple grow segments
docs: document the Linux tray-anchor carve-out
```

Keep commits focused and logically scoped. Reference issues with `#123` where
relevant.

Please do **not** add `Co-Authored-By` trailers or other tooling attribution to
commits.

## Submitting a pull request

1. Fork the repo and create a topic branch off `main`.
2. Make your change with tests and docs.
3. Ensure the full quality gate above passes locally.
4. Open a PR against `main` using the PR template; describe the change and the
   platforms you tested on (the renderer is macOS-first, so note whether a
   change is platform-agnostic data-model work or backend-specific).

## Design authority

muri follows a finalized design. Architectural decisions (the single
custom-drawn raster look on every OS, the Linux tray-anchor carve-out, the
`winit` + `softbuffer` + `tiny-skia` + `cosmic-text` stack, AccessKit-based
a11y) are fixed — see the README's design section. Propose changes to those via
an issue before a large PR.

## Code of conduct

By participating you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).
