use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// How a layer combines with what is below it: the CSS `mix-blend-mode`
/// set. Names match the classic `blendMode` element param.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlendMode {
    #[default]
    Normal,
    Darken,
    Multiply,
    ColorBurn,
    Lighten,
    Screen,
    PlusLighter,
    ColorDodge,
    Overlay,
    SoftLight,
    HardLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    pub const ALL: [Self; 17] = [
        Self::Normal,
        Self::Darken,
        Self::Multiply,
        Self::ColorBurn,
        Self::Lighten,
        Self::Screen,
        Self::PlusLighter,
        Self::ColorDodge,
        Self::Overlay,
        Self::SoftLight,
        Self::HardLight,
        Self::Difference,
        Self::Exclusion,
        Self::Hue,
        Self::Saturation,
        Self::Color,
        Self::Luminosity,
    ];

    /// The mode's id in `shaders/blend.wgsl`.
    pub const fn shader_code(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Darken => 1,
            Self::Multiply => 2,
            Self::ColorBurn => 3,
            Self::Lighten => 4,
            Self::Screen => 5,
            Self::PlusLighter => 6,
            Self::ColorDodge => 7,
            Self::Overlay => 8,
            Self::SoftLight => 9,
            Self::HardLight => 10,
            Self::Difference => 11,
            Self::Exclusion => 12,
            Self::Hue => 13,
            Self::Saturation => 14,
            Self::Color => 15,
            Self::Luminosity => 16,
        }
    }

    /// The kebab-case name, as stored in the `blendMode` param.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Darken => "darken",
            Self::Multiply => "multiply",
            Self::ColorBurn => "color-burn",
            Self::Lighten => "lighten",
            Self::Screen => "screen",
            Self::PlusLighter => "plus-lighter",
            Self::ColorDodge => "color-dodge",
            Self::Overlay => "overlay",
            Self::SoftLight => "soft-light",
            Self::HardLight => "hard-light",
            Self::Difference => "difference",
            Self::Exclusion => "exclusion",
            Self::Hue => "hue",
            Self::Saturation => "saturation",
            Self::Color => "color",
            Self::Luminosity => "luminosity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown blend mode {0:?}")]
pub struct UnknownBlendMode(pub String);

impl FromStr for BlendMode {
    type Err = UnknownBlendMode;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .ok_or_else(|| UnknownBlendMode(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_match_serde() {
        for mode in BlendMode::ALL {
            assert_eq!(mode.as_str().parse::<BlendMode>(), Ok(mode));
            assert_eq!(
                serde_json::to_value(mode).unwrap(),
                serde_json::Value::String(mode.as_str().to_owned())
            );
        }
        assert!("bogus".parse::<BlendMode>().is_err());
    }

    #[test]
    fn shader_codes_are_distinct_and_in_order() {
        for (index, mode) in BlendMode::ALL.into_iter().enumerate() {
            assert_eq!(mode.shader_code() as usize, index);
        }
    }
}
