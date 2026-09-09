//! Theming: the [`Theme`] surface, where it comes from ([`ThemeSource`]), the
//! per-popup [`MenuOptions`], and the pure resolution of a semantic [`Color`]
//! into a concrete [`Rgba`].
//!
//! Semantic colors (`Label`, `Accent`, `SystemRed`, …) are resolved against the
//! active theme at draw time. On the real backends `FollowSystem` pulls the live
//! OS dark/light appearance and accent; this pure layer resolves against a fixed
//! theme value (the built-in light/dark palettes, or a consumer's `Custom`
//! theme) and is fully unit-testable without any GUI.

use crate::geometry::Insets;
use crate::style::{Color, Font, Rgba, Weight};

/// Where the theme comes from.
#[derive(Clone, Debug, Default)]
pub enum ThemeSource {
    /// Track the OS dark/light appearance and accent color. The default.
    #[default]
    FollowSystem,
    /// Force the light theme.
    Light,
    /// Force the dark theme.
    Dark,
    /// Use a fully custom theme.
    Custom(Theme),
}

impl ThemeSource {
    /// Resolve the source to a concrete [`Theme`]. `FollowSystem` resolves to the
    /// light theme in this pure layer; the platform backend substitutes the live
    /// OS palette at runtime.
    pub fn resolve_theme(&self, system_is_dark: bool) -> Theme {
        match self {
            ThemeSource::FollowSystem => {
                if system_is_dark {
                    Theme::dark()
                } else {
                    Theme::light()
                }
            }
            ThemeSource::Light => Theme::light(),
            ThemeSource::Dark => Theme::dark(),
            ThemeSource::Custom(theme) => theme.clone(),
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

    #[test]
    fn theme_source_resolves() {
        assert_eq!(
            ThemeSource::FollowSystem
                .resolve_theme(true)
                .resolve(Color::Label),
            Rgba::WHITE
        );
        assert_eq!(
            ThemeSource::FollowSystem
                .resolve_theme(false)
                .resolve(Color::Label),
            Rgba::BLACK
        );
        assert_eq!(
            ThemeSource::Dark.resolve_theme(false).resolve(Color::Label),
            Rgba::WHITE
        );
    }
}
