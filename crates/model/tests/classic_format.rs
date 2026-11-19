//! The model must read and write the classic app's v31 project JSON.

use opencut_model::time::{FrameRate, MediaTime};
use opencut_model::{
    AudioSource, Background, Element, ElementKind, ElementRef, MediaAsset, MediaType,
    PROJECT_VERSION, ParamValue, Project, TrackKind,
};
use pretty_assertions::assert_eq;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/classic-v31.json");

fn load() -> Project {
    serde_json::from_str(FIXTURE).expect("fixture parses")
}

/// JSON numbers `1` and `1.0` compare unequal in `serde_json::Value`; params
/// are stored as `f64`, so compare every number as `f64`.
fn normalize(value: Value) -> Value {
    match value {
        Value::Number(number) => number
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, normalize(value)))
                .collect(),
        ),
        other => other,
    }
}

#[test]
fn reads_a_classic_v31_project() {
    let project = load();
    project.validate().expect("fixture is valid");
    assert!(project.media_assets.is_empty());

    assert_eq!(project.version, PROJECT_VERSION);
    assert_eq!(project.metadata.name, "Holiday edit");
    assert_eq!(project.settings.fps, FrameRate::FPS_29_97);
    assert_eq!(
        project.settings.background,
        Background::Blur {
            blur_intensity: 10.0
        }
    );

    let scene = project.current_scene().expect("current scene");
    assert_eq!(scene.bookmarks.len(), 2);

    let tracks = &scene.tracks;
    let kinds: Vec<TrackKind> = tracks.iter().map(|track| track.kind).collect();
    assert_eq!(
        kinds,
        [
            TrackKind::Text,
            TrackKind::Graphic,
            TrackKind::Effect,
            TrackKind::Video,
            TrackKind::Audio
        ]
    );
    assert!(tracks.track("track-graphic").unwrap().is_hidden());
    assert!(tracks.track("track-audio").unwrap().is_muted());
    assert_eq!(tracks.element_count(), 8);
    assert_eq!(tracks.duration(), MediaTime::from_ticks(1_224_000));

    let clip = tracks
        .element(&ElementRef::new("track-main", "el-clip-1"))
        .unwrap();
    assert_eq!(clip.kind(), ElementKind::Video);
    assert_eq!(clip.media_id(), Some("media-beach"));
    assert_eq!(clip.retime().map(|retime| retime.rate), Some(2.0));
    assert_eq!(
        clip.base().params.get("volume"),
        Some(&ParamValue::Number(0.8))
    );
    let Element::Video(video) = clip else {
        unreachable!()
    };
    assert_eq!(video.effects.as_ref().unwrap()[0].effect_type, "blur");
    assert_eq!(video.masks.as_ref().unwrap()[0].mask_type, "rectangle");

    let title = tracks
        .element(&ElementRef::new("track-text", "el-title"))
        .unwrap();
    assert_eq!(
        title.base().params_at(MediaTime::from_ticks(30_000))["opacity"],
        ParamValue::Number(0.5)
    );

    let whoosh = tracks
        .element(&ElementRef::new("track-audio", "el-whoosh"))
        .unwrap();
    let Element::Audio(audio) = whoosh else {
        unreachable!()
    };
    assert_eq!(
        audio.source,
        AudioSource::Library {
            source_url: "https://example.com/whoosh.mp3".to_owned()
        }
    );
    assert_eq!(whoosh.is_hidden(), None);
}

#[test]
fn round_trips_without_losing_data() {
    let original: Value = serde_json::from_str(FIXTURE).unwrap();
    let written = serde_json::to_value(load()).unwrap();
    assert_eq!(normalize(written), normalize(original));
}

#[test]
fn new_project_is_valid_and_serializes() {
    let project = Project::new("Untitled", chrono::Utc::now());
    project.validate().unwrap();
    assert_eq!(project.scenes.len(), 1);
    assert!(project.scenes[0].is_main);

    let json = serde_json::to_string(&project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, project);
}

#[test]
fn media_assets_are_stored_under_media_assets_and_omitted_when_empty() {
    let mut project = Project::new("Untitled", chrono::Utc::now());
    let empty = serde_json::to_value(&project).unwrap();
    assert!(empty.get("mediaAssets").is_none());

    project.media_assets.push(MediaAsset {
        id: "media-1".to_owned(),
        name: "beach.mp4".to_owned(),
        media_type: MediaType::Video,
        size: 1024,
        last_modified: 1_700_000_000_000,
        width: Some(1920),
        height: Some(1080),
        duration: Some(12.5),
        fps: Some(29.97),
        has_audio: Some(true),
        ephemeral: None,
        thumbnail_url: None,
    });
    let json = serde_json::to_value(&project).unwrap();
    assert_eq!(
        json["mediaAssets"][0],
        serde_json::json!({
            "id": "media-1",
            "name": "beach.mp4",
            "type": "video",
            "size": 1024,
            "lastModified": 1_700_000_000_000_i64,
            "width": 1920,
            "height": 1080,
            "duration": 12.5,
            "fps": 29.97,
            "hasAudio": true,
        })
    );
    let back: Project = serde_json::from_value(json).unwrap();
    assert_eq!(back, project);
    assert_eq!(
        back.media_asset("media-1").map(|asset| asset.size),
        Some(1024)
    );
}
