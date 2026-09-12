//! Visual style primitives: colors, fonts, and weights. These are the building
//! blocks a consumer attaches to segments and rows; they are resolved against a
//! [`Theme`](crate::Theme) into concrete pixels by the renderer.

/// A color: either a literal RGBA value, or a **semantic** role that resolves
/// against the active [`Theme`](crate::Theme) (and, on macOS, to the matching
/// system `NSColor`) so dark/light and accent adapt automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// A literal 8-bit-per-channel RGBA color.
    Rgba(u8, u8, u8, u8),
    /// Primary text color (`labelColor`).
    Label,
    /// De-emphasized text color (`secondaryLabelColor`), e.g. a version tail.
    SecondaryLabel,
    /// The user's accent color (`controlAccentColor`).
    Accent,
    /// Separator / hairline color.
    Separator,
    /// System red — used for critical/over-limit severity.
    SystemRed,
    /// System orange — used for warning severity.
    SystemOrange,
    /// System green — used for healthy severity.
    SystemGreen,
    /// System yellow.
    SystemYellow,
}

impl Color {
    /// An opaque literal color from 8-bit channels.
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgba(r, g, b, 255)
    }

    /// Whether this is a literal (non-semantic) color.
    pub fn is_literal(&self) -> bool {
        matches!(self, Color::Rgba(..))
    }
}

/// A concrete, fully-resolved 8-bit-per-channel RGBA color. This is what the
/// scene drawer actually rasterizes with; semantic [`Color`]s become `Rgba`
/// after theme resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel (255 = opaque).
    pub a: u8,
}

impl Rgba {
    /// Opaque black.
    pub const BLACK: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    /// Opaque white.
    pub const WHITE: Rgba = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    /// Fully transparent.
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    /// A color from its four channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba { r, g, b, a }
    }

    /// An opaque color from its three channels.
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }
}

/// A font family selector. `System`/`SystemMono` resolve to the platform UI
/// font so menus match the OS; `Named` looks up an installed family.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontFamily {
    /// The platform UI font (San Francisco, Segoe UI, system default).
    #[default]
    System,
    /// The platform monospace UI font.
    SystemMono,
    /// A specific installed family by name.
    Named(String),
}

/// Font weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Weight {
    /// Regular / normal weight. The default.
    #[default]
    Regular,
    /// Medium weight.
    Medium,
    /// Semibold weight.
    Semibold,
    /// Bold weight.
    Bold,
}

impl Weight {
    /// The OpenType numeric weight (100–900) for this weight, for backends that
    /// select fonts by numeric weight.
    pub fn ot_weight(&self) -> u16 {
        match self {
            Weight::Regular => 400,
            Weight::Medium => 500,
            Weight::Semibold => 600,
            Weight::Bold => 700,
        }
    }
}

/// A resolved font: family, size (in logical points), weight, tracking, and
/// optional optical size.
///
/// Construct via [`Font::system`] / [`Font::mono`] and the `with_*` builders
/// rather than a struct literal — the type is `#[non_exhaustive]` so future
/// rendering knobs can be added without a breaking change (adding
/// [`optical_size`](Self::optical_size) in 0.13 was the last field addition that
/// required a major bump).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Font {
    /// The font family.
    pub family: FontFamily,
    /// Size in logical points.
    pub size: f32,
    /// Weight.
    pub weight: Weight,
    /// Extra inter-glyph spacing (**tracking**) in logical points, added to every
    /// glyph advance. `0.0` is the metrics-only default. Native UI text engines
    /// (CoreText for San Francisco) apply a small size-dependent tracking that a
    /// bare shaper does not, which reads slightly looser than a real menu; the
    /// macOS `System` theme sets this so menu text matches native tracking (#42).
    /// Negative tightens, positive loosens.
    pub letter_spacing: f32,
    /// Optical size (`opsz` axis) to instance the face at, in logical points, or
    /// `None` to leave the face at its default optical master. A variable UI font
    /// (San Francisco, Segoe UI Variable) carries an `opsz` axis whose masters are
    /// tuned per size; the value is clamped into the face's own `opsz` range. macOS
    /// SFNS defaults to the condensed *Display* master (`opsz` default 28), so a
    /// 13pt menu left at the default renders narrower/"squished" than native — the
    /// `System` theme sets this to the menu point size so CoreText's *Text* master
    /// (clamped to `opsz` 17) is used instead (#77). `None` keeps prior behavior.
    pub optical_size: Option<f32>,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            family: FontFamily::System,
            size: 13.0,
            weight: Weight::Regular,
            letter_spacing: 0.0,
            optical_size: None,
        }
    }
}

impl Font {
    /// A system-font at the given size and weight.
    pub fn system(size: f32, weight: Weight) -> Self {
        Font {
            family: FontFamily::System,
            size,
            weight,
            letter_spacing: 0.0,
            optical_size: None,
        }
    }

    /// A system monospace font at the given size and weight.
    pub fn mono(size: f32, weight: Weight) -> Self {
        Font {
            family: FontFamily::SystemMono,
            size,
            weight,
            letter_spacing: 0.0,
            optical_size: None,
        }
    }

    /// Return a copy of this font with a different weight.
    pub fn with_weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }

    /// Return a copy of this font with the given tracking (extra inter-glyph
    /// spacing, logical points). See the [`letter_spacing`](Self::letter_spacing)
    /// field.
    pub fn with_letter_spacing(mut self, points: f32) -> Self {
        self.letter_spacing = points;
        self
    }

    /// Return a copy of this font instanced at the given optical size (`opsz` axis,
    /// logical points). See the [`optical_size`](Self::optical_size) field. The
    /// value is clamped into the face's own `opsz` range at shaping time; on a face
    /// with no `opsz` axis it has no effect.
    pub fn with_optical_size(mut self, points: f32) -> Self {
        self.optical_size = Some(points);
        self
    }
}
