use std::sync::Arc;

use opencut_model::time::TICKS_PER_SECOND;
use opencut_model::{AudioSource, Element, MediaAsset, Project, RetimeConfig, Track, TrackKind};

/// The mix is always interleaved stereo; the device layer maps it to the
/// device's channel count.
pub const CHANNELS: usize = 2;

const VOLUME_DB_MIN: f64 = -60.0;
const VOLUME_DB_MAX: f64 = 20.0;

/// Classic `dBToLinear` (volume is stored in dB, clamped to -60..+20).
#[allow(clippy::cast_possible_truncation)]
pub fn db_to_gain(db: f64) -> f32 {
    let db = if db.is_finite() { db } else { 0.0 };
    10f64.powf(db.clamp(VOLUME_DB_MIN, VOLUME_DB_MAX) / 20.0) as f32
}

/// Ticks to whole frames at `rate`, rounded to nearest.
pub fn ticks_to_frames(ticks: i64, rate: u32) -> u64 {
    let ticks = i128::from(ticks.max(0));
    let frames = (ticks * i128::from(rate) + i128::from(TICKS_PER_SECOND) / 2)
        / i128::from(TICKS_PER_SECOND);
    u64::try_from(frames).unwrap_or(u64::MAX)
}

/// One element's audio placed on the output timeline.
pub struct Clip {
    /// Output frame where the clip begins, and how many frames it spans.
    pub start: u64,
    pub len: u64,
    /// Interleaved stereo at the mixer's rate: the whole source file.
    pub source: Arc<[f32]>,
    /// Source frame played at `start`.
    pub source_start: f64,
    /// Source frames consumed per output frame (retime rate).
    pub rate: f64,
    pub gain: f32,
}

/// All of a project's audible clips. Stateless: [`mix`](Self::mix) renders
/// any block from its absolute position, so seeking costs nothing.
#[derive(Default)]
pub struct Mixer {
    clips: Vec<Clip>,
}

impl Mixer {
    pub fn new(clips: Vec<Clip>) -> Self {
        Self { clips }
    }

    /// Collects the current scene's audible elements: audio elements, and
    /// video elements with source audio enabled, on unmuted tracks, with
    /// the element's own `muted` and `volume` (dB) params. `source` returns
    /// an asset's audio already decoded to stereo at `sample_rate`; assets
    /// it returns `None` for are silent.
    // shortcut: retimed audio resamples by nearest sample, so pitch shifts
    // with speed; upgrade to a pitch-preserving stretch with the P7 retime work.
    pub fn from_project(
        project: &Project,
        sample_rate: u32,
        mut source: impl FnMut(&MediaAsset) -> Option<Arc<[f32]>>,
    ) -> Self {
        let Some(scene) = project.current_scene() else {
            return Self::default();
        };
        let clips = scene
            .tracks
            .iter()
            .filter(|track| audible(track))
            .flat_map(|track| &track.elements)
            .filter_map(|element| clip_for(element, project, sample_rate, &mut source))
            .collect();
        Self { clips }
    }

    /// Frames the mix spans.
    pub fn len(&self) -> u64 {
        self.clips
            .iter()
            .map(|clip| clip.start + clip.len)
            .max()
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Overwrites `out` (interleaved stereo) with the mix starting at output
    /// frame `start`.
    pub fn mix(&self, start: u64, out: &mut [f32]) {
        out.fill(0.0);
        let end = start + (out.len() / CHANNELS) as u64;
        for clip in &self.clips {
            let from = start.max(clip.start);
            let to = end.min(clip.start + clip.len);
            for frame in from..to {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let src = (clip.source_start + (frame - clip.start) as f64 * clip.rate) as usize;
                let Some(sample) = clip.source.get(src * CHANNELS..(src + 1) * CHANNELS) else {
                    break;
                };
                let at = (frame - start) as usize * CHANNELS;
                out[at] += sample[0] * clip.gain;
                out[at + 1] += sample[1] * clip.gain;
            }
        }
        for sample in out {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

fn audible(track: &Track) -> bool {
    matches!(track.kind, TrackKind::Audio | TrackKind::Video) && !track.is_muted()
}

#[allow(clippy::cast_precision_loss)]
fn clip_for(
    element: &Element,
    project: &Project,
    rate: u32,
    source: &mut impl FnMut(&MediaAsset) -> Option<Arc<[f32]>>,
) -> Option<Clip> {
    let has_audio = match element {
        Element::Audio(audio) => matches!(audio.source, AudioSource::Upload { .. }),
        Element::Video(video) => video.is_source_audio_enabled != Some(false),
        _ => false,
    };
    let base = element.base();
    if !has_audio || base.params.get("muted").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    let asset = project
        .media_assets
        .iter()
        .find(|asset| Some(asset.id.as_str()) == element.media_id())?;
    let start = ticks_to_frames(base.start_time.as_ticks(), rate);
    let end = ticks_to_frames(base.end_time().as_ticks(), rate);
    let db = base
        .params
        .get("volume")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    Some(Clip {
        start,
        len: end.saturating_sub(start),
        source: source(asset)?,
        source_start: ticks_to_frames(base.trim_start.as_ticks(), rate) as f64,
        rate: RetimeConfig::safe_rate(element.retime()),
        gain: db_to_gain(db),
    })
}
