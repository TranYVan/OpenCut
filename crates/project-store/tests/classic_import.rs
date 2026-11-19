//! Importing zips shaped like the classic export script's output.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use opencut_project_store::{MEDIA_DIR, ProjectStore, StoreError};
use serde_json::{Value, json};
use zip::write::{SimpleFileOptions, ZipWriter};

const CLASSIC_FIXTURE: &str = include_str!("../../model/tests/fixtures/classic-v31.json");

fn now() -> DateTime<Utc> {
    "2026-11-19T12:00:00.000Z".parse().unwrap()
}

fn write_zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
    let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn media_item(id: &str, kind: &str, path: Option<&str>) -> Value {
    json!({
        "id": id,
        "status": if path.is_some() { "ok" } else { "missing-file" },
        "metadata": { "id": id, "name": format!("{id}.MP4"), "type": kind, "size": 4, "lastModified": 1 },
        "file": path.map(|path| json!({ "path": path, "size": 4, "mimeType": "video/mp4", "lastModified": 1 })),
    })
}

fn setup() -> (tempfile::TempDir, ProjectStore, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let store = ProjectStore::new(dir.path().join("projects"));
    let zip = dir.path().join("export.zip");
    (dir, store, zip)
}

fn project_folders(store: &ProjectStore) -> usize {
    fs::read_dir(store.root()).map_or(0, |entries| entries.count())
}

#[test]
fn imports_a_v31_export_with_its_media() {
    let (_dir, store, zip) = setup();
    let record: Value = serde_json::from_str(CLASSIC_FIXTURE).unwrap();
    let beach = media_item("media-beach", "video", Some("media/media-beach/beach.MP4"));
    // An id that isn't a safe file name gets a new one, and elements follow.
    let mut record = record;
    let text = record.to_string().replace("media-sunset", "../sunset");
    record = serde_json::from_str(&text).unwrap();
    let media = json!({ "items": [
        beach,
        media_item("../sunset", "image", Some("media/sunset/sunset.png")),
        media_item("media-song", "audio", None),
    ] });
    write_zip(
        &zip,
        &[
            ("project.json", json_bytes(&record)),
            ("media.json", json_bytes(&media)),
            ("media/media-beach/beach.MP4", b"beac".to_vec()),
            ("media/sunset/sunset.png", b"suns".to_vec()),
            ("export-info.json", b"{}".to_vec()),
        ],
    );

    let project = store.import_classic_zip(&zip, now()).unwrap();
    assert_ne!(project.metadata.id, record["metadata"]["id"]);
    assert_eq!(store.load(&project.metadata.id).unwrap(), project);

    let ids: Vec<&str> = project
        .media_assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect();
    assert_eq!(ids.len(), 2, "the missing file isn't imported");
    assert_eq!(ids[0], "media-beach");
    let sunset_id = ids[1];
    assert_ne!(sunset_id, "../sunset");

    let media_dir = store
        .project_dir(&project.metadata.id)
        .unwrap()
        .join(MEDIA_DIR);
    assert_eq!(
        fs::read(media_dir.join("media-beach.mp4")).unwrap(),
        b"beac"
    );
    let saved = fs::read_to_string(
        store
            .project_dir(&project.metadata.id)
            .unwrap()
            .join("project.json"),
    )
    .unwrap();
    assert!(!saved.contains("\"mediaId\": \"../sunset\""));
    assert!(saved.contains(&format!("\"mediaId\": \"{sunset_id}\"")));
    assert_eq!(
        fs::read(media_dir.join(format!("{sunset_id}.mp4"))).unwrap(),
        b"suns"
    );
}

#[test]
fn imports_an_old_export_through_the_migrations() {
    let (_dir, store, zip) = setup();
    let record = json!({ "id": "old", "name": "Old project", "createdAt": "2024-01-15T10:00:00.000Z",
        "updatedAt": "2024-01-15T12:00:00.000Z", "fps": 30, "canvasSize": { "width": 1280, "height": 720 } });
    let legacy = json!({ "database": "video-editor-timelines-old", "store": "timeline", "entries": [{
        "key": "timeline",
        "value": { "tracks": [{ "id": "t1", "type": "media", "name": "Media", "elements": [
            { "id": "e1", "name": "Clip", "type": "media", "mediaId": "m1", "duration": 2, "startTime": 0, "trimStart": 0, "trimEnd": 0 },
        ] }] },
    }] });
    write_zip(
        &zip,
        &[
            ("project.json", json_bytes(&record)),
            (
                "media.json",
                json_bytes(
                    &json!({ "items": [media_item("m1", "video", Some("media/m1/m1.mp4"))] }),
                ),
            ),
            ("media/m1/m1.mp4", b"clip".to_vec()),
            (
                "legacy/video-editor-timelines-old.json",
                json_bytes(&legacy),
            ),
        ],
    );

    let project = store.import_classic_zip(&zip, now()).unwrap();
    assert_eq!(project.metadata.name, "Old project");
    assert_eq!(
        project.current_scene().unwrap().tracks.main.elements.len(),
        1
    );
    assert_eq!(project.media_assets.len(), 1);
}

#[test]
fn a_failed_import_leaves_no_folder_behind() {
    let (_dir, store, zip) = setup();
    write_zip(
        &zip,
        &[(
            "project.json",
            json_bytes(&json!({ "id": "x", "version": 99 })),
        )],
    );
    assert!(matches!(
        store.import_classic_zip(&zip, now()),
        Err(StoreError::Migration(_))
    ));

    write_zip(&zip, &[("media.json", b"{}".to_vec())]);
    assert!(matches!(
        store.import_classic_zip(&zip, now()),
        Err(StoreError::NotClassicExport(_))
    ));

    // media.json points at an entry that isn't in the zip: copying fails.
    let record: Value = serde_json::from_str(CLASSIC_FIXTURE).unwrap();
    let media =
        json!({ "items": [media_item("media-beach", "video", Some("media/gone/beach.mp4"))] });
    write_zip(
        &zip,
        &[
            ("project.json", json_bytes(&record)),
            ("media.json", json_bytes(&media)),
        ],
    );
    assert!(matches!(
        store.import_classic_zip(&zip, now()),
        Err(StoreError::Zip { .. })
    ));
    assert_eq!(project_folders(&store), 0);
}

#[test]
fn zip_entry_names_never_become_paths() {
    let (dir, store, zip) = setup();
    let record: Value = serde_json::from_str(CLASSIC_FIXTURE).unwrap();
    let escape = "media/../../escaped.mp4";
    let media = json!({ "items": [media_item("media-beach", "video", Some(escape))] });
    write_zip(
        &zip,
        &[
            ("project.json", json_bytes(&record)),
            ("media.json", json_bytes(&media)),
            (escape, b"evil".to_vec()),
        ],
    );
    assert!(store.import_classic_zip(&zip, now()).is_err());
    assert!(!dir.path().join("escaped.mp4").exists());
    assert_eq!(project_folders(&store), 0);
}
