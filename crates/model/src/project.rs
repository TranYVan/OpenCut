use chrono::{DateTime, SubsecRound, Utc};
use opencut_time::{FrameRate, MediaTime};
use serde::{Deserialize, Serialize};

use crate::new_id;
use crate::track::SceneTracks;

/// The classic app's last storage version. Desktop project files use the same
/// JSON shape, so a classic project migrated to this version loads directly.
pub const PROJECT_VERSION: u32 = 31;

pub const DEFAULT_FPS: FrameRate = FrameRate::FPS_30;
pub const DEFAULT_CANVAS_SIZE: CanvasSize = CanvasSize {
    width: 1920,
    height: 1080,
};
pub const DEFAULT_BACKGROUND_COLOR: &str = "#000000";
pub const MAIN_SCENE_NAME: &str = "Main scene";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub time: MediaTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<MediaTime>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub id: String,
    pub name: String,
    pub is_main: bool,
    pub tracks: SceneTracks,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(with = "crate::timestamp")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "crate::timestamp")]
    pub updated_at: DateTime<Utc>,
}

impl Scene {
    /// `now` is truncated to milliseconds, the precision the file stores.
    pub fn new(name: impl Into<String>, is_main: bool, now: DateTime<Utc>) -> Self {
        let now = now.trunc_subsecs(3);
        Self {
            id: new_id(),
            name: name.into(),
            is_main,
            tracks: SceneTracks::new(new_id()),
            bookmarks: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanvasSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CanvasSizeMode {
    Preset,
    Custom,
}

/// What shows behind the canvas where no element covers it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Background {
    Color {
        color: String,
    },
    Blur {
        #[serde(rename = "blurIntensity")]
        blur_intensity: f64,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    pub fps: FrameRate,
    pub canvas_size: CanvasSize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_size_mode: Option<CanvasSizeMode>,
    #[serde(default)]
    pub last_custom_canvas_size: Option<CanvasSize>,
    #[serde(default)]
    pub original_canvas_size: Option<CanvasSize>,
    pub background: Background,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            fps: DEFAULT_FPS,
            canvas_size: DEFAULT_CANVAS_SIZE,
            canvas_size_mode: Some(CanvasSizeMode::Preset),
            last_custom_canvas_size: None,
            original_canvas_size: None,
            background: Background::Color {
                color: DEFAULT_BACKGROUND_COLOR.to_owned(),
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMetadata {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    pub duration: MediaTime,
    #[serde(with = "crate::timestamp")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "crate::timestamp")]
    pub updated_at: DateTime<Utc>,
}

/// Where the timeline was scrolled and zoomed when the project was saved.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineViewState {
    pub zoom_level: f64,
    pub scroll_left: f64,
    pub playhead_time: MediaTime,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub metadata: ProjectMetadata,
    pub scenes: Vec<Scene>,
    pub current_scene_id: String,
    pub settings: ProjectSettings,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_view_state: Option<TimelineViewState>,
}

impl Project {
    /// A new project with one empty main scene and default settings.
    /// `now` is truncated to milliseconds, the precision the file stores.
    pub fn new(name: impl Into<String>, now: DateTime<Utc>) -> Self {
        let now = now.trunc_subsecs(3);
        let scene = Scene::new(MAIN_SCENE_NAME, true, now);
        Self {
            metadata: ProjectMetadata {
                id: new_id(),
                name: name.into(),
                thumbnail: None,
                duration: MediaTime::ZERO,
                created_at: now,
                updated_at: now,
            },
            current_scene_id: scene.id.clone(),
            scenes: vec![scene],
            settings: ProjectSettings::default(),
            version: PROJECT_VERSION,
            timeline_view_state: None,
        }
    }

    pub fn scene(&self, scene_id: &str) -> Option<&Scene> {
        self.scenes.iter().find(|scene| scene.id == scene_id)
    }

    pub fn scene_mut(&mut self, scene_id: &str) -> Option<&mut Scene> {
        self.scenes.iter_mut().find(|scene| scene.id == scene_id)
    }

    pub fn current_scene(&self) -> Option<&Scene> {
        self.scene(&self.current_scene_id)
    }

    pub fn current_scene_mut(&mut self) -> Option<&mut Scene> {
        let id = self.current_scene_id.clone();
        self.scene_mut(&id)
    }

    /// Recomputes `metadata.duration` from the main scene, or the current
    /// scene when no scene is marked main.
    pub fn refresh_duration(&mut self) {
        let scene = self
            .scenes
            .iter()
            .find(|scene| scene.is_main)
            .or_else(|| self.current_scene());
        self.metadata.duration = scene.map_or(MediaTime::ZERO, |scene| scene.tracks.duration());
    }

    /// Checks invariants serde cannot express: a valid frame rate, an existing
    /// current scene, and every element on a track of the matching kind.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.settings.fps.ticks_per_frame().is_none() {
            return Err(ValidationError::UnsupportedFrameRate(self.settings.fps));
        }
        if self.current_scene().is_none() {
            return Err(ValidationError::MissingCurrentScene(
                self.current_scene_id.clone(),
            ));
        }
        for scene in &self.scenes {
            if let Some(misplaced) = scene.tracks.misplaced_elements().into_iter().next() {
                return Err(ValidationError::MisplacedElement {
                    scene_id: scene.id.clone(),
                    track_id: misplaced.track_id,
                    element_id: misplaced.element_id,
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    UnsupportedFrameRate(FrameRate),
    MissingCurrentScene(String),
    MisplacedElement {
        scene_id: String,
        track_id: String,
        element_id: String,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedFrameRate(rate) => write!(
                f,
                "frame rate {}/{} does not divide into whole ticks",
                rate.numerator, rate.denominator
            ),
            Self::MissingCurrentScene(id) => write!(f, "current scene {id} does not exist"),
            Self::MisplacedElement {
                scene_id,
                track_id,
                element_id,
            } => write!(
                f,
                "element {element_id} sits on a track of the wrong kind ({track_id}, scene {scene_id})"
            ),
        }
    }
}

impl std::error::Error for ValidationError {}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Image,
    Video,
    Audio,
}

/// Metadata of an imported media file (classic `MediaAssetData`). The file
/// itself is stored next to the project, not in this document.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaAsset {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub media_type: MediaType,
    pub size: u64,
    pub last_modified: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
}
