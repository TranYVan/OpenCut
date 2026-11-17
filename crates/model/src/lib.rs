//! The OpenCut project document.
//!
//! Mirrors the classic app's storage format at version [`PROJECT_VERSION`]
//! (tag `pre-rewrite`, `apps/web/src/timeline/types.ts` and
//! `apps/web/src/project/types.ts`), with the same camelCase JSON, so classic
//! projects can be imported. Pure data: no I/O, no editing rules. Editing
//! lives in `opencut-editor`.

mod element;
mod params;
mod project;
mod timestamp;
mod track;

pub use element::{
    Animations, AudioElement, AudioSource, Effect, EffectElement, Element, ElementBase,
    ElementKind, GraphicElement, ImageElement, Mask, RetimeConfig, StickerElement, TextElement,
    VideoElement,
};
pub use params::{ParamValue, ParamValues};
pub use project::{
    Background, Bookmark, CanvasSize, CanvasSizeMode, DEFAULT_BACKGROUND_COLOR,
    DEFAULT_CANVAS_SIZE, DEFAULT_FPS, MAIN_SCENE_NAME, MediaAsset, MediaType, PROJECT_VERSION,
    Project, ProjectMetadata, ProjectSettings, Scene, TimelineViewState, ValidationError,
};
pub use track::{ElementRef, MAIN_TRACK_NAME, SceneTracks, Track, TrackKind};

pub use opencut_time as time;

/// A fresh random id (UUID v4), the format the classic app used.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
