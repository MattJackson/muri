//! Theming: the [`Theme`] surface, where it comes from ([`ThemeSource`]), the
//! per-popup [`MenuOptions`], and the pure resolution of a semantic [`Color`]
//! into a concrete [`Rgba`].
//!
//! A [`ThemeSource`] chooses the look: `System(..)` matches the host OS (and
//! alone receives the backend's live accent/color/font injection),
//! `MacOs`/`Windows`/`Gnome` force a specific platform look on any host,
//! `Preset` is a built-in skin, and `Custom` is fully hand-built. This pure
//! layer resolves a source to a concrete [`Theme`], fully unit-testable
//! without any GUI.

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

impl OsFamily {
    /// The target OS's real UI font family names, most-preferred first.
    ///
    /// This is a **target** face list, not a host-resolution list: it names what
    /// the *forced* platform look (issue #54) should render in, regardless of
    /// which OS muri is currently running on. Segoe UI/SF Pro are proprietary and
    /// not always installed, so this is only *tried first* (see
    /// [`crate::render::RasterDrawer::with_forced_theme`]); when absent, the
    /// renderer falls back to [`OsFamily::fallback_font_families`] rather than
    /// silently substituting the host's own native UI font.
    pub fn ui_font_families(&self) -> &'static [&'static str] {
        match self {
            OsFamily::MacOs => &[
                "SF Pro Text",
                "SF Pro",
                ".AppleSystemUIFont",
                "Helvetica Neue",
            ],
            OsFamily::Windows => &["Segoe UI"],
            OsFamily::Gnome => &["Cantarell", "Ubuntu"],
        }
    }

    /// Free, broadly-preinstalled families to try when none of
    /// [`ui_font_families`](OsFamily::ui_font_families) is installed — used only
    /// by a **forced** theme, which must never fall back to the host's own
    /// native UI font (that would silently reproduce issue #54).
    ///
    /// None of these are metrically identical to Segoe UI or SF Pro; muri does
    /// not claim metrics it can't achieve. GNOME's own Cantarell/Ubuntu are
    /// already listed in [`ui_font_families`](OsFamily::ui_font_families), so
    /// only macOS/Windows need a distinct fallback here.
    pub fn fallback_font_families(&self) -> &'static [&'static str] {
        match self {
            OsFamily::MacOs => &["DejaVu Sans", "Liberation Sans", "Noto Sans", "Arial"],
            OsFamily::Windows => &["Liberation Sans", "DejaVu Sans", "Noto Sans", "Arial"],
            OsFamily::Gnome => &["Cantarell", "Ubuntu", "DejaVu Sans", "Liberation Sans"],
        }
    }
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

    /// The [`OsFamily`] this source **forces**, if any — `Some` only for
    /// [`MacOs`](ThemeSource::MacOs) / [`Windows`](ThemeSource::Windows) /
    /// [`Gnome`](ThemeSource::Gnome); `None` for `System`/`Preset`/`Custom`.
    ///
    /// This is the seam a platform backend uses to pick the right drawer
    /// constructor (issue #54): when `Some(family)`, the popup must build its
    /// drawer with
    /// [`RasterDrawer::with_forced_theme`](crate::render::RasterDrawer::with_forced_theme)
    /// (pins the *target* OS's font) instead of
    /// [`RasterDrawer::new_native`](crate::render::RasterDrawer::new_native)
    /// (pins the *host's* font — correct only for `System(..)`).
    pub fn forced_family(&self) -> Option<OsFamily> {
        match self {
            ThemeSource::MacOs(_) => Some(OsFamily::MacOs),
            ThemeSource::Windows(_) => Some(OsFamily::Windows),
            ThemeSource::Gnome(_) => Some(OsFamily::Gnome),
            ThemeSource::System(_) | ThemeSource::Preset(_) | ThemeSource::Custom(_) => None,
        }
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
    /// Corner radius of the popup panel in logical points. (The hover highlight
    /// uses its own fixed selection radius, independent of this value.)
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

// -- macOS `NSMenu` (Big Sur+) geometry references (#57) ----------------------
//
// Named Big Sur+ menu metrics the [`Theme::macos`] preset targets (rows
// previously read too tall/loose). Values still needing a pixel-accurate
// capture are flagged `DEVICE-VERIFY(0.10.8)`.

/// Standard `NSMenu` item height in logical points. The core "rows too tall" fix
/// (#57). Native reference: AppKit's standard menu item height (~22pt on Big Sur+).
///
/// `pub(crate)` so the macOS backend's live-`System` row-pitch helper
/// (`macos_system_row_height` in `src/platform/mac.rs`) can floor against this
/// one baseline rather than duplicating the constant across the platform seam.
pub(crate) const MACOS_ROW_HEIGHT: f32 = 22.0;

/// The macOS menu font size in logical points. Native reference:
/// `+[NSFont menuFontOfSize:0]` (~13pt). The live System menu size still
/// overrides this at draw time; this is the headless / forced-theme base.
const MACOS_MENU_FONT_SIZE: f32 = 13.0;

/// Vertical content inset (top and bottom) of the popup, in logical points.
/// Native reference: the `NSMenu`'s ~4pt top/bottom content padding.
const MACOS_VERTICAL_INSET: f32 = 4.0;

/// Leading text inset (no checkmark gutter) in logical points — where a plain
/// row's text starts. Native reference: `NSMenu` text begins ~14pt in.
/// `pub(crate)` so the macOS backend's live native-menu metrics read can use it
/// as the fallback when the live read is unavailable (0.12.1). DEVICE-VERIFY(0.10.8).
pub(crate) const MACOS_LEADING_INSET: f32 = 14.0;

/// Horizontal gap between the leading icon, text segments, and the trailing
/// column, in logical points. Native reference: the `NSMenu` inter-column rhythm
/// (~8pt). DEVICE-VERIFY(0.10.8).
const MACOS_COLUMN_GAP: f32 = 8.0;

/// Popup corner radius in logical points for the forced/offscreen `Theme::macos`
/// preset. Native reference: the Big Sur..Sequoia `NSMenu` rounded-corner radius
/// (~6pt). `pub(crate)` so the macOS backend's live version-gated read
/// (`read_system_corner_radius` in `src/platform/mac.rs`) can reuse it as the
/// pre-Tahoe value while bumping the radius on Tahoe (#67), where Apple's
/// Liquid Glass redesign enlarged it. No public API exists for the live value,
/// so this stays a DEVICE-VERIFY estimate. DEVICE-VERIFY(0.10.8).
pub(crate) const MACOS_CORNER_RADIUS: f32 = 6.0;

/// Tracking (letter-spacing) as a fraction of the point size applied to macOS
/// San Francisco UI text tracking, in fraction-of-em per point (#42/#57/#66).
///
/// Device-verified against a live `NSMenu` on Tahoe (#66): a native menu adds
/// **no** extra tracking beyond the SF face's own advances. An earlier negative
/// value (`-0.012`) overcorrected (adjacent letters touched on the live path),
/// so `0.0` is now the single source of truth for the forced preset, its
/// offscreen goldens, and the live-system read path ([`macos_sf_tracking`]).
pub(crate) const MACOS_SF_TRACKING_FRACTION: f32 = 0.0;

/// Extra tracking in logical points for macOS SF UI text at `size` points.
/// See [`MACOS_SF_TRACKING_FRACTION`]. At the 13pt native menu size this is
/// ~-0.16pt of tightening.
pub(crate) fn macos_sf_tracking(size: f32) -> f32 {
    size * MACOS_SF_TRACKING_FRACTION
}

/// Tracking for the Windows 11 menu font (Segoe UI), in logical points. Segoe UI
/// menu text uses metrics-only spacing — no extra tracking — so this is `0.0`.
const SEGOE_UI_TRACKING: f32 = 0.0;

/// Tracking for the GNOME/Adwaita menu font (Cantarell/Ubuntu), in logical
/// points. Like Segoe UI, these use metrics-only spacing — no tracking — `0.0`.
const CANTARELL_TRACKING: f32 = 0.0;

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
    /// #6); every other field stays the same as `light()`. The alpha here is a
    /// fixed **~82%** (`209 / 255`), approximating the macOS vibrancy material.
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
    /// Metrics are the named `MACOS_*` reference constants (each documents its
    /// native `NSMenu` source); values still needing a pixel-accurate capture are
    /// flagged `DEVICE-VERIFY(0.10.8)` at their definition.
    pub fn macos(dark: bool) -> Theme {
        let base = if dark { Theme::dark() } else { Theme::light() };
        Theme {
            // Translucent so `NSVisualEffectView` vibrancy shows through.
            background: if dark {
                Color::Rgba(40, 40, 40, 204)
            } else {
                Color::Rgba(246, 246, 246, 209)
            },
            corner_radius: MACOS_CORNER_RADIUS,
            // Matched to the Big Sur+ `NSMenu` rhythm via the named `MACOS_*`
            // reference constants above (#57: earlier builds read too loose).
            row_height: MACOS_ROW_HEIGHT,
            padding: Insets::symmetric(MACOS_LEADING_INSET, MACOS_VERTICAL_INSET),
            column_gap: MACOS_COLUMN_GAP,
            // Bake SF tracking into the preset so a FORCED theme carries it on
            // any host; `platform::mac` overwrites (not adds) for the live size.
            row_font: Font::system(MACOS_MENU_FONT_SIZE, Weight::Regular)
                .with_letter_spacing(macos_sf_tracking(MACOS_MENU_FONT_SIZE))
                .with_optical_size(MACOS_MENU_FONT_SIZE),
            header_font: Font::system(MACOS_MENU_FONT_SIZE, Weight::Bold)
                .with_letter_spacing(macos_sf_tracking(MACOS_MENU_FONT_SIZE))
                .with_optical_size(MACOS_MENU_FONT_SIZE),
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
            // Sensible headless/fallback base; live OS point size overrides this.
            // Set tracking explicitly so a forced theme never inherits SF tracking.
            row_font: Font::system(14.0, Weight::Regular).with_letter_spacing(SEGOE_UI_TRACKING),
            header_font: Font::system(14.0, Weight::Bold).with_letter_spacing(SEGOE_UI_TRACKING),
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
                // Cantarell/Ubuntu use metrics-only spacing (no tracking); set
                // explicitly so a forced GNOME theme never inherits SF tracking.
                row_font: Font::system(11.0, Weight::Regular)
                    .with_letter_spacing(CANTARELL_TRACKING),
                header_font: Font::system(11.0, Weight::Bold)
                    .with_letter_spacing(CANTARELL_TRACKING),
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
                row_font: Font::system(11.0, Weight::Regular)
                    .with_letter_spacing(CANTARELL_TRACKING),
                header_font: Font::system(11.0, Weight::Bold)
                    .with_letter_spacing(CANTARELL_TRACKING),
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

/// How the popup reserves the leading (checkmark/icon) gutter (#43).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GutterPolicy {
    /// Reserve the shared gutter only when the menu has checkmarks, so checked and
    /// unchecked rows align (native `NSMenu`); an icon-only menu stays inline. The
    /// OEM default.
    #[default]
    Auto,
    /// Always reserve the gutter (every row's text aligns past it).
    Always,
    /// Never reserve a gutter — leading icons/checkmarks are per-row inline and
    /// only offset their own row.
    Never,
}

/// How the popup reserves the trailing (submenu chevron / trailing accessory)
/// gutter (#60). Symmetric to the leading [`GutterPolicy`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrailingGutterPolicy {
    /// Reserve the shared trailing column only when the menu needs it — it holds
    /// at least one submenu row (its `›` chevron) or a row carrying an explicit
    /// trailing accessory — so every row's right edge aligns past it (native
    /// `NSMenu`). A menu with neither reserves nothing and its right-aligned
    /// content reaches the true right edge. The OEM default.
    #[default]
    Auto,
    /// Always reserve the trailing column (every row's content ends short of the
    /// right edge, even with no submenu or trailing accessory anywhere).
    Always,
    /// Never reserve a trailing column — right-aligned content always reaches the
    /// edge. A submenu chevron (or trailing accessory), if present, still draws
    /// but overlays the normal content area rather than getting its own column,
    /// mirroring how the leading [`GutterPolicy::Never`] keeps icons inline.
    Never,
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
    /// Leading-gutter reservation policy (#43). Defaults to
    /// [`GutterPolicy::Auto`] — the OEM-native behavior.
    pub gutter: GutterPolicy,
    /// Trailing-gutter (submenu chevron / accessory column) reservation policy
    /// (#60). Defaults to [`TrailingGutterPolicy::Auto`] — the OEM-native
    /// behavior.
    pub trailing_gutter: TrailingGutterPolicy,
}

impl MenuOptions {
    /// Set the theme source.
    pub fn theme(mut self, t: ThemeSource) -> Self {
        self.theme = t;
        self
    }

    /// Set the leading-gutter reservation policy.
    pub fn gutter(mut self, g: GutterPolicy) -> Self {
        self.gutter = g;
        self
    }

    /// Set the trailing-gutter (submenu chevron / accessory column) reservation
    /// policy.
    pub fn trailing_gutter(mut self, g: TrailingGutterPolicy) -> Self {
        self.trailing_gutter = g;
        self
    }

    /// Set the minimum popup width, in logical points.
    ///
    /// `w` is clamped to be non-negative and non-`NaN`: a `NaN` input leaves
    /// `min_width` unset, and a negative input is clamped up to `0.0`. If a
    /// `max_width` is already set and `w` would exceed it, `max_width` is
    /// raised to match `w` so `min <= max` always holds.
    pub fn min_width(mut self, w: f32) -> Self {
        if w.is_nan() {
            return self;
        }
        let w = w.max(0.0);
        self.min_width = Some(w);
        if let Some(max) = self.max_width {
            if max < w {
                self.max_width = Some(w);
            }
        }
        self
    }

    /// Set the maximum popup width, in logical points.
    ///
    /// `w` is clamped to be non-negative and non-`NaN`: a `NaN` input leaves
    /// `max_width` unset, and a negative input is clamped up to `0.0`. If a
    /// `min_width` is already set and exceeds `w`, `w` is raised to match
    /// `min_width` so `min <= max` always holds.
    pub fn max_width(mut self, w: f32) -> Self {
        if w.is_nan() {
            return self;
        }
        let mut w = w.max(0.0);
        if let Some(min) = self.min_width {
            if w < min {
                w = min;
            }
        }
        self.max_width = Some(w);
        self
    }
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

    #[test]
    fn menu_options_builder_sets_theme_and_gutter() {
        let opts = MenuOptions::default()
            .theme(ThemeSource::Preset(Preset::Nord))
            .gutter(GutterPolicy::Always);
        assert!(matches!(opts.theme, ThemeSource::Preset(Preset::Nord)));
        assert!(matches!(opts.gutter, GutterPolicy::Always));
    }

    #[test]
    fn menu_options_trailing_gutter_defaults_to_auto() {
        let opts = MenuOptions::default();
        assert!(matches!(opts.trailing_gutter, TrailingGutterPolicy::Auto));
    }

    #[test]
    fn menu_options_builder_sets_trailing_gutter() {
        let opts = MenuOptions::default().trailing_gutter(TrailingGutterPolicy::Never);
        assert!(matches!(opts.trailing_gutter, TrailingGutterPolicy::Never));
    }

    #[test]
    fn menu_options_width_clamps_negative() {
        let opts = MenuOptions::default().min_width(-5.0).max_width(-1.0);
        assert_eq!(opts.min_width, Some(0.0));
        assert_eq!(opts.max_width, Some(0.0));
    }

    #[test]
    fn menu_options_width_rejects_nan() {
        let opts = MenuOptions::default()
            .min_width(f32::NAN)
            .max_width(f32::NAN);
        assert_eq!(opts.min_width, None);
        assert_eq!(opts.max_width, None);
    }

    #[test]
    fn menu_options_max_below_min_is_raised_to_min() {
        // max_width set first, then a larger min_width raises it.
        let opts = MenuOptions::default().max_width(50.0).min_width(100.0);
        assert_eq!(opts.min_width, Some(100.0));
        assert_eq!(opts.max_width, Some(100.0));
    }

    #[test]
    fn menu_options_min_above_existing_max_raises_max() {
        // min_width set first with a larger value, then max_width set smaller
        // gets raised back up to min.
        let opts = MenuOptions::default().min_width(80.0).max_width(20.0);
        assert_eq!(opts.min_width, Some(80.0));
        assert_eq!(opts.max_width, Some(80.0));
    }

    #[test]
    fn menu_options_normal_widths_pass_through() {
        let opts = MenuOptions::default().min_width(100.0).max_width(300.0);
        assert_eq!(opts.min_width, Some(100.0));
        assert_eq!(opts.max_width, Some(300.0));
    }

    /// Issue #54: each forced OS family names its own real UI font first —
    /// Segoe UI for Windows, an SF-Pro-family face for macOS, Cantarell/Ubuntu
    /// for GNOME — never the host's face.
    #[test]
    fn os_family_ui_font_families_name_the_target_os_font() {
        assert_eq!(OsFamily::Windows.ui_font_families(), &["Segoe UI"]);
        assert_eq!(
            OsFamily::MacOs.ui_font_families(),
            &[
                "SF Pro Text",
                "SF Pro",
                ".AppleSystemUIFont",
                "Helvetica Neue"
            ]
        );
        assert_eq!(OsFamily::Gnome.ui_font_families(), &["Cantarell", "Ubuntu"]);

        // Every family's fallback list is non-empty (there is always something
        // to try before giving up and leaving `FontFamily::System` unpinned).
        assert!(!OsFamily::MacOs.fallback_font_families().is_empty());
        assert!(!OsFamily::Windows.fallback_font_families().is_empty());
        assert!(!OsFamily::Gnome.fallback_font_families().is_empty());
    }

    /// `forced_family` is the platform-backend wiring seam: `Some` only for
    /// the three OS-forcing sources, `None` for everything else.
    #[test]
    fn forced_family_identifies_only_the_os_forcing_sources() {
        assert_eq!(
            ThemeSource::MacOs(ThemeMode::Auto).forced_family(),
            Some(OsFamily::MacOs)
        );
        assert_eq!(
            ThemeSource::Windows(ThemeMode::Auto).forced_family(),
            Some(OsFamily::Windows)
        );
        assert_eq!(
            ThemeSource::Gnome(ThemeMode::Auto).forced_family(),
            Some(OsFamily::Gnome)
        );
        assert_eq!(ThemeSource::System(ThemeMode::Auto).forced_family(), None);
        assert_eq!(ThemeSource::Preset(Preset::Nord).forced_family(), None);
        assert_eq!(ThemeSource::Custom(Box::default()).forced_family(), None);
    }

    /// #57: the macOS preset's geometry is pinned to the named Big Sur+ `NSMenu`
    /// reference constants, and light/dark share identical geometry (only colors
    /// differ between the two appearances).
    #[test]
    fn macos_metrics_match_nsmenu_reference() {
        // The reference constants themselves (guards against accidental drift).
        assert_eq!(MACOS_ROW_HEIGHT, 22.0); // NSMenu standard item height
        assert_eq!(MACOS_MENU_FONT_SIZE, 13.0); // +[NSFont menuFontOfSize:0]
        assert_eq!(MACOS_VERTICAL_INSET, 4.0); // ~4pt top/bottom content inset
        assert_eq!(MACOS_LEADING_INSET, 14.0); // ~14pt leading text inset
        assert_eq!(MACOS_COLUMN_GAP, 8.0); // ~8pt inter-column gap
        assert_eq!(MACOS_CORNER_RADIUS, 6.0); // Big Sur ~6pt radius

        // The preset wires each constant into the right `Theme` field.
        let mac = Theme::macos(false);
        assert_eq!(mac.row_height, MACOS_ROW_HEIGHT);
        assert_eq!(mac.corner_radius, MACOS_CORNER_RADIUS);
        assert_eq!(mac.column_gap, MACOS_COLUMN_GAP);
        assert_eq!(mac.padding.left, MACOS_LEADING_INSET);
        assert_eq!(mac.padding.right, MACOS_LEADING_INSET);
        assert_eq!(mac.padding.top, MACOS_VERTICAL_INSET);
        assert_eq!(mac.padding.bottom, MACOS_VERTICAL_INSET);
        assert_eq!(mac.row_font.size, MACOS_MENU_FONT_SIZE);
        assert_eq!(mac.header_font.size, MACOS_MENU_FONT_SIZE);

        // Light/dark geometry parity: same metrics, only the palette differs.
        let dark = Theme::macos(true);
        assert_eq!(mac.row_height, dark.row_height);
        assert_eq!(mac.corner_radius, dark.corner_radius);
        assert_eq!(mac.column_gap, dark.column_gap);
        assert_eq!(mac.padding.left, dark.padding.left);
        assert_eq!(mac.padding.top, dark.padding.top);
        assert_eq!(mac.row_font, dark.row_font);
        assert_eq!(mac.header_font, dark.header_font);
    }

    /// #57/#66: forced-theme tracking travels with the theme — the forced macOS
    /// preset carries the single-source-of-truth [`macos_sf_tracking`] value on any
    /// host (not only when the live system font is read). Device-verified (#66),
    /// that value is now metrics-only `0`, matching native NSMenu and every other
    /// OS preset; the invariant is that the preset equals the shared function, not
    /// that it is non-zero.
    #[test]
    fn forced_theme_tracking_travels_with_the_preset() {
        let mac = Theme::macos(false);
        // macOS SF tracking is the shared value, baked into both fonts (now 0, #66).
        assert_eq!(
            mac.row_font.letter_spacing,
            macos_sf_tracking(MACOS_MENU_FONT_SIZE)
        );
        assert_eq!(
            mac.header_font.letter_spacing,
            macos_sf_tracking(MACOS_MENU_FONT_SIZE)
        );
        // Same for the dark variant — forced on any host.
        assert_eq!(
            Theme::macos(true).row_font.letter_spacing,
            macos_sf_tracking(MACOS_MENU_FONT_SIZE)
        );

        // Windows / GNOME use metrics-only spacing (Segoe UI / Cantarell ≈ 0).
        for win in [Theme::windows(false), Theme::windows(true)] {
            assert_eq!(win.row_font.letter_spacing, SEGOE_UI_TRACKING);
            assert_eq!(win.header_font.letter_spacing, SEGOE_UI_TRACKING);
            assert_eq!(win.row_font.letter_spacing, 0.0);
        }
        for gnome in [Theme::gnome(false), Theme::gnome(true)] {
            assert_eq!(gnome.row_font.letter_spacing, CANTARELL_TRACKING);
            assert_eq!(gnome.header_font.letter_spacing, CANTARELL_TRACKING);
            assert_eq!(gnome.row_font.letter_spacing, 0.0);
        }
    }

    /// #77: the macOS preset opts into optical sizing at the menu point size (so
    /// SF renders the *Text* master, not the condensed *Display* default), while
    /// Windows / GNOME leave it `None` — the mechanism is general (Segoe UI
    /// Variable has `opsz` too) but the policy is macOS-only for now.
    #[test]
    fn macos_preset_sets_optical_size_others_leave_it_default() {
        for mac in [Theme::macos(false), Theme::macos(true)] {
            assert_eq!(mac.row_font.optical_size, Some(MACOS_MENU_FONT_SIZE));
            assert_eq!(mac.header_font.optical_size, Some(MACOS_MENU_FONT_SIZE));
        }
        for t in [
            Theme::windows(false),
            Theme::windows(true),
            Theme::gnome(false),
            Theme::gnome(true),
        ] {
            assert_eq!(t.row_font.optical_size, None);
            assert_eq!(t.header_font.optical_size, None);
        }
    }

    /// #57: the live-system read path and the forced-preset path yield a single,
    /// consistent tracking value — no double application. The preset bakes in
    /// `macos_sf_tracking(13pt)`; the System path OVERWRITES (assign, not add) with
    /// `macos_sf_tracking(live_size)`. At the same size the two must match exactly,
    /// and re-applying (overwriting) must be idempotent (never compound).
    #[test]
    fn macos_tracking_reconciles_system_and_forced_paths() {
        // Forced preset value (baked in at the 13pt base).
        let preset = Theme::macos(false).row_font.letter_spacing;
        // System path recomputes for the live size; at the same 13pt size it is
        // numerically identical (single source of truth).
        assert_eq!(preset, macos_sf_tracking(MACOS_MENU_FONT_SIZE));

        // Overwrite (assign) is idempotent — no compounding on top of the preset.
        let mut font = Theme::macos(false).row_font;
        let once = font.letter_spacing;
        font.letter_spacing = macos_sf_tracking(font.size);
        assert_eq!(font.letter_spacing, once);
        // Device-verified (#66): native NSMenu adds no tracking beyond the font's
        // own metrics, so the fraction is 0 and every size resolves to 0.
        assert_eq!(MACOS_SF_TRACKING_FRACTION, 0.0);
        assert_eq!(macos_sf_tracking(13.0), 0.0);
        assert_eq!(macos_sf_tracking(13.0), macos_sf_tracking(15.0));
    }

    #[test]
    fn menu_options_struct_literal_still_compiles() {
        // Backward compatibility: bare struct-literal construction must still work.
        let opts = MenuOptions {
            min_width: Some(10.0),
            max_width: Some(20.0),
            theme: ThemeSource::default(),
            gutter: GutterPolicy::Never,
            trailing_gutter: TrailingGutterPolicy::Never,
        };
        assert_eq!(opts.min_width, Some(10.0));
    }
}
