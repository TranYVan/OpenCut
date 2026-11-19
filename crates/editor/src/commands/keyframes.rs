//! Keyframe edits. Ported from the classic `commands/timeline/element/keyframes/`
//! and `commands/timeline/clipboard/paste-keyframes.ts`.
//!
//! A keyframed path is a param key (`opacity`, `transform.positionX`, …);
//! key times are element-relative and clamped to the element.

use opencut_model::animation::{Channel, ChannelData, Interpolation};
use opencut_model::time::MediaTime;
use opencut_model::{Element, ElementRef, ParamDefinition, ParamValue, Project, find_param};

use crate::animation::{self, CurvePatch, Easing};
use crate::command::{Command, Outcome, current_tracks};
use crate::commands::elements::element_mut;
use crate::error::EditError;

/// Sets `path`'s value at `time`: updates the key `keyframe_id` (moving it
/// to `time`) or the key at `time`, else adds one. The value is checked
/// against the param like [`SetElementParam`](super::SetElementParam).
/// After it runs, `keyframe_id` holds the key's id.
pub struct UpsertKeyframe {
    pub element: ElementRef,
    pub path: String,
    pub time: MediaTime,
    pub value: ParamValue,
    /// The segment after the key; `None` keeps it (linear for a new key).
    pub interpolation: Option<Interpolation>,
    pub keyframe_id: Option<String>,
}

impl UpsertKeyframe {
    pub fn new(
        element: ElementRef,
        path: impl Into<String>,
        time: MediaTime,
        value: impl Into<ParamValue>,
    ) -> Self {
        Self {
            element,
            path: path.into(),
            time,
            value: value.into(),
            interpolation: None,
            keyframe_id: None,
        }
    }

    /// Edits of the same param of the same element share a key.
    pub fn coalesce_key(&self) -> String {
        format!("keyframe:{}:{}", self.element.element_id, self.path)
    }
}

impl Command for UpsertKeyframe {
    fn label(&self) -> &'static str {
        "Set keyframe"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let element = element_mut(current_tracks(project)?, &self.element)?;
        let value = coerce(element, &self.path, &self.value)?;
        let base = element.base_mut();
        let time = self.time.clamp(MediaTime::ZERO, base.duration);
        let id = animation::upsert(
            &mut base.animations,
            &self.path,
            time,
            value,
            self.interpolation,
            self.keyframe_id.as_deref(),
        );
        self.keyframe_id = Some(id);
        Ok(Outcome::unchanged())
    }
}

/// Removes a key. When it was the path's last one, `value_at_playhead`
/// (what the user was seeing) becomes the param's value, so it doesn't jump.
pub struct RemoveKeyframe {
    pub element: ElementRef,
    pub path: String,
    pub keyframe_id: String,
    pub value_at_playhead: Option<ParamValue>,
}

impl Command for RemoveKeyframe {
    fn label(&self) -> &'static str {
        "Remove keyframe"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let element = element_mut(current_tracks(project)?, &self.element)?;
        let base = element.base_mut();
        animation::remove(&mut base.animations, &self.path, &self.keyframe_id);
        if let Some(value) = &self.value_at_playhead
            && !animation::has_keyframes(base.animations.as_ref(), &self.path)
        {
            let value = coerce(element, &self.path, value)?;
            element.base_mut().params.insert(self.path.clone(), value);
        }
        Ok(Outcome::unchanged())
    }
}

/// Moves a key to `time`. Run several in a [`Batch`](crate::Batch) to drag
/// a keyframe selection.
pub struct RetimeKeyframe {
    pub element: ElementRef,
    pub path: String,
    pub keyframe_id: String,
    pub time: MediaTime,
}

impl Command for RetimeKeyframe {
    fn label(&self) -> &'static str {
        "Move keyframe"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let element = element_mut(current_tracks(project)?, &self.element)?;
        let base = element.base_mut();
        let time = self.time.clamp(MediaTime::ZERO, base.duration);
        animation::retime(&mut base.animations, &self.path, &self.keyframe_id, time);
        Ok(Outcome::unchanged())
    }
}

/// Gives the segment from `keyframe_id` to the next key of `path`'s
/// `component` an easing curve (the graph editor's edit).
#[derive(Clone)]
pub struct SetKeyframeEasing {
    pub element: ElementRef,
    pub path: String,
    pub component: String,
    pub keyframe_id: String,
    pub easing: Easing,
}

impl SetKeyframeEasing {
    pub fn coalesce_key(&self) -> String {
        format!(
            "easing:{}:{}:{}",
            self.element.element_id, self.path, self.keyframe_id
        )
    }
}

impl Command for SetKeyframeEasing {
    fn label(&self) -> &'static str {
        "Edit curve"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let element = element_mut(current_tracks(project)?, &self.element)?;
        let base = element.base_mut();
        let channel = base
            .animations
            .as_ref()
            .and_then(|animations| animations.get(&self.path))
            .and_then(|data| {
                data.channels()
                    .into_iter()
                    .find(|(component, _)| *component == self.component)
            });
        let Some((_, Channel::Scalar(channel))) = channel else {
            return Err(EditError::Empty);
        };
        let (left, (right_id, right)) =
            animation::easing_patches(channel, &self.keyframe_id, self.easing)
                .ok_or(EditError::Empty)?;
        let animations = &mut base.animations;
        animation::update_curve(
            animations,
            &self.path,
            &self.component,
            &self.keyframe_id,
            &left,
        );
        animation::update_curve(animations, &self.path, &self.component, &right_id, &right);
        Ok(Outcome::unchanged())
    }
}

/// One copied keyframe (classic `KeyframeClipboardItem`).
#[derive(Clone, Debug, PartialEq)]
pub struct KeyframeClipboardItem {
    pub path: String,
    /// From the earliest copied key.
    pub offset: MediaTime,
    pub value: ParamValue,
    /// Scalar keys only: the curve to restore.
    pub curve: Option<CurvePatch>,
}

/// Copies the keys `(path, keyframe_id)` of a leaf channel of `element`.
pub fn copy_keyframes(element: &Element, keys: &[(String, String)]) -> Vec<KeyframeClipboardItem> {
    let animations = element.base().animations.as_ref();
    let mut items: Vec<(MediaTime, KeyframeClipboardItem)> = keys
        .iter()
        .filter_map(|(path, id)| {
            let Some(ChannelData::Leaf(channel)) = animations?.get(path) else {
                return None;
            };
            let (time, value, curve) = match channel {
                Channel::Scalar(channel) => {
                    let key = channel.keys.iter().find(|key| &key.id == id)?;
                    (key.time, key.value.into(), Some(CurvePatch::of(key)))
                }
                Channel::Discrete(channel) => {
                    let key = channel.keys.iter().find(|key| &key.id == id)?;
                    (key.time, key.value.clone(), None)
                }
            };
            let item = KeyframeClipboardItem {
                path: path.clone(),
                offset: time,
                value,
                curve,
            };
            Some((time, item))
        })
        .collect();
    let earliest = items
        .iter()
        .map(|(time, _)| *time)
        .min()
        .unwrap_or_default();
    for (_, item) in &mut items {
        item.offset -= earliest;
    }
    items.into_iter().map(|(_, item)| item).collect()
}

/// Pastes copied keys onto `element` starting at `time` (element-relative),
/// with fresh ids. Items for params the element doesn't have are skipped.
pub struct PasteKeyframes {
    pub element: ElementRef,
    pub time: MediaTime,
    pub items: Vec<KeyframeClipboardItem>,
}

impl Command for PasteKeyframes {
    fn label(&self) -> &'static str {
        "Paste keyframes"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let element = element_mut(current_tracks(project)?, &self.element)?;
        let mut pasted = false;
        for item in &self.items {
            let Ok(value) = coerce(element, &item.path, &item.value) else {
                continue;
            };
            let base = element.base_mut();
            let time = (self.time + item.offset).clamp(MediaTime::ZERO, base.duration);
            let id = animation::upsert(&mut base.animations, &item.path, time, value, None, None);
            if let Some(curve) = &item.curve {
                animation::update_curve(&mut base.animations, &item.path, "value", &id, curve);
            }
            pasted = true;
        }
        if pasted {
            Ok(Outcome::unchanged())
        } else {
            Err(EditError::Empty)
        }
    }
}

fn coerce(element: &Element, path: &str, value: &ParamValue) -> Result<ParamValue, EditError> {
    let definition: ParamDefinition =
        find_param(element.kind(), path).ok_or_else(|| EditError::UnknownParam {
            element: element.kind(),
            key: path.to_owned(),
        })?;
    definition
        .coerce(value)
        .ok_or_else(|| EditError::InvalidParamValue {
            key: path.to_owned(),
            value: value.clone(),
        })
}
