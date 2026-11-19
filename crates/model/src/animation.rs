//! Keyframe animation: typed channels in the classic storage shape and their
//! evaluation. Port of the classic `animation/types.ts`, `interpolation.ts`,
//! `bezier.ts` and `resolve.ts` (tag `pre-rewrite`).
//!
//! Key times are relative to the element start. A number param animates
//! through a [`ScalarChannel`] (linear, hold or bezier segments); booleans
//! and selects through a [`DiscreteChannel`] (always hold).

use std::collections::BTreeMap;

use opencut_time::MediaTime;
use serde::{Deserialize, Serialize};

use crate::params::{ParamValue, ParamValues};

/// Keyframe channels keyed by property path (`opacity`,
/// `transform.positionX`, `effects.<id>.params.<key>`, …).
pub type Animations = BTreeMap<String, ChannelData>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum ChannelData {
    Leaf(Channel),
    /// One channel per component, e.g. a colour's `r`, `g`, `b`, `a`.
    Composite(BTreeMap<String, Channel>),
    /// Kept verbatim: the classic v30 migration leaves the legacy `bindings`
    /// and `channels` entries next to the per-path ones.
    Other(serde_json::Value),
}

/// Classic tells the two apart by shape: scalar keys carry `segmentToNext`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Channel {
    Scalar(ScalarChannel),
    Discrete(DiscreteChannel),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScalarChannel {
    pub keys: Vec<ScalarKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extrapolation: Option<Extrapolation>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScalarKey {
    pub id: String,
    pub time: MediaTime,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_handle: Option<CurveHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_handle: Option<CurveHandle>,
    pub segment_to_next: SegmentType,
    #[serde(default)]
    pub tangent_mode: TangentMode,
}

/// A bezier handle as an offset from its key. Unset handles default to a
/// third of the segment towards the neighbour.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct CurveHandle {
    pub dt: MediaTime,
    pub dv: f64,
}

/// How a key moves to the next one.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SegmentType {
    Step,
    Linear,
    Bezier,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TangentMode {
    Auto,
    Aligned,
    Broken,
    #[default]
    Flat,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extrapolation {
    pub before: ExtrapolationMode,
    pub after: ExtrapolationMode,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExtrapolationMode {
    Hold,
    Linear,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiscreteChannel {
    pub keys: Vec<DiscreteKey>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiscreteKey {
    pub id: String,
    pub time: MediaTime,
    pub value: ParamValue,
}

/// What a keyframe asks of the segment after it (classic
/// `AnimationInterpolation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Hold,
    Bezier,
}

impl From<Interpolation> for SegmentType {
    fn from(interpolation: Interpolation) -> Self {
        match interpolation {
            Interpolation::Linear => Self::Linear,
            Interpolation::Hold => Self::Step,
            Interpolation::Bezier => Self::Bezier,
        }
    }
}

impl From<SegmentType> for Interpolation {
    fn from(segment: SegmentType) -> Self {
        match segment {
            SegmentType::Linear => Self::Linear,
            SegmentType::Step => Self::Hold,
            SegmentType::Bezier => Self::Bezier,
        }
    }
}

impl ChannelData {
    /// `(component, channel)` pairs; a leaf is the `"value"` component.
    pub fn channels(&self) -> Vec<(&str, &Channel)> {
        match self {
            Self::Leaf(channel) => vec![("value", channel)],
            Self::Composite(components) => components
                .iter()
                .map(|(key, channel)| (key.as_str(), channel))
                .collect(),
            Self::Other(_) => Vec::new(),
        }
    }
}

impl Channel {
    /// `(id, time)` of every key, in stored order.
    pub fn key_times(&self) -> Vec<(&str, MediaTime)> {
        match self {
            Self::Scalar(channel) => channel
                .keys
                .iter()
                .map(|key| (key.id.as_str(), key.time))
                .collect(),
            Self::Discrete(channel) => channel
                .keys
                .iter()
                .map(|key| (key.id.as_str(), key.time))
                .collect(),
        }
    }
}

impl ScalarChannel {
    /// Keys sorted by time with handles clamped inside their segment; the
    /// first key has no left handle and the last no right handle (classic
    /// `normalizeScalarChannel`).
    #[must_use]
    pub fn normalized(&self) -> Self {
        let mut keys = self.keys.clone();
        keys.sort_by_key(|key| key.time);
        let times: Vec<MediaTime> = keys.iter().map(|key| key.time).collect();
        for (index, key) in keys.iter_mut().enumerate() {
            let span = |left: usize| {
                MediaTime::from_ticks((times[left + 1] - times[left]).as_ticks().max(1))
            };
            key.left_handle = match index {
                0 => None,
                _ => key.left_handle.map(|handle| CurveHandle {
                    dt: handle.dt.clamp(-span(index - 1), MediaTime::ZERO),
                    dv: handle.dv,
                }),
            };
            key.right_handle = if index + 1 < times.len() {
                key.right_handle.map(|handle| CurveHandle {
                    dt: handle.dt.clamp(MediaTime::ZERO, span(index)),
                    dv: handle.dv,
                })
            } else {
                None
            };
        }
        Self {
            keys,
            extrapolation: self.extrapolation,
        }
    }

    /// The curve's value at `time`; `None` without keys (classic
    /// `getScalarChannelValueAtTime`).
    pub fn value_at(&self, time: MediaTime) -> Option<f64> {
        let channel = self.normalized();
        let keys = &channel.keys;
        let (first, last) = (keys.first()?, keys.last()?);
        let extrapolation = channel.extrapolation;
        if time < first.time {
            let mode = extrapolation.map_or(ExtrapolationMode::Hold, |mode| mode.before);
            return Some(extrapolate(mode, first, keys.get(1), time));
        }
        if time > last.time {
            let mode = extrapolation.map_or(ExtrapolationMode::Hold, |mode| mode.after);
            return Some(extrapolate(mode, last, keys.iter().rev().nth(1), time));
        }
        if time == first.time {
            return Some(first.value);
        }
        let segment = keys
            .windows(2)
            .find(|pair| time <= pair[1].time)
            .expect("time is within the keys");
        let (left, right) = (&segment[0], &segment[1]);
        if time == right.time {
            return Some(right.value);
        }
        Some(match left.segment_to_next {
            SegmentType::Step => left.value,
            SegmentType::Linear => {
                let progress = (time - left.time).as_ticks() as f64
                    / (right.time - left.time).as_ticks() as f64;
                left.value + (right.value - left.value) * progress.clamp(0.0, 1.0)
            }
            SegmentType::Bezier => Bezier::new(left, right).value_at(time),
        })
    }
}

#[allow(clippy::cast_precision_loss)]
fn extrapolate(
    mode: ExtrapolationMode,
    edge: &ScalarKey,
    neighbor: Option<&ScalarKey>,
    time: MediaTime,
) -> f64 {
    let Some(neighbor) = neighbor.filter(|_| mode == ExtrapolationMode::Linear) else {
        return edge.value;
    };
    let span = (neighbor.time - edge.time).as_ticks();
    if span == 0 {
        return edge.value;
    }
    edge.value + (time - edge.time).as_ticks() as f64 / span as f64 * (neighbor.value - edge.value)
}

/// One bezier segment's four control points as `(time ticks, value)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezier {
    pub points: [(f64, f64); 4],
}

const BEZIER_SOLVE_ITERATIONS: usize = 20;

impl Bezier {
    /// The segment from `left` to `right`, with missing handles at their
    /// default third (classic `getDefaultRightHandle`/`getDefaultLeftHandle`).
    #[allow(clippy::cast_precision_loss)]
    pub fn new(left: &ScalarKey, right: &ScalarKey) -> Self {
        let (t0, t3) = (left.time.as_ticks() as f64, right.time.as_ticks() as f64);
        let (span, delta) = (t3 - t0, right.value - left.value);
        let (out_dt, out_dv) = left
            .right_handle
            .map_or((span / 3.0, delta / 3.0), |handle| {
                (handle.dt.as_ticks() as f64, handle.dv)
            });
        let (in_dt, in_dv) = right
            .left_handle
            .map_or((-span / 3.0, -delta / 3.0), |handle| {
                (handle.dt.as_ticks() as f64, handle.dv)
            });
        Self {
            points: [
                (t0, left.value),
                (t0 + out_dt, left.value + out_dv),
                (t3 + in_dt, right.value + in_dv),
                (t3, right.value),
            ],
        }
    }

    /// The point at curve parameter `progress` (0–1).
    pub fn point(&self, progress: f64) -> (f64, f64) {
        let [p0, p1, p2, p3] = self.points;
        let at = |a: f64, b: f64, c: f64, d: f64| {
            let mt = 1.0 - progress;
            mt * mt * mt * a
                + 3.0 * mt * mt * progress * b
                + 3.0 * mt * progress * progress * c
                + progress * progress * progress * d
        };
        (at(p0.0, p1.0, p2.0, p3.0), at(p0.1, p1.1, p2.1, p3.1))
    }

    /// The curve parameter whose time is `time`, by bisection (classic
    /// `solveBezierProgressForTime`).
    #[allow(clippy::cast_precision_loss)]
    pub fn progress_at(&self, time: MediaTime) -> f64 {
        let time = time.as_ticks() as f64;
        let (mut lower, mut upper) = (0.0, 1.0);
        for _ in 0..BEZIER_SOLVE_ITERATIONS {
            let mid = f64::midpoint(lower, upper);
            if self.point(mid).0 < time {
                lower = mid;
            } else {
                upper = mid;
            }
        }
        f64::midpoint(lower, upper)
    }

    pub fn value_at(&self, time: MediaTime) -> f64 {
        self.point(self.progress_at(time)).1
    }
}

impl DiscreteChannel {
    /// The last key at or before `time`; `None` before the first key.
    pub fn value_at(&self, time: MediaTime) -> Option<&ParamValue> {
        self.keys
            .iter()
            .filter(|key| key.time <= time)
            .max_by_key(|key| key.time)
            .map(|key| &key.value)
    }
}

/// `path`'s value `time` into the element: the keyframed value if animated,
/// else `base`. A channel of the wrong type for `base` is ignored, as in
/// classic `resolveAnimationPathValueAtTime`.
///
/// shortcut: composite (colour) channels resolve to `base`; port the colour
/// layout when a renderer draws animated colours (text, P7).
pub fn value_at(
    animations: Option<&Animations>,
    path: &str,
    time: MediaTime,
    base: Option<&ParamValue>,
) -> Option<ParamValue> {
    let animated = match animations.and_then(|animations| animations.get(path)) {
        Some(ChannelData::Leaf(Channel::Scalar(channel)))
            if matches!(base, None | Some(ParamValue::Number(_))) =>
        {
            channel.value_at(time).map(ParamValue::Number)
        }
        Some(ChannelData::Leaf(Channel::Discrete(channel)))
            if !matches!(base, Some(ParamValue::Number(_))) =>
        {
            channel.value_at(time).cloned()
        }
        _ => None,
    };
    animated.or_else(|| base.cloned())
}

/// `params` with every animated leaf path replaced by its value at `time`.
pub fn params_at(
    params: &ParamValues,
    animations: Option<&Animations>,
    time: MediaTime,
) -> ParamValues {
    let mut resolved = params.clone();
    for path in animations.into_iter().flat_map(BTreeMap::keys) {
        if let Some(value) = value_at(animations, path, time, params.get(path)) {
            resolved.insert(path.clone(), value);
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn t(ticks: i64) -> MediaTime {
        MediaTime::from_ticks(ticks)
    }

    fn key(id: &str, time: i64, value: f64, segment: SegmentType) -> ScalarKey {
        ScalarKey {
            id: id.to_owned(),
            time: t(time),
            value,
            left_handle: None,
            right_handle: None,
            segment_to_next: segment,
            tangent_mode: TangentMode::Flat,
        }
    }

    fn channel(keys: Vec<ScalarKey>) -> ScalarChannel {
        ScalarChannel {
            keys,
            extrapolation: None,
        }
    }

    #[test]
    fn linear_segments_interpolate_and_hold_past_the_ends() {
        let channel = channel(vec![
            key("b", 200, 10.0, SegmentType::Linear),
            key("a", 100, 0.0, SegmentType::Linear),
        ]);
        assert_eq!(channel.value_at(t(0)), Some(0.0));
        assert_eq!(channel.value_at(t(100)), Some(0.0));
        assert_eq!(channel.value_at(t(150)), Some(5.0));
        assert_eq!(channel.value_at(t(200)), Some(10.0));
        assert_eq!(channel.value_at(t(500)), Some(10.0));
        assert_eq!(
            ScalarChannel {
                keys: vec![],
                extrapolation: None
            }
            .value_at(t(0)),
            None
        );
    }

    #[test]
    fn step_segments_hold_the_left_value() {
        let channel = channel(vec![
            key("a", 0, 1.0, SegmentType::Step),
            key("b", 100, 2.0, SegmentType::Linear),
        ]);
        assert_eq!(channel.value_at(t(99)), Some(1.0));
        assert_eq!(channel.value_at(t(100)), Some(2.0));
    }

    #[test]
    fn linear_extrapolation_continues_the_edge_slope() {
        let mut channel = channel(vec![
            key("a", 100, 0.0, SegmentType::Linear),
            key("b", 200, 10.0, SegmentType::Linear),
        ]);
        channel.extrapolation = Some(Extrapolation {
            before: ExtrapolationMode::Linear,
            after: ExtrapolationMode::Linear,
        });
        assert_eq!(channel.value_at(t(0)), Some(-10.0));
        assert_eq!(channel.value_at(t(300)), Some(20.0));
    }

    #[test]
    fn default_bezier_handles_follow_the_straight_line() {
        let channel = channel(vec![
            key("a", 0, 0.0, SegmentType::Bezier),
            key("b", 300, 3.0, SegmentType::Linear),
        ]);
        let value = channel.value_at(t(150)).unwrap();
        assert!((value - 1.5).abs() < 1e-4, "{value}");
    }

    #[test]
    fn eased_bezier_starts_slow() {
        let mut left = key("a", 0, 0.0, SegmentType::Bezier);
        left.right_handle = Some(CurveHandle {
            dt: t(100),
            dv: 0.0,
        });
        let mut right = key("b", 300, 3.0, SegmentType::Linear);
        right.left_handle = Some(CurveHandle {
            dt: t(-100),
            dv: 0.0,
        });
        let channel = channel(vec![left, right]);
        let early = channel.value_at(t(30)).unwrap();
        assert!(early < 0.1, "{early}");
        let middle = channel.value_at(t(150)).unwrap();
        assert!((middle - 1.5).abs() < 1e-3, "{middle}");
    }

    #[test]
    fn normalizing_clamps_handles_and_drops_edge_ones() {
        let mut first = key("a", 0, 0.0, SegmentType::Bezier);
        first.left_handle = Some(CurveHandle { dt: t(-5), dv: 1.0 });
        first.right_handle = Some(CurveHandle {
            dt: t(500),
            dv: 1.0,
        });
        let normalized = channel(vec![first, key("b", 100, 1.0, SegmentType::Linear)]).normalized();
        assert_eq!(normalized.keys[0].left_handle, None);
        assert_eq!(normalized.keys[0].right_handle.unwrap().dt, t(100));
    }

    #[test]
    fn discrete_channels_hold_from_each_key() {
        let channel = DiscreteChannel {
            keys: vec![
                DiscreteKey {
                    id: "a".into(),
                    time: t(100),
                    value: "multiply".into(),
                },
                DiscreteKey {
                    id: "b".into(),
                    time: t(200),
                    value: "screen".into(),
                },
            ],
        };
        assert_eq!(channel.value_at(t(50)), None);
        assert_eq!(channel.value_at(t(150)), Some(&"multiply".into()));
        assert_eq!(channel.value_at(t(200)), Some(&"screen".into()));
    }

    #[test]
    fn values_resolve_against_the_base_type() {
        let animations: Animations = serde_json::from_value(json!({
            "opacity": { "keys": [
                { "id": "a", "time": 0, "value": 0, "segmentToNext": "linear", "tangentMode": "flat" },
                { "id": "b", "time": 100, "value": 1, "segmentToNext": "linear", "tangentMode": "flat" }
            ] },
            "blendMode": { "keys": [{ "id": "c", "time": 50, "value": "screen" }] }
        }))
        .unwrap();
        let animations = Some(&animations);
        let half = ParamValue::Number(0.5);
        assert_eq!(
            value_at(animations, "opacity", t(50), Some(&1.0.into())),
            Some(half)
        );
        // A string base ignores a scalar channel.
        assert_eq!(
            value_at(animations, "opacity", t(50), Some(&"x".into())),
            Some("x".into())
        );
        assert_eq!(
            value_at(animations, "blendMode", t(10), Some(&"normal".into())),
            Some("normal".into())
        );
        assert_eq!(
            value_at(animations, "blendMode", t(60), Some(&"normal".into())),
            Some("screen".into())
        );
        let params = params_at(&ParamValues::new(), animations, t(100));
        assert_eq!(params.get("opacity"), Some(&1.0.into()));
    }

    #[test]
    fn channel_shapes_round_trip() {
        let stored = json!({
            "opacity": { "keys": [
                { "id": "a", "time": 0, "value": 0.0, "segmentToNext": "bezier", "tangentMode": "auto",
                  "rightHandle": { "dt": 10, "dv": 0.5 } }
            ], "extrapolation": { "before": "hold", "after": "linear" } },
            "color": {
                "r": { "keys": [{ "id": "b", "time": 0, "value": 1.0, "segmentToNext": "linear", "tangentMode": "flat" }] }
            },
            "params.visible": { "keys": [{ "id": "c", "time": 0, "value": true }] },
            "bindings": { "x": { "path": "opacity", "components": [] } },
            "channels": { "legacy": { "kind": "scalar", "keys": [] } }
        });
        let animations: Animations = serde_json::from_value(stored.clone()).unwrap();
        assert!(matches!(
            animations["opacity"],
            ChannelData::Leaf(Channel::Scalar(_))
        ));
        assert!(matches!(animations["color"], ChannelData::Composite(_)));
        assert!(matches!(
            animations["params.visible"],
            ChannelData::Leaf(Channel::Discrete(_))
        ));
        assert!(matches!(animations["bindings"], ChannelData::Other(_)));
        assert!(matches!(animations["channels"], ChannelData::Other(_)));
        assert_eq!(serde_json::to_value(&animations).unwrap(), stored);
    }
}
