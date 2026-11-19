//! Projects survive a trip through the disk: save, list, load, rename,
//! delete, in a temp dir.

use std::fs;

use chrono::{DateTime, TimeZone, Utc};
use opencut_project_store::model::time::MediaTime;
use opencut_project_store::model::{MediaAsset, MediaType, Project, TimelineViewState};
use opencut_project_store::{
    MEDIA_DIR, PROJECT_FILE, ProjectStore, SortKey, SortOrder, StoreError, sort_projects,
};
use pretty_assertions::assert_eq;
use serde_json::Value;

const CLASSIC_FIXTURE: &str = include_str!("../../model/tests/fixtures/classic-v31.json");

fn at(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + seconds, 0).unwrap()
}

fn store() -> (tempfile::TempDir, ProjectStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ProjectStore::new(dir.path().join("projects"));
    (dir, store)
}

/// A project with everything the editor touches: media, view state.
fn edited_project() -> Project {
    let mut project = Project::new("Trip", at(0));
    project.media_assets.push(MediaAsset {
        id: "media-1".to_owned(),
        name: "beach.mp4".to_owned(),
        media_type: MediaType::Video,
        size: 2048,
        last_modified: 1_700_000_000_000,
        width: Some(1920),
        height: Some(1080),
        duration: Some(4.0),
        fps: Some(30.0),
        has_audio: Some(true),
        ephemeral: None,
        thumbnail_url: None,
    });
    project.timeline_view_state = Some(TimelineViewState {
        zoom_level: 2.5,
        scroll_left: 310.0,
        playhead_time: MediaTime::from_ticks(240_000),
    });
    project
}

#[test]
fn an_empty_or_missing_store_lists_nothing() {
    let (_dir, store) = store();
    let list = store.list().unwrap();
    assert!(list.projects.is_empty());
    assert!(list.unreadable.is_empty());
}

#[test]
fn saved_projects_load_back_exactly() {
    let (_dir, store) = store();
    let project = edited_project();
    store.save(&project).unwrap();

    let dir = store.project_dir(&project.metadata.id).unwrap();
    assert!(dir.join(PROJECT_FILE).is_file());
    assert_eq!(store.load(&project.metadata.id).unwrap(), project);

    // Saving again replaces the file and leaves no temp files behind.
    let mut renamed = project.clone();
    renamed.metadata.name = "Trip, final".to_owned();
    store.save(&renamed).unwrap();
    assert_eq!(store.load(&project.metadata.id).unwrap(), renamed);
    let files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, [PROJECT_FILE]);
}

#[test]
fn classic_v31_projects_round_trip_through_the_store() {
    let (_dir, store) = store();
    let classic: Project = serde_json::from_str(CLASSIC_FIXTURE).unwrap();
    store.save(&classic).unwrap();

    let loaded = store.load(&classic.metadata.id).unwrap();
    assert_eq!(loaded, classic);
    assert_eq!(
        loaded.timeline_view_state,
        Some(TimelineViewState {
            zoom_level: 1.5,
            scroll_left: 240.0,
            playhead_time: MediaTime::from_ticks(360_000),
        })
    );

    // The file on disk is still classic JSON: same keys, same values.
    let path = store
        .project_dir(&classic.metadata.id)
        .unwrap()
        .join(PROJECT_FILE);
    let on_disk: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let original: Value = serde_json::from_str(CLASSIC_FIXTURE).unwrap();
    assert_eq!(normalize(on_disk), normalize(original));
}

#[test]
fn create_list_rename_and_delete() {
    let (_dir, store) = store();
    let first = store.create("  First  ", at(0)).unwrap();
    let second = store.create("second", at(10)).unwrap();
    assert_eq!(first.metadata.name, "First");

    // Media lives inside the project folder and goes with it on delete.
    let media = store
        .project_dir(&first.metadata.id)
        .unwrap()
        .join(MEDIA_DIR);
    fs::create_dir_all(&media).unwrap();
    fs::write(media.join("clip.mp4"), b"not really a video").unwrap();

    let mut list = store.list().unwrap().projects;
    sort_projects(&mut list, SortKey::UpdatedAt, SortOrder::Descending);
    let names: Vec<_> = list.iter().map(|meta| meta.name.as_str()).collect();
    assert_eq!(names, ["second", "First"]);

    let renamed = store.rename(&first.metadata.id, "Zeta", at(20)).unwrap();
    assert_eq!(renamed.name, "Zeta");
    assert_eq!(renamed.updated_at, at(20));
    assert_eq!(renamed.created_at, first.metadata.created_at);
    let reloaded = store.load(&first.metadata.id).unwrap();
    assert_eq!(reloaded.metadata, renamed);
    assert_eq!(reloaded.scenes, first.scenes);
    assert!(matches!(
        store.rename(&first.metadata.id, "   ", at(30)),
        Err(StoreError::EmptyName)
    ));

    let mut list = store.list().unwrap().projects;
    sort_projects(&mut list, SortKey::UpdatedAt, SortOrder::Descending);
    let names: Vec<_> = list.iter().map(|meta| meta.name.as_str()).collect();
    assert_eq!(names, ["Zeta", "second"]);
    sort_projects(&mut list, SortKey::Name, SortOrder::Ascending);
    let names: Vec<_> = list.iter().map(|meta| meta.name.as_str()).collect();
    assert_eq!(names, ["second", "Zeta"]);
    sort_projects(&mut list, SortKey::CreatedAt, SortOrder::Ascending);
    let names: Vec<_> = list.iter().map(|meta| meta.name.as_str()).collect();
    assert_eq!(names, ["Zeta", "second"]);

    store.delete(&first.metadata.id).unwrap();
    assert!(!store.project_dir(&first.metadata.id).unwrap().exists());
    let list = store.list().unwrap().projects;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, second.metadata.id);
    assert!(matches!(
        store.delete(&first.metadata.id),
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.load(&first.metadata.id),
        Err(StoreError::NotFound(_))
    ));
}

#[test]
fn list_skips_non_projects_and_reports_broken_ones() {
    let (_dir, store) = store();
    let good = store.create("Good", at(0)).unwrap();

    // A media-only folder (no project.json) isn't a project.
    fs::create_dir_all(store.root().join("leftover/media")).unwrap();
    // A stray file at the root is ignored.
    fs::write(store.root().join("notes.txt"), "hi").unwrap();
    // Corrupt JSON is reported, not fatal.
    let broken = store.root().join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(PROJECT_FILE), "{ not json").unwrap();

    let list = store.list().unwrap();
    assert_eq!(list.projects.len(), 1);
    assert_eq!(list.projects[0].id, good.metadata.id);
    assert_eq!(list.unreadable.len(), 1);
    assert_eq!(list.unreadable[0].0, broken);
}

#[test]
fn refuses_unsupported_versions_and_unsafe_ids() {
    let (_dir, store) = store();
    let mut project = Project::new("Old", at(0));
    project.version = 30;
    store.save(&project).unwrap();
    assert!(matches!(
        store.load(&project.metadata.id),
        Err(StoreError::UnsupportedVersion(30))
    ));

    for id in ["../escape", "a/b", ".."] {
        assert!(matches!(store.load(id), Err(StoreError::InvalidId(_))));
        assert!(matches!(store.delete(id), Err(StoreError::InvalidId(_))));
    }
    let mut escaping = Project::new("Escaping", at(0));
    escaping.metadata.id = "../outside".to_owned();
    assert!(matches!(
        store.save(&escaping),
        Err(StoreError::InvalidId(_))
    ));
}

/// JSON numbers `1` and `1.0` compare unequal in `serde_json::Value`.
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
