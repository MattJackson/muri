//! Theming: the [`Theme`] surface, where it comes from ([`ThemeSource`]), the
//! per-popup [`MenuOptions`], and the pure resolution of a semantic [`Color`]
//! into a concrete [`Rgba`].
//!
//! Semantic colors (`Label`, `Accent`, `SystemRed`, …) are resolved against the
//! active theme at draw time. A [`ThemeSource`] chooses the look: `System(..)`
//! matches the host OS (and alone receives the backend's live accent/color/font
//! injection), `MacOs`/`Windows`/`Gnome` force a specific platform look on any
//! host, `Preset` is a built-in skin, and `Custom` is fully hand-built. This pure
//! layer resolves a source to a concrete [`Theme`] against fixed inputs and is
//! fully unit-testable without any GUI.

use crate::geometry::Insets;
use crate::style::{Color, Font, Rgba, Weight};

/// A platform's native menu look. Selected by [`ThemeSource`] and built by
/// [`Theme::for_family`]. The host family is supplied by the platform backend
/// (no `cfg` crosses the seam), so a caller can also request a *foreign* family
/// — e.g. a macOS app asking for the [`Windows`](OsFamily::Windows) look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsFamily {
    /// The macOS `NSMenu` look (vibrancy, SF, tight rows).
    MacOs,
    /// The Windows 11 menu-flyout look (acrylic, Segoe UI, rounded).
    Windows,
    /// The GNOME/Adwaita popover look (flat, rounded, roomy).
    Gnome,
}

/// Light/dark selection for a themed OS look. `Auto` follows the host OS
/// appearance; `Light`/`Dark` force it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    /// Follow the OS light/dark setting. The default.
    #[default]
    Auto,
    /// Force the light palette.
    Light,
    /// Force the dark palette.
    Dark,
}

impl ThemeMode {
    /// Resolve to a concrete dark/light boolean against the live OS appearance.
    fn is_dark(self, system_is_dark: bool) -> bool {
        match self {
            ThemeMode::Auto => system_is_dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        }
    }
}

/// A built-in non-OS theme skin, selectable via [`ThemeSource::Preset`]. These
/// render as-authored on every platform (no live OS accent/color/font injection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Green-on-black monospace, an old-school terminal.
    OldSchoolTerminal,
    /// Maximum-contrast black background / white text for accessibility.
    HighContrast,
    /// The Solarized Dark palette.
    Solarized,
    /// The Nord (Polar Night) palette.
    Nord,
}

impl Preset {
    /// The concrete [`Theme`] for this preset.
    pub fn theme(self) -> Theme {
        match self {
            Preset::OldSchoolTerminal => Theme {
                background: Color::rgb(0, 0, 0),
                label: Color::rgb(51, 255, 51),
                secondary_label: Color::rgb(0, 170, 0),
                accent: Color::rgb(0, 200, 0),
                separator: Color::rgb(0, 90, 0),
                row_highlight: Color::Accent,
                row_font: Font::mono(13.0, Weight::Regular),
                header_font: Font::mono(13.0, Weight::Bold),
                row_height: 22.0,
                corner_radius: 0.0,
                padding: Insets::symmetric(6.0, 4.0),
                column_gap: 10.0,
            },
            Preset::HighContrast => Theme {
                background: Color::rgb(0, 0, 0),
                label: Color::rgb(255, 255, 255),
                secondary_label: Color::rgb(220, 220, 220),
                accent: Color::rgb(255, 255, 0),
                separator: Color::rgb(255, 255, 255),
                row_highlight: Color::Accent,
                row_font: Font::system(13.0, Weight::Regular),
                header_font: Font::system(13.0, Weight::Bold),
                row_height: 24.0,
                corner_radius: 0.0,
                padding: Insets::symmetric(8.0, 5.0),
                column_gap: 12.0,
            },
            Preset::Solarized => Theme {
                background: Color::rgb(0, 43, 54),         // base03
                label: Color::rgb(147, 161, 161),          // base1
                secondary_label: Color::rgb(88, 110, 117), // base01
                accent: Color::rgb(38, 139, 210),          // blue
                separator: Color::rgb(7, 54, 66),          // base02
                row_highlight: Color::Accent,
                row_font: Font::system(13.0, Weight::Regular),
                header_font: Font::system(13.0, Weight::Bold),
                row_height: 24.0,
                corner_radius: 6.0,
                padding: Insets::symmetric(8.0, 5.0),
                column_gap: 12.0,
            },
            Preset::Nord => Theme {
                background: Color::rgb(46, 52, 64),         // nord0
                label: Color::rgb(216, 222, 233),           // nord4
                secondary_label: Color::rgb(143, 153, 172), // nord3-ish
                accent: Color::rgb(136, 192, 208),          // nord8 frost
                separator: Color::rgb(59, 66, 82),          // nord1
                row_highlight: Color::Accent,
                row_font: Font::system(13.0, Weight::Regular),
                header_font: Font::system(13.0, Weight::Bold),
                row_height: 24.0,
                corner_radius: 8.0,
                padding: Insets::symmetric(8.0, 5.0),
                column_gap: 12.0,
            },
        }
    }
}

/// Where the theme comes from.
///
/// - [`System`](ThemeSource::System) matches the **host** OS look and is the only
///   source that gets live OS accent/color/font injection (the native default).
/// - [`MacOs`](ThemeSource::MacOs) / [`Windows`](ThemeSource::Windows) /
///   [`Gnome`](ThemeSource::Gnome) force a specific platform look on **any** host
///   (a macOS app can render a Windows menu), rendered as-authored.
/// - [`Preset`](ThemeSource::Preset) is a built-in non-OS skin;
///   [`Custom`](ThemeSource::Custom) is a fully hand-built [`Theme`].
#[derive(Clone, Debug)]
pub enum ThemeSource {
    /// Match the host OS look; `System(Auto)` (the default) follows light/dark.
    System(ThemeMode),
    /// Force the macOS look on any host.
    MacOs(ThemeMode),
    /// Force the Windows look on any host.
    Windows(ThemeMode),
    /// Force the GNOME/Adwaita look on any host.
    Gnome(ThemeMode),
    /// A built-in non-OS preset skin.
    Preset(Preset),
    /// A fully custom theme.
    Custom(Box<Theme>),
}

impl Default for ThemeSource {
    fn default() -> Self {
        ThemeSource::System(ThemeMode::Auto)
    }
}

impl ThemeSource {
    /// Whether this source should receive live OS injection (accent, menu colors,
    /// real system font) — true only for [`System`](ThemeSource::System), i.e.
    /// when the caller asked to match *their* OS. Explicit family / preset /
    /// custom themes render exactly as authored.
    pub fn injects_system(&self) -> bool {
        matches!(self, ThemeSource::System(_))
    }

    /// Resolve to a concrete [`Theme`], given the **host** OS family and its live
    /// dark/light appearance (both supplied by the platform backend; the pure
    /// layer can pass any fixed values for testing). Injection of live OS
    /// accent/colors/font is layered on by the backend afterwards (see
    /// [`injects_system`](ThemeSource::injects_system)).
    pub fn resolve(&self, host: OsFamily, system_is_dark: bool) -> Theme {
        match self {
            ThemeSource::System(mode) => Theme::for_family(host, mode.is_dark(system_is_dark)),
            ThemeSource::MacOs(mode) => {
                Theme::for_family(OsFamily::MacOs, mode.is_dark(system_is_dark))
            }
            ThemeSource::Windows(mode) => {
                Theme::for_family(OsFamily::Windows, mode.is_dark(system_is_dark))
            }
            ThemeSource::Gnome(mode) => {
                Theme::for_family(OsFamily::Gnome, mode.is_dark(system_is_dark))
            }
            ThemeSource::Preset(p) => p.theme(),
            ThemeSource::Custom(theme) => (**theme).clone(),
        }
    }
}

/// The full visual theme. Every semantic [`Color`] resolves against one of these,
/// and spacing/radius/row-height are tunable so a consumer can fully restyle.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Popup background fill.
    pub background: Color,
    /// Primary text color.
    pub label: Color,
    /// De-emphasized text color.
    pub secondary_label: Color,
    /// Accent color (selection, checkmarks).
    pub accent: Color,
    /// Separator / hairline color.
    pub separator: Color,
    /// Row background when hovered/selected.
    pub row_highlight: Color,
    /// Default row font.
    pub row_font: Font,
    /// Section-header font.
    pub header_font: Font,
    /// Default row height in logical points.
    pub row_height: f32,
    /// Corner radius of the popup and highlight in logical points.
    pub corner_radius: f32,
    /// Inner padding of the popup.
    pub padding: Insets,
    /// Horizontal gap between leading icon, segments, and trailing column.
    pub column_gap: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}

/// The default concrete accent used when the theme's accent is itself the
/// semantic [`Color::Accent`] (i.e. the live OS accent hasn't been injected). A
/// neutral system blue.
const ACCENT_FALLBACK: Rgba = Rgba::opaque(0, 122, 255);

impl Theme {
    /// The default light theme.
    pub fn light() -> Self {
        Theme {
            background: Color::rgb(246, 246, 246),
            label: Color::rgb(0, 0, 0),
            secondary_label: Color::rgb(140, 140, 140),
            accent: Color::Accent,
            separator: Color::rgb(210, 210, 210),
            row_highlight: Color::Accent,
            row_font: Font::system(13.0, Weight::Regular),
            header_font: Font::system(13.0, Weight::Bold),
            row_height: 22.0,
            corner_radius: 8.0,
            padding: Insets::symmetric(6.0, 5.0),
            column_gap: 10.0,
        }
    }

    /// The default dark theme.
    pub fn dark() -> Self {
        Theme {
            background: Color::rgb(40, 40, 40),
            label: Color::rgb(255, 255, 255),
            secondary_label: Color::rgb(150, 150, 150),
            separator: Color::rgb(70, 70, 70),
            ..Theme::light()
        }
    }

    /// The native light theme: [`light()`](Theme::light) with a **translucent**
    /// background so OS vibrancy shows through.
    ///
    /// The platform present path composites the raster surface over a native
    /// effect backdrop (`NSVisualEffectView` on macOS, DWM acrylic on Windows)
    /// using per-pixel alpha (spec `10-rendering-layout.md` §11, locked decision
    /// #6). Setting `background` to a reduced-alpha [`Color::Rgba`] — rather
    /// than the opaque literal `light()` uses — is what lets that backdrop blur
    /// through the panel; every other field stays the same opaque/semantic
    /// value as `light()`. The alpha here is a fixed **~82%** (`209 / 255`), a
    /// reasonable approximation of the macOS menu vibrancy material.
    pub fn native() -> Self {
        Theme {
            background: Color::Rgba(246, 246, 246, 209),
            ..Theme::light()
        }
    }

    /// The native dark theme: [`dark()`](Theme::dark) with a **translucent**
    /// background so OS vibrancy shows through.
    ///
    /// See [`native()`](Theme::native) for the mechanism. The alpha here is a
    /// fixed **~80%** (`204 / 255`), a reasonable approximation of the macOS
    /// dark menu vibrancy material.
    pub fn native_dark() -> Self {
        Theme {
            background: Color::Rgba(40, 40, 40, 204),
            ..Theme::dark()
        }
    }

    /// The macOS menu theme (Big Sur+ `NSMenu`): translucent vibrancy background,
    /// tight rows, ~6pt corner radius, SF at the OS menu size. The platform
    /// backend injects the live accent, label/separator colors (`NSColor`), and
    /// the SF face + point size on top; these are the native-accurate defaults.
    ///
    /// DEVICE-VERIFY: metrics are matched to the macOS menu by eye; nudge here.
    pub fn macos(dark: bool) -> Theme {
        let base = if dark { Theme::dark() } else { Theme::light() };
        Theme {
            // Translucent so `NSVisualEffectView` vibrancy shows through.
            background: if dark {
                Color::Rgba(40, 40, 40, 204)
            } else {
                Color::Rgba(246, 246, 246, 209)
            },
            corner_radius: 6.0,
            row_height: 22.0,
            padding: Insets::symmetric(6.0, 5.0),
            column_gap: 10.0,
            row_font: Font::system(13.0, Weight::Regular),
            header_font: Font::system(13.0, Weight::Bold),
            ..base
        }
    }

    /// The Windows 11 menu-flyout theme: acrylic (translucent) background,
    /// generously padded rows, 8pt rounded corners, Segoe UI at the OS size. The
    /// Windows backend injects the live accent (`DwmGetColorizationColor`),
    /// menu text/graytext colors (`GetSysColor`), and the Segoe UI face + size.
    ///
    /// DEVICE-VERIFY: metrics matched to the Win11 flyout by eye; nudge here.
    pub fn windows(dark: bool) -> Theme {
        let base = if dark { Theme::dark() } else { Theme::light() };
        Theme {
            // Win11 menus are acrylic; keep a translucent fill for the DWM
            // backdrop, a touch more opaque than macOS vibrancy.
            background: if dark {
                Color::Rgba(43, 43, 43, 214)
            } else {
                Color::Rgba(249, 249, 249, 214)
            },
            corner_radius: 8.0,
            row_height: 28.0,
            padding: Insets::symmetric(6.0, 4.0),
            column_gap: 12.0,
            // Segoe UI at 9pt is the Win11 menu default; the live OS point size
            // overrides this, but a sensible base for the headless/fallback path.
            row_font: Font::system(14.0, Weight::Regular),
            header_font: Font::system(14.0, Weight::Bold),
            ..base
        }
    }

    /// The GNOME/Adwaita menu theme (GTK4 popover): a **flat, opaque** surface
    /// (Linux/X11 has no vibrancy backdrop), 12pt rounded corners, roomy rows,
    /// Adwaita colors. The Linux backend injects the live accent and, where the
    /// desktop exposes it, the UI font; menu text colors are Adwaita constants
    /// (GNOME does not expose them over `gsettings`).
    ///
    /// DEVICE-VERIFY: metrics matched to the Adwaita popover by eye; nudge here.
    pub fn gnome(dark: bool) -> Theme {
        if dark {
            Theme {
                background: Color::rgb(48, 48, 48),
                label: Color::rgb(255, 255, 255),
                secondary_label: Color::rgb(154, 153, 150),
                separator: Color::rgb(61, 61, 61),
                accent: Color::Accent,
                row_highlight: Color::Accent,
                row_font: Font::system(11.0, Weight::Regular),
                header_font: Font::system(11.0, Weight::Bold),
                row_height: 30.0,
                corner_radius: 12.0,
                padding: Insets::symmetric(6.0, 6.0),
                column_gap: 12.0,
            }
        } else {
            Theme {
                background: Color::rgb(250, 250, 250),
                label: Color::rgb(0, 0, 0),
                secondary_label: Color::rgb(119, 118, 123),
                separator: Color::rgb(226, 226, 226),
                accent: Color::Accent,
                row_highlight: Color::Accent,
                row_font: Font::system(11.0, Weight::Regular),
                header_font: Font::system(11.0, Weight::Bold),
                row_height: 30.0,
                corner_radius: 12.0,
                padding: Insets::symmetric(6.0, 6.0),
                column_gap: 12.0,
            }
        }
    }

    /// The native base theme for an [`OsFamily`] in the given mode. Dispatches to
    /// [`macos`](Theme::macos) / [`windows`](Theme::windows) / [`gnome`](Theme::gnome).
    pub fn for_family(family: OsFamily, dark: bool) -> Theme {
        match family {
            OsFamily::MacOs => Theme::macos(dark),
            OsFamily::Windows => Theme::windows(dark),
            OsFamily::Gnome => Theme::gnome(dark),
        }
    }

    /// Force the background fully opaque (alpha 255), preserving its RGB. Used
    /// when the user has disabled OS transparency/vibrancy so the menu is solid
    /// instead of translucent-over-nothing.
    pub fn make_opaque(&mut self) {
        let (r, g, b) = match self.background {
            Color::Rgba(r, g, b, _) => (r, g, b),
            _ => return,
        };
        self.background = Color::Rgba(r, g, b, 255);
    }

    /// Resolve a (possibly semantic) [`Color`] into a concrete [`Rgba`].
    ///
    /// Literal `Rgba` colors pass through unchanged. Semantic roles map to the
    /// matching theme field (or a fixed fallback for the system palette and for
    /// the accent when it hasn't been populated from the OS).
    pub fn resolve(&self, color: Color) -> Rgba {
        match color {
            Color::Rgba(r, g, b, a) => Rgba::new(r, g, b, a),
            Color::Label => literal(self.label, Rgba::BLACK),
            Color::SecondaryLabel => literal(self.secondary_label, Rgba::opaque(140, 140, 140)),
            Color::Accent => literal(self.accent, ACCENT_FALLBACK),
            Color::Separator => literal(self.separator, Rgba::opaque(210, 210, 210)),
            Color::SystemRed => Rgba::opaque(255, 59, 48),
            Color::SystemOrange => Rgba::opaque(255, 149, 0),
            Color::SystemGreen => Rgba::opaque(52, 199, 89),
            Color::SystemYellow => Rgba::opaque(255, 204, 0),
        }
    }
}

/// Resolve a theme field that is expected to be a literal color, falling back to
/// a fixed value if it is itself semantic (which would otherwise recurse).
fn literal(color: Color, fallback: Rgba) -> Rgba {
    match color {
        Color::Rgba(r, g, b, a) => Rgba::new(r, g, b, a),
        _ => fallback,
    }
}

/// Tunable popup options layered on top of the [`Theme`].
#[derive(Clone, Debug, Default)]
pub struct MenuOptions {
    /// Minimum popup width in logical points.
    pub min_width: Option<f32>,
    /// Maximum popup width in logical points.
    pub max_width: Option<f32>,
    /// Theme source.
    pub theme: ThemeSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_color_passes_through() {
        let t = Theme::light();
        assert_eq!(t.resolve(Color::Rgba(1, 2, 3, 4)), Rgba::new(1, 2, 3, 4));
        assert_eq!(t.resolve(Color::rgb(10, 20, 30)), Rgba::opaque(10, 20, 30));
    }

    #[test]
    fn label_resolves_per_theme() {
        assert_eq!(Theme::light().resolve(Color::Label), Rgba::BLACK);
        assert_eq!(Theme::dark().resolve(Color::Label), Rgba::WHITE);
    }

    #[test]
    fn separator_differs_between_light_and_dark() {
        let light = Theme::light().resolve(Color::Separator);
        let dark = Theme::dark().resolve(Color::Separator);
        assert_ne!(light, dark);
    }

    /// The three per-OS base themes are genuinely distinct native looks: their
    /// metrics differ (each matched to its platform's menu), macOS/Windows keep a
    /// translucent background for vibrancy/acrylic while GNOME is flat opaque, and
    /// each honors dark/light.
    #[test]
    fn per_os_themes_are_distinct() {
        let mac = Theme::macos(false);
        let win = Theme::windows(false);
        let gnome = Theme::gnome(false);

        // Corner radius: macOS 6, Win11 8, Adwaita 12 — all different.
        assert_eq!(mac.corner_radius, 6.0);
        assert_eq!(win.corner_radius, 8.0);
        assert_eq!(gnome.corner_radius, 12.0);

        // Row height: Windows/GNOME are roomier than the tight macOS menu.
        assert!(win.row_height > mac.row_height);
        assert!(gnome.row_height > mac.row_height);

        // macOS and Windows backgrounds are translucent (alpha < 255) for the
        // vibrancy/acrylic backdrop; GNOME is a flat opaque fill (no Linux
        // vibrancy).
        assert!(matches!(mac.background, Color::Rgba(_, _, _, a) if a < 255));
        assert!(matches!(win.background, Color::Rgba(_, _, _, a) if a < 255));
        assert!(matches!(gnome.background, Color::Rgba(_, _, _, 255)));

        // Dark variants differ from light for every OS.
        assert_ne!(
            Theme::gnome(true).resolve(Color::Label),
            Theme::gnome(false).resolve(Color::Label)
        );
        assert_ne!(
            Theme::macos(true).resolve(Color::Separator),
            Theme::macos(false).resolve(Color::Separator)
        );
    }

    /// A [`SystemPalette`](crate::platform::SystemPalette) applies only its set
    /// fields, leaving unset fields at the theme's base value.
    #[test]
    fn system_palette_applies_only_set_fields() {
        use crate::platform::SystemPalette;
        let mut theme = Theme::macos(false);
        let base_bg = theme.background;
        let pal = SystemPalette {
            label: Some((10, 20, 30, 255)),
            separator: Some((1, 2, 3, 40)),
            ..SystemPalette::default()
        };
        pal.apply_to(&mut theme);
        assert_eq!(theme.resolve(Color::Label), Rgba::new(10, 20, 30, 255));
        assert_eq!(theme.resolve(Color::Separator), Rgba::new(1, 2, 3, 40));
        // Unset fields untouched.
        assert_eq!(theme.background, base_bg);
    }

    #[test]
    fn system_colors_are_fixed_regardless_of_theme() {
        assert_eq!(
            Theme::light().resolve(Color::SystemRed),
            Theme::dark().resolve(Color::SystemRed)
        );
        assert_eq!(
            Theme::light().resolve(Color::SystemRed),
            Rgba::opaque(255, 59, 48)
        );
    }

    #[test]
    fn accent_does_not_recurse_and_has_a_fallback() {
        // Both default themes set accent = Color::Accent; resolving must not loop
        // and must yield the concrete fallback.
        assert_eq!(Theme::light().resolve(Color::Accent), ACCENT_FALLBACK);
    }

    #[test]
    fn custom_accent_is_honored() {
        let mut theme = Theme::light();
        theme.accent = Color::rgb(200, 0, 100);
        assert_eq!(theme.resolve(Color::Accent), Rgba::opaque(200, 0, 100));
    }

    /// Extract the alpha channel of a theme's `background` field, panicking if
    /// it isn't a literal `Rgba` (all of `light()`/`dark()`/`native()`/
    /// `native_dark()` set a literal background).
    fn background_alpha(theme: &Theme) -> u8 {
        match theme.background {
            Color::Rgba(_, _, _, a) => a,
            other => panic!("expected a literal Rgba background, got {other:?}"),
        }
    }

    #[test]
    fn light_and_dark_backgrounds_are_opaque() {
        assert_eq!(background_alpha(&Theme::light()), 255);
        assert_eq!(background_alpha(&Theme::dark()), 255);
    }

    #[test]
    fn native_backgrounds_are_translucent() {
        assert!(background_alpha(&Theme::native()) < 255);
        assert!(background_alpha(&Theme::native_dark()) < 255);
    }

    #[test]
    fn native_mirrors_light_and_dark_for_non_background_fields() {
        // Only `background` should differ from light()/dark(); every other
        // field (label/accent/separator/fonts/metrics) stays the same
        // semantic/opaque value.
        let light = Theme::light();
        let native = Theme::native();
        assert_eq!(native.label, light.label);
        assert_eq!(native.accent, light.accent);
        assert_eq!(native.separator, light.separator);
        assert_eq!(native.row_highlight, light.row_highlight);
        assert_eq!(native.row_font, light.row_font);
        assert_eq!(native.corner_radius, light.corner_radius);

        let dark = Theme::dark();
        let native_dark = Theme::native_dark();
        assert_eq!(native_dark.label, dark.label);
        assert_eq!(native_dark.separator, dark.separator);
    }

    #[test]
    fn system_source_resolves_to_host_family_and_appearance() {
        // `System(Auto)` resolves to the HOST family's theme and follows the live
        // appearance; only `System(..)` is subject to the backend's live injection.
        let dark = ThemeSource::System(ThemeMode::Auto).resolve(OsFamily::MacOs, true);
        assert_eq!(dark.resolve(Color::Label), Rgba::WHITE);
        // macOS base is translucent for vibrancy.
        assert!(matches!(dark.background, Color::Rgba(_, _, _, a) if a < 255));

        let light = ThemeSource::System(ThemeMode::Auto).resolve(OsFamily::MacOs, false);
        assert_eq!(light.resolve(Color::Label), Rgba::BLACK);
    }

    #[test]
    fn theme_source_resolves() {
        // `System(Auto)` follows appearance...
        assert_eq!(
            ThemeSource::System(ThemeMode::Auto)
                .resolve(OsFamily::MacOs, true)
                .resolve(Color::Label),
            Rgba::WHITE
        );
        assert_eq!(
            ThemeSource::System(ThemeMode::Auto)
                .resolve(OsFamily::MacOs, false)
                .resolve(Color::Label),
            Rgba::BLACK
        );
        // ...a forced mode ignores the system appearance...
        assert_eq!(
            ThemeSource::System(ThemeMode::Dark)
                .resolve(OsFamily::MacOs, false)
                .resolve(Color::Label),
            Rgba::WHITE
        );
        // ...and a foreign family renders regardless of host (Windows on a Mac).
        let win = ThemeSource::Windows(ThemeMode::Light).resolve(OsFamily::MacOs, true);
        assert_eq!(win.corner_radius, 8.0);
        assert!(!ThemeSource::Windows(ThemeMode::Light).injects_system());
        assert!(ThemeSource::System(ThemeMode::Auto).injects_system());
    }

    #[test]
    fn presets_render_distinct_skins() {
        // Presets are as-authored, no live injection.
        assert!(!ThemeSource::Preset(Preset::Nord).injects_system());
        let term = Preset::OldSchoolTerminal.theme();
        // Green-on-black terminal: label is green, background opaque black.
        assert_eq!(term.resolve(Color::Label), Rgba::opaque(51, 255, 51));
        assert!(matches!(term.background, Color::Rgba(0, 0, 0, 255)));
        let nord = Preset::Nord.theme();
        assert_ne!(nord.resolve(Color::Label), term.resolve(Color::Label));
    }
}
