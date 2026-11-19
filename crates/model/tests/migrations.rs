//! Port of the classic migration tests
//! (`apps/web/src/services/storage/migrations/__tests__/`, tag `pre-rewrite`),
//! plus whole-chain checks that every fixture loads as a v31 project.

#![recursion_limit = "256"]

use chrono::{DateTime, Utc};
use opencut_model::Project;
use opencut_model::migrations::{self, LegacyData, MigrationResult, V1ToV2Context};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let file = match name {
        "projectMalformed"
        | "projectWithNoId"
        | "projectWithNullValues"
        | "v0Project"
        | "v0ProjectEmpty"
        | "v0ProjectWithMetadata" => include_str!("fixtures/migrations/v0.json"),
        "v1Project" | "v1ProjectWithMultipleScenes" => include_str!("fixtures/migrations/v1.json"),
        "v2Project"
        | "v2ProjectWithBlurBackground"
        | "v2ProjectEmptyScenes"
        | "v2ProjectSceneWithoutTracks" => include_str!("fixtures/migrations/v2.json"),
        "v3Project" => include_str!("fixtures/migrations/v3.json"),
        "v5Project" => include_str!("fixtures/migrations/v5.json"),
        _ => panic!("unknown fixture {name}"),
    };
    let all: Value = serde_json::from_str(file).unwrap();
    all[name].clone()
}

fn fixed_date() -> DateTime<Utc> {
    "2024-06-01T12:00:00.000Z".parse().unwrap()
}

fn done(result: MigrationResult) -> Value {
    assert_eq!(result.skipped, None);
    result.project
}

fn skipped(result: MigrationResult, reason: &str) {
    assert_eq!(result.skipped.as_deref(), Some(reason));
}

/// `{ ...base, ...overrides }`.
fn merge(base: &Value, overrides: Value) -> Value {
    let mut merged = base.clone();
    for (key, value) in overrides.as_object().unwrap() {
        merged[key] = value.clone();
    }
    merged
}

fn first_element(project: &Value) -> &Value {
    &project["scenes"][0]["tracks"][0]["elements"][0]
}

/// Numbers as f64 so `1` and `1.0` compare equal.
fn normalize(value: Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap()),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}

fn v1_to_v2(project: Value) -> MigrationResult {
    migrations::v1_to_v2(project, &V1ToV2Context::default(), fixed_date())
}

// --- v0 → v1 ---------------------------------------------------------------

mod v0_to_v1 {
    use super::*;

    fn run(project: Value) -> MigrationResult {
        migrations::v0_to_v1(project, fixed_date())
    }

    #[test]
    fn adds_scenes_array_to_v0_project() {
        let project = done(run(fixture("v0Project")));
        assert_eq!(project["version"], 1);
        assert_eq!(project["scenes"].as_array().unwrap().len(), 1);
        assert!(project["currentSceneId"].is_string());
    }

    #[test]
    fn creates_main_scene_with_correct_structure() {
        let project = done(run(fixture("v0Project")));
        let scene = &project["scenes"][0];
        assert_eq!(scene["isMain"], true);
        assert_eq!(scene["name"], "Main scene");
        assert!(scene["id"].is_string());
        assert!(scene["tracks"].is_array());
        assert!(scene["bookmarks"].is_array());
    }

    #[test]
    fn updates_metadata_updated_at_when_metadata_exists() {
        let project = done(run(fixture("v0ProjectWithMetadata")));
        assert_eq!(project["metadata"]["updatedAt"], "2024-06-01T12:00:00.000Z");
    }

    #[test]
    fn updates_root_updated_at_when_no_metadata() {
        let project = done(run(fixture("v0ProjectEmpty")));
        assert_eq!(project["updatedAt"], "2024-06-01T12:00:00.000Z");
    }

    #[test]
    fn skips_project_that_already_has_scenes() {
        skipped(run(fixture("v1Project")), "already has scenes");
    }

    #[test]
    fn preserves_original_project_properties() {
        let original = fixture("v0Project");
        let project = done(run(original.clone()));
        for key in ["id", "name", "fps", "canvasSize"] {
            assert_eq!(project[key], original[key]);
        }
    }

    #[test]
    fn project_id_from_root_level() {
        assert_eq!(
            migrations::project_id(&fixture("v0Project")),
            Some("project-v0-123")
        );
    }

    #[test]
    fn project_id_from_metadata_when_root_id_missing() {
        let project = json!({ "metadata": { "id": "from-metadata" } });
        assert_eq!(migrations::project_id(&project), Some("from-metadata"));
    }

    #[test]
    fn project_id_none_when_no_id_found() {
        assert_eq!(migrations::project_id(&fixture("projectWithNoId")), None);
    }

    #[test]
    fn project_id_of_malformed_project() {
        assert_eq!(
            migrations::project_id(&fixture("projectMalformed")),
            Some("project-malformed")
        );
    }
}

// --- v1 → v2 ---------------------------------------------------------------

mod v1_to_v2 {
    use super::*;

    #[test]
    fn creates_metadata_object_from_flat_properties() {
        let original = fixture("v1Project");
        let project = done(v1_to_v2(original.clone()));
        assert_eq!(project["version"], 2);
        let metadata = &project["metadata"];
        assert_eq!(metadata["id"], original["id"]);
        assert_eq!(metadata["name"], original["name"]);
        assert!(metadata["createdAt"].is_string());
        assert!(metadata["updatedAt"].is_string());
    }

    #[test]
    fn creates_settings_object_from_flat_properties() {
        let original = fixture("v1Project");
        let project = done(v1_to_v2(original.clone()));
        let settings = &project["settings"];
        assert_eq!(settings["fps"], original["fps"]);
        assert_eq!(settings["canvasSize"], original["canvasSize"]);
        assert_eq!(settings["originalCanvasSize"], Value::Null);
    }

    #[test]
    fn converts_color_background_correctly() {
        let original = fixture("v1Project");
        let project = done(v1_to_v2(original.clone()));
        let background = &project["settings"]["background"];
        assert_eq!(background["type"], "color");
        assert_eq!(background["color"], original["backgroundColor"]);
    }

    #[test]
    fn converts_blur_background_correctly() {
        let project = merge(
            &fixture("v1Project"),
            json!({ "backgroundType": "blur", "blurIntensity": 30 }),
        );
        let project = done(v1_to_v2(project));
        let background = &project["settings"]["background"];
        assert_eq!(background["type"], "blur");
        assert_eq!(background["blurIntensity"], 30);
    }

    #[test]
    fn applies_legacy_bookmarks_to_main_scene() {
        let original = fixture("v1Project");
        let project = done(v1_to_v2(original.clone()));
        let main = project["scenes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["isMain"] == true)
            .unwrap();
        assert_eq!(main["bookmarks"], original["bookmarks"]);
    }

    #[test]
    fn preserves_existing_scene_bookmarks() {
        let project = done(v1_to_v2(fixture("v1ProjectWithMultipleScenes")));
        let intro = project["scenes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "Intro")
            .unwrap();
        assert_eq!(
            normalize(intro["bookmarks"].clone()),
            normalize(json!([1.0]))
        );
    }

    #[test]
    fn skips_project_that_already_has_v2_structure() {
        skipped(v1_to_v2(fixture("v2Project")), "already v2");
    }

    #[test]
    fn skips_project_with_no_id() {
        skipped(v1_to_v2(fixture("projectWithNoId")), "no project id");
    }

    #[test]
    fn handles_null_values_gracefully() {
        let project = done(v1_to_v2(fixture("projectWithNullValues")));
        assert_eq!(project["settings"]["fps"], 30);
        assert_eq!(
            project["settings"]["canvasSize"],
            json!({ "width": 1920, "height": 1080 })
        );
    }

    #[test]
    fn uses_default_values_for_missing_properties() {
        let project = done(v1_to_v2(
            json!({ "id": "minimal", "version": 1, "scenes": [] }),
        ));
        let settings = &project["settings"];
        assert_eq!(settings["fps"], 30);
        assert_eq!(
            settings["canvasSize"],
            json!({ "width": 1920, "height": 1080 })
        );
        assert_eq!(settings["background"]["type"], "color");
        assert_eq!(settings["background"]["color"], "#000000");
    }

    #[test]
    fn uses_default_blur_intensity_when_missing() {
        let project = json!({ "id": "blur-no-intensity", "version": 1, "backgroundType": "blur", "scenes": [] });
        let project = done(v1_to_v2(project));
        assert_eq!(project["settings"]["background"]["blurIntensity"], 10);
    }

    #[test]
    fn preserves_current_scene_id() {
        let original = fixture("v1Project");
        let project = done(v1_to_v2(original.clone()));
        assert_eq!(project["currentSceneId"], original["currentSceneId"]);
    }

    #[test]
    fn finds_main_scene_id_when_current_scene_id_missing() {
        let mut project = fixture("v1Project");
        project.as_object_mut().unwrap().remove("currentSceneId");
        let project = done(v1_to_v2(project));
        assert_eq!(project["currentSceneId"], "scene-main");
    }

    #[test]
    fn skips_loading_tracks_if_scene_already_has_tracks() {
        let project = merge(
            &fixture("v1Project"),
            json!({ "scenes": [{
                "id": "scene-main", "name": "Main scene", "isMain": true,
                "tracks": [{ "id": "track-1", "type": "video", "name": "Existing Track", "elements": [] }],
                "bookmarks": [],
                "createdAt": "2024-01-15T10:00:00.000Z", "updatedAt": "2024-01-15T12:00:00.000Z",
            }] }),
        );
        let project = done(v1_to_v2(project));
        let tracks = project["scenes"][0]["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["name"], "Existing Track");
    }

    #[test]
    fn loads_legacy_tracks_and_transforms_media_track_to_video_track() {
        let context = V1ToV2Context {
            legacy_tracks_by_scene_id: [(
                "scene-main".to_owned(),
                vec![json!({
                    "id": "legacy-track-1", "type": "media", "name": "Legacy media track",
                    "elements": [{
                        "id": "media-element-1", "name": "Test video clip", "type": "media",
                        "mediaId": "media-1", "duration": 120, "startTime": 0, "trimStart": 0, "trimEnd": 0,
                    }],
                })],
            )]
            .into(),
            media_types_by_id: [("media-1".to_owned(), "video".to_owned())].into(),
        };
        let project = merge(
            &fixture("v1Project"),
            json!({ "scenes": [{
                "id": "scene-main", "name": "Main scene", "isMain": true, "tracks": [], "bookmarks": [],
                "createdAt": "2024-01-15T10:00:00.000Z", "updatedAt": "2024-01-15T12:00:00.000Z",
            }] }),
        );
        let project = done(migrations::v1_to_v2(project, &context, fixed_date()));
        let tracks = project["scenes"][0]["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["type"], "video");
        assert_eq!(tracks[0]["isMain"], true);
    }

    #[test]
    fn transforms_text_element_preserving_opacity_and_migrating_position() {
        let context = V1ToV2Context {
            legacy_tracks_by_scene_id: [(
                "scene-1".to_owned(),
                vec![json!({
                    "id": "legacy-text-track", "type": "text", "name": "Text",
                    "elements": [{
                        "id": "text-element-1", "name": "Title", "type": "text", "content": "Hello",
                        "x": 120, "y": 240, "rotation": 15, "opacity": 0.5,
                        "duration": 90, "startTime": 10, "trimStart": 0, "trimEnd": 0,
                    }],
                })],
            )]
            .into(),
            media_types_by_id: Default::default(),
        };
        let project = json!({
            "id": "project-text", "version": 1, "name": "Text Project",
            "scenes": [{
                "id": "scene-1", "name": "Scene", "isMain": true, "tracks": [], "bookmarks": [],
                "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-01-01T00:00:00.000Z",
            }],
        });
        let project = done(migrations::v1_to_v2(project, &context, fixed_date()));
        let element = first_element(&project);
        assert_eq!(element["opacity"], 0.5);
        assert_eq!(
            element["transform"],
            json!({ "scale": 1, "position": { "x": 120, "y": 240 }, "rotate": 15 })
        );
    }

    #[test]
    fn project_id_from_root_level() {
        assert_eq!(
            migrations::project_id(&fixture("v1Project")),
            Some("project-v1-123")
        );
    }

    #[test]
    fn project_id_from_metadata() {
        assert_eq!(
            migrations::project_id(&fixture("v1ProjectWithMultipleScenes")),
            Some("project-v1-multi")
        );
    }
}

// --- v2 → v3 ---------------------------------------------------------------

mod v2_to_v3 {
    use super::*;
    use migrations::v2_to_v3 as run;

    #[test]
    fn adds_duration_to_metadata() {
        let project = done(run(fixture("v2Project")));
        assert_eq!(project["version"], 3);
        assert!(project["metadata"]["duration"].is_number());
    }

    #[test]
    fn calculates_duration_from_scene_tracks() {
        assert_eq!(
            done(run(fixture("v2Project")))["metadata"]["duration"],
            15.5
        );
    }

    #[test]
    fn handles_project_with_blur_background() {
        assert_eq!(
            done(run(fixture("v2ProjectWithBlurBackground")))["metadata"]["duration"],
            30
        );
    }

    #[test]
    fn handles_empty_scenes_with_zero_duration() {
        assert_eq!(
            done(run(fixture("v2ProjectEmptyScenes")))["metadata"]["duration"],
            0
        );
    }

    #[test]
    fn handles_scene_without_tracks_property() {
        assert_eq!(
            done(run(fixture("v2ProjectSceneWithoutTracks")))["metadata"]["duration"],
            0
        );
    }

    #[test]
    fn skips_project_that_already_has_v3_structure() {
        skipped(run(fixture("v3Project")), "already v3");
    }

    #[test]
    fn skips_project_that_has_duration_in_metadata() {
        let mut project = fixture("v2Project");
        project["metadata"]["duration"] = json!(10);
        skipped(run(project), "already v3");
    }

    #[test]
    fn skips_project_with_no_id() {
        skipped(run(fixture("projectWithNoId")), "no project id");
    }

    #[test]
    fn preserves_existing_metadata_fields() {
        let original = fixture("v2Project");
        let project = done(run(original.clone()));
        for key in ["id", "name", "thumbnail", "createdAt", "updatedAt"] {
            assert_eq!(project["metadata"][key], original["metadata"][key]);
        }
    }

    #[test]
    fn preserves_settings_object() {
        let original = fixture("v2Project");
        assert_eq!(
            done(run(original.clone()))["settings"],
            original["settings"]
        );
    }

    #[test]
    fn preserves_scenes_array() {
        let original = fixture("v2Project");
        assert_eq!(done(run(original.clone()))["scenes"], original["scenes"]);
    }

    #[test]
    fn handles_project_without_metadata_object() {
        let project = done(run(
            json!({ "id": "no-metadata", "version": 2, "scenes": [] }),
        ));
        assert_eq!(project["metadata"]["duration"], 0);
    }

    #[test]
    fn calculates_duration_from_main_scene_only() {
        let project = json!({
            "id": "multi-scene", "version": 2, "metadata": { "id": "multi-scene", "name": "Multi" },
            "scenes": [
                { "id": "scene-1", "isMain": true, "tracks": [{ "type": "video", "elements": [{ "startTime": 0, "duration": 10 }] }] },
                { "id": "scene-2", "isMain": false, "tracks": [{ "type": "video", "elements": [{ "startTime": 0, "duration": 20 }] }] },
            ],
        });
        assert_eq!(done(run(project))["metadata"]["duration"], 10);
    }

    #[test]
    fn project_id_from_root_level() {
        assert_eq!(
            migrations::project_id(&json!({ "id": "root-id", "metadata": {} })),
            Some("root-id")
        );
    }

    #[test]
    fn project_id_from_metadata_when_root_id_missing() {
        assert_eq!(
            migrations::project_id(&fixture("v2Project")),
            Some("project-v2-123")
        );
    }

    #[test]
    fn prefers_root_id_over_metadata_id() {
        let project = json!({ "id": "root-id", "metadata": { "id": "metadata-id" } });
        assert_eq!(migrations::project_id(&project), Some("root-id"));
    }
}

// --- v3 → v4, v4 → v5 ------------------------------------------------------

fn with_tracks(base: &Value, version: Option<u32>, tracks: Value) -> Value {
    let mut project = base.clone();
    if let Some(version) = version {
        project["version"] = json!(version);
    }
    project["scenes"][0]["tracks"] = tracks;
    project["scenes"] = json!([project["scenes"][0].clone()]);
    project
}

fn sticker(extra: Value) -> Value {
    merge(
        &json!({
            "id": "sticker-1", "type": "sticker", "name": "Home", "duration": 5, "startTime": 0,
            "trimStart": 0, "trimEnd": 0,
            "transform": { "scale": 1, "position": { "x": 0, "y": 0 }, "rotate": 0 }, "opacity": 1,
        }),
        extra,
    )
}

fn sticker_track(element: Value) -> Value {
    json!([{ "id": "track-sticker", "type": "sticker", "name": "Sticker Track", "hidden": false, "elements": [element] }])
}

mod v3_to_v4 {
    use super::*;
    use migrations::v3_to_v4 as run;

    #[test]
    fn normalizes_legacy_text_font_weight_values() {
        let text = json!({
            "id": "text-1", "type": "text", "name": "Title", "content": "Hello", "duration": 5,
            "startTime": 0, "trimStart": 0, "trimEnd": 0, "fontSize": 64, "fontFamily": "Inter",
            "color": "#ffffff", "backgroundColor": "transparent", "textAlign": "center",
            "fontWeight": "bold", "fontStyle": "normal", "textDecoration": "none",
            "transform": { "scale": 1, "position": { "x": 0, "y": 0 }, "rotate": 0 }, "opacity": 1,
        });
        let tracks = json!([{ "id": "track-text", "type": "text", "name": "Text Track", "hidden": false, "elements": [text] }]);
        let project = done(run(with_tracks(&fixture("v3Project"), None, tracks)));
        assert_eq!(project["version"], 4);
        assert_eq!(first_element(&project)["fontWeight"], "700");
    }

    #[test]
    fn does_not_mutate_non_text_tracks() {
        let tracks = sticker_track(sticker(json!({ "name": "Flag", "iconName": "mdi:home" })));
        let project = done(run(with_tracks(&fixture("v3Project"), None, tracks)));
        assert_eq!(first_element(&project)["iconName"], "mdi:home");
    }

    #[test]
    fn skips_projects_that_are_already_v4() {
        skipped(
            run(merge(&fixture("v3Project"), json!({ "version": 4 }))),
            "already v4",
        );
    }

    #[test]
    fn skips_projects_with_no_id() {
        skipped(run(json!({ "version": 3, "scenes": [] })), "no project id");
    }
}

mod v4_to_v5 {
    use super::*;
    use migrations::v4_to_v5 as run;

    #[test]
    fn migrates_sticker_icon_name_to_sticker_id_and_removes_legacy_color() {
        let tracks = sticker_track(sticker(
            json!({ "iconName": "mdi:home", "color": "#ff0000" }),
        ));
        let project = done(run(with_tracks(&fixture("v3Project"), Some(4), tracks)));
        assert_eq!(project["version"], 5);
        let element = first_element(&project);
        assert_eq!(element["stickerId"], "icons:mdi:home");
        assert!(element.get("iconName").is_none());
        assert!(element.get("color").is_none());
    }

    #[test]
    fn keeps_provider_prefixed_sticker_id_values_unchanged() {
        let tracks = sticker_track(sticker(json!({ "name": "Flag", "stickerId": "flags:AD" })));
        let project = done(run(with_tracks(&fixture("v3Project"), Some(4), tracks)));
        assert_eq!(first_element(&project)["stickerId"], "flags:AD");
    }

    #[test]
    fn skips_projects_that_are_already_v5() {
        skipped(
            run(merge(&fixture("v3Project"), json!({ "version": 5 }))),
            "already v5",
        );
    }

    #[test]
    fn skips_projects_with_no_id() {
        skipped(run(json!({ "version": 4, "scenes": [] })), "no project id");
    }
}

// --- v5 → v6 ---------------------------------------------------------------

mod v5_to_v6 {
    use super::*;
    use migrations::v5_to_v6 as run;

    #[test]
    fn converts_number_bookmarks_to_bookmark_objects() {
        let project = done(run(fixture("v5Project")));
        assert_eq!(project["version"], 6);
        assert_eq!(
            normalize(project["scenes"][0]["bookmarks"].clone()),
            normalize(json!([{ "time": 2.0 }, { "time": 5.5 }, { "time": 12.0 }]))
        );
        assert_eq!(project["scenes"][1]["bookmarks"], json!([]));
    }

    #[test]
    fn skips_projects_that_are_already_v6() {
        let mut project = merge(&fixture("v5Project"), json!({ "version": 6 }));
        project["scenes"][0]["bookmarks"] = json!([{ "time": 2 }, { "time": 5 }]);
        skipped(run(project), "already v6");
    }

    #[test]
    fn skips_projects_with_no_id() {
        skipped(run(json!({ "version": 5, "scenes": [] })), "no project id");
    }

    #[test]
    fn preserves_existing_bookmark_objects_with_note_color_duration() {
        let bookmarks = json!([{ "time": 1, "note": "Intro", "color": "#ef4444" }, { "time": 5.5, "duration": 2 }]);
        let mut project = fixture("v5Project");
        project["scenes"][0]["bookmarks"] = bookmarks.clone();
        project["scenes"] = json!([project["scenes"][0].clone()]);
        let project = done(run(project));
        assert_eq!(project["scenes"][0]["bookmarks"], bookmarks);
    }
}

// --- v8 → v9 ---------------------------------------------------------------

mod v8_to_v9 {
    use super::*;
    use migrations::v8_to_v9 as run;

    fn v8_project(tracks: Value) -> Value {
        json!({
            "id": "project-v8-text", "version": 8,
            "metadata": { "id": "project-v8-text", "name": "V8 Project with Text",
                "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-01-01T00:00:00.000Z" },
            "settings": { "fps": 30, "canvasSize": { "width": 1920, "height": 1080 },
                "background": { "type": "color", "color": "#000000" } },
            "currentSceneId": "scene-main",
            "scenes": [{ "id": "scene-main", "name": "Main scene", "isMain": true, "tracks": tracks,
                "bookmarks": [], "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-01-01T00:00:00.000Z" }],
        })
    }

    fn text_track(elements: Value) -> Value {
        json!([{ "id": "track-text", "type": "text", "name": "Text Track", "hidden": false, "elements": elements }])
    }

    fn default_project() -> Value {
        v8_project(text_track(json!([
            { "id": "el-1", "type": "text", "content": "With color", "startTime": 0, "duration": 5,
              "background": { "color": "#ff0000", "cornerRadius": 0, "paddingX": 8, "paddingY": 4 } },
            { "id": "el-2", "type": "text", "content": "Transparent", "startTime": 5, "duration": 5,
              "background": { "color": "transparent", "paddingX": 30, "paddingY": 42 } },
        ])))
    }

    #[test]
    fn adds_background_enabled_from_color() {
        let project = done(run(default_project()));
        assert_eq!(project["version"], 9);
        let elements = &project["scenes"][0]["tracks"][0]["elements"];
        assert_eq!(elements[0]["background"]["enabled"], true);
        assert_eq!(elements[0]["background"]["color"], "#ff0000");
        assert_eq!(elements[1]["background"]["enabled"], false);
        assert_eq!(elements[1]["background"]["color"], "transparent");
    }

    #[test]
    fn preserves_existing_background_enabled_if_already_present() {
        let project = v8_project(text_track(json!([{
            "id": "el-1", "type": "text", "content": "Already has enabled", "startTime": 0, "duration": 5,
            "background": { "enabled": false, "color": "#00ff00" },
        }])));
        assert_eq!(
            first_element(&done(run(project)))["background"]["enabled"],
            false
        );
    }

    #[test]
    fn skips_non_text_elements_and_tracks() {
        let project = v8_project(
            json!([{ "id": "track-video", "type": "video", "name": "Video Track", "isMain": true, "elements": [] }]),
        );
        assert_eq!(done(run(project))["version"], 9);
    }

    #[test]
    fn skips_projects_that_are_already_v9() {
        skipped(
            run(merge(&default_project(), json!({ "version": 9 }))),
            "already v9",
        );
    }

    #[test]
    fn skips_projects_with_no_id() {
        skipped(run(json!({ "version": 8, "scenes": [] })), "no project id");
    }
}

// --- v15 → v16 … v19 → v20 -------------------------------------------------

fn v15_era_project(id: &str, version: u32, tracks: Value) -> Value {
    json!({
        "id": id, "version": version,
        "metadata": { "id": id, "name": "Project", "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-01-01T00:00:00.000Z" },
        "settings": { "fps": 30, "canvasSize": { "width": 1920, "height": 1080 }, "background": { "type": "color", "color": "#000000" } },
        "currentSceneId": "scene-main",
        "scenes": [{ "id": "scene-main", "name": "Main scene", "isMain": true, "tracks": tracks, "bookmarks": [],
            "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-01-01T00:00:00.000Z" }],
    })
}

mod v15_to_v16 {
    use super::*;
    use migrations::v15_to_v16 as run;

    #[test]
    fn renames_sticker_tracks_to_graphic_tracks() {
        let tracks = json!([{ "id": "track-graphic", "type": "sticker", "name": "Sticker Track", "hidden": false,
            "elements": [{ "id": "sticker-1", "type": "sticker", "name": "Logo", "stickerId": "icons:mdi:home",
                "duration": 5, "startTime": 0, "trimStart": 0, "trimEnd": 0,
                "transform": { "scaleX": 1, "scaleY": 1, "position": { "x": 0, "y": 0 }, "rotate": 0 }, "opacity": 1 }] }]);
        let project = done(run(v15_era_project("project-v15", 15, tracks)));
        assert_eq!(project["version"], 16);
        assert_eq!(project["scenes"][0]["tracks"][0]["type"], "graphic");
    }

    #[test]
    fn skips_projects_already_on_v16() {
        skipped(
            run(json!({ "id": "project-v16", "version": 16 })),
            "already v16",
        );
    }

    #[test]
    fn skips_projects_with_no_id() {
        skipped(run(json!({ "version": 15, "scenes": [] })), "no project id");
    }
}

mod v16_to_v17 {
    use super::*;
    use migrations::v16_to_v17 as run;

    #[test]
    fn adds_center_stroke_alignment_to_masks_that_do_not_have_it() {
        let params = json!({ "feather": 0, "inverted": false, "strokeColor": "#ffffff", "strokeWidth": 8,
            "centerX": 0, "centerY": 0, "width": 0.6, "height": 0.6, "rotation": 0, "scale": 1 });
        let tracks = json!([{ "id": "track-video", "type": "video", "name": "Video Track", "hidden": false,
            "elements": [{ "id": "video-1", "type": "video", "name": "Clip", "mediaId": "media-1",
                "duration": 5, "startTime": 0, "trimStart": 0, "trimEnd": 0,
                "transform": { "scaleX": 1, "scaleY": 1, "position": { "x": 0, "y": 0 }, "rotate": 0 }, "opacity": 1,
                "masks": [
                    { "id": "mask-1", "type": "rectangle", "params": params },
                    { "id": "mask-2", "type": "ellipse", "params": merge(&params, json!({ "strokeAlign": "outside" })) },
                ] }] }]);
        let project = done(run(v15_era_project("project-v16", 16, tracks)));
        assert_eq!(project["version"], 17);
        let masks = &first_element(&project)["masks"];
        assert_eq!(masks[0]["params"]["strokeAlign"], "center");
        assert_eq!(masks[1]["params"]["strokeAlign"], "outside");
    }

    #[test]
    fn skips_projects_already_on_v17() {
        skipped(
            run(json!({ "id": "project-v17", "version": 17 })),
            "already v17",
        );
    }

    #[test]
    fn skips_projects_with_no_id() {
        skipped(run(json!({ "version": 16, "scenes": [] })), "no project id");
    }
}

mod v18_to_v19 {
    use super::*;
    use migrations::v18_to_v19 as run;

    #[test]
    fn adds_canvas_size_mode_and_empty_remembered_custom_size_defaults() {
        let mut project = v15_era_project("project-v18-defaults", 18, json!([]));
        project["scenes"] = json!([]);
        project["settings"]["originalCanvasSize"] = json!({ "width": 1920, "height": 1080 });
        let project = done(run(project));
        assert_eq!(project["version"], 19);
        let settings = &project["settings"];
        assert_eq!(settings["canvasSizeMode"], "preset");
        assert_eq!(settings["lastCustomCanvasSize"], Value::Null);
        assert_eq!(
            settings["originalCanvasSize"],
            json!({ "width": 1920, "height": 1080 })
        );
    }

    #[test]
    fn skips_projects_already_on_v19() {
        skipped(
            run(json!({ "id": "project-v19", "version": 19 })),
            "already v19",
        );
    }

    #[test]
    fn skips_projects_with_no_id() {
        skipped(run(json!({ "version": 18, "scenes": [] })), "no project id");
    }
}

mod v19_to_v20 {
    use super::*;
    use migrations::v19_to_v20 as run;

    #[test]
    fn backfills_source_audio_enabled_state_on_video_elements() {
        let tracks = json!([
            { "id": "track-video", "type": "video", "name": "Video", "isMain": true, "muted": false, "hidden": false,
              "elements": [{ "id": "video-1", "type": "video", "name": "Clip", "mediaId": "media-1",
                "duration": 5, "startTime": 0, "trimStart": 0, "trimEnd": 0,
                "transform": { "position": { "x": 0, "y": 0 }, "scale": { "x": 1, "y": 1 }, "rotation": 0 }, "opacity": 1 }] },
            { "id": "track-audio", "type": "audio", "name": "Audio", "muted": false,
              "elements": [{ "id": "audio-1", "type": "audio", "sourceType": "upload", "mediaId": "media-audio-1",
                "name": "Audio", "duration": 5, "startTime": 0, "trimStart": 0, "trimEnd": 0, "volume": 0 }] },
        ]);
        let project = done(run(v15_era_project("project-v19-source-audio", 19, tracks)));
        assert_eq!(project["version"], 20);
        let tracks = &project["scenes"][0]["tracks"];
        assert_eq!(tracks[0]["elements"][0]["id"], "video-1");
        assert_eq!(tracks[0]["elements"][0]["isSourceAudioEnabled"], true);
        assert!(
            tracks[1]["elements"][0]
                .get("isSourceAudioEnabled")
                .is_none()
        );
    }

    #[test]
    fn preserves_existing_explicit_source_audio_state() {
        let project = json!({ "id": "project-v19-existing-state", "version": 19,
            "scenes": [{ "tracks": [{ "elements": [{ "type": "video", "isSourceAudioEnabled": false }] }] }] });
        assert_eq!(
            first_element(&done(run(project)))["isSourceAudioEnabled"],
            false
        );
    }

    #[test]
    fn skips_projects_already_on_v20() {
        skipped(
            run(json!({ "id": "project-v20", "version": 20 })),
            "already v20",
        );
    }
}

// --- v20 → v21 … v22 → v23 -------------------------------------------------

mod v20_to_v21 {
    use super::*;
    use migrations::v20_to_v21 as run;

    #[test]
    fn multiplies_blur_background_intensity_by_5() {
        let mut project = v15_era_project("project-v20-blur", 20, json!([]));
        project["scenes"] = json!([]);
        project["settings"]["background"] = json!({ "type": "blur", "blurIntensity": 100 });
        let project = done(run(project));
        assert_eq!(project["version"], 21);
        assert_eq!(project["settings"]["background"]["blurIntensity"], 500);
    }

    #[test]
    fn uses_default_blur_intensity_when_blur_type_but_intensity_missing() {
        let project = json!({ "id": "p", "version": 20, "settings": { "background": { "type": "blur" } }, "scenes": [] });
        assert_eq!(
            done(run(project))["settings"]["background"]["blurIntensity"],
            50
        );
    }

    #[test]
    fn leaves_color_background_unchanged() {
        let background = json!({ "type": "color", "color": "#000000" });
        let project = json!({ "id": "p", "version": 20, "settings": { "background": background }, "scenes": [] });
        assert_eq!(done(run(project))["settings"]["background"], background);
    }

    #[test]
    fn skips_projects_already_on_v21() {
        skipped(
            run(json!({ "id": "project-v21", "version": 21 })),
            "already v21",
        );
    }

    #[test]
    fn skips_projects_not_on_v20() {
        skipped(
            run(json!({ "id": "project-v19", "version": 19 })),
            "not v20",
        );
    }
}

mod v21_to_v22 {
    use super::*;
    use migrations::v21_to_v22 as run;

    #[test]
    fn migrates_legacy_animation_channels_to_bindings_and_component_channels() {
        let project = json!({ "id": "project-v21-animations", "version": 21, "scenes": [{ "id": "scene-1",
            "tracks": [{ "id": "track-1", "elements": [{ "id": "element-1", "type": "text", "animations": { "channels": {
                "opacity": { "valueKind": "number", "keyframes": [{ "id": "opacity-1", "time": 1, "value": 0.5, "interpolation": "linear" }] },
                "transform.position": { "valueKind": "vector", "keyframes": [{ "id": "position-1", "time": 2, "value": { "x": 10, "y": 20 }, "interpolation": "hold" }] },
                "color": { "valueKind": "color", "keyframes": [{ "id": "color-1", "time": 3, "value": "#ff0000", "interpolation": "linear" }] },
                "effects.effect-1.params.enabled": { "valueKind": "discrete", "keyframes": [{ "id": "enabled-1", "time": 4, "value": true, "interpolation": "hold" }] },
            } } }] }] }] });
        let project = done(run(project));
        assert_eq!(project["version"], 22);
        let animations = &first_element(&project)["animations"];
        let (bindings, channels) = (&animations["bindings"], &animations["channels"]);

        assert_eq!(
            bindings["opacity"],
            json!({ "path": "opacity", "kind": "number",
            "components": [{ "key": "value", "channelId": "opacity:value" }] })
        );
        assert_eq!(
            bindings["transform.position"],
            json!({ "path": "transform.position", "kind": "vector2",
            "components": [{ "key": "x", "channelId": "transform.position:x" }, { "key": "y", "channelId": "transform.position:y" }] })
        );
        assert_eq!(
            bindings["color"],
            json!({ "path": "color", "kind": "color", "colorSpace": "srgb-linear",
            "components": [{ "key": "r", "channelId": "color:r" }, { "key": "g", "channelId": "color:g" },
                { "key": "b", "channelId": "color:b" }, { "key": "a", "channelId": "color:a" }] })
        );
        assert_eq!(
            bindings["effects.effect-1.params.enabled"],
            json!({ "path": "effects.effect-1.params.enabled",
            "kind": "discrete", "components": [{ "key": "value", "channelId": "effects.effect-1.params.enabled:value" }] })
        );

        let scalar = |id: &str, time: u32, value: Value, segment: &str| {
            json!({ "kind": "scalar", "keys": [{
            "id": id, "time": time, "value": value, "segmentToNext": segment, "tangentMode": "flat" }] })
        };
        assert_eq!(
            channels["opacity:value"],
            scalar("opacity-1", 1, json!(0.5), "linear")
        );
        assert_eq!(
            channels["transform.position:x"],
            scalar("position-1", 2, json!(10), "step")
        );
        assert_eq!(
            channels["transform.position:y"],
            scalar("position-1", 2, json!(20), "step")
        );
        assert_eq!(
            channels["color:r"],
            scalar("color-1", 3, json!(1), "linear")
        );
        assert_eq!(
            channels["color:g"],
            scalar("color-1", 3, json!(0), "linear")
        );
        assert_eq!(
            channels["color:b"],
            scalar("color-1", 3, json!(0), "linear")
        );
        assert_eq!(
            channels["color:a"],
            scalar("color-1", 3, json!(1), "linear")
        );
        assert_eq!(
            channels["effects.effect-1.params.enabled:value"],
            json!({ "kind": "discrete", "keys": [{ "id": "enabled-1", "time": 4, "value": true }] })
        );
    }

    #[test]
    fn skips_projects_already_on_v22() {
        skipped(
            run(json!({ "id": "project-v22", "version": 22 })),
            "already v22",
        );
    }

    #[test]
    fn skips_projects_not_on_v21() {
        skipped(
            run(json!({ "id": "project-v20", "version": 20 })),
            "not v21",
        );
    }
}

mod v22_to_v23 {
    use super::*;
    use migrations::v22_to_v23 as run;

    #[test]
    fn converts_seconds_to_ticks_and_fps_to_a_frame_rate_object() {
        let project = json!({
            "id": "project-v22-time", "version": 22,
            "metadata": { "id": "project-v22-time", "name": "Project", "duration": 15.5,
                "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z" },
            "settings": { "fps": 29.97, "canvasSize": { "width": 1920, "height": 1080 }, "background": { "type": "color", "color": "#000000" } },
            "timelineViewState": { "zoomLevel": 1, "scrollLeft": 120, "playheadTime": 1.25 },
            "scenes": [{ "id": "scene-1",
                "bookmarks": [{ "time": 2.5, "duration": 0.75, "note": "Marker", "color": "#ff0000" }, { "time": 4.5 }],
                "tracks": [{ "id": "track-1", "type": "video", "elements": [{ "id": "element-1", "type": "video",
                    "startTime": 1.25, "duration": 5.5, "trimStart": 0.25, "trimEnd": 0.5, "sourceDuration": 6.25,
                    "animations": {
                        "bindings": { "opacity": { "path": "opacity", "kind": "number", "components": [{ "key": "value", "channelId": "opacity:value" }] } },
                        "channels": { "opacity:value": { "kind": "scalar", "keys": [
                            { "id": "key-1", "time": 0.5, "value": 1, "segmentToNext": "bezier", "tangentMode": "flat", "rightHandle": { "dt": 0.25, "dv": 0.2 } },
                            { "id": "key-2", "time": 1.0, "value": 0.4, "segmentToNext": "linear", "tangentMode": "flat", "leftHandle": { "dt": -0.125, "dv": -0.1 } },
                        ] } },
                    } }] }],
                "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z" }],
        });
        let project = done(run(project));
        assert_eq!(project["version"], 23);
        assert_eq!(project["metadata"]["duration"], 1_860_000);
        assert_eq!(
            project["settings"]["fps"],
            json!({ "numerator": 30_000, "denominator": 1_001 })
        );
        assert_eq!(project["timelineViewState"]["playheadTime"], 150_000);
        assert_eq!(project["timelineViewState"]["scrollLeft"], 120);
        assert_eq!(
            project["scenes"][0]["bookmarks"],
            json!([
            { "time": 300_000, "duration": 90_000, "note": "Marker", "color": "#ff0000" }, { "time": 540_000 }])
        );
        let element = first_element(&project);
        assert_eq!(element["startTime"], 150_000);
        assert_eq!(element["duration"], 660_000);
        assert_eq!(element["trimStart"], 30_000);
        assert_eq!(element["trimEnd"], 60_000);
        assert_eq!(element["sourceDuration"], 750_000);
        assert_eq!(
            element["animations"]["channels"]["opacity:value"],
            json!({ "kind": "scalar", "keys": [
            { "id": "key-1", "time": 60_000, "value": 1, "segmentToNext": "bezier", "tangentMode": "flat", "rightHandle": { "dt": 30_000, "dv": 0.2 } },
            { "id": "key-2", "time": 120_000, "value": 0.4, "segmentToNext": "linear", "tangentMode": "flat", "leftHandle": { "dt": -15_000, "dv": -0.1 } },
        ] })
        );
    }

    #[test]
    fn skips_projects_already_on_v23() {
        skipped(
            run(json!({ "id": "project-v23", "version": 23 })),
            "already v23",
        );
    }

    #[test]
    fn skips_projects_not_on_v22() {
        skipped(
            run(json!({ "id": "project-v21", "version": 21 })),
            "not v22",
        );
    }
}

// --- v26 → v27 … v30 → v31 -------------------------------------------------

fn grouped_project(id: &str, version: u32, main_elements: Value) -> Value {
    json!({ "id": id, "version": version, "scenes": [{ "id": "scene-1",
        "tracks": { "main": { "id": "main", "type": "video", "elements": main_elements }, "overlay": [], "audio": [] } }] })
}

fn main_element(project: &Value) -> &Value {
    &project["scenes"][0]["tracks"]["main"]["elements"][0]
}

mod v26_to_v27 {
    use super::*;

    #[test]
    fn converts_custom_mask_paths_from_json_strings_to_typed_point_arrays() {
        let point =
            json!({ "id": "point-1", "x": 0, "y": 0, "inX": 0, "inY": 0, "outX": 0.1, "outY": 0 });
        let project = grouped_project(
            "project-v26-custom-mask",
            26,
            json!([{ "id": "element-1", "type": "image", "masks": [
            { "id": "mask-custom", "type": "custom", "params": { "path": json!([point]).to_string(), "closed": false } },
            { "id": "mask-rectangle", "type": "rectangle", "params": { "width": 0.5 } },
        ] }]),
        );
        let project = done(migrations::v26_to_v27(project));
        assert_eq!(project["version"], 27);
        let masks = &main_element(&project)["masks"];
        assert_eq!(masks[0]["params"]["path"], json!([point]));
        assert_eq!(masks[1]["params"]["width"], 0.5);
    }
}

mod v27_to_v28 {
    use super::*;
    use migrations::v27_to_v28 as run;

    #[test]
    fn rounds_persisted_media_time_floats_back_to_integer_ticks() {
        let project = json!({
            "id": "project-v27-float-time", "version": 27,
            "metadata": { "id": "project-v27-float-time", "name": "Project", "duration": 2_152_466.367_713_004_4,
                "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z" },
            "settings": { "fps": { "numerator": 30, "denominator": 1 }, "canvasSize": { "width": 1920, "height": 1080 },
                "background": { "type": "color", "color": "#000000" } },
            "timelineViewState": { "zoomLevel": 1.25, "scrollLeft": 120, "playheadTime": 301_234.8 },
            "scenes": [{ "id": "scene-1", "name": "Scene 1", "isMain": true,
                "bookmarks": [{ "time": 120_000.4, "duration": 60_000.6, "note": "Marker", "color": "#ff0000" }],
                "tracks": { "main": { "id": "main-track", "type": "video", "name": "Main", "muted": false, "hidden": false,
                    "elements": [{ "id": "element-1", "type": "video", "name": "Clip", "startTime": 300_000.49,
                        "duration": 2_152_466.367_713_004_4, "trimStart": 30_000.2, "trimEnd": 15_000.7,
                        "sourceDuration": 2_197_467.6, "mediaId": "media-1",
                        "transform": { "scaleX": 1, "scaleY": 1, "position": { "x": 0, "y": 0 }, "rotate": 0 }, "opacity": 1,
                        "animations": { "channels": { "opacity": { "kind": "scalar", "keys": [{ "id": "key-1", "time": 1_000.6,
                            "value": 1, "segmentToNext": "bezier", "tangentMode": "flat",
                            "leftHandle": { "dt": -120.6, "dv": 0 }, "rightHandle": { "dt": 60.4, "dv": 0 } }] } } } }] },
                    "overlay": [], "audio": [] },
                "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z" }],
        });
        let project = done(run(project));
        assert_eq!(project["version"], 28);
        assert_eq!(project["metadata"]["duration"], 2_152_466);
        let view = &project["timelineViewState"];
        assert_eq!(view["playheadTime"], 301_235);
        assert_eq!(view["zoomLevel"], 1.25);
        assert_eq!(view["scrollLeft"], 120);
        assert_eq!(
            project["scenes"][0]["bookmarks"],
            json!([{ "time": 120_000, "duration": 60_001, "note": "Marker", "color": "#ff0000" }])
        );
        let element = main_element(&project);
        assert_eq!(element["startTime"], 300_000);
        assert_eq!(element["duration"], 2_152_466);
        assert_eq!(element["trimStart"], 30_000);
        assert_eq!(element["trimEnd"], 15_001);
        assert_eq!(element["sourceDuration"], 2_197_468);
        assert_eq!(
            element["animations"]["channels"]["opacity"]["keys"],
            json!([{ "id": "key-1", "time": 1_001,
            "value": 1, "segmentToNext": "bezier", "tangentMode": "flat",
            "leftHandle": { "dt": -121, "dv": 0 }, "rightHandle": { "dt": 60, "dv": 0 } }])
        );
    }

    #[test]
    fn keeps_already_integer_media_time_values_unchanged() {
        let element = json!({ "id": "element-1", "type": "video", "name": "Clip", "startTime": 10_000, "duration": 20_000,
            "trimStart": 1_000, "trimEnd": 2_000, "sourceDuration": 23_000, "mediaId": "media-1",
            "transform": { "scaleX": 1, "scaleY": 1, "position": { "x": 0, "y": 0 }, "rotate": 0 }, "opacity": 1 });
        let view = json!({ "zoomLevel": 2, "scrollLeft": 300, "playheadTime": 30_000 });
        let mut project = grouped_project("project-v27-integer-time", 27, json!([element]));
        project["metadata"] = json!({ "id": "project-v27-integer-time", "name": "Project", "duration": 120_000,
            "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z" });
        project["timelineViewState"] = view.clone();
        project["scenes"][0]["bookmarks"] = json!([{ "time": 60_000, "duration": 15_000 }]);
        let project = done(run(project));
        assert_eq!(project["version"], 28);
        assert_eq!(project["metadata"]["duration"], 120_000);
        assert_eq!(project["timelineViewState"], view);
        assert_eq!(
            project["scenes"][0]["bookmarks"],
            json!([{ "time": 60_000, "duration": 15_000 }])
        );
        assert_eq!(*main_element(&project), element);
    }

    #[test]
    fn skips_projects_already_on_v28() {
        skipped(
            run(json!({ "id": "project-v28", "version": 28 })),
            "already v28",
        );
    }

    #[test]
    fn skips_projects_not_on_v27() {
        skipped(
            run(json!({ "id": "project-v26", "version": 26 })),
            "not v27",
        );
    }
}

mod v28_to_v29 {
    use super::*;

    #[test]
    fn copies_built_in_element_fields_into_params_without_deleting_source_fields() {
        let mut project = grouped_project(
            "project-v28-builtins",
            28,
            json!([{
                "id": "video-1", "type": "video", "name": "Video",
                "transform": { "position": { "x": 12, "y": -4 }, "scaleX": 1.25, "scaleY": 0.75, "rotate": 9 },
                "opacity": 0.5, "blendMode": "multiply", "volume": -6, "muted": true,
            }]),
        );
        project["scenes"][0]["tracks"]["overlay"] = json!([{ "id": "text-track", "type": "text", "elements": [{
            "id": "text-1", "type": "text", "name": "Text", "content": "Hello", "fontSize": 20, "fontFamily": "Inter",
            "color": "#eeeeee", "textAlign": "left", "fontWeight": "bold", "fontStyle": "italic",
            "textDecoration": "underline", "letterSpacing": 1.5, "lineHeight": 1.4,
            "background": { "enabled": true, "color": "#111111", "paddingX": 10, "paddingY": 12 },
            "transform": { "position": { "x": 1, "y": 2 }, "scaleX": 1, "scaleY": 1, "rotate": 0 }, "opacity": 1,
        }] }]);
        let project = done(migrations::v28_to_v29(project));
        assert_eq!(project["version"], 29);
        let video = main_element(&project);
        assert_eq!(
            video["transform"],
            json!({ "position": { "x": 12, "y": -4 }, "scaleX": 1.25, "scaleY": 0.75, "rotate": 9 })
        );
        assert_eq!(video["opacity"], 0.5);
        assert_eq!(video["volume"], -6);
        assert_eq!(video["muted"], true);
        assert_eq!(
            video["params"],
            json!({ "transform.positionX": 12, "transform.positionY": -4,
            "transform.scaleX": 1.25, "transform.scaleY": 0.75, "transform.rotate": 9,
            "opacity": 0.5, "blendMode": "multiply", "volume": -6, "muted": true })
        );

        let text = &project["scenes"][0]["tracks"]["overlay"][0]["elements"][0];
        assert_eq!(text["content"], "Hello");
        assert_eq!(
            text["background"],
            json!({ "enabled": true, "color": "#111111", "paddingX": 10, "paddingY": 12 })
        );
        let expected = json!({ "content": "Hello", "fontSize": 20, "fontFamily": "Inter", "color": "#eeeeee",
            "textAlign": "left", "fontWeight": "bold", "fontStyle": "italic", "textDecoration": "underline",
            "letterSpacing": 1.5, "lineHeight": 1.4, "background.enabled": true, "background.color": "#111111",
            "background.paddingX": 10, "background.paddingY": 12, "transform.positionX": 1, "transform.positionY": 2 });
        for (key, value) in expected.as_object().unwrap() {
            assert_eq!(&text["params"][key], value, "{key}");
        }
    }
}

mod v29_to_v30 {
    use super::*;
    use migrations::v29_to_v30 as run;

    #[test]
    fn flattens_animation_bindings_and_channels_by_path() {
        let project = grouped_project(
            "project-v29-animations",
            29,
            json!([{ "id": "text-1", "type": "text", "animations": {
            "bindings": {
                "opacity": { "path": "opacity", "kind": "number", "components": [{ "key": "value", "channelId": "opacity:value" }] },
                "color": { "path": "color", "kind": "color", "colorSpace": "srgb-linear", "components": [
                    { "key": "r", "channelId": "color:r" }, { "key": "g", "channelId": "color:g" },
                    { "key": "b", "channelId": "color:b" }, { "key": "a", "channelId": "color:a" }] },
                "params.label": { "path": "params.label", "kind": "discrete", "components": [{ "key": "value", "channelId": "params.label:value" }] },
            },
            "channels": {
                "opacity:value": { "kind": "scalar", "keys": [{ "id": "o1", "time": 0, "value": 0.5 }] },
                "color:r": { "kind": "scalar", "keys": [{ "id": "c1", "time": 0, "value": 1 }] },
                "color:g": { "kind": "scalar", "keys": [{ "id": "c1", "time": 0, "value": 0.5 }] },
                "color:b": { "kind": "scalar", "keys": [{ "id": "c1", "time": 0, "value": 0 }] },
                "color:a": { "kind": "scalar", "keys": [{ "id": "c1", "time": 0, "value": 1 }] },
                "params.label:value": { "kind": "discrete", "keys": [{ "id": "d1", "time": 0, "value": "Title" }] },
            },
        } }]),
        );
        let project = done(run(project));
        assert_eq!(project["version"], 30);
        let animations = &main_element(&project)["animations"];
        assert!(animations["bindings"].is_object());
        assert!(animations["channels"].is_object());
        assert_eq!(
            animations["opacity"],
            json!({ "keys": [{ "id": "o1", "time": 0, "value": 0.5 }] })
        );
        assert_eq!(
            animations["color"],
            json!({
                "r": { "keys": [{ "id": "c1", "time": 0, "value": 1 }] },
                "g": { "keys": [{ "id": "c1", "time": 0, "value": 0.5 }] },
                "b": { "keys": [{ "id": "c1", "time": 0, "value": 0 }] },
                "a": { "keys": [{ "id": "c1", "time": 0, "value": 1 }] },
            })
        );
        assert_eq!(
            animations["params.label"],
            json!({ "keys": [{ "id": "d1", "time": 0, "value": "Title" }] })
        );
    }

    #[test]
    fn leaves_elements_without_animations_unchanged() {
        let project = json!({ "id": "project-v29-no-animations", "version": 29,
            "scenes": [{ "tracks": { "main": { "elements": [{ "id": "image-1", "type": "image" }] } } }] });
        assert_eq!(
            *main_element(&done(run(project))),
            json!({ "id": "image-1", "type": "image" })
        );
    }
}

mod v30_to_v31 {
    use super::*;
    use migrations::v30_to_v31 as run;

    #[test]
    fn renames_custom_masks_to_freeform_without_changing_params() {
        let params = json!({ "feather": 0, "inverted": false, "strokeColor": "#ffffff", "strokeWidth": 0,
            "strokeAlign": "center", "path": [{ "id": "p1", "x": 0, "y": 0, "inX": 0, "inY": 0, "outX": 0, "outY": 0 }],
            "closed": true, "centerX": 0, "centerY": 0, "rotation": 0, "scale": 1 });
        let project = grouped_project(
            "project-v30-freeform",
            30,
            json!([{ "id": "image-1", "type": "image", "masks": [{ "id": "mask-1", "type": "custom", "params": params }] }]),
        );
        let project = done(run(project));
        assert_eq!(project["version"], 31);
        let mask = &main_element(&project)["masks"][0];
        assert_eq!(
            *mask,
            json!({ "id": "mask-1", "type": "freeform", "params": params })
        );
        assert!(mask.get("legacyType").is_none());
    }

    #[test]
    fn leaves_a_freeform_mask_in_a_v30_project_unchanged() {
        let project = grouped_project(
            "p",
            30,
            json!([{ "id": "elem-1", "masks": [{ "id": "m1", "type": "freeform", "params": {} }] }]),
        );
        let project = done(run(project));
        assert_eq!(main_element(&project)["masks"][0]["type"], "freeform");
    }

    #[test]
    fn leaves_elements_without_a_masks_array_unchanged() {
        let project = grouped_project("p", 30, json!([{ "id": "elem-1", "type": "video" }]));
        assert_eq!(
            *main_element(&done(run(project))),
            json!({ "id": "elem-1", "type": "video" })
        );
    }

    #[test]
    fn skips_a_project_that_is_already_v31() {
        let project = json!({ "id": "p1", "version": 31, "scenes": [] });
        let result = run(project.clone());
        assert_eq!(result.project, project);
        skipped(result, "already v31");
    }

    #[test]
    fn skips_a_project_that_is_not_v30() {
        let project = json!({ "id": "p1", "version": 29, "scenes": [] });
        let result = run(project.clone());
        assert_eq!(result.project, project);
        skipped(result, "not v30");
    }
}

// --- the whole chain -------------------------------------------------------

fn migrate_and_load(project: Value, legacy: &LegacyData) -> Project {
    let migrated = migrations::migrate(project, legacy, fixed_date()).expect("migrates");
    let project: Project = serde_json::from_value(migrated).expect("deserializes as v31");
    project.validate().expect("valid");
    project
}

#[test]
fn every_classic_fixture_migrates_to_v31() {
    for name in [
        "v0Project",
        "v0ProjectEmpty",
        "v0ProjectWithMetadata",
        "v1Project",
        "v1ProjectWithMultipleScenes",
        "v2Project",
        "v2ProjectWithBlurBackground",
        "v2ProjectSceneWithoutTracks",
        "v3Project",
        "v5Project",
    ] {
        let migrated = migrations::migrate(fixture(name), &LegacyData::default(), fixed_date())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(migrated["version"], 31, "{name}");
    }
}

#[test]
fn fixtures_with_complete_elements_load_as_valid_projects() {
    // The other fixtures' elements are partial (no `name`, trims), which
    // classic's own data never was; the model requires those fields.
    for name in [
        "v0Project",
        "v0ProjectEmpty",
        "v0ProjectWithMetadata",
        "v1Project",
        "v1ProjectWithMultipleScenes",
        "v5Project",
    ] {
        let project = migrate_and_load(fixture(name), &LegacyData::default());
        assert_eq!(project.version, 31, "{name}");
    }
}

#[test]
fn v2_fixture_keeps_its_timeline_through_the_chain() {
    let mut original = fixture("v2Project");
    for track in original["scenes"][0]["tracks"].as_array_mut().unwrap() {
        for element in track["elements"].as_array_mut().unwrap() {
            element["name"] = json!("Clip");
            element["trimStart"] = json!(0);
            element["trimEnd"] = json!(0);
        }
    }
    let project = migrate_and_load(original, &LegacyData::default());
    let scene = project.current_scene().unwrap();
    assert_eq!(project.metadata.duration.as_ticks(), 1_860_000);
    assert_eq!(scene.tracks.main.elements.len(), 1);
    assert_eq!(scene.tracks.overlay.len(), 1);
    assert_eq!(project.settings.fps.numerator, 30);
}

#[test]
fn v0_project_reads_its_legacy_timeline() {
    let legacy = LegacyData {
        timelines: [(
            "video-editor-timelines-project-v0-123".to_owned(),
            json!({ "tracks": [{ "id": "t1", "type": "media", "name": "Media", "elements": [
                { "id": "e1", "name": "Clip", "type": "media", "mediaId": "m1", "duration": 2, "startTime": 0, "trimStart": 0, "trimEnd": 0 },
                { "id": "e2", "name": "Photo", "type": "media", "mediaId": "m2", "duration": 1, "startTime": 2, "trimStart": 0, "trimEnd": 0 },
            ] }] }),
        )]
        .into(),
        media_types: [("m1".to_owned(), "video".to_owned()), ("m2".to_owned(), "image".to_owned())].into(),
    };
    let project = migrate_and_load(fixture("v0Project"), &legacy);
    let main = &project.current_scene().unwrap().tracks.main;
    assert_eq!(main.elements.len(), 2);
    assert_eq!(project.metadata.duration.as_ticks(), 360_000);
}

#[test]
fn chain_rejects_records_it_cannot_migrate() {
    let legacy = LegacyData::default();
    let error = migrations::migrate(fixture("projectWithNoId"), &legacy, fixed_date()).unwrap_err();
    assert_eq!(error.reason, "no project id");
    let error = migrations::migrate(json!({ "id": "x", "version": 32 }), &legacy, fixed_date())
        .unwrap_err();
    assert_eq!(error.version, 32.0);
    // v1 records already shaped like v2 are skipped by classic, so they stop at v1.
    let error = migrations::migrate(
        merge(&fixture("v2Project"), json!({ "version": 1 })),
        &legacy,
        fixed_date(),
    );
    assert!(error.is_err());
}
