use opencut_time::MediaTime;
use serde::{Deserialize, Serialize};

use crate::element::Element;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Video,
    Text,
    Audio,
    Graphic,
    Effect,
}

impl TrackKind {
    pub const fn default_name(self) -> &'static str {
        match self {
            Self::Video => "Video track",
            Self::Text => "Text track",
            Self::Audio => "Audio track",
            Self::Graphic => "Graphic track",
            Self::Effect => "Effect track",
        }
    }

    pub const fn can_have_audio(self) -> bool {
        matches!(self, Self::Video | Self::Audio)
    }

    pub const fn can_be_hidden(self) -> bool {
        !matches!(self, Self::Audio)
    }
}

/// One timeline row. `muted` is present only on video and audio tracks and
/// `hidden` only on non-audio tracks, matching the classic JSON shape.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Track {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: TrackKind,
    pub elements: Vec<Element>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

impl Track {
    pub fn new(id: impl Into<String>, kind: TrackKind) -> Self {
        Self::with_name(id, kind, kind.default_name())
    }

    pub fn with_name(id: impl Into<String>, kind: TrackKind, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            elements: Vec::new(),
            muted: kind.can_have_audio().then_some(false),
            hidden: kind.can_be_hidden().then_some(false),
        }
    }

    pub fn is_muted(&self) -> bool {
        self.muted.unwrap_or(false)
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden.unwrap_or(false)
    }

    pub fn element(&self, element_id: &str) -> Option<&Element> {
        self.elements
            .iter()
            .find(|element| element.id() == element_id)
    }

    pub fn element_mut(&mut self, element_id: &str) -> Option<&mut Element> {
        self.elements
            .iter_mut()
            .find(|element| element.id() == element_id)
    }

    /// End of the last element, or zero for an empty track.
    pub fn end_time(&self) -> MediaTime {
        self.elements
            .iter()
            .map(Element::end_time)
            .max()
            .unwrap_or(MediaTime::ZERO)
    }

    /// Whether `start..end` overlaps any element, ignoring `exclude` ids.
    pub fn is_span_free(&self, start: MediaTime, end: MediaTime, exclude: &[&str]) -> bool {
        self.elements.iter().all(|element| {
            exclude.contains(&element.id())
                || element.end_time() <= start
                || element.start_time() >= end
        })
    }
}

/// Points at one element on one track.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ElementRef {
    pub track_id: String,
    pub element_id: String,
}

impl ElementRef {
    pub fn new(track_id: impl Into<String>, element_id: impl Into<String>) -> Self {
        Self {
            track_id: track_id.into(),
            element_id: element_id.into(),
        }
    }
}

/// A scene's tracks. Display order top to bottom: `overlay` (first is
/// top-most), then `main`, then `audio`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SceneTracks {
    pub overlay: Vec<Track>,
    pub main: Track,
    pub audio: Vec<Track>,
}

pub const MAIN_TRACK_NAME: &str = "Main Track";

impl SceneTracks {
    pub fn new(main_track_id: impl Into<String>) -> Self {
        Self {
            overlay: Vec::new(),
            main: Track::with_name(main_track_id, TrackKind::Video, MAIN_TRACK_NAME),
            audio: Vec::new(),
        }
    }

    /// All tracks in display order.
    pub fn iter(&self) -> impl Iterator<Item = &Track> {
        self.overlay
            .iter()
            .chain(std::iter::once(&self.main))
            .chain(self.audio.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Track> {
        self.overlay
            .iter_mut()
            .chain(std::iter::once(&mut self.main))
            .chain(self.audio.iter_mut())
    }

    pub fn track(&self, track_id: &str) -> Option<&Track> {
        self.iter().find(|track| track.id == track_id)
    }

    pub fn track_mut(&mut self, track_id: &str) -> Option<&mut Track> {
        self.iter_mut().find(|track| track.id == track_id)
    }

    pub fn is_main(&self, track_id: &str) -> bool {
        self.main.id == track_id
    }

    pub fn element(&self, element_ref: &ElementRef) -> Option<&Element> {
        self.track(&element_ref.track_id)?
            .element(&element_ref.element_id)
    }

    pub fn element_mut(&mut self, element_ref: &ElementRef) -> Option<&mut Element> {
        self.track_mut(&element_ref.track_id)?
            .element_mut(&element_ref.element_id)
    }

    pub fn element_count(&self) -> usize {
        self.iter().map(|track| track.elements.len()).sum()
    }

    /// End of the last element on any track.
    pub fn duration(&self) -> MediaTime {
        self.iter()
            .map(Track::end_time)
            .max()
            .unwrap_or(MediaTime::ZERO)
    }

    /// Element/track kind mismatches, as `(track id, element id)` pairs.
    pub fn misplaced_elements(&self) -> Vec<ElementRef> {
        self.iter()
            .flat_map(|track| {
                track
                    .elements
                    .iter()
                    .filter(|element| element.kind().track_kind() != track.kind)
                    .map(|element| ElementRef::new(&track.id, element.id()))
            })
            .collect()
    }
}
