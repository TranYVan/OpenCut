//! What each element kind's params are: key, range, step, unit, default and
//! when they apply. Ported from the classic `params/` and
//! `params/registry.ts` (tag `pre-rewrite`). The values live in
//! [`ParamValues`]; this says how to read, validate and show them.
//!
//! Covered: transform, opacity, blend mode, volume and mute. Text params
//! come with text (roadmap P7).

use crate::element::ElementKind;
use crate::params::{ParamValue, ParamValues};

/// Lowest `transform.scaleX/Y`; below it the element would vanish (classic
/// `MIN_TRANSFORM_SCALE`).
pub const MIN_TRANSFORM_SCALE: f64 = 0.01;
/// Volume range in dB (classic `audio-constants`).
pub const VOLUME_DB_MIN: f64 = -60.0;
pub const VOLUME_DB_MAX: f64 = 20.0;

pub const BLEND_MODE_OPTIONS: [(&str, &str); 17] = [
    ("normal", "Normal"),
    ("darken", "Darken"),
    ("multiply", "Multiply"),
    ("color-burn", "Color Burn"),
    ("lighten", "Lighten"),
    ("screen", "Screen"),
    ("plus-lighter", "Plus Lighter"),
    ("color-dodge", "Color Dodge"),
    ("overlay", "Overlay"),
    ("soft-light", "Soft Light"),
    ("hard-light", "Hard Light"),
    ("difference", "Difference"),
    ("exclusion", "Exclusion"),
    ("hue", "Hue"),
    ("saturation", "Saturation"),
    ("color", "Color"),
    ("luminosity", "Luminosity"),
];

/// How a number is shown: `stored * multiplier` followed by `suffix`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    None,
    /// Stored 0–1, shown 0–100%.
    Percent,
    Degrees,
    Pixels,
    Decibels,
}

impl Unit {
    pub const fn multiplier(self) -> f64 {
        match self {
            Self::Percent => 100.0,
            _ => 1.0,
        }
    }

    pub const fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Percent => "%",
            Self::Degrees => "°",
            Self::Pixels => " px",
            Self::Decibels => " dB",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    /// `min`, `max`, `step` and `default` are in stored units.
    Number {
        min: f64,
        max: Option<f64>,
        step: f64,
        default: f64,
        unit: Unit,
    },
    Boolean {
        default: bool,
    },
    Select {
        default: &'static str,
        options: &'static [(&'static str, &'static str)],
    },
}

/// The param shows only while `param` equals `equals` (classic
/// `dependencies`).
#[derive(Clone, Debug, PartialEq)]
pub struct Dependency {
    pub param: &'static str,
    pub equals: ParamValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamDefinition {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub dependencies: Vec<Dependency>,
}

impl ParamDefinition {
    fn new(key: &'static str, label: &'static str, kind: ParamKind) -> Self {
        Self {
            key,
            label,
            kind,
            dependencies: Vec::new(),
        }
    }

    fn number(
        key: &'static str,
        label: &'static str,
        (min, max): (f64, Option<f64>),
        step: f64,
        default: f64,
        unit: Unit,
    ) -> Self {
        Self::new(
            key,
            label,
            ParamKind::Number {
                min,
                max,
                step,
                default,
                unit,
            },
        )
    }

    pub fn default_value(&self) -> ParamValue {
        match &self.kind {
            ParamKind::Number { default, .. } => ParamValue::Number(*default),
            ParamKind::Boolean { default } => ParamValue::Bool(*default),
            ParamKind::Select { default, .. } => ParamValue::String((*default).to_owned()),
        }
    }

    /// `value` made valid for this param: numbers snap to the step and clamp
    /// to the range, selects must be one of the options. `None` when the
    /// value is the wrong type (or not finite).
    pub fn coerce(&self, value: &ParamValue) -> Option<ParamValue> {
        match (&self.kind, value) {
            (ParamKind::Number { min, max, step, .. }, ParamValue::Number(number))
                if number.is_finite() =>
            {
                let snapped = (number / step).round() * step;
                // Drops float noise like 0.30000000000000004.
                let snapped = (snapped * 1e6).round() / 1e6;
                Some(ParamValue::Number(
                    snapped.clamp(*min, max.unwrap_or(f64::INFINITY)),
                ))
            }
            (ParamKind::Boolean { .. }, ParamValue::Bool(_)) => Some(value.clone()),
            (ParamKind::Select { options, .. }, ParamValue::String(choice))
                if options.iter().any(|(option, _)| option == choice) =>
            {
                Some(value.clone())
            }
            _ => None,
        }
    }
}

/// The params an element of `kind` has, in display order.
pub fn element_params(kind: ElementKind) -> Vec<ParamDefinition> {
    let mut params = Vec::new();
    if kind.is_visual() {
        params.extend([
            ParamDefinition::number(
                "transform.positionX",
                "Position X",
                (-100_000.0, None),
                1.0,
                0.0,
                Unit::Pixels,
            ),
            ParamDefinition::number(
                "transform.positionY",
                "Position Y",
                (-100_000.0, None),
                1.0,
                0.0,
                Unit::Pixels,
            ),
            ParamDefinition::number(
                "transform.scaleX",
                "Scale X",
                (MIN_TRANSFORM_SCALE, None),
                0.01,
                1.0,
                Unit::Percent,
            ),
            ParamDefinition::number(
                "transform.scaleY",
                "Scale Y",
                (MIN_TRANSFORM_SCALE, None),
                0.01,
                1.0,
                Unit::Percent,
            ),
            ParamDefinition::number(
                "transform.rotate",
                "Rotate",
                (-360.0, Some(360.0)),
                1.0,
                0.0,
                Unit::Degrees,
            ),
            ParamDefinition::number(
                "opacity",
                "Opacity",
                (0.0, Some(1.0)),
                0.01,
                1.0,
                Unit::Percent,
            ),
            ParamDefinition::new(
                "blendMode",
                "Blend Mode",
                ParamKind::Select {
                    default: "normal",
                    options: &BLEND_MODE_OPTIONS,
                },
            ),
        ]);
    }
    if kind.has_volume() {
        params.extend([
            ParamDefinition::number(
                "volume",
                "Volume",
                (VOLUME_DB_MIN, Some(VOLUME_DB_MAX)),
                0.1,
                0.0,
                Unit::Decibels,
            ),
            ParamDefinition::new("muted", "Muted", ParamKind::Boolean { default: false }),
        ]);
    }
    params
}

/// The definition of `key` for `kind`, if the kind has it.
pub fn find_param(kind: ElementKind, key: &str) -> Option<ParamDefinition> {
    element_params(kind)
        .into_iter()
        .find(|param| param.key == key)
}

/// `key`'s stored value, or its default when unset.
pub fn param_value(definition: &ParamDefinition, values: &ParamValues) -> ParamValue {
    values
        .get(definition.key)
        .cloned()
        .unwrap_or_else(|| definition.default_value())
}

/// The params to show for `values`: those whose dependencies hold.
pub fn visible_params(kind: ElementKind, values: &ParamValues) -> Vec<ParamDefinition> {
    filter_visible(element_params(kind), values)
}

fn filter_visible(all: Vec<ParamDefinition>, values: &ParamValues) -> Vec<ParamDefinition> {
    all.iter()
        .filter(|param| {
            param.dependencies.iter().all(|dependency| {
                let current = all
                    .iter()
                    .find(|other| other.key == dependency.param)
                    .map(|other| param_value(other, values));
                current.as_ref() == Some(&dependency.equals)
            })
        })
        .cloned()
        .collect()
}

impl ElementKind {
    /// Whether the element has `volume` and `muted` params.
    pub const fn has_volume(self) -> bool {
        matches!(self, Self::Video | Self::Audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: f64) -> ParamValue {
        ParamValue::Number(value)
    }

    #[test]
    fn numbers_snap_and_clamp() {
        let opacity = find_param(ElementKind::Video, "opacity").unwrap();
        assert_eq!(opacity.coerce(&number(0.304)), Some(number(0.3)));
        assert_eq!(opacity.coerce(&number(7.0)), Some(number(1.0)));
        assert_eq!(opacity.coerce(&number(-1.0)), Some(number(0.0)));
        assert_eq!(opacity.coerce(&number(f64::NAN)), None);
        assert_eq!(opacity.coerce(&ParamValue::Bool(true)), None);

        let scale = find_param(ElementKind::Image, "transform.scaleX").unwrap();
        assert_eq!(
            scale.coerce(&number(0.0)),
            Some(number(MIN_TRANSFORM_SCALE))
        );
        assert_eq!(scale.coerce(&number(2.5)), Some(number(2.5)));
    }

    #[test]
    fn selects_only_accept_their_options() {
        let blend = find_param(ElementKind::Video, "blendMode").unwrap();
        assert_eq!(blend.default_value(), ParamValue::from("normal"));
        assert!(blend.coerce(&ParamValue::from("multiply")).is_some());
        assert!(blend.coerce(&ParamValue::from("bogus")).is_none());
    }

    #[test]
    fn kinds_get_their_params() {
        let keys = |kind| -> Vec<&str> {
            element_params(kind)
                .into_iter()
                .map(|param| param.key)
                .collect()
        };
        assert!(keys(ElementKind::Video).contains(&"volume"));
        assert!(keys(ElementKind::Video).contains(&"opacity"));
        assert_eq!(keys(ElementKind::Audio), ["volume", "muted"]);
        assert!(keys(ElementKind::Image).iter().all(|key| *key != "volume"));
        assert!(keys(ElementKind::Effect).is_empty());
    }

    #[test]
    fn dependencies_hide_params_until_they_hold() {
        // No built-in param has a dependency yet (they're text's), so build
        // the case by hand against the same filter.
        let mut all = element_params(ElementKind::Audio);
        all[0].dependencies.push(Dependency {
            param: "muted",
            equals: ParamValue::Bool(false),
        });
        let visible = |values: &ParamValues| filter_visible(all.clone(), values).len();
        let mut values = ParamValues::new();
        assert_eq!(visible(&values), 2);
        values.insert("muted".to_owned(), ParamValue::Bool(true));
        assert_eq!(visible(&values), 1);
    }
}
