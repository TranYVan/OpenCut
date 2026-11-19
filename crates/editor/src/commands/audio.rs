use opencut_model::{
    AudioElement, AudioSource, Element, ElementBase, ElementKind, ElementRef, ParamValue,
    ParamValues, Project, Track, TrackKind, new_id,
};

use crate::animation;
use crate::command::{Command, Outcome, current_tracks};
use crate::error::EditError;
use crate::placement::{check_no_overlap, highest_insert_index, insert_track};

const MUTED: &str = "muted";
const VOLUME: &str = "volume";

fn can_have_audio(element: &Element) -> bool {
    matches!(element.kind(), ElementKind::Video | ElementKind::Audio)
}

fn is_muted(element: &Element) -> bool {
    element
        .base()
        .params
        .get(MUTED)
        .and_then(ParamValue::as_bool)
        == Some(true)
}

/// Mutes the elements if any of them is audible, otherwise unmutes them all
/// (classic `toggleElementsMuted`). Only video and audio elements count.
pub struct ToggleElementsMuted {
    pub elements: Vec<ElementRef>,
}

impl Command for ToggleElementsMuted {
    fn label(&self) -> &'static str {
        "Toggle mute"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let mut targets = Vec::new();
        for element_ref in &self.elements {
            let element = tracks
                .element(element_ref)
                .ok_or_else(|| EditError::ElementNotFound(element_ref.clone()))?;
            if can_have_audio(element) {
                targets.push((element_ref, is_muted(element)));
            }
        }
        if targets.is_empty() {
            return Err(EditError::Empty);
        }
        let mute = targets.iter().any(|(_, muted)| !muted);
        for (element_ref, _) in targets {
            if let Some(element) = tracks.element_mut(element_ref) {
                element
                    .base_mut()
                    .params
                    .insert(MUTED.to_owned(), ParamValue::Bool(mute));
            }
        }
        Ok(Outcome::unchanged())
    }
}

/// Splits a video's sound onto its own audio track, or takes it back
/// (classic `ToggleSourceAudioSeparationCommand`). Recovering only re-enables
/// the video's audio; the separated audio element stays until deleted.
pub struct ToggleSourceAudio {
    pub element: ElementRef,
}

impl Command for ToggleSourceAudio {
    fn label(&self) -> &'static str {
        "Toggle source audio"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let (separate, source) = {
            let tracks = &project
                .current_scene()
                .ok_or(EditError::NoCurrentScene)?
                .tracks;
            let element = tracks
                .element(&self.element)
                .ok_or_else(|| EditError::ElementNotFound(self.element.clone()))?;
            let Element::Video(video) = element else {
                return Err(EditError::Empty);
            };
            let enabled = video.is_source_audio_enabled != Some(false);
            // Classic only extracts from media that has audio.
            let has_audio = project
                .media_asset(&video.media_id)
                .is_some_and(|asset| asset.has_audio != Some(false));
            if enabled && !has_audio {
                return Err(EditError::Empty);
            }
            (enabled, element.clone())
        };

        let tracks = current_tracks(project)?;
        if separate {
            let separated = separated_audio(&source);
            let (start, end) = (separated.start_time(), separated.end_time());
            // Classic "first available": any audio track with room, else a new one.
            let free = tracks
                .audio
                .iter()
                .find(|track| track.is_span_free(start, end, &[]))
                .map(|track| track.id.clone());
            let target = free.unwrap_or_else(|| {
                let track = Track::new(new_id(), TrackKind::Audio);
                let id = track.id.clone();
                let index = highest_insert_index(tracks, TrackKind::Audio);
                insert_track(tracks, track, index);
                id
            });
            let track = tracks
                .track_mut(&target)
                .ok_or_else(|| EditError::TrackNotFound(target.clone()))?;
            track.elements.push(separated);
            check_no_overlap(track)?;
        }
        if let Some(Element::Video(video)) = tracks.element_mut(&self.element) {
            video.is_source_audio_enabled = Some(!separate);
        }
        Ok(Outcome::unchanged())
    }
}

/// The audio element a video's sound becomes (classic `buildSeparatedAudioElement`).
fn separated_audio(video: &Element) -> Element {
    let Element::Video(source) = video else {
        unreachable!("only called for video elements");
    };
    let base = &source.base;
    let mut params = ParamValues::new();
    let volume = base
        .params
        .get(VOLUME)
        .and_then(ParamValue::as_f64)
        .unwrap_or(1.0);
    params.insert(VOLUME.to_owned(), ParamValue::Number(volume));
    params.insert(MUTED.to_owned(), ParamValue::Bool(is_muted(video)));
    let volume_animation = base
        .animations
        .as_ref()
        .and_then(|animations| animations.get(VOLUME))
        .map(|channel| [(VOLUME.to_owned(), channel.clone())].into_iter().collect());
    Element::Audio(AudioElement {
        base: ElementBase {
            id: new_id(),
            name: base.name.clone(),
            duration: base.duration,
            start_time: base.start_time,
            trim_start: base.trim_start,
            trim_end: base.trim_end,
            source_duration: base.source_duration,
            animations: animation::clone_with_new_ids(volume_animation.as_ref()),
            params,
        },
        source: AudioSource::Upload {
            media_id: source.media_id.clone(),
        },
        retime: source.retime.clone(),
    })
}
