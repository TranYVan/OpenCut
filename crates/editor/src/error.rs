use opencut_model::time::MediaTime;
use opencut_model::{ElementKind, ElementRef, ParamValue, TrackKind, ValidationError};

/// Why a command refused to run. A failed command leaves the project untouched.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EditError {
    #[error("the project has no current scene")]
    NoCurrentScene,
    #[error("track {0} not found")]
    TrackNotFound(String),
    #[error("element {} not found on track {}", .0.element_id, .0.track_id)]
    ElementNotFound(ElementRef),
    #[error("{element:?} elements cannot be placed on {track:?} tracks")]
    IncompatibleTrack {
        element: ElementKind,
        track: TrackKind,
    },
    #[error("element {element_id} would overlap another element on track {track_id}")]
    Overlap {
        track_id: String,
        element_id: String,
    },
    #[error("the main track cannot be removed")]
    CannotRemoveMainTrack,
    #[error("{0:?} tracks cannot be muted")]
    CannotMute(TrackKind),
    #[error("{0:?} tracks cannot be hidden")]
    CannotHide(TrackKind),
    #[error("element times must be non-negative and the duration positive")]
    InvalidTiming,
    #[error("split time {0:?} is not inside any of the given elements")]
    NothingToSplit(MediaTime),
    #[error("invalid project settings: {0}")]
    InvalidSettings(ValidationError),
    #[error("media asset {0} not found")]
    MediaAssetNotFound(String),
    #[error("a media asset with id {0} already exists")]
    DuplicateMediaAsset(String),
    #[error("{element:?} elements have no {key} param")]
    UnknownParam { element: ElementKind, key: String },
    #[error("{value:?} is not a valid value for {key}")]
    InvalidParamValue { key: String, value: ParamValue },
    #[error("{0:?} elements cannot be retimed")]
    NotRetimable(ElementKind),
    #[error("the element has no source audio to separate")]
    NoSourceAudio,
    #[error("{0:?} elements have no source audio")]
    NoSourceAudioOn(ElementKind),
    #[error("nothing to do")]
    Empty,
}
