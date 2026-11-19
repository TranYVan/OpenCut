//! Keyframe edits on an element's [`Animations`]. Port of the classic
//! `animation/keyframes.ts` and `curve-bridge.ts` (tag `pre-rewrite`).
//!
//! Every function keeps the invariants classic relies on: channels are
//! normalized (sorted, handles clamped), empty channels are dropped and an
//! element without channels has `None`.

use std::collections::{BTreeMap, HashMap};

use opencut_model::animation::{
    Bezier, Channel, ChannelData, CurveHandle, DiscreteChannel, DiscreteKey, Interpolation,
    ScalarChannel, ScalarKey, SegmentType, TangentMode,
};
use opencut_model::time::MediaTime;
use opencut_model::{Animations, ParamValue, new_id};

/// A cubic-bezier easing `[x1, y1, x2, y2]` over a segment scaled to 0–1,
/// as in CSS `cubic-bezier()`.
pub type Easing = [f64; 4];

pub const LINEAR_EASING: Easing = [0.0, 0.0, 1.0, 1.0];

/// Graph editor presets (classic `easing-presets.ts`).
pub const EASING_PRESETS: [(&str, Easing); 6] = [
    ("Smooth", [0.25, 0.1, 0.25, 1.0]),
    ("Ease out", [0.0, 0.0, 0.2, 1.0]),
    ("Ease in", [0.8, 0.0, 1.0, 1.0]),
    ("In out", [0.4, 0.0, 0.2, 1.0]),
    ("Pop", [0.175, 0.885, 0.32, 1.275]),
    ("Linear", LINEAR_EASING),
];

const EPSILON: f64 = 1e-6;

/// Changes to one scalar key's curve; `None` fields are left alone
/// (classic `ScalarCurveKeyframePatch`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurvePatch {
    pub left_handle: Option<Option<CurveHandle>>,
    pub right_handle: Option<Option<CurveHandle>>,
    pub segment_to_next: Option<SegmentType>,
    pub tangent_mode: Option<TangentMode>,
}

impl CurvePatch {
    /// Everything about `key`'s curve, to restore it on a pasted copy.
    pub fn of(key: &ScalarKey) -> Self {
        Self {
            left_handle: Some(key.left_handle),
            right_handle: Some(key.right_handle),
            segment_to_next: Some(key.segment_to_next),
            tangent_mode: Some(key.tangent_mode),
        }
    }
}

/// Sets the value at `time` on `path`'s leaf channel: updates the key with
/// `keyframe_id` (moving it to `time`) or the key at `time`, else adds one.
/// A number makes a scalar key, a bool or string a discrete (hold) key; a
/// channel of the other type is replaced. Returns the key's id.
pub fn upsert(
    animations: &mut Option<Animations>,
    path: &str,
    time: MediaTime,
    value: ParamValue,
    interpolation: Option<Interpolation>,
    keyframe_id: Option<&str>,
) -> String {
    let all = animations.get_or_insert_with(Animations::new);
    let existing = all.remove(path);
    let id;
    let channel = match value {
        ParamValue::Number(value) => {
            let mut channel = match existing {
                Some(ChannelData::Leaf(Channel::Scalar(channel))) => channel,
                _ => ScalarChannel {
                    keys: Vec::new(),
                    extrapolation: None,
                },
            };
            let found = keyframe_id
                .and_then(|wanted| channel.keys.iter().position(|key| key.id == wanted))
                .or_else(|| channel.keys.iter().position(|key| key.time == time));
            if let Some(index) = found {
                let key = &mut channel.keys[index];
                if keyframe_id == Some(key.id.as_str()) {
                    key.time = time;
                }
                key.value = value;
                if let Some(interpolation) = interpolation {
                    key.segment_to_next = interpolation.into();
                }
                id = key.id.clone();
            } else {
                id = keyframe_id.map_or_else(new_id, str::to_owned);
                channel.keys.push(scalar_key(
                    id.clone(),
                    time,
                    value,
                    interpolation.map_or(SegmentType::Linear, SegmentType::from),
                ));
            }
            Channel::Scalar(channel.normalized())
        }
        value => {
            let mut channel = match existing {
                Some(ChannelData::Leaf(Channel::Discrete(channel))) => channel,
                _ => DiscreteChannel { keys: Vec::new() },
            };
            let found = keyframe_id
                .and_then(|wanted| channel.keys.iter().position(|key| key.id == wanted))
                .or_else(|| channel.keys.iter().position(|key| key.time == time));
            if let Some(index) = found {
                let key = &mut channel.keys[index];
                if keyframe_id == Some(key.id.as_str()) {
                    key.time = time;
                }
                key.value = value;
                id = key.id.clone();
            } else {
                id = keyframe_id.map_or_else(new_id, str::to_owned);
                channel.keys.push(DiscreteKey {
                    id: id.clone(),
                    time,
                    value,
                });
            }
            Channel::Discrete(normalized(channel))
        }
    };
    all.insert(path.to_owned(), ChannelData::Leaf(channel));
    id
}

/// Removes the key `keyframe_id` from every component of `path`.
pub fn remove(animations: &mut Option<Animations>, path: &str, keyframe_id: &str) {
    edit_channels(animations, path, |channel| match channel {
        Channel::Scalar(channel) => {
            channel.keys.retain(|key| key.id != keyframe_id);
            *channel = channel.normalized();
        }
        Channel::Discrete(channel) => channel.keys.retain(|key| key.id != keyframe_id),
    });
}

/// Moves the key `keyframe_id` of every component of `path` to `time`.
pub fn retime(animations: &mut Option<Animations>, path: &str, keyframe_id: &str, time: MediaTime) {
    edit_channels(animations, path, |channel| match channel {
        Channel::Scalar(channel) => {
            for key in channel.keys.iter_mut().filter(|key| key.id == keyframe_id) {
                key.time = time;
            }
            *channel = channel.normalized();
        }
        Channel::Discrete(channel) => {
            for key in channel.keys.iter_mut().filter(|key| key.id == keyframe_id) {
                key.time = time;
            }
            *channel = normalized(channel.clone());
        }
    });
}

/// Applies `patch` to the scalar key `keyframe_id` of `path`'s `component`
/// (`"value"` for a leaf channel).
pub fn update_curve(
    animations: &mut Option<Animations>,
    path: &str,
    component: &str,
    keyframe_id: &str,
    patch: &CurvePatch,
) {
    let Some(channel) = scalar_channel_mut(animations, path, component) else {
        return;
    };
    for key in channel.keys.iter_mut().filter(|key| key.id == keyframe_id) {
        if let Some(handle) = patch.left_handle {
            key.left_handle = handle;
        }
        if let Some(handle) = patch.right_handle {
            key.right_handle = handle;
        }
        if let Some(segment) = patch.segment_to_next {
            key.segment_to_next = segment;
        }
        if let Some(mode) = patch.tangent_mode {
            key.tangent_mode = mode;
        }
    }
    *channel = channel.normalized();
}

/// Whether `path` still has any key.
pub fn has_keyframes(animations: Option<&Animations>, path: &str) -> bool {
    animations
        .and_then(|animations| animations.get(path))
        .is_some_and(|data| {
            data.channels()
                .iter()
                .any(|(_, channel)| !channel.key_times().is_empty())
        })
}

/// The easing of the bezier or linear segment starting at `keyframe_id`
/// (classic `getNormalizedCubicBezierForScalarSegment`). `None` for the
/// last key, a hold segment or a flat or zero-length one.
pub fn segment_easing(channel: &ScalarChannel, keyframe_id: &str) -> Option<Easing> {
    let channel = channel.normalized();
    let (left, right) = segment(&channel, keyframe_id)?;
    match left.segment_to_next {
        SegmentType::Step => None,
        SegmentType::Linear => Some(LINEAR_EASING),
        SegmentType::Bezier => {
            let span_time = (right.time - left.time).as_ticks() as f64;
            let span_value = right.value - left.value;
            if span_time == 0.0 || span_value.abs() <= EPSILON {
                return None;
            }
            let [p0, p1, p2, p3] = Bezier::new(left, right).points;
            Some([
                ((p1.0 - p0.0) / span_time).clamp(0.0, 1.0),
                (p1.1 - p0.1) / span_value,
                (1.0 + (p2.0 - p3.0) / span_time).clamp(0.0, 1.0),
                1.0 + (p2.1 - p3.1) / span_value,
            ])
        }
    }
}

/// The patches that give the segment starting at `keyframe_id` `easing`:
/// `(left key patch, (right key id, right key patch))` (classic
/// `buildGraphEditorCurvePatches`). `None` when the segment can't take it.
pub fn easing_patches(
    channel: &ScalarChannel,
    keyframe_id: &str,
    easing: Easing,
) -> Option<(CurvePatch, (String, CurvePatch))> {
    let channel = channel.normalized();
    let (left, right) = segment(&channel, keyframe_id)?;
    let is_linear = easing
        .iter()
        .zip(LINEAR_EASING)
        .all(|(value, linear)| (value - linear).abs() <= EPSILON);
    if is_linear {
        return Some((
            CurvePatch {
                segment_to_next: Some(SegmentType::Linear),
                right_handle: Some(None),
                ..CurvePatch::default()
            },
            (
                right.id.clone(),
                CurvePatch {
                    left_handle: Some(None),
                    ..CurvePatch::default()
                },
            ),
        ));
    }
    let span_time = (right.time - left.time).as_ticks() as f64;
    let span_value = right.value - left.value;
    if span_time == 0.0 || span_value.abs() <= EPSILON {
        return None;
    }
    let [x1, y1, x2, y2] = easing;
    let ticks = |value: f64| MediaTime::from_ticks(value.round() as i64);
    Some((
        CurvePatch {
            segment_to_next: Some(SegmentType::Bezier),
            right_handle: Some(Some(CurveHandle {
                dt: ticks(span_time * x1.clamp(0.0, 1.0)),
                dv: span_value * y1,
            })),
            ..CurvePatch::default()
        },
        (
            right.id.clone(),
            CurvePatch {
                left_handle: Some(Some(CurveHandle {
                    dt: ticks(span_time * (x2.clamp(0.0, 1.0) - 1.0)),
                    dv: span_value * (y2 - 1.0),
                })),
                ..CurvePatch::default()
            },
        ),
    ))
}

/// Splits keyframes at `split` (element-relative). The left half keeps keys
/// at or before the split, the right half keys at or after it, shifted so
/// the split is time zero. Where no key sits on the split, boundary keys
/// with the curve's value there are added (a bezier segment is subdivided),
/// so neither half jumps at the cut (classic `splitAnimationsAtTime`).
pub fn split_at(
    animations: Option<&Animations>,
    split: MediaTime,
) -> (Option<Animations>, Option<Animations>) {
    let (mut left, mut right) = (Animations::new(), Animations::new());
    for (path, data) in animations.into_iter().flatten() {
        // Components of one property share key ids.
        let (left_id, right_id) = (new_id(), new_id());
        for (component, channel) in data.channels() {
            let (left_half, right_half) = match channel {
                Channel::Scalar(channel) => {
                    let (l, r) = split_scalar(channel, split, &left_id, &right_id);
                    (l.map(Channel::Scalar), r.map(Channel::Scalar))
                }
                Channel::Discrete(channel) => {
                    let (l, r) = split_discrete(channel, split, &right_id);
                    (l.map(Channel::Discrete), r.map(Channel::Discrete))
                }
            };
            if let Some(channel) = left_half {
                set_component(&mut left, path, component, channel);
            }
            if let Some(channel) = right_half {
                set_component(&mut right, path, component, channel);
            }
        }
    }
    (
        (!left.is_empty()).then_some(left),
        (!right.is_empty()).then_some(right),
    )
}

/// Drops keys after `duration`, keeping the curve's value at the new end
/// (classic `clampAnimationsToDuration`).
pub fn clamp_to_duration(
    animations: Option<&Animations>,
    duration: MediaTime,
) -> Option<Animations> {
    if duration <= MediaTime::ZERO {
        return None;
    }
    split_at(animations, duration).0
}

/// Copies keyframes with fresh ids, for duplicated elements. Components of
/// one property keep sharing ids.
pub fn clone_with_new_ids(animations: Option<&Animations>) -> Option<Animations> {
    let mut animations = animations?.clone();
    for data in animations.values_mut() {
        let mut ids: HashMap<String, String> = HashMap::new();
        let mut fresh = |id: &mut String| {
            *id = ids.entry(id.clone()).or_insert_with(new_id).clone();
        };
        let channels: Vec<&mut Channel> = match data {
            ChannelData::Leaf(channel) => vec![channel],
            ChannelData::Composite(components) => components.values_mut().collect(),
            ChannelData::Other(_) => Vec::new(),
        };
        for channel in channels {
            match channel {
                Channel::Scalar(channel) => channel.keys.iter_mut().for_each(|k| fresh(&mut k.id)),
                Channel::Discrete(channel) => {
                    channel.keys.iter_mut().for_each(|k| fresh(&mut k.id));
                }
            }
        }
    }
    Some(animations)
}

fn scalar_key(id: String, time: MediaTime, value: f64, segment: SegmentType) -> ScalarKey {
    ScalarKey {
        id,
        time,
        value,
        left_handle: None,
        right_handle: None,
        segment_to_next: segment,
        tangent_mode: TangentMode::Flat,
    }
}

fn normalized(mut channel: DiscreteChannel) -> DiscreteChannel {
    channel.keys.sort_by_key(|key| key.time);
    channel
}

/// The key `keyframe_id` and the one after it.
fn segment<'a>(
    channel: &'a ScalarChannel,
    keyframe_id: &str,
) -> Option<(&'a ScalarKey, &'a ScalarKey)> {
    let index = channel.keys.iter().position(|key| key.id == keyframe_id)?;
    Some((&channel.keys[index], channel.keys.get(index + 1)?))
}

fn scalar_channel_mut<'a>(
    animations: &'a mut Option<Animations>,
    path: &str,
    component: &str,
) -> Option<&'a mut ScalarChannel> {
    let channel = match animations.as_mut()?.get_mut(path)? {
        ChannelData::Leaf(channel) if component == "value" => channel,
        ChannelData::Composite(components) => components.get_mut(component)?,
        _ => return None,
    };
    match channel {
        Channel::Scalar(channel) => Some(channel),
        Channel::Discrete(_) => None,
    }
}

/// Runs `edit` on every channel of `path`, then drops whatever it emptied.
fn edit_channels(
    animations: &mut Option<Animations>,
    path: &str,
    mut edit: impl FnMut(&mut Channel),
) {
    let Some(all) = animations.as_mut() else {
        return;
    };
    let Some(data) = all.get_mut(path) else {
        return;
    };
    let keep = match data {
        ChannelData::Leaf(channel) => {
            edit(channel);
            !channel.key_times().is_empty()
        }
        ChannelData::Composite(components) => {
            for channel in components.values_mut() {
                edit(channel);
            }
            components.retain(|_, channel| !channel.key_times().is_empty());
            !components.is_empty()
        }
        ChannelData::Other(_) => true,
    };
    if !keep {
        all.remove(path);
    }
    if all.is_empty() {
        *animations = None;
    }
}

fn set_component(animations: &mut Animations, path: &str, component: &str, channel: Channel) {
    if component == "value" {
        animations.insert(path.to_owned(), ChannelData::Leaf(channel));
        return;
    }
    let data = animations
        .entry(path.to_owned())
        .or_insert_with(|| ChannelData::Composite(BTreeMap::new()));
    if let ChannelData::Composite(components) = data {
        components.insert(component.to_owned(), channel);
    }
}

fn shifted<T: Clone>(key: &T, time: impl FnOnce(&mut T)) -> T {
    let mut key = key.clone();
    time(&mut key);
    key
}

fn split_scalar(
    channel: &ScalarChannel,
    split: MediaTime,
    left_id: &str,
    right_id: &str,
) -> (Option<ScalarChannel>, Option<ScalarChannel>) {
    let channel = channel.normalized();
    let keys = &channel.keys;
    let Some(value) = channel.value_at(split) else {
        return (None, None);
    };
    let mut left: Vec<ScalarKey> = keys
        .iter()
        .filter(|key| key.time <= split)
        .cloned()
        .collect();
    let mut right: Vec<ScalarKey> = keys
        .iter()
        .filter(|key| key.time >= split)
        .map(|key| shifted(key, |key| key.time -= split))
        .collect();

    if !keys.iter().any(|key| key.time == split) {
        let inside = keys
            .windows(2)
            .find(|pair| pair[0].time < split && split < pair[1].time);
        match inside {
            Some([before, after]) if before.segment_to_next == SegmentType::Bezier => {
                let (left_part, right_part) =
                    subdivide(before, after, split, value, left_id, right_id);
                left.pop();
                left.extend(left_part);
                right.splice(..1, right_part);
            }
            Some([before, ..]) => {
                left.push(scalar_key(
                    left_id.to_owned(),
                    split,
                    value,
                    SegmentType::Linear,
                ));
                let boundary = scalar_key(
                    right_id.to_owned(),
                    MediaTime::ZERO,
                    value,
                    before.segment_to_next,
                );
                right.insert(0, boundary);
            }
            // Outside the keys only the empty half needs one, holding the
            // extrapolated value.
            _ => {
                if left.is_empty() {
                    left.push(scalar_key(
                        left_id.to_owned(),
                        split,
                        value,
                        SegmentType::Linear,
                    ));
                }
                if right.is_empty() {
                    let boundary = scalar_key(
                        right_id.to_owned(),
                        MediaTime::ZERO,
                        value,
                        SegmentType::Linear,
                    );
                    right.push(boundary);
                }
            }
        }
    }

    let wrap = |keys: Vec<ScalarKey>| {
        (!keys.is_empty()).then(|| {
            ScalarChannel {
                keys,
                extrapolation: channel.extrapolation,
            }
            .normalized()
        })
    };
    (wrap(left), wrap(right))
}

/// Splits the bezier segment `before`→`after` at `split` with de Casteljau,
/// so both halves trace the original curve. Returns the left half's last
/// two keys (`before` with its new handle, the boundary) and the right
/// half's first two (the boundary, `after` with its new handle), right
/// times already shifted.
fn subdivide(
    before: &ScalarKey,
    after: &ScalarKey,
    split: MediaTime,
    value: f64,
    left_id: &str,
    right_id: &str,
) -> ([ScalarKey; 2], [ScalarKey; 2]) {
    let bezier = Bezier::new(before, after);
    let progress = bezier.progress_at(split);
    let lerp =
        |a: (f64, f64), b: (f64, f64)| (a.0 + (b.0 - a.0) * progress, a.1 + (b.1 - a.1) * progress);
    let [p0, p1, p2, p3] = bezier.points;
    let (q0, q1, q2) = (lerp(p0, p1), lerp(p1, p2), lerp(p2, p3));
    let (r0, r1) = (lerp(q0, q1), lerp(q1, q2));
    let point = lerp(r0, r1);
    let handle = |to: (f64, f64), from: (f64, f64)| CurveHandle {
        dt: MediaTime::from_ticks((to.0 - from.0).round() as i64),
        dv: to.1 - from.1,
    };

    let mut left_key = before.clone();
    left_key.right_handle = Some(handle(q0, p0));
    let mut left_boundary = scalar_key(left_id.to_owned(), split, value, SegmentType::Bezier);
    left_boundary.left_handle = Some(handle(r0, point));
    left_boundary.tangent_mode = before.tangent_mode;

    let mut right_boundary = scalar_key(
        right_id.to_owned(),
        MediaTime::ZERO,
        value,
        SegmentType::Bezier,
    );
    right_boundary.right_handle = Some(handle(r1, point));
    right_boundary.tangent_mode = before.tangent_mode;
    let mut right_key = after.clone();
    right_key.time -= split;
    right_key.left_handle = Some(handle(q2, p3));

    ([left_key, left_boundary], [right_boundary, right_key])
}

fn split_discrete(
    channel: &DiscreteChannel,
    split: MediaTime,
    right_id: &str,
) -> (Option<DiscreteChannel>, Option<DiscreteChannel>) {
    let channel = normalized(channel.clone());
    let left: Vec<DiscreteKey> = channel
        .keys
        .iter()
        .filter(|key| key.time <= split)
        .cloned()
        .collect();
    let mut right: Vec<DiscreteKey> = channel
        .keys
        .iter()
        .filter(|key| key.time >= split)
        .map(|key| shifted(key, |key| key.time -= split))
        .collect();
    // A hold curve needs no boundary on the left; the right half starts
    // with the value held at the cut (none before the first key).
    if let Some(value) = channel
        .value_at(split)
        .filter(|_| !channel.keys.iter().any(|key| key.time == split))
    {
        let boundary = DiscreteKey {
            id: right_id.to_owned(),
            time: MediaTime::ZERO,
            value: value.clone(),
        };
        right.insert(0, boundary);
    }
    let wrap = |keys: Vec<DiscreteKey>| (!keys.is_empty()).then_some(DiscreteChannel { keys });
    (wrap(left), wrap(right))
}

#[cfg(test)]
mod tests {
    use opencut_model::animation;
    use serde_json::json;

    use super::*;

    fn t(ticks: i64) -> MediaTime {
        MediaTime::from_ticks(ticks)
    }

    fn animations() -> Animations {
        serde_json::from_value(json!({
            "opacity": { "keys": [
                { "id": "a", "time": 0, "value": 0.0, "segmentToNext": "linear", "tangentMode": "flat" },
                { "id": "b", "time": 100, "value": 1.0, "segmentToNext": "bezier", "tangentMode": "flat",
                  "rightHandle": { "dt": 60, "dv": 0.0 } },
                { "id": "c", "time": 300, "value": 0.0, "segmentToNext": "linear", "tangentMode": "flat",
                  "leftHandle": { "dt": -30, "dv": 0.4 } }
            ] },
            "color": {
                "r": { "keys": [{ "id": "d", "time": 50, "value": 1.0, "segmentToNext": "linear", "tangentMode": "flat" }] },
                "g": { "keys": [{ "id": "e", "time": 250, "value": 1.0, "segmentToNext": "linear", "tangentMode": "flat" }] }
            },
            "blendMode": { "keys": [{ "id": "f", "time": 20, "value": "screen" }] }
        }))
        .unwrap()
    }

    fn value(animations: Option<&Animations>, path: &str, time: i64) -> f64 {
        animation::value_at(animations, path, t(time), Some(&0.5.into()))
            .and_then(|value| value.as_f64())
            .unwrap()
    }

    fn scalar<'a>(animations: &'a Animations, path: &str) -> &'a ScalarChannel {
        match &animations[path] {
            ChannelData::Leaf(Channel::Scalar(channel)) => channel,
            other => panic!("not scalar: {other:?}"),
        }
    }

    #[test]
    fn split_keeps_the_curve_on_both_sides_of_the_cut() {
        let original = animations();
        for cut in [50, 100, 180, 320] {
            let (left, right) = split_at(Some(&original), t(cut));
            for time in (0..=400).step_by(10) {
                let expected = value(Some(&original), "opacity", time);
                let actual = if time <= cut {
                    value(left.as_ref(), "opacity", time)
                } else {
                    value(right.as_ref(), "opacity", time - cut)
                };
                assert!(
                    (expected - actual).abs() < 1e-3,
                    "cut {cut} at {time}: {expected} vs {actual}"
                );
            }
        }
    }

    #[test]
    fn split_rebases_the_right_half_with_boundary_keys() {
        let (left, right) = split_at(Some(&animations()), t(50));
        let (left, right) = (left.unwrap(), right.unwrap());
        let times = |channel: &ScalarChannel| {
            channel
                .keys
                .iter()
                .map(|k| k.time.as_ticks())
                .collect::<Vec<_>>()
        };
        assert_eq!(times(scalar(&left, "opacity")), [0, 50]);
        assert_eq!(times(scalar(&right, "opacity")), [0, 50, 250]);
        assert_eq!(scalar(&right, "opacity").keys[0].value, 0.5);
        // The discrete channel holds its value across the cut.
        let ChannelData::Leaf(Channel::Discrete(blend)) = &right["blendMode"] else {
            panic!()
        };
        assert_eq!(blend.keys[0].value, "screen".into());
        // A component whose keys all lie before the cut still holds its value.
        let ChannelData::Composite(color) = &right["color"] else {
            panic!()
        };
        assert_eq!(color["r"].key_times()[0].1, t(0));
    }

    #[test]
    fn split_on_a_key_adds_no_boundary() {
        let (left, right) = split_at(Some(&animations()), t(100));
        assert_eq!(scalar(&left.unwrap(), "opacity").keys.len(), 2);
        let right = right.unwrap();
        let keys = &scalar(&right, "opacity").keys;
        assert_eq!((keys[0].id.as_str(), keys[0].time), ("b", t(0)));
    }

    #[test]
    fn upsert_adds_updates_and_retypes_keys() {
        let mut animations = None;
        let id = upsert(&mut animations, "opacity", t(10), 0.2.into(), None, None);
        let again = upsert(
            &mut animations,
            "opacity",
            t(10),
            0.4.into(),
            Some(Interpolation::Hold),
            None,
        );
        assert_eq!(id, again);
        upsert(&mut animations, "opacity", t(0), 0.0.into(), None, None);
        let channel = scalar(animations.as_ref().unwrap(), "opacity");
        assert_eq!(channel.keys.len(), 2);
        assert_eq!(channel.keys[1].value, 0.4);
        assert_eq!(channel.keys[1].segment_to_next, SegmentType::Step);

        // By id the key moves.
        upsert(
            &mut animations,
            "opacity",
            t(50),
            0.9.into(),
            None,
            Some(&id),
        );
        assert_eq!(
            scalar(animations.as_ref().unwrap(), "opacity").keys[1].time,
            t(50)
        );

        upsert(
            &mut animations,
            "blendMode",
            t(0),
            "screen".into(),
            None,
            None,
        );
        assert!(matches!(
            animations.as_ref().unwrap()["blendMode"],
            ChannelData::Leaf(Channel::Discrete(_))
        ));
    }

    #[test]
    fn removing_the_last_keys_clears_the_animations() {
        let mut animations = None;
        let id = upsert(&mut animations, "opacity", t(0), 1.0.into(), None, None);
        remove(&mut animations, "opacity", "missing");
        assert!(has_keyframes(animations.as_ref(), "opacity"));
        remove(&mut animations, "opacity", &id);
        assert_eq!(animations, None);
    }

    #[test]
    fn retime_resorts_keys() {
        let mut animations = Some(animations());
        retime(&mut animations, "opacity", "a", t(200));
        let keys = &scalar(animations.as_ref().unwrap(), "opacity").keys;
        let order: Vec<&str> = keys.iter().map(|key| key.id.as_str()).collect();
        assert_eq!(order, ["b", "a", "c"]);
        assert_eq!(keys[0].left_handle, None);
    }

    #[test]
    fn easing_round_trips_through_handles() {
        let mut animations = Some(animations());
        let ease = [0.4, 0.0, 0.2, 1.0];
        let channel = scalar(animations.as_ref().unwrap(), "opacity").clone();
        let (left_patch, (right_id, right_patch)) = easing_patches(&channel, "a", ease).unwrap();
        update_curve(&mut animations, "opacity", "value", "a", &left_patch);
        update_curve(&mut animations, "opacity", "value", &right_id, &right_patch);
        let channel = scalar(animations.as_ref().unwrap(), "opacity");
        assert_eq!(channel.keys[0].segment_to_next, SegmentType::Bezier);
        let read = segment_easing(channel, "a").unwrap();
        for (read, wanted) in read.iter().zip(ease) {
            assert!((read - wanted).abs() < 1e-6, "{read:?}");
        }

        let (left_patch, _) = easing_patches(channel, "a", LINEAR_EASING).unwrap();
        assert_eq!(left_patch.segment_to_next, Some(SegmentType::Linear));
        assert_eq!(segment_easing(channel, "c"), None);
    }

    #[test]
    fn clones_get_fresh_shared_ids() {
        let cloned = clone_with_new_ids(Some(&animations())).unwrap();
        let id = &scalar(&cloned, "opacity").keys[0].id;
        assert_ne!(id, "a");
        assert_eq!(scalar(&cloned, "opacity").keys[0].time, t(0));
    }

    #[test]
    fn clamping_keeps_the_value_at_the_new_end() {
        let clamped = clamp_to_duration(Some(&animations()), t(200)).unwrap();
        let channel = scalar(&clamped, "opacity");
        assert_eq!(channel.keys.last().unwrap().time, t(200));
        assert!(
            (value(Some(&clamped), "opacity", 200) - value(Some(&animations()), "opacity", 200))
                .abs()
                < 1e-3
        );
    }
}
