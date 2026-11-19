//! The classic app's storage migrations, v0 → [`PROJECT_VERSION`].
//!
//! A faithful port of `apps/web/src/services/storage/migrations/` (tag
//! `pre-rewrite`): each step works on the raw JSON record, as classic did,
//! because old records don't match today's types. [`migrate`] runs the chain
//! like classic's runner; the result deserializes into [`Project`](crate::Project).
//!
//! The classic code only ever adds fields (see its `AGENTS.md`), except
//! where a step says otherwise; keep it that way when porting changes.
//!
//! JS semantics kept on purpose: `typeof x === "number"` is [`Value::is_number`],
//! `isRecord` is a JSON object, and computed numbers are written the way
//! `JSON.stringify` would (whole numbers without a fraction).

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use crate::{PROJECT_VERSION, new_id};

type Object = Map<String, Value>;

/// What one step did: the (possibly) migrated record, or why it was left as is.
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationResult {
    pub project: Value,
    /// Classic's `reason` when the step skipped the record.
    pub skipped: Option<String>,
}

impl MigrationResult {
    fn done(project: Value) -> Self {
        Self {
            project,
            skipped: None,
        }
    }

    fn skip(project: Value, reason: impl Into<String>) -> Self {
        Self {
            project,
            skipped: Some(reason.into()),
        }
    }
}

/// Why [`migrate`] couldn't bring a record to [`PROJECT_VERSION`].
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationError {
    /// The storage version the record was at when migrating stopped.
    pub version: f64,
    pub reason: String,
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "can't migrate a storage v{} project: {}",
            self.version, self.reason
        )
    }
}

impl std::error::Error for MigrationError {}

/// Data classic kept outside the project record that the v1 → v2 step reads.
#[derive(Debug, Clone, Default)]
pub struct LegacyData {
    /// The `"timeline"` record of each pre-v2 timeline database, by database
    /// name (`video-editor-timelines-<project id>[-<scene id>]`).
    pub timelines: HashMap<String, Value>,
    /// Each media asset's stored `type` (`image`, `video`, `audio`), by id.
    pub media_types: HashMap<String, String>,
}

/// What [`v1_to_v2`] reads besides the record (classic `V1ToV2Context`).
#[derive(Debug, Clone, Default)]
pub struct V1ToV2Context {
    pub legacy_tracks_by_scene_id: HashMap<String, Vec<Value>>,
    pub media_types_by_id: HashMap<String, String>,
}

/// Migrates a classic project record to [`PROJECT_VERSION`], like classic's
/// `runStorageMigrations` does for one project. `now` stamps the fields the
/// early steps create.
pub fn migrate(
    mut project: Value,
    legacy: &LegacyData,
    now: DateTime<Utc>,
) -> Result<Value, MigrationError> {
    let fail = |version: f64, reason: &str| MigrationError {
        version,
        reason: reason.to_owned(),
    };
    if !project.is_object() {
        return Err(fail(0.0, "the project record isn't an object"));
    }
    if project_id(&project).is_none() {
        return Err(fail(0.0, "no project id"));
    }

    let target = f64::from(PROJECT_VERSION);
    let mut version = project_version(&project);
    while version < target {
        if version < 0.0 || version.fract() != 0.0 {
            return Err(fail(version, "no migration starts at this version"));
        }
        let from = version;
        let result = match from as u32 {
            0 => v0_to_v1(project, now),
            1 => {
                let context = v1_to_v2_context(&project, legacy);
                v1_to_v2(project, &context, now)
            }
            2 => v2_to_v3(project),
            3 => v3_to_v4(project),
            4 => v4_to_v5(project),
            5 => v5_to_v6(project),
            6 => v6_to_v7(project),
            7 => v7_to_v8(project),
            8 => v8_to_v9(project),
            9 => v9_to_v10(project),
            10 => v10_to_v11(project),
            11 => v11_to_v12(project),
            12 => v12_to_v13(project),
            13 => v13_to_v14(project),
            14 => v14_to_v15(project),
            15 => v15_to_v16(project),
            16 => v16_to_v17(project),
            17 => v17_to_v18(project),
            18 => v18_to_v19(project),
            19 => v19_to_v20(project),
            20 => v20_to_v21(project),
            21 => v21_to_v22(project),
            22 => v22_to_v23(project),
            23 => v23_to_v24(project),
            24 => v24_to_v25(project),
            25 => v25_to_v26(project),
            26 => v26_to_v27(project),
            27 => v27_to_v28(project),
            28 => v28_to_v29(project),
            29 => v29_to_v30(project),
            30 => v30_to_v31(project),
            _ => return Err(fail(from, "no migration starts at this version")),
        };
        if let Some(reason) = result.skipped {
            return Err(fail(from, &reason));
        }
        project = result.project;
        version = from + 1.0;
    }
    if version > target {
        return Err(fail(version, "made by a newer version of OpenCut"));
    }
    Ok(project)
}

/// Classic `getProjectId`: the root `id`, else `metadata.id`.
pub fn project_id(project: &Value) -> Option<&str> {
    non_empty_str(project.get("id")).or_else(|| non_empty_str(project.get("metadata")?.get("id")))
}

fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value?.as_str().filter(|text| !text.is_empty())
}

/// Classic runner's `getProjectVersion`: `version` from v2 on, v1 when it
/// has scenes, else v0.
pub fn project_version(project: &Value) -> f64 {
    if let Some(version) = project.get("version").and_then(Value::as_f64) {
        return version;
    }
    match project.get("scenes") {
        Some(Value::Array(scenes)) if !scenes.is_empty() => 1.0,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// A number as `JSON.stringify` writes it: whole numbers without a fraction,
/// `NaN`/`Infinity` as `null`.
fn num(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    value?.as_f64()
}

fn string(value: Option<&Value>) -> Option<&str> {
    value?.as_str()
}

fn is(value: Option<&Value>, expected: &str) -> bool {
    string(value) == Some(expected)
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The objects in `value` when it is an array.
fn objects(value: Option<&mut Value>) -> Vec<&mut Object> {
    match value {
        Some(Value::Array(items)) => items.iter_mut().filter_map(Value::as_object_mut).collect(),
        _ => Vec::new(),
    }
}

fn scenes(project: &mut Value) -> Vec<&mut Object> {
    objects(project.get_mut("scenes"))
}

/// Tracks while scenes held a flat `tracks` array (up to v23).
fn flat_tracks(project: &mut Value) -> Vec<&mut Object> {
    scenes(project)
        .into_iter()
        .flat_map(|scene| objects(scene.get_mut("tracks")))
        .collect()
}

/// Tracks once scenes held `{ main, overlay, audio }` (v24 on).
fn grouped_tracks(project: &mut Value) -> Vec<&mut Object> {
    let mut tracks = Vec::new();
    for scene in scenes(project) {
        let Some(Value::Object(groups)) = scene.get_mut("tracks") else {
            continue;
        };
        for (key, value) in groups.iter_mut() {
            match key.as_str() {
                "main" => tracks.extend(value.as_object_mut()),
                "overlay" | "audio" => tracks.extend(objects(Some(value))),
                _ => {}
            }
        }
    }
    tracks
}

fn elements(track: &mut Object) -> Vec<&mut Object> {
    objects(track.get_mut("elements"))
}

fn flat_elements(project: &mut Value) -> Vec<&mut Object> {
    flat_tracks(project)
        .into_iter()
        .flat_map(elements)
        .collect()
}

fn grouped_elements(project: &mut Value) -> Vec<&mut Object> {
    grouped_tracks(project)
        .into_iter()
        .flat_map(elements)
        .collect()
}

fn masks(element: &mut Object) -> Vec<&mut Object> {
    objects(element.get_mut("masks"))
}

fn set_version(mut project: Value, version: u32) -> MigrationResult {
    project["version"] = json!(version);
    MigrationResult::done(project)
}

/// The checks of steps v2–v20: an id, and not already at `to`.
fn loose_skip(project: &Value, to: u32) -> Option<String> {
    if project_id(project).is_none() {
        return Some("no project id".to_owned());
    }
    match number(project.get("version")) {
        Some(version) if version >= f64::from(to) => Some(format!("already v{to}")),
        _ => None,
    }
}

/// The checks of steps v20 on: an id, and exactly at `to - 1`.
fn strict_skip(project: &Value, to: u32) -> Option<String> {
    if project_id(project).is_none() {
        return Some("no project id".to_owned());
    }
    let Some(version) = number(project.get("version")) else {
        return Some("invalid version".to_owned());
    };
    if version >= f64::from(to) {
        return Some(format!("already v{to}"));
    }
    if version != f64::from(to - 1) {
        return Some(format!("not v{}", to - 1));
    }
    None
}

macro_rules! skip_unless {
    ($check:ident, $project:ident, $to:expr) => {
        if let Some(reason) = $check(&$project, $to) {
            return MigrationResult::skip($project, reason);
        }
    };
}

// ---------------------------------------------------------------------------
// v0 → v1: a main scene
// ---------------------------------------------------------------------------

pub fn v0_to_v1(mut project: Value, now: DateTime<Utc>) -> MigrationResult {
    if matches!(project.get("scenes"), Some(Value::Array(scenes)) if !scenes.is_empty()) {
        return MigrationResult::skip(project, "already has scenes");
    }
    let now = iso(now);
    let scene_id = new_id();
    project["scenes"] = json!([{
        "id": scene_id,
        "name": "Main scene",
        "isMain": true,
        "tracks": [],
        "bookmarks": [],
        "createdAt": now,
        "updatedAt": now,
    }]);
    project["currentSceneId"] = json!(scene_id);
    project["version"] = json!(1);
    match project.get_mut("metadata") {
        Some(Value::Object(metadata)) => {
            metadata.insert("updatedAt".to_owned(), json!(now));
        }
        _ => project["updatedAt"] = json!(now),
    }
    MigrationResult::done(project)
}

// ---------------------------------------------------------------------------
// v1 → v2: metadata/settings objects, timelines folded into scenes
// ---------------------------------------------------------------------------

const V2_DEFAULT_BLUR_INTENSITY: f64 = 10.0;
const V2_DEFAULT_BACKGROUND_COLOR: &str = "#000000";
const V2_DEFAULT_FPS: f64 = 30.0;

/// Classic `loadV1ToV2Context`: each scene's tracks from its pre-v2 timeline
/// database (the main scene falls back to the project-wide one).
pub fn v1_to_v2_context(project: &Value, legacy: &LegacyData) -> V1ToV2Context {
    let mut context = V1ToV2Context {
        media_types_by_id: legacy.media_types.clone(),
        ..V1ToV2Context::default()
    };
    let Some(project_id) = project_id(project) else {
        return context;
    };
    let Some(Value::Array(scenes)) = project.get("scenes") else {
        return context;
    };
    for scene in scenes {
        let Some(scene_id) = string(scene.get("id")) else {
            continue;
        };
        let mut timeline = legacy
            .timelines
            .get(&format!("video-editor-timelines-{project_id}-{scene_id}"));
        if timeline.is_none() && scene.get("isMain") == Some(&Value::Bool(true)) {
            timeline = legacy
                .timelines
                .get(&format!("video-editor-timelines-{project_id}"));
        }
        let tracks = match timeline.and_then(|timeline| timeline.get("tracks")) {
            Some(Value::Array(tracks)) => tracks.clone(),
            _ => Vec::new(),
        };
        context
            .legacy_tracks_by_scene_id
            .insert(scene_id.to_owned(), tracks);
    }
    context
}

pub fn v1_to_v2(project: Value, context: &V1ToV2Context, now: DateTime<Utc>) -> MigrationResult {
    let Some(project_id) = project_id(&project).map(str::to_owned) else {
        return MigrationResult::skip(project, "no project id");
    };
    let already_v2 = number(project.get("version")).is_some_and(|version| version >= 2.0)
        || (project.get("metadata").is_some_and(Value::is_object)
            && project.get("settings").is_some_and(Value::is_object));
    if already_v2 {
        return MigrationResult::skip(project, "already v2");
    }

    let now = iso(now);
    let date = |value: Option<&Value>| string(value).map_or_else(|| now.clone(), str::to_owned);
    let get = |key: &str| project.get(key);

    // `metadata` from the v1 metadata object, else from the flat v0 fields.
    let (source, id) = match get("metadata") {
        Some(Value::Object(metadata)) => (
            metadata.clone(),
            string(metadata.get("id")).unwrap_or(&project_id).to_owned(),
        ),
        _ => (
            project.as_object().cloned().unwrap_or_default(),
            project_id.clone(),
        ),
    };
    let source = |key: &str| source.get(key);
    let mut metadata = Object::new();
    metadata.insert("id".to_owned(), json!(id));
    metadata.insert(
        "name".to_owned(),
        json!(string(source("name")).unwrap_or("")),
    );
    if let Some(thumbnail) = string(source("thumbnail")) {
        metadata.insert("thumbnail".to_owned(), json!(thumbnail));
    }
    metadata.insert("createdAt".to_owned(), json!(date(source("createdAt"))));
    metadata.insert("updatedAt".to_owned(), json!(date(source("updatedAt"))));

    let scenes: Vec<Value> = match get("scenes") {
        Some(Value::Array(scenes)) => scenes
            .iter()
            .map(|scene| migrate_v1_scene(scene, context))
            .collect(),
        _ => Vec::new(),
    };
    let scenes = apply_legacy_bookmarks(scenes, get("bookmarks"));

    let settings = match get("settings") {
        Some(Value::Object(settings)) => json!({
            "fps": num_or(settings.get("fps"), V2_DEFAULT_FPS),
            "canvasSize": canvas_size(settings.get("canvasSize")),
            "background": background(settings.get("background"), None, None, None),
            "originalCanvasSize": null,
        }),
        _ => json!({
            "fps": num_or(get("fps"), V2_DEFAULT_FPS),
            "canvasSize": canvas_size(get("canvasSize")),
            "background": background(
                get("background"),
                get("backgroundType"),
                get("backgroundColor"),
                get("blurIntensity"),
            ),
            "originalCanvasSize": null,
        }),
    };

    let current_scene_id = match string(get("currentSceneId")) {
        Some(id) if !id.is_empty() => id.to_owned(),
        _ => main_scene_id(&scenes).unwrap_or_default(),
    };

    let mut project = project;
    project["metadata"] = Value::Object(metadata);
    project["scenes"] = Value::Array(scenes);
    project["currentSceneId"] = json!(current_scene_id);
    project["settings"] = settings;
    project["version"] = json!(2);
    MigrationResult::done(project)
}

fn migrate_v1_scene(scene: &Value, context: &V1ToV2Context) -> Value {
    let Some(scene_id) = string(scene.get("id")) else {
        return scene.clone();
    };
    if matches!(scene.get("tracks"), Some(Value::Array(tracks)) if !tracks.is_empty()) {
        return scene.clone();
    }
    let legacy = context
        .legacy_tracks_by_scene_id
        .get(scene_id)
        .map_or(&[][..], Vec::as_slice);
    let mut scene = scene.clone();
    scene["tracks"] = Value::Array(transform_legacy_tracks(legacy, context));
    scene
}

fn transform_legacy_tracks(tracks: &[Value], context: &V1ToV2Context) -> Vec<Value> {
    let mut found_media_track = false;
    tracks
        .iter()
        .filter_map(|track| {
            let track = track.as_object()?;
            match string(track.get("type"))? {
                "media" => {
                    let is_main = !found_media_track;
                    found_media_track = true;
                    Some(transform_media_track(track, context, is_main))
                }
                "text" => Some(transform_text_track(track)),
                "audio" => Some(transform_audio_track(track)),
                _ => None,
            }
        })
        .collect()
}

fn str_or(value: Option<&Value>, fallback: &str) -> Value {
    json!(string(value).unwrap_or(fallback))
}

/// The value itself when it is a number, else `fallback`.
fn num_or(value: Option<&Value>, fallback: f64) -> Value {
    match value {
        Some(value) if value.is_number() => value.clone(),
        _ => num(fallback),
    }
}

fn legacy_elements(track: &Object) -> &[Value] {
    match track.get("elements") {
        Some(Value::Array(elements)) => elements,
        _ => &[],
    }
}

fn timing(element: &Object) -> [(&'static str, Value); 4] {
    [
        ("duration", num_or(element.get("duration"), 0.0)),
        ("startTime", num_or(element.get("startTime"), 0.0)),
        ("trimStart", num_or(element.get("trimStart"), 0.0)),
        ("trimEnd", num_or(element.get("trimEnd"), 0.0)),
    ]
}

fn with_timing(mut value: Value, element: &Object) -> Value {
    for (key, time) in timing(element) {
        value[key] = time;
    }
    value
}

fn transform_media_track(track: &Object, context: &V1ToV2Context, is_main: bool) -> Value {
    let elements: Vec<Value> = legacy_elements(track)
        .iter()
        .filter_map(|element| {
            let element = element.as_object()?;
            if !is(element.get("type"), "media") {
                return None;
            }
            let media_id = string(element.get("mediaId"))?;
            let is_image = context
                .media_types_by_id
                .get(media_id)
                .is_some_and(|kind| kind == "image");
            let transform = json!({ "scale": 1, "position": { "x": 0, "y": 0 }, "rotate": 0 });
            let id = str_or(element.get("id"), "");
            let name = str_or(element.get("name"), "");
            let value = if is_image {
                json!({
                    "id": id, "name": name, "type": "image", "mediaId": media_id,
                    "hidden": false, "transform": transform, "opacity": 1,
                })
            } else {
                json!({
                    "id": id, "name": name, "type": "video", "mediaId": media_id,
                    "muted": element.get("muted") == Some(&Value::Bool(true)),
                    "hidden": false, "transform": transform, "opacity": 1,
                })
            };
            Some(with_timing(value, element))
        })
        .collect();
    json!({
        "id": str_or(track.get("id"), ""),
        "name": str_or(track.get("name"), ""),
        "type": "video",
        "elements": elements,
        "isMain": is_main,
        "muted": false,
        "hidden": false,
    })
}

fn one_of(value: Option<&Value>, allowed: &[&str], fallback: &str) -> Value {
    let value = string(value).filter(|value| allowed.contains(value));
    json!(value.unwrap_or(fallback))
}

fn transform_text_track(track: &Object) -> Value {
    let elements: Vec<Value> = legacy_elements(track)
        .iter()
        .filter_map(|element| {
            let element = element.as_object()?;
            if !is(element.get("type"), "text") {
                return None;
            }
            let value = json!({
                "id": str_or(element.get("id"), ""),
                "name": str_or(element.get("name"), ""),
                "type": "text",
                "content": str_or(element.get("content"), ""),
                "fontSize": num_or(element.get("fontSize"), 16.0),
                "fontFamily": str_or(element.get("fontFamily"), "Arial"),
                "color": str_or(element.get("color"), "#000000"),
                "background": {
                    "enabled": false,
                    "color": str_or(element.get("backgroundColor"), "transparent"),
                    "cornerRadius": 0,
                    "paddingX": 8,
                    "paddingY": 4,
                    "offsetX": 0,
                    "offsetY": 0,
                },
                "textAlign": one_of(element.get("textAlign"), &["left", "center", "right"], "left"),
                "fontWeight": one_of(element.get("fontWeight"), &["normal", "bold"], "normal"),
                "fontStyle": one_of(element.get("fontStyle"), &["normal", "italic"], "normal"),
                "textDecoration": one_of(
                    element.get("textDecoration"),
                    &["none", "underline", "line-through"],
                    "none",
                ),
                "hidden": false,
                "transform": {
                    "scale": 1,
                    "position": {
                        "x": num_or(element.get("x"), 0.0),
                        "y": num_or(element.get("y"), 0.0),
                    },
                    "rotate": num_or(element.get("rotation"), 0.0),
                },
                "opacity": num_or(element.get("opacity"), 1.0),
            });
            Some(with_timing(value, element))
        })
        .collect();
    json!({
        "id": str_or(track.get("id"), ""),
        "name": str_or(track.get("name"), ""),
        "type": "text",
        "elements": elements,
        "hidden": false,
    })
}

fn transform_audio_track(track: &Object) -> Value {
    let elements: Vec<Value> = legacy_elements(track)
        .iter()
        .filter_map(|element| {
            let element = element.as_object()?;
            if !is(element.get("type"), "audio") {
                return None;
            }
            let media_id = string(element.get("mediaId"))?;
            let value = json!({
                "id": str_or(element.get("id"), ""),
                "name": str_or(element.get("name"), ""),
                "type": "audio",
                "sourceType": "upload",
                "mediaId": media_id,
                "volume": 1,
            });
            Some(with_timing(value, element))
        })
        .collect();
    json!({
        "id": str_or(track.get("id"), ""),
        "name": str_or(track.get("name"), ""),
        "type": "audio",
        "elements": elements,
        "muted": false,
    })
}

/// Classic `findMainSceneId`: the scene marked main, else the first with an id.
fn main_scene_id(scenes: &[Value]) -> Option<String> {
    let id = |scene: &Value| string(scene.get("id")).map(str::to_owned);
    scenes
        .iter()
        .filter(|scene| scene.get("isMain") == Some(&Value::Bool(true)))
        .find_map(id)
        .or_else(|| scenes.iter().find_map(id))
}

fn apply_legacy_bookmarks(scenes: Vec<Value>, bookmarks: Option<&Value>) -> Vec<Value> {
    let Some(Value::Array(bookmarks)) = bookmarks else {
        return scenes;
    };
    if bookmarks.is_empty() {
        return scenes;
    }
    let main_id = main_scene_id(&scenes);
    scenes
        .into_iter()
        .map(|mut scene| {
            if !scene.is_object() {
                return scene;
            }
            if let Some(main_id) = &main_id
                && string(scene.get("id")) != Some(main_id.as_str())
            {
                return scene;
            }
            if matches!(scene.get("bookmarks"), Some(Value::Array(own)) if !own.is_empty()) {
                return scene;
            }
            scene["bookmarks"] = Value::Array(bookmarks.clone());
            scene
        })
        .collect()
}

fn canvas_size(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Object(size)) => json!({
            "width": num_or(size.get("width"), 1920.0),
            "height": num_or(size.get("height"), 1080.0),
        }),
        _ => json!({ "width": 1920, "height": 1080 }),
    }
}

fn background(
    value: Option<&Value>,
    background_type: Option<&Value>,
    background_color: Option<&Value>,
    blur_intensity: Option<&Value>,
) -> Value {
    let blur = |intensity: Option<&Value>| json!({ "type": "blur", "blurIntensity": num_or(intensity, V2_DEFAULT_BLUR_INTENSITY) });
    let color = |color: Option<&Value>| json!({ "type": "color", "color": str_or(color, V2_DEFAULT_BACKGROUND_COLOR) });
    match value {
        Some(Value::Object(value)) if is(value.get("type"), "blur") => {
            blur(value.get("blurIntensity"))
        }
        Some(Value::Object(value)) => color(value.get("color")),
        _ if is(background_type, "blur") => blur(blur_intensity),
        _ => color(background_color),
    }
}

// ---------------------------------------------------------------------------
// v2 → v3: metadata.duration
// ---------------------------------------------------------------------------

pub fn v2_to_v3(mut project: Value) -> MigrationResult {
    if project_id(&project).is_none() {
        return MigrationResult::skip(project, "no project id");
    }
    let already_v3 = number(project.get("version")).is_some_and(|version| version >= 3.0)
        || number(
            project
                .get("metadata")
                .and_then(|metadata| metadata.get("duration")),
        )
        .is_some();
    if already_v3 {
        return MigrationResult::skip(project, "already v3");
    }

    let duration = {
        let scenes = scenes(&mut project);
        let main = scenes
            .iter()
            .position(|scene| scene.get("isMain") == Some(&Value::Bool(true)))
            .or((!scenes.is_empty()).then_some(0));
        let mut max_end = 0.0_f64;
        if let Some(main) = main
            && let Some(Value::Array(tracks)) = scenes[main].get("tracks")
        {
            for track in tracks {
                let Some(Value::Array(elements)) = track.get("elements") else {
                    continue;
                };
                for element in elements.iter().filter(|element| element.is_object()) {
                    let start = number(element.get("startTime")).unwrap_or(0.0);
                    let duration = number(element.get("duration")).unwrap_or(0.0);
                    max_end = max_end.max(start + duration);
                }
            }
        }
        max_end
    };

    match project.get_mut("metadata") {
        Some(Value::Object(metadata)) => {
            metadata.insert("duration".to_owned(), num(duration));
        }
        _ => project["metadata"] = json!({ "duration": num(duration) }),
    }
    set_version(project, 3)
}

// ---------------------------------------------------------------------------
// v3 → v4: numeric text font weights
// ---------------------------------------------------------------------------

const NUMERIC_FONT_WEIGHTS: [&str; 9] = [
    "100", "200", "300", "400", "500", "600", "700", "800", "900",
];

pub fn v3_to_v4(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 4);
    for track in flat_tracks(&mut project) {
        if !is(track.get("type"), "text") {
            continue;
        }
        for element in elements(track) {
            if !is(element.get("type"), "text") {
                continue;
            }
            if let Some(weight) = element.get("fontWeight").and_then(normalize_font_weight) {
                element.insert("fontWeight".to_owned(), weight);
            }
        }
    }
    set_version(project, 4)
}

/// The normalized weight, or `None` to leave the value as is.
fn normalize_font_weight(value: &Value) -> Option<Value> {
    if let Some(weight) = value.as_f64() {
        let text = num(weight).to_string();
        return NUMERIC_FONT_WEIGHTS
            .contains(&text.as_str())
            .then(|| json!(text));
    }
    let normalized = value.as_str()?.trim().to_lowercase();
    match normalized.as_str() {
        "normal" => Some(json!("400")),
        "bold" => Some(json!("700")),
        weight if NUMERIC_FONT_WEIGHTS.contains(&weight) => Some(json!(weight)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// v4 → v5: sticker iconName → stickerId
// ---------------------------------------------------------------------------

pub fn v4_to_v5(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 5);
    for element in flat_elements(&mut project) {
        if !is(element.get("type"), "sticker") {
            continue;
        }
        let existing = string(element.get("stickerId")).map(str::to_owned);
        let icon_name = string(element.get("iconName"));
        let normalized = existing
            .as_deref()
            .or(icon_name)
            .and_then(normalize_sticker_id);
        let changed = element.contains_key("iconName")
            || element.contains_key("color")
            || normalized != existing;
        if !changed {
            continue;
        }
        element.remove("iconName");
        element.remove("color");
        if let Some(sticker_id) = normalized {
            element.insert("stickerId".to_owned(), json!(sticker_id));
        }
    }
    set_version(project, 5)
}

fn normalize_sticker_id(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    match value.split_once(':') {
        Some((provider, _)) if ["icons", "emoji", "flags", "shapes"].contains(&provider) => {
            Some(value.to_owned())
        }
        _ => Some(format!("icons:{value}")),
    }
}

// ---------------------------------------------------------------------------
// v5 → v6: bookmarks become objects
// ---------------------------------------------------------------------------

pub fn v5_to_v6(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 6);
    for scene in scenes(&mut project) {
        let Some(Value::Array(bookmarks)) = scene.get_mut("bookmarks") else {
            continue;
        };
        for bookmark in bookmarks.iter_mut().filter(|bookmark| bookmark.is_number()) {
            *bookmark = json!({ "time": bookmark.clone() });
        }
    }
    set_version(project, 6)
}

// ---------------------------------------------------------------------------
// v6 → v7: text background object
// ---------------------------------------------------------------------------

pub fn v6_to_v7(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 7);
    for element in flat_elements(&mut project) {
        if !is(element.get("type"), "text")
            || element.get("background").is_some_and(Value::is_object)
        {
            continue;
        }
        let color = str_or(element.get("backgroundColor"), "transparent");
        element.remove("backgroundColor");
        element.insert(
            "background".to_owned(),
            json!({
                "color": color,
                "cornerRadius": 0,
                "paddingX": 8,
                "paddingY": 4,
                "offsetX": 0,
                "offsetY": 0,
            }),
        );
    }
    set_version(project, 7)
}

// ---------------------------------------------------------------------------
// v7 → v8: sourceDuration on video and audio
// ---------------------------------------------------------------------------

pub fn v7_to_v8(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 8);
    for element in flat_elements(&mut project) {
        let media = is(element.get("type"), "video") || is(element.get("type"), "audio");
        if !media || number(element.get("sourceDuration")).is_some() {
            continue;
        }
        let total = ["trimStart", "duration", "trimEnd"]
            .iter()
            .map(|key| number(element.get(*key)).unwrap_or(0.0))
            .sum();
        element.insert("sourceDuration".to_owned(), num(total));
    }
    set_version(project, 8)
}

// ---------------------------------------------------------------------------
// v8 → v9: text background.enabled
// ---------------------------------------------------------------------------

pub fn v8_to_v9(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 9);
    for track in flat_tracks(&mut project) {
        if !is(track.get("type"), "text") {
            continue;
        }
        for element in elements(track) {
            if !is(element.get("type"), "text") {
                continue;
            }
            let Some(Value::Object(background)) = element.get_mut("background") else {
                continue;
            };
            if background.get("enabled").is_some_and(Value::is_boolean) {
                continue;
            }
            let enabled = string(background.get("color")).unwrap_or("transparent") != "transparent";
            background.insert("enabled".to_owned(), json!(enabled));
        }
    }
    set_version(project, 9)
}

// ---------------------------------------------------------------------------
// v9 → v10: version bump only
// ---------------------------------------------------------------------------

pub fn v9_to_v10(project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 10);
    set_version(project, 10)
}

// ---------------------------------------------------------------------------
// v10 → v11: transform.scale → scaleX/scaleY
// ---------------------------------------------------------------------------

pub fn v10_to_v11(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 11);
    for element in flat_elements(&mut project) {
        let Some(Value::Object(transform)) = element.get_mut("transform") else {
            continue;
        };
        let Some(scale) = transform
            .get("scale")
            .filter(|scale| scale.is_number())
            .cloned()
        else {
            continue;
        };
        transform.remove("scale");
        transform.insert("scaleX".to_owned(), scale.clone());
        transform.insert("scaleY".to_owned(), scale);

        let Some(Value::Object(channels)) = element
            .get_mut("animations")
            .and_then(|animations| animations.get_mut("channels"))
        else {
            continue;
        };
        let has_keyframes = channels
            .get("transform.scale")
            .is_some_and(|channel| channel.get("keyframes").is_some_and(Value::is_array));
        if let (true, Some(channel)) = (has_keyframes, channels.remove("transform.scale")) {
            channels.insert("transform.scaleX".to_owned(), channel.clone());
            channels.insert("transform.scaleY".to_owned(), channel);
        }
    }
    set_version(project, 11)
}

// ---------------------------------------------------------------------------
// v11 → v12: position.x/position.y channels → one vector channel
// ---------------------------------------------------------------------------

struct ScalarKeyframe {
    id: String,
    time: f64,
    value: f64,
    interpolation: String,
}

/// JS truthiness of an optional JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(_) => true,
    }
}

pub fn v11_to_v12(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 12);
    for element in flat_elements(&mut project) {
        let position = element
            .get("transform")
            .and_then(|transform| transform.get("position"));
        let base_x = number(position.and_then(|position| position.get("x"))).unwrap_or(0.0);
        let base_y = number(position.and_then(|position| position.get("y"))).unwrap_or(0.0);

        let Some(Value::Object(channels)) = element
            .get_mut("animations")
            .filter(|animations| animations.is_object())
            .and_then(|animations| animations.get_mut("channels"))
        else {
            continue;
        };
        let x_channel = channels.get("transform.position.x");
        let y_channel = channels.get("transform.position.y");
        if !truthy(x_channel) && !truthy(y_channel) {
            continue;
        }
        let x_keys = scalar_keyframes(x_channel);
        let y_keys = scalar_keyframes(y_channel);

        let mut times: Vec<f64> = Vec::new();
        for time in x_keys.iter().chain(&y_keys).map(|key| key.time) {
            if !times.contains(&time) {
                times.push(time);
            }
        }
        times.sort_by(f64::total_cmp);

        let near = |keys: &'_ [ScalarKeyframe], time: f64| {
            keys.iter().position(|key| (key.time - time).abs() < 0.001)
        };
        let keyframes: Vec<Value> = times
            .into_iter()
            .map(|time| {
                let x_key = near(&x_keys, time).map(|index| &x_keys[index]);
                let y_key = near(&y_keys, time).map(|index| &y_keys[index]);
                let x = x_key.map_or_else(|| interpolate(&x_keys, time, base_x), |key| key.value);
                let y = y_key.map_or_else(|| interpolate(&y_keys, time, base_y), |key| key.value);
                let either = x_key.or(y_key);
                json!({
                    "id": either.map_or_else(new_id, |key| key.id.clone()),
                    "time": num(time),
                    "value": { "x": num(x), "y": num(y) },
                    "interpolation": either.map_or("linear", |key| key.interpolation.as_str()),
                })
            })
            .collect();

        channels.remove("transform.position.x");
        channels.remove("transform.position.y");
        channels.insert(
            "transform.position".to_owned(),
            json!({ "valueKind": "vector", "keyframes": keyframes }),
        );
    }
    set_version(project, 12)
}

fn scalar_keyframes(channel: Option<&Value>) -> Vec<ScalarKeyframe> {
    let Some(Value::Array(keyframes)) = channel.and_then(|channel| channel.get("keyframes")) else {
        return Vec::new();
    };
    keyframes
        .iter()
        .filter_map(|keyframe| {
            Some(ScalarKeyframe {
                id: string(keyframe.get("id"))?.to_owned(),
                time: number(keyframe.get("time"))?,
                value: number(keyframe.get("value"))?,
                interpolation: string(keyframe.get("interpolation"))
                    .unwrap_or("linear")
                    .to_owned(),
            })
        })
        .collect()
}

fn interpolate(keys: &[ScalarKeyframe], time: f64, fallback: f64) -> f64 {
    let mut sorted: Vec<&ScalarKeyframe> = keys.iter().collect();
    sorted.sort_by(|a, b| a.time.total_cmp(&b.time));
    let (Some(first), Some(last)) = (sorted.first(), sorted.last()) else {
        return fallback;
    };
    if time <= first.time {
        return first.value;
    }
    if time >= last.time {
        return last.value;
    }
    for pair in sorted.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        if time < left.time || time > right.time {
            continue;
        }
        if left.interpolation == "hold" {
            return left.value;
        }
        let t = (time - left.time) / (right.time - left.time);
        return left.value + (right.value - left.value) * t;
    }
    last.value
}

// ---------------------------------------------------------------------------
// v12 → v13: mask feather/inverted/stroke move into params
// ---------------------------------------------------------------------------

pub fn v12_to_v13(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 13);
    for element in flat_elements(&mut project) {
        for mask in masks(element) {
            let mut params = match mask.get("params") {
                Some(Value::Object(params)) => params.clone(),
                _ => Object::new(),
            };
            let stroke = mask.get("stroke").filter(|stroke| stroke.is_object());
            params.insert("feather".to_owned(), num_or(mask.get("feather"), 0.0));
            params.insert(
                "inverted".to_owned(),
                json!(
                    mask.get("inverted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                ),
            );
            params.insert(
                "strokeColor".to_owned(),
                str_or(stroke.and_then(|stroke| stroke.get("color")), "#ffffff"),
            );
            params.insert(
                "strokeWidth".to_owned(),
                num_or(stroke.and_then(|stroke| stroke.get("width")), 0.0),
            );
            mask.insert("params".to_owned(), Value::Object(params));
            mask.remove("feather");
            mask.remove("inverted");
            mask.remove("stroke");
        }
    }
    set_version(project, 13)
}

// ---------------------------------------------------------------------------
// v13 → v14: split mask position → x/y anchor
// ---------------------------------------------------------------------------

pub fn v13_to_v14(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 14);
    for element in flat_elements(&mut project) {
        for mask in masks(element) {
            if !is(mask.get("type"), "split") {
                continue;
            }
            let mut params = match mask.get("params") {
                Some(Value::Object(params)) => params.clone(),
                _ => Object::new(),
            };
            let position = number(params.get("position")).unwrap_or(0.5);
            let rotation = number(params.get("rotation")).unwrap_or(0.0);
            let angle = rotation.to_radians();
            params.remove("position");
            params.insert("x".to_owned(), num((position - 0.5) * angle.cos()));
            params.insert("y".to_owned(), num((position - 0.5) * angle.sin()));
            mask.insert("params".to_owned(), Value::Object(params));
        }
    }
    set_version(project, 14)
}

// ---------------------------------------------------------------------------
// v14 → v15: sticker intrinsic size
// ---------------------------------------------------------------------------

pub fn v14_to_v15(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 15);
    for element in flat_elements(&mut project) {
        let sized = number(element.get("intrinsicWidth")).is_some()
            && number(element.get("intrinsicHeight")).is_some();
        if is(element.get("type"), "sticker") && !sized {
            element.insert("intrinsicWidth".to_owned(), json!(200));
            element.insert("intrinsicHeight".to_owned(), json!(200));
        }
    }
    set_version(project, 15)
}

// ---------------------------------------------------------------------------
// v15 → v16: sticker tracks → graphic tracks
// ---------------------------------------------------------------------------

pub fn v15_to_v16(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 16);
    for track in flat_tracks(&mut project) {
        if is(track.get("type"), "sticker") {
            track.insert("type".to_owned(), json!("graphic"));
        }
    }
    set_version(project, 16)
}

// ---------------------------------------------------------------------------
// v16 → v17: mask strokeAlign
// ---------------------------------------------------------------------------

pub fn v16_to_v17(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 17);
    for element in flat_elements(&mut project) {
        for mask in masks(element) {
            if let Some(Value::Object(params)) = mask.get_mut("params")
                && !params.get("strokeAlign").is_some_and(Value::is_string)
            {
                params.insert("strokeAlign".to_owned(), json!("center"));
            }
        }
    }
    set_version(project, 17)
}

// ---------------------------------------------------------------------------
// v17 → v18: linear volume → dB
// ---------------------------------------------------------------------------

pub fn v17_to_v18(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 18);
    for element in flat_elements(&mut project) {
        if !is(element.get("type"), "audio") && !is(element.get("type"), "video") {
            continue;
        }
        let volume = number(element.get("volume")).map_or(0.0, linear_gain_to_db);
        element.insert("volume".to_owned(), num(volume));
    }
    set_version(project, 18)
}

fn linear_gain_to_db(gain: f64) -> f64 {
    const MIN_DB: f64 = -60.0;
    const MAX_DB: f64 = 20.0;
    if gain <= 0.0 {
        return MIN_DB;
    }
    (20.0 * gain.log10()).clamp(MIN_DB, MAX_DB)
}

// ---------------------------------------------------------------------------
// v18 → v19: canvas size mode
// ---------------------------------------------------------------------------

pub fn v18_to_v19(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 19);
    if let Some(Value::Object(settings)) = project.get_mut("settings") {
        settings.insert("canvasSizeMode".to_owned(), json!("preset"));
        settings.insert("lastCustomCanvasSize".to_owned(), Value::Null);
    }
    set_version(project, 19)
}

// ---------------------------------------------------------------------------
// v19 → v20: video source audio on by default
// ---------------------------------------------------------------------------

pub fn v19_to_v20(mut project: Value) -> MigrationResult {
    skip_unless!(loose_skip, project, 20);
    for element in flat_elements(&mut project) {
        if is(element.get("type"), "video")
            && !element
                .get("isSourceAudioEnabled")
                .is_some_and(Value::is_boolean)
        {
            element.insert("isSourceAudioEnabled".to_owned(), json!(true));
        }
    }
    set_version(project, 20)
}

// ---------------------------------------------------------------------------
// v20 → v21: background blur intensity scale
// ---------------------------------------------------------------------------

pub fn v20_to_v21(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 21);
    if let Some(Value::Object(background)) = project
        .get_mut("settings")
        .and_then(|settings| settings.get_mut("background"))
        && is(background.get("type"), "blur")
    {
        let intensity = number(background.get("blurIntensity")).map_or(50.0, |raw| raw * 5.0);
        background.insert("blurIntensity".to_owned(), num(intensity));
    }
    set_version(project, 21)
}

// ---------------------------------------------------------------------------
// v21 → v22: animation channels → bindings + component channels
// ---------------------------------------------------------------------------

pub fn v21_to_v22(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 22);
    for element in flat_elements(&mut project) {
        let Some(Value::Object(animations)) = element.get("animations") else {
            continue;
        };
        if animations.get("bindings").is_some_and(Value::is_object) {
            continue;
        }
        match migrate_legacy_animations(animations) {
            Some(animations) => element.insert("animations".to_owned(), animations),
            None => element.remove("animations"),
        };
    }
    set_version(project, 22)
}

fn migrate_legacy_animations(animations: &Object) -> Option<Value> {
    let Some(Value::Object(legacy)) = animations.get("channels") else {
        return None;
    };
    let mut bindings = Object::new();
    let mut channels = Object::new();
    for (path, channel) in legacy {
        let Some((binding, new_channels)) = migrate_legacy_channel(path, channel) else {
            continue;
        };
        bindings.insert(path.clone(), binding);
        channels.extend(new_channels);
    }
    if bindings.is_empty() {
        return None;
    }
    Some(json!({ "bindings": bindings, "channels": channels }))
}

struct LegacyKey<'a> {
    id: &'a str,
    time: &'a Value,
    value: &'a Value,
    hold: bool,
}

/// Keyframes with a string id, a numeric time and a value `valid` accepts.
fn legacy_keys<'a>(channel: &'a Object, valid: impl Fn(&Value) -> bool) -> Vec<LegacyKey<'a>> {
    let Some(Value::Array(keyframes)) = channel.get("keyframes") else {
        return Vec::new();
    };
    keyframes
        .iter()
        .filter_map(|keyframe| {
            let time = keyframe.get("time").filter(|time| time.is_number())?;
            let value = keyframe.get("value").filter(|value| valid(value))?;
            Some(LegacyKey {
                id: string(keyframe.get("id"))?,
                time,
                value,
                hold: is(keyframe.get("interpolation"), "hold"),
            })
        })
        .collect()
}

fn scalar_key(key: &LegacyKey, value: Value) -> Value {
    json!({
        "id": key.id,
        "time": key.time,
        "value": value,
        "segmentToNext": if key.hold { "step" } else { "linear" },
        "tangentMode": "flat",
    })
}

fn binding(path: &str, kind: &str, components: &[&str]) -> Value {
    let components: Vec<Value> = components
        .iter()
        .map(|key| json!({ "key": key, "channelId": format!("{path}:{key}") }))
        .collect();
    json!({ "path": path, "kind": kind, "components": components })
}

fn migrate_legacy_channel(path: &str, channel: &Value) -> Option<(Value, Object)> {
    let channel = channel.as_object()?;
    let channel_id = |component: &str| format!("{path}:{component}");
    let scalar = |keys: Vec<Value>| json!({ "kind": "scalar", "keys": keys });
    let mut channels = Object::new();
    match string(channel.get("valueKind"))? {
        "number" => {
            let keys = legacy_keys(channel, Value::is_number);
            if keys.is_empty() {
                return None;
            }
            let keys = keys
                .iter()
                .map(|key| scalar_key(key, key.value.clone()))
                .collect();
            channels.insert(channel_id("value"), scalar(keys));
            Some((binding(path, "number", &["value"]), channels))
        }
        "discrete" => {
            let keys = legacy_keys(channel, |value| value.is_string() || value.is_boolean());
            if keys.is_empty() {
                return None;
            }
            let keys: Vec<Value> = keys
                .iter()
                .map(|key| json!({ "id": key.id, "time": key.time, "value": key.value }))
                .collect();
            channels.insert(
                channel_id("value"),
                json!({ "kind": "discrete", "keys": keys }),
            );
            Some((binding(path, "discrete", &["value"]), channels))
        }
        "vector" => {
            let keys = legacy_keys(channel, |value| {
                number(value.get("x")).is_some() && number(value.get("y")).is_some()
            });
            if keys.is_empty() {
                return None;
            }
            for axis in ["x", "y"] {
                let axis_keys = keys
                    .iter()
                    .map(|key| scalar_key(key, key.value[axis].clone()))
                    .collect();
                channels.insert(channel_id(axis), scalar(axis_keys));
            }
            Some((binding(path, "vector2", &["x", "y"]), channels))
        }
        "color" => {
            let keys = legacy_keys(channel, Value::is_string);
            let colors: Vec<(&LegacyKey, [f64; 4])> = keys
                .iter()
                .filter_map(|key| Some((key, parse_color_to_linear_rgba(key.value.as_str()?)?)))
                .collect();
            if colors.is_empty() {
                return None;
            }
            for (index, component) in ["r", "g", "b", "a"].into_iter().enumerate() {
                let component_keys = colors
                    .iter()
                    .map(|(key, rgba)| scalar_key(key, num(rgba[index])))
                    .collect();
                channels.insert(channel_id(component), scalar(component_keys));
            }
            let mut binding = binding(path, "color", &["r", "g", "b", "a"]);
            binding["colorSpace"] = json!("srgb-linear");
            Some((binding, channels))
        }
        _ => None,
    }
}

/// A CSS color as linear-light RGBA, like classic's culori `parse` + sRGB
/// transfer. shortcut: reads hex, `rgb()`/`rgba()` and `transparent` only
/// (what classic's color pickers wrote); other CSS color syntaxes drop the
/// keyframe as unparsable. Add them if real projects turn up with them.
fn parse_color_to_linear_rgba(color: &str) -> Option<[f64; 4]> {
    let [r, g, b, a] = parse_srgb(color.trim())?;
    let linear = |value: f64| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    Some([linear(r), linear(g), linear(b), a.clamp(0.0, 1.0)])
}

fn parse_srgb(color: &str) -> Option<[f64; 4]> {
    let lower = color.to_ascii_lowercase();
    if lower == "transparent" {
        return Some([0.0, 0.0, 0.0, 0.0]);
    }
    if let Some(hex) = lower.strip_prefix('#') {
        let digits: Vec<u32> = hex.chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
        let channel = |pair: &[u32]| f64::from(pair[0] * 16 + pair[1]) / 255.0;
        let short = |digit: u32| f64::from(digit * 17) / 255.0;
        return match digits.len() {
            3 => Some([short(digits[0]), short(digits[1]), short(digits[2]), 1.0]),
            4 => Some([
                short(digits[0]),
                short(digits[1]),
                short(digits[2]),
                short(digits[3]),
            ]),
            6 => Some([
                channel(&digits[0..2]),
                channel(&digits[2..4]),
                channel(&digits[4..6]),
                1.0,
            ]),
            8 => Some([
                channel(&digits[0..2]),
                channel(&digits[2..4]),
                channel(&digits[4..6]),
                channel(&digits[6..8]),
            ]),
            _ => None,
        };
    }
    let inner = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    let component = |part: &str, scale: f64| -> Option<f64> {
        match part.strip_suffix('%') {
            Some(percent) => Some(percent.parse::<f64>().ok()? / 100.0),
            None => Some(part.parse::<f64>().ok()? / scale),
        }
    };
    match parts.as_slice() {
        [r, g, b] => Some([
            component(r, 255.0)?,
            component(g, 255.0)?,
            component(b, 255.0)?,
            1.0,
        ]),
        [r, g, b, a] => Some([
            component(r, 255.0)?,
            component(g, 255.0)?,
            component(b, 255.0)?,
            component(a, 1.0)?,
        ]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// v22 → v23: seconds → ticks, fps → frame-rate object
// ---------------------------------------------------------------------------

const TICKS_PER_SECOND: f64 = 120_000.0;

/// JS `Math.round`: halves round up.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

fn map_times(record: &mut Object, keys: &[&str], convert: &impl Fn(f64) -> f64) {
    for key in keys {
        if let Some(value) = record.get_mut(*key)
            && let Some(number) = value.as_f64()
        {
            *value = num(convert(number));
        }
    }
}

/// Applies `convert` to every stored media time, as v22 → v23 and
/// v27 → v28 do. `tracks_of` picks the tracks of the era's layout.
fn convert_project_times(
    project: &mut Value,
    tracks_of: fn(&mut Value) -> Vec<&mut Object>,
    convert: impl Fn(f64) -> f64,
) {
    if let Some(Value::Object(metadata)) = project.get_mut("metadata") {
        map_times(metadata, &["duration"], &convert);
    }
    if let Some(Value::Object(view)) = project.get_mut("timelineViewState") {
        map_times(view, &["playheadTime"], &convert);
    }
    for scene in scenes(project) {
        for bookmark in objects(scene.get_mut("bookmarks")) {
            map_times(bookmark, &["time", "duration"], &convert);
        }
    }
    for track in tracks_of(project) {
        for element in elements(track) {
            map_times(
                element,
                &[
                    "duration",
                    "startTime",
                    "trimStart",
                    "trimEnd",
                    "sourceDuration",
                ],
                &convert,
            );
            let Some(Value::Object(channels)) = element
                .get_mut("animations")
                .filter(|animations| animations.is_object())
                .and_then(|animations| animations.get_mut("channels"))
            else {
                continue;
            };
            for channel in channels.values_mut() {
                for keyframe in objects(channel.get_mut("keys")) {
                    map_times(keyframe, &["time"], &convert);
                    for handle in ["leftHandle", "rightHandle"] {
                        if let Some(Value::Object(handle)) = keyframe.get_mut(handle) {
                            map_times(handle, &["dt"], &convert);
                        }
                    }
                }
            }
        }
    }
}

pub fn v22_to_v23(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 23);
    convert_project_times(&mut project, flat_tracks, |seconds| {
        js_round(seconds * TICKS_PER_SECOND)
    });
    if let Some(Value::Object(settings)) = project.get_mut("settings")
        && let Some(fps) = settings.get_mut("fps")
    {
        *fps = migrate_frame_rate(fps.clone());
    }
    set_version(project, 23)
}

fn migrate_frame_rate(fps: Value) -> Value {
    const STANDARD: [(f64, u64, u64); 10] = [
        (24_000.0 / 1_001.0, 24_000, 1_001),
        (24.0, 24, 1),
        (25.0, 25, 1),
        (30_000.0 / 1_001.0, 30_000, 1_001),
        (30.0, 30, 1),
        (48.0, 48, 1),
        (50.0, 50, 1),
        (60_000.0 / 1_001.0, 60_000, 1_001),
        (60.0, 60, 1),
        (120.0, 120, 1),
    ];
    const ARBITRARY_DENOMINATOR: f64 = 1_000_000.0;

    let Some(rate) = fps.as_f64().filter(|rate| *rate > 0.0) else {
        return fps;
    };
    if let Some((_, numerator, denominator)) = STANDARD
        .iter()
        .find(|(value, _, _)| (rate - value).abs() <= 0.01)
    {
        return json!({ "numerator": numerator, "denominator": denominator });
    }
    if rate.fract() == 0.0 {
        return json!({ "numerator": fps, "denominator": 1 });
    }
    let numerator = js_round(rate * ARBITRARY_DENOMINATOR);
    let divisor = gcd(numerator, ARBITRARY_DENOMINATOR);
    json!({
        "numerator": num(numerator / divisor),
        "denominator": num(ARBITRARY_DENOMINATOR / divisor),
    })
}

fn gcd(left: f64, right: f64) -> f64 {
    let (mut left, mut right) = (left.abs(), right.abs());
    while right != 0.0 {
        (left, right) = (right, left % right);
    }
    if left == 0.0 { 1.0 } else { left }
}

// ---------------------------------------------------------------------------
// v23 → v24 and v25 → v26: flat tracks → { main, overlay, audio }
// ---------------------------------------------------------------------------

/// Splits a flat track list into classic's grouped layout. Tracks lose
/// `isMain`; without a video track, an empty main track is made.
fn group_tracks(tracks: Vec<Value>) -> Value {
    let main_index = tracks
        .iter()
        .position(|track| {
            is(track.get("type"), "video") && track.get("isMain") == Some(&Value::Bool(true))
        })
        .or_else(|| {
            tracks
                .iter()
                .position(|track| is(track.get("type"), "video"))
        });
    let strip = |track: &Value| {
        let mut track = track.clone();
        if let Some(track) = track.as_object_mut() {
            track.remove("isMain");
        }
        track
    };
    let main = main_index.map_or_else(
        || {
            json!({
                "id": new_id(),
                "name": "Main",
                "type": "video",
                "elements": [],
                "muted": false,
                "hidden": false,
            })
        },
        |index| strip(&tracks[index]),
    );
    let overlay: Vec<Value> = tracks
        .iter()
        .enumerate()
        .filter(|(index, track)| {
            Some(*index) != main_index && track.is_object() && !is(track.get("type"), "audio")
        })
        .map(|(_, track)| strip(track))
        .collect();
    let audio: Vec<Value> = tracks
        .iter()
        .filter(|track| track.is_object() && is(track.get("type"), "audio"))
        .map(strip)
        .collect();
    json!({ "main": main, "overlay": overlay, "audio": audio })
}

pub fn v23_to_v24(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 24);
    for scene in scenes(&mut project) {
        if let Some(Value::Array(tracks)) = scene.get_mut("tracks") {
            let grouped = group_tracks(std::mem::take(tracks));
            scene.insert("tracks".to_owned(), grouped);
        }
    }
    set_version(project, 24)
}

// ---------------------------------------------------------------------------
// v24 → v25: position vector2 binding → positionX/positionY
// ---------------------------------------------------------------------------

pub fn v24_to_v25(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 25);
    for element in grouped_elements(&mut project) {
        let Some(Value::Object(animations)) = element.get_mut("animations") else {
            continue;
        };
        let (Some(Value::Object(bindings)), Some(Value::Object(_))) =
            (animations.get("bindings"), animations.get("channels"))
        else {
            continue;
        };
        if !is(
            bindings
                .get("transform.position")
                .filter(|binding| binding.is_object())
                .and_then(|binding| binding.get("kind")),
            "vector2",
        ) {
            continue;
        }
        let bindings = animations["bindings"]
            .as_object_mut()
            .expect("checked above");
        bindings.remove("transform.position");
        bindings.insert(
            "transform.positionX".to_owned(),
            binding("transform.positionX", "number", &["value"]),
        );
        bindings.insert(
            "transform.positionY".to_owned(),
            binding("transform.positionY", "number", &["value"]),
        );
        let channels = animations["channels"]
            .as_object_mut()
            .expect("checked above");
        let x = channels.remove("transform.position:x");
        let y = channels.remove("transform.position:y");
        if let Some(x) = x.filter(Value::is_object) {
            channels.insert("transform.positionX:value".to_owned(), x);
        }
        if let Some(y) = y.filter(Value::is_object) {
            channels.insert("transform.positionY:value".to_owned(), y);
        }
    }
    set_version(project, 25)
}

// ---------------------------------------------------------------------------
// v25 → v26: repair scenes a buggy v24 → v25 left without grouped tracks
// ---------------------------------------------------------------------------

pub fn v25_to_v26(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 26);
    for scene in scenes(&mut project) {
        let tracks = match scene.remove("tracks") {
            Some(Value::Object(tracks))
                if tracks.get("overlay").is_some_and(Value::is_array)
                    && tracks.get("audio").is_some_and(Value::is_array) =>
            {
                scene.insert("tracks".to_owned(), Value::Object(tracks));
                continue;
            }
            Some(Value::Array(tracks)) => tracks,
            // `Object.values`: integer keys ascending first, as JS orders them.
            Some(Value::Object(tracks)) => {
                let mut entries: Vec<(String, Value)> = tracks.into_iter().collect();
                entries.sort_by_key(|(key, _)| key.parse::<u32>().map_or(u64::MAX, u64::from));
                entries.into_iter().map(|(_, track)| track).collect()
            }
            _ => Vec::new(),
        };
        scene.insert("tracks".to_owned(), group_tracks(tracks));
    }
    set_version(project, 26)
}

// ---------------------------------------------------------------------------
// v26 → v27: custom mask path JSON string → point array
// ---------------------------------------------------------------------------

pub fn v26_to_v27(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 27);
    for element in grouped_elements(&mut project) {
        for mask in masks(element) {
            if !is(mask.get("type"), "custom") {
                continue;
            }
            let Some(Value::Object(params)) = mask.get_mut("params") else {
                continue;
            };
            let Some(path) = string(params.get("path")) else {
                continue;
            };
            let points = parse_freeform_path(path);
            params.insert("path".to_owned(), Value::Array(points));
        }
    }
    set_version(project, 27)
}

fn parse_freeform_path(path: &str) -> Vec<Value> {
    let Ok(Value::Array(points)) = serde_json::from_str::<Value>(path) else {
        return Vec::new();
    };
    points
        .into_iter()
        .filter(|point| {
            string(point.get("id")).is_some()
                && ["x", "y", "inX", "inY", "outX", "outY"]
                    .iter()
                    .all(|key| number(point.get(*key)).is_some())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// v27 → v28: round media times to whole ticks
// ---------------------------------------------------------------------------

pub fn v27_to_v28(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 28);
    // Classic `roundMediaTime`: halves away from zero, never -0.
    convert_project_times(&mut project, grouped_tracks, f64::round);
    set_version(project, 28)
}

// ---------------------------------------------------------------------------
// v28 → v29: built-in element fields copied into params
// ---------------------------------------------------------------------------

const TEXT_PARAM_KEYS: [&str; 10] = [
    "content",
    "fontSize",
    "fontFamily",
    "color",
    "textAlign",
    "fontWeight",
    "fontStyle",
    "textDecoration",
    "letterSpacing",
    "lineHeight",
];
const TEXT_BACKGROUND_PARAM_KEYS: [&str; 7] = [
    "enabled",
    "color",
    "cornerRadius",
    "paddingX",
    "paddingY",
    "offsetX",
    "offsetY",
];

pub fn v28_to_v29(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 29);
    for element in grouped_elements(&mut project) {
        let mut params = match element.get("params") {
            Some(Value::Object(params)) => params.clone(),
            _ => Object::new(),
        };
        let mut copy = |source: Option<&Object>, key: &str, param: String| {
            if let Some(value) = source.and_then(|source| source.get(key))
                && (value.is_number() || value.is_string() || value.is_boolean())
            {
                params.insert(param, value.clone());
            }
        };
        let source = Some(&*element);
        let transform = element.get("transform").and_then(Value::as_object);
        let position = transform
            .and_then(|transform| transform.get("position"))
            .and_then(Value::as_object);
        copy(position, "x", "transform.positionX".to_owned());
        copy(position, "y", "transform.positionY".to_owned());
        for key in ["scaleX", "scaleY", "rotate"] {
            copy(transform, key, format!("transform.{key}"));
        }
        copy(source, "opacity", "opacity".to_owned());
        copy(source, "blendMode", "blendMode".to_owned());
        let kind = string(element.get("type"));
        if matches!(kind, Some("audio" | "video")) {
            copy(source, "volume", "volume".to_owned());
            copy(source, "muted", "muted".to_owned());
        }
        if kind == Some("text") {
            for key in TEXT_PARAM_KEYS {
                copy(source, key, key.to_owned());
            }
            let background = element.get("background").and_then(Value::as_object);
            for key in TEXT_BACKGROUND_PARAM_KEYS {
                copy(background, key, format!("background.{key}"));
            }
        }
        element.insert("params".to_owned(), Value::Object(params));
    }
    set_version(project, 29)
}

// ---------------------------------------------------------------------------
// v29 → v30: animations keyed by property path
// ---------------------------------------------------------------------------

pub fn v29_to_v30(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 30);
    for element in grouped_elements(&mut project) {
        let Some(Value::Object(animations)) = element.get_mut("animations") else {
            continue;
        };
        let (Some(Value::Object(bindings)), Some(Value::Object(channels))) =
            (animations.get("bindings"), animations.get("channels"))
        else {
            continue;
        };
        let mut by_path: Vec<(String, Value)> = Vec::new();
        for binding in bindings.values() {
            let (Some(path), Some(Value::Array(components))) =
                (string(binding.get("path")), binding.get("components"))
            else {
                continue;
            };
            let entries: Vec<(&str, Value)> = components
                .iter()
                .filter_map(|component| {
                    let key = string(component.get("key"))?;
                    let channel = channels
                        .get(string(component.get("channelId"))?)?
                        .as_object()?;
                    let mut channel = channel.clone();
                    channel.remove("kind");
                    Some((key, Value::Object(channel)))
                })
                .collect();
            let value = match entries.as_slice() {
                [] => continue,
                [("value", channel)] => channel.clone(),
                _ => Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, channel)| (key.to_owned(), channel))
                        .collect(),
                ),
            };
            by_path.push((path.to_owned(), value));
        }
        animations.extend(by_path);
    }
    set_version(project, 30)
}

// ---------------------------------------------------------------------------
// v30 → v31: custom masks renamed freeform
// ---------------------------------------------------------------------------

pub fn v30_to_v31(mut project: Value) -> MigrationResult {
    skip_unless!(strict_skip, project, 31);
    for element in grouped_elements(&mut project) {
        for mask in masks(element) {
            if is(mask.get("type"), "custom") {
                mask.insert("type".to_owned(), json!("freeform"));
            }
        }
    }
    set_version(project, 31)
}
