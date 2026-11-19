//! Inspector edits: element params, speed and source-audio separation.
//! Ported from the classic `UpdateElementsCommand` (with its retime rule in
//! `timeline/update-pipeline`) and `ToggleSourceAudioSeparationCommand`.

use opencut_model::{
    Animations, AudioElement, AudioSource, Element, ElementBase, ElementRef, ParamValue,
    ParamValues, Project, RetimeConfig, Track, TrackKind, find_param, new_id,
};

use crate::animation;
use crate::command::{Command, Outcome, current_tracks};
use crate::commands::elements::element_mut;
use crate::error::EditError;
use crate::placement::{check_no_overlap, default_insert_index, find_free_track, insert_track};

/// Sets one param (`opacity`, `transform.rotate`, …) after checking it
/// against the element kind's definition: numbers snap to the step and clamp
/// to the range.
///
/// Run it with [`Editor::execute_coalescing`](crate::Editor::execute_coalescing)
/// and [`SetElementParam::coalesce_key`] while a slider or scrub is dragged,
/// so the whole drag is one undo step.
pub struct SetElementParam {
    pub element: ElementRef,
    pub key: String,
    pub value: ParamValue,
}

impl SetElementParam {
    pub fn new(element: ElementRef, key: impl Into<String>, value: impl Into<ParamValue>) -> Self {
        Self {
            element,
            key: key.into(),
            value: value.into(),
        }
    }

    /// Edits of the same param of the same element share a key.
    pub fn coalesce_key(&self) -> String {
        format!("param:{}:{}", self.element.element_id, self.key)
    }
}

impl Command for SetElementParam {
    fn label(&self) -> &'static str {
        "Edit property"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let element = element_mut(tracks, &self.element)?;
        let kind = element.kind();
        let definition = find_param(kind, &self.key).ok_or_else(|| EditError::UnknownParam {
            element: kind,
            key: self.key.clone(),
        })?;
        let value = definition
            .coerce(&self.value)
            .ok_or_else(|| EditError::InvalidParamValue {
                key: self.key.clone(),
                value: self.value.clone(),
            })?;
        element.base_mut().params.insert(self.key.clone(), value);
        Ok(Outcome::unchanged())
    }
}

/// Sets the playback speed (clamped to 0.01–5x) and whether pitch is kept.
/// The element's timeline length follows: the visible source span divided by
/// the new rate. Fails if the longer element would overlap its neighbour.
///
/// shortcut: keyframes past a shortened end aren't clamped (classic clamps
/// them); upgrade with the animation system (P9).
pub struct SetElementRetime {
    pub element: ElementRef,
    pub rate: f64,
    pub maintain_pitch: bool,
}

impl SetElementRetime {
    pub fn coalesce_key(&self) -> String {
        format!("retime:{}", self.element.element_id)
    }
}

impl Command for SetElementRetime {
    fn label(&self) -> &'static str {
        "Change speed"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let element = element_mut(tracks, &self.element)?;
        if !element.kind().is_retimable() {
            return Err(EditError::NotRetimable(element.kind()));
        }
        let old_rate = RetimeConfig::safe_rate(element.retime());
        let retime = RetimeConfig::build(self.rate, self.maintain_pitch);
        let new_rate = RetimeConfig::safe_rate(retime.as_ref());

        let base = element.base_mut();
        let span = match base.source_duration {
            Some(source) => source - base.trim_start - base.trim_end,
            None => base
                .duration
                .mul_f64_round(old_rate)
                .ok_or(EditError::InvalidTiming)?,
        };
        base.duration = span
            .div_f64_round(new_rate)
            .filter(|duration| duration.as_ticks() > 0)
            .ok_or(EditError::InvalidTiming)?;
        element.set_retime(retime);

        let track = tracks
            .track(&self.element.track_id)
            .ok_or_else(|| EditError::TrackNotFound(self.element.track_id.clone()))?;
        check_no_overlap(track)?;
        Ok(Outcome::unchanged())
    }
}

/// Extracts a video's own audio onto an audio track as a separate element
/// (silencing the video), or, if already extracted, turns the video's audio
/// back on. Like classic, recovering leaves the extracted element in place.
pub struct ToggleSourceAudioSeparation {
    pub element: ElementRef,
}

impl Command for ToggleSourceAudioSeparation {
    fn label(&self) -> &'static str {
        "Toggle source audio"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let source = match project
            .current_scene()
            .and_then(|scene| scene.tracks.element(&self.element))
        {
            Some(Element::Video(video)) => video.clone(),
            Some(other) => return Err(EditError::NoSourceAudioOn(other.kind())),
            None => return Err(EditError::ElementNotFound(self.element.clone())),
        };
        let separated = source.is_source_audio_enabled == Some(false);
        if !separated
            && project
                .media_asset(&source.media_id)
                .is_none_or(|asset| asset.has_audio == Some(false))
        {
            return Err(EditError::NoSourceAudio);
        }

        let tracks = current_tracks(project)?;
        if !separated {
            let element = separated_audio(&source.base, &source.media_id, source.retime.clone());
            let kind = TrackKind::Audio;
            let track_id = find_free_track(tracks, kind, element.start_time(), element.end_time())
                .unwrap_or_else(|| {
                    let track = Track::new(new_id(), kind);
                    let id = track.id.clone();
                    let index = default_insert_index(tracks, kind);
                    insert_track(tracks, track, index);
                    id
                });
            tracks
                .track_mut(&track_id)
                .ok_or(EditError::TrackNotFound(track_id))?
                .elements
                .push(element);
        }
        if let Element::Video(video) = element_mut(tracks, &self.element)? {
            video.is_source_audio_enabled = Some(separated);
        }
        Ok(Outcome::unchanged())
    }
}

/// The audio element a video's source audio becomes: same timing, volume and
/// mute, retime and volume keyframes (classic `buildSeparatedAudioElement`).
fn separated_audio(source: &ElementBase, media_id: &str, retime: Option<RetimeConfig>) -> Element {
    let mut base = ElementBase::new(new_id(), &source.name, source.start_time, source.duration);
    base.trim_start = source.trim_start;
    base.trim_end = source.trim_end;
    base.source_duration = source.source_duration;
    base.params = ["volume", "muted"]
        .into_iter()
        .filter_map(|key| Some((key.to_owned(), source.params.get(key)?.clone())))
        .collect::<ParamValues>();
    let volume: Option<Animations> = source
        .animations
        .as_ref()
        .and_then(|all| all.get("volume"))
        .map(|channel| Animations::from([("volume".to_owned(), channel.clone())]));
    base.animations = animation::clone_with_new_ids(volume.as_ref());
    Element::Audio(AudioElement {
        base,
        source: AudioSource::Upload {
            media_id: media_id.to_owned(),
        },
        retime,
    })
}
