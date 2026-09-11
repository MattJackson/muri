# Bundled UI-font substitutes (`bundled-fonts` feature)

muri can force a foreign platform look on any host (`ThemeSource::MacOs` /
`Windows` / `Gnome`). Pixel-parity for a forced look wants the *target* OS's UI
font, but the genuine originals for two of the three are proprietary and their
EULAs forbid redistribution (Apple **SF Pro**, Microsoft **Segoe UI**), so muri
cannot bundle them.

The **`bundled-fonts`** cargo feature (OFF by default) instead embeds the closest
freely-redistributable OSS substitutes below, and wires them as a *fallback tier*
in forced-theme font resolution: the real target font is still preferred when it
happens to be installed on the host; the bundled substitute is only used when it
is absent, and it always beats reaching for the wrong host UI font.

| Proprietary target | Bundled substitute | Why | License | Source |
| --- | --- | --- | --- | --- |
| Segoe UI (Windows) | **Selawik** | Microsoft's own metric-compatible OFL replacement for Segoe UI, released for exactly this cross-platform use | SIL OFL 1.1 | https://github.com/microsoft/Selawik |
| SF Pro (macOS) | **Inter** | The standard OSS grotesque substitute for San Francisco | SIL OFL 1.1 | https://github.com/rsms/inter |
| Cantarell (GNOME) | **Cantarell** (the genuine article) | Cantarell is itself OFL, so muri vendors the real font | SIL OFL 1.1 | https://gitlab.gnome.org/GNOME/cantarell-fonts (files taken from Google Fonts `ofl/cantarell`) |

All three are **SIL Open Font License 1.1**. Each family's license text is
vendored alongside its `.ttf` files (`OFL.txt` / `LICENSE.txt`) as the OFL
requires. Only the static Regular / SemiBold / Bold weights are vendored (no
italics, no variable fonts) to keep the embedded payload lean.

## Files

- `inter/` — `Inter-Regular.ttf`, `Inter-SemiBold.ttf`, `Inter-Bold.ttf`, `OFL.txt`
- `selawik/` — `Selawik-Regular.ttf`, `Selawik-SemiBold.ttf`, `Selawik-Bold.ttf`, `LICENSE.txt`
- `cantarell/` — `Cantarell-Regular.ttf`, `Cantarell-Bold.ttf`, `OFL.txt`

These are only compiled into the crate when the `bundled-fonts` feature is
enabled (via `include_bytes!`); the default build embeds none of them.
