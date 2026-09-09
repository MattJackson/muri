# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial crate scaffold (Phase 0): the complete public API surface as compiling
  types with `todo!()` renderer bodies — `Tray`, `ContextMenu`, `Menu`, `Item`,
  `Row`, `Segment`, `StyleRun`, `Align`, `Flex`, `Color`, `Font`/`FontFamily`/
  `Weight`, `Icon`, `Theme`/`ThemeSource`, `MenuOptions`, `Insets`, `Edge`,
  `LogicalPoint`, `MenuId`, `MenuEvent`, `Error`/`Unsupported`.
- Module structure toward the macOS-first implementation: a pure `menu` data
  model, `theme` (with semantic-color resolution), `layout` (flex/alignment
  resolution), `geometry`, `error`, a `render` scene-drawer trait, and a `tray`
  per-OS anchoring trait with macOS / Windows / Linux shim stubs.
- Unit tests for the pure data-model layer: menu building, `Flex`/`Align`
  resolution, semantic color/theme resolution, and the usagio `RowStyle` → muri
  mapping.
- `examples/usagio_menu.rs` rebuilds usagio's real tray menu through the API
  (provider headers with logos, flush-right colored percentages with no chevron
  column, per-account detail submenus, icons, and a greyed version tail).
- Standard open-source project files: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`,
  `SECURITY.md`, issue/PR templates, and Dependabot config.
- GitHub Actions CI running `cargo fmt --check`, `cargo clippy -D warnings`, and
  `cargo build` on macOS.

[Unreleased]: https://github.com/MattJackson/muri/commits/main
