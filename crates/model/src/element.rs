use std::collections::BTreeMap;

use opencut_time::MediaTime;
use serde::{Deserialize, Serialize};

use crate::params::ParamValues;
use crate::track::TrackKind;

/// Keyframe channels keyed by property path (`transform.positionX`,
/// `effects.<id>.params.<key>`, …).
///
/// Kept as raw JSON until the animation system is ported (roadmap P9), so
/// classic projects round-trip without losing keyframes.
pub type Animations = BTreeMap<String, serde_json::Value>;

/// Fields every timeline element has. Times are in ticks; `trim_start` and
/// `trim_end` are measured in source time, `duration` in timeline time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ElementBase {
    pub id: String,
    pub name: String,
    pub duration: MediaTime,
    pub start_time: MediaTime,
    pub trim_start: MediaTime,
    pub trim_end: MediaTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_duration: Option<MediaTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<Animations>,
    #[serde(default)]
    pub params: ParamValues,
}

impl ElementBase {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        start: MediaTime,
        duration: MediaTime,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            duration,
            start_time: start,
            trim_start: MediaTime::ZERO,
            trim_end: MediaTime::ZERO,
            source_duration: None,
            animations: None,
            params: ParamValues::new(),
        }
    }

    pub fn end_time(&self) -> MediaTime {
        self.start_time + self.duration
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RetimeConfig {
    pub rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintain_pitch: Option<bool>,
}

impl RetimeConfig {
    pub const DEFAULT_RATE: f64 = 1.0;
    pub const MIN_RATE: f64 = 0.01;
    pub const MAX_RATE: f64 = 5.0;

    /// The rate clamped to the supported range; invalid rates read as 1.
    pub fn safe_rate(retime: Option<&Self>) -> f64 {
        let rate = retime.map_or(Self::DEFAULT_RATE, |retime| retime.rate);
        if !rate.is_finite() || rate <= 0.0 {
            return Self::DEFAULT_RATE;
        }
        rate.clamp(Self::MIN_RATE, Self::MAX_RATE)
    }

    /// The config for `rate` (clamped), or `None` for plain 1x playback that
    /// keeps no pitch (classic speed tab's `buildRetime`).
    pub fn build(rate: f64, maintain_pitch: bool) -> Option<Self> {
        let rate = Self::safe_rate(Some(&Self {
            rate,
            maintain_pitch: None,
        }));
        (rate != Self::DEFAULT_RATE || maintain_pitch).then_some(Self {
            rate,
            maintain_pitch: Some(maintain_pitch),
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Effect {
    pub id: String,
    #[serde(rename = "type")]
    pub effect_type: String,
    #[serde(default)]
    pub params: ParamValues,
    pub enabled: bool,
}

/// A mask on a visual element. Params stay raw JSON until masks are ported
/// (roadmap P8); freeform masks carry a point path, not just scalars.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Mask {
    pub id: String,
    #[serde(rename = "type")]
    pub mask_type: String,
    pub params: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VideoElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub media_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_source_audio_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retime: Option<RetimeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<Effect>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masks: Option<Vec<Mask>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub media_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<Effect>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masks: Option<Vec<Mask>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextElement {
    #[serde(flatten)]
    pub base: ElementBase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<Effect>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StickerElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub sticker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intrinsic_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intrinsic_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<Effect>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphicElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub definition_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<Effect>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masks: Option<Vec<Mask>>,
}

/// An adjustment-layer element: applies `effect_type` to everything below it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EffectElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub effect_type: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "sourceType", rename_all = "lowercase")]
pub enum AudioSource {
    /// A file the user imported into the project's media.
    Upload {
        #[serde(rename = "mediaId")]
        media_id: String,
    },
    /// A clip from the online sound library.
    Library {
        #[serde(rename = "sourceUrl")]
        source_url: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioElement {
    #[serde(flatten)]
    pub base: ElementBase,
    #[serde(flatten)]
    pub source: AudioSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retime: Option<RetimeConfig>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ElementKind {
    Video,
    Image,
    Text,
    Sticker,
    Graphic,
    Effect,
    Audio,
}

impl ElementKind {
    /// The only track kind this element may sit on.
    pub const fn track_kind(self) -> TrackKind {
        match self {
            Self::Video | Self::Image => TrackKind::Video,
            Self::Text => TrackKind::Text,
            Self::Sticker | Self::Graphic => TrackKind::Graphic,
            Self::Effect => TrackKind::Effect,
            Self::Audio => TrackKind::Audio,
        }
    }

    pub const fn is_visual(self) -> bool {
        !matches!(self, Self::Audio | Self::Effect)
    }

    pub const fn is_retimable(self) -> bool {
        matches!(self, Self::Video | Self::Audio)
    }

    pub const fn is_maskable(self) -> bool {
        matches!(self, Self::Video | Self::Image | Self::Graphic)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Element {
    Video(VideoElement),
    Image(ImageElement),
    Text(TextElement),
    Sticker(StickerElement),
    Graphic(GraphicElement),
    Effect(EffectElement),
    Audio(AudioElement),
}

impl Element {
    pub const fn kind(&self) -> ElementKind {
        match self {
            Self::Video(_) => ElementKind::Video,
            Self::Image(_) => ElementKind::Image,
            Self::Text(_) => ElementKind::Text,
            Self::Sticker(_) => ElementKind::Sticker,
            Self::Graphic(_) => ElementKind::Graphic,
            Self::Effect(_) => ElementKind::Effect,
            Self::Audio(_) => ElementKind::Audio,
        }
    }

    pub const fn base(&self) -> &ElementBase {
        match self {
            Self::Video(element) => &element.base,
            Self::Image(element) => &element.base,
            Self::Text(element) => &element.base,
            Self::Sticker(element) => &element.base,
            Self::Graphic(element) => &element.base,
            Self::Effect(element) => &element.base,
            Self::Audio(element) => &element.base,
        }
    }

    pub const fn base_mut(&mut self) -> &mut ElementBase {
        match self {
            Self::Video(element) => &mut element.base,
            Self::Image(element) => &mut element.base,
            Self::Text(element) => &mut element.base,
            Self::Sticker(element) => &mut element.base,
            Self::Graphic(element) => &mut element.base,
            Self::Effect(element) => &mut element.base,
            Self::Audio(element) => &mut element.base,
        }
    }

    pub fn id(&self) -> &str {
        &self.base().id
    }

    pub fn start_time(&self) -> MediaTime {
        self.base().start_time
    }

    pub fn end_time(&self) -> MediaTime {
        self.base().end_time()
    }

    pub fn retime(&self) -> Option<&RetimeConfig> {
        match self {
            Self::Video(element) => element.retime.as_ref(),
            Self::Audio(element) => element.retime.as_ref(),
            _ => None,
        }
    }

    /// The media asset this element plays, if it comes from imported media.
    pub fn media_id(&self) -> Option<&str> {
        match self {
            Self::Video(element) => Some(&element.media_id),
            Self::Image(element) => Some(&element.media_id),
            Self::Audio(AudioElement {
                source: AudioSource::Upload { media_id },
                ..
            }) => Some(media_id),
            _ => None,
        }
    }

    /// `None` for element kinds that cannot be hidden (audio, effect).
    pub fn is_hidden(&self) -> Option<bool> {
        let hidden = match self {
            Self::Video(element) => element.hidden,
            Self::Image(element) => element.hidden,
            Self::Text(element) => element.hidden,
            Self::Sticker(element) => element.hidden,
            Self::Graphic(element) => element.hidden,
            Self::Effect(_) | Self::Audio(_) => return None,
        };
        Some(hidden.unwrap_or(false))
    }

    /// Returns `false` when this element kind cannot be hidden.
    pub fn set_hidden(&mut self, value: bool) -> bool {
        let hidden = match self {
            Self::Video(element) => &mut element.hidden,
            Self::Image(element) => &mut element.hidden,
            Self::Text(element) => &mut element.hidden,
            Self::Sticker(element) => &mut element.hidden,
            Self::Graphic(element) => &mut element.hidden,
            Self::Effect(_) | Self::Audio(_) => return false,
        };
        *hidden = Some(value);
        true
    }

    /// Sets the retime; returns `false` for kinds that can't be retimed.
    pub fn set_retime(&mut self, retime: Option<RetimeConfig>) -> bool {
        match self {
            Self::Video(element) => element.retime = retime,
            Self::Audio(element) => element.retime = retime,
            _ => return false,
        }
        true
    }
}
