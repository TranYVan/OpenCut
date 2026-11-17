//! Keyframe handling on the raw-JSON animations the model keeps until the
//! animation system is ported (roadmap P9).
//!
//! A channel is `{ "keys": [{ "id", "time", … }] }`; a composite channel (e.g.
//! a colour) is an object of such channels.

use opencut_model::time::MediaTime;
use opencut_model::{Animations, new_id};
use serde_json::{Value, json};

/// Splits keyframes at `split` (element-relative). The left half keeps keys at
/// or before the split; the right half keeps keys at or after it, shifted so
/// the split becomes time zero.
///
/// TODO(P9): classic also inserts an interpolated key on both sides of the
/// split so the curve value at the cut is preserved. Needs curve evaluation.
pub fn split_at(
    animations: Option<&Animations>,
    split: MediaTime,
) -> (Option<Animations>, Option<Animations>) {
    let Some(animations) = animations else {
        return (None, None);
    };
    let left = map_channels(animations, |key| {
        (key_time(key)? <= split).then(|| key.clone())
    });
    let right = map_channels(animations, |key| {
        let time = key_time(key)?;
        (time >= split).then(|| with_time(key, time - split))
    });
    (left, right)
}

/// Copies keyframes with fresh ids, for duplicated elements.
pub fn clone_with_new_ids(animations: Option<&Animations>) -> Option<Animations> {
    map_channels(animations?, |key| {
        let mut key = key.clone();
        if let Some(object) = key.as_object_mut() {
            object.insert("id".to_owned(), Value::String(new_id()));
        }
        Some(key)
    })
}

fn key_time(key: &Value) -> Option<MediaTime> {
    key.get("time")?.as_i64().map(MediaTime::from_ticks)
}

fn with_time(key: &Value, time: MediaTime) -> Value {
    let mut key = key.clone();
    if let Some(object) = key.as_object_mut() {
        object.insert("time".to_owned(), json!(time.as_ticks()));
    }
    key
}

/// Rewrites every key through `map_key` (dropping `None`s) and drops channels
/// left without keys. Unknown shapes pass through untouched.
fn map_channels(
    animations: &Animations,
    map_key: impl Fn(&Value) -> Option<Value> + Copy,
) -> Option<Animations> {
    let result: Animations = animations
        .iter()
        .filter_map(|(path, channel)| Some((path.clone(), map_channel(channel, map_key)?)))
        .collect();
    (!result.is_empty()).then_some(result)
}

fn map_channel(channel: &Value, map_key: impl Fn(&Value) -> Option<Value> + Copy) -> Option<Value> {
    let object = channel.as_object()?;
    if let Some(keys) = object.get("keys").and_then(Value::as_array) {
        let keys: Vec<Value> = keys.iter().filter_map(map_key).collect();
        if keys.is_empty() {
            return None;
        }
        let mut channel = object.clone();
        channel.insert("keys".to_owned(), Value::Array(keys));
        return Some(Value::Object(channel));
    }
    // Composite: one channel per component.
    let components: serde_json::Map<String, Value> = object
        .iter()
        .filter_map(|(component, value)| Some((component.clone(), map_channel(value, map_key)?)))
        .collect();
    (!components.is_empty()).then_some(Value::Object(components))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn animations() -> Animations {
        serde_json::from_value(json!({
            "opacity": { "keys": [
                { "id": "a", "time": 0, "value": 0 },
                { "id": "b", "time": 100, "value": 1 },
                { "id": "c", "time": 300, "value": 0 }
            ] },
            "color": {
                "r": { "keys": [{ "id": "d", "time": 50, "value": 1 }] },
                "g": { "keys": [{ "id": "e", "time": 250, "value": 1 }] }
            }
        }))
        .unwrap()
    }

    #[test]
    fn splits_keys_and_rebases_the_right_half() {
        let (left, right) = split_at(Some(&animations()), MediaTime::from_ticks(100));
        let left = serde_json::to_value(left).unwrap();
        let right = serde_json::to_value(right).unwrap();

        assert_eq!(
            left,
            json!({
                "opacity": { "keys": [
                    { "id": "a", "time": 0, "value": 0 },
                    { "id": "b", "time": 100, "value": 1 }
                ] },
                "color": { "r": { "keys": [{ "id": "d", "time": 50, "value": 1 }] } }
            })
        );
        assert_eq!(
            right,
            json!({
                "opacity": { "keys": [
                    { "id": "b", "time": 0, "value": 1 },
                    { "id": "c", "time": 200, "value": 0 }
                ] },
                "color": { "g": { "keys": [{ "id": "e", "time": 150, "value": 1 }] } }
            })
        );
    }

    #[test]
    fn regenerates_key_ids() {
        let cloned = clone_with_new_ids(Some(&animations())).unwrap();
        let id = &cloned["opacity"]["keys"][0]["id"];
        assert_ne!(id, "a");
        assert_eq!(cloned["opacity"]["keys"][0]["time"], 0);
    }
}
