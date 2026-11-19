//! Importing a zip made by the classic export script
//! (`apps/web/public/classic-export/`): `project.json` (any storage
//! version), `media.json`, `media/<id>/<file>` and `legacy/*.json`.
//!
//! The zip is untrusted input. Its entry names are only used to look entries
//! up, never as paths on disk: media lands at `media/<asset id>.<ext>`, with
//! ids that aren't safe file names replaced. Everything is parsed and
//! validated before the new project folder is created, and the folder is
//! removed again if copying fails.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::Path;

use chrono::{DateTime, Utc};
use opencut_model::migrations::{LegacyData, migrate};
use opencut_model::{MediaAsset, MediaType, Project, new_id};
use serde_json::Value;
use zip::ZipArchive;

use crate::{MEDIA_DIR, ProjectStore, StoreError, is_valid_id, media_file_name};

/// Larger JSON entries are refused rather than read into memory.
const MAX_JSON_BYTES: u64 = 256 * 1024 * 1024;

type Archive = ZipArchive<BufReader<File>>;

impl ProjectStore {
    /// Imports a classic export zip as a new project (a fresh id, so it
    /// never replaces an existing one) and returns it. Media the export
    /// couldn't include is left out; elements using it keep their ids.
    pub fn import_classic_zip(
        &self,
        zip_path: &Path,
        now: DateTime<Utc>,
    ) -> Result<Project, StoreError> {
        let zip_error = |source| StoreError::Zip {
            path: zip_path.to_owned(),
            source,
        };
        let file = File::open(zip_path).map_err(|source| StoreError::io(zip_path, source))?;
        let mut archive = ZipArchive::new(BufReader::new(file)).map_err(zip_error)?;

        let record = read_json(&mut archive, "project.json", zip_path)?
            .ok_or_else(|| StoreError::NotClassicExport(zip_path.to_owned()))?;
        let media = read_json(&mut archive, "media.json", zip_path)?.unwrap_or(Value::Null);
        let items = media["items"].as_array().cloned().unwrap_or_default();

        let mut legacy = LegacyData::default();
        let legacy_entries: Vec<String> = archive
            .file_names()
            .filter_map(Result::ok)
            .filter(|name| name.starts_with("legacy/") && name.ends_with(".json"))
            .map(|name| name.into_owned())
            .collect();
        for name in legacy_entries {
            let Some(dump) = read_json(&mut archive, &name, zip_path)? else {
                continue;
            };
            let (Some(database), Some(entries)) =
                (dump["database"].as_str(), dump["entries"].as_array())
            else {
                continue;
            };
            if let Some(entry) = entries.iter().find(|entry| entry["key"] == "timeline") {
                legacy
                    .timelines
                    .insert(database.to_owned(), entry["value"].clone());
            }
        }
        for item in &items {
            if let (Some(id), Some(kind)) = (item["id"].as_str(), item["metadata"]["type"].as_str())
            {
                legacy.media_types.insert(id.to_owned(), kind.to_owned());
            }
        }

        let mut record = migrate(record, &legacy, now).map_err(StoreError::Migration)?;

        // Media ids become file names: keep the safe ones, replace the rest.
        let mut ids = HashMap::new();
        let mut media = Vec::new();
        for item in &items {
            let (Some(old_id), Some(entry)) = (item["id"].as_str(), item["file"]["path"].as_str())
            else {
                continue; // no file in the zip (missing or unreadable in the browser)
            };
            if ids.contains_key(old_id) || !entry.starts_with("media/") {
                continue;
            }
            let Some(mut asset) = media_asset(item, entry) else {
                continue;
            };
            asset.id = if is_valid_id(old_id) {
                old_id.to_owned()
            } else {
                new_id()
            };
            ids.insert(old_id.to_owned(), asset.id.clone());
            media.push((entry.to_owned(), asset));
        }
        remap_media_ids(&mut record, &ids);

        let mut project: Project =
            serde_json::from_value(record).map_err(|source| StoreError::json(zip_path, source))?;
        project.metadata.id = new_id();
        project.validate().map_err(StoreError::Invalid)?;

        let dir = self.project_dir(&project.metadata.id)?;
        if dir.exists() {
            return Err(StoreError::io(&dir, io::ErrorKind::AlreadyExists.into()));
        }
        let result = (|| {
            let media_dir = dir.join(MEDIA_DIR);
            fs::create_dir_all(&media_dir).map_err(|source| StoreError::io(&media_dir, source))?;
            for (entry, asset) in media {
                let mut source = archive.by_name(&entry).map_err(zip_error)?;
                if source.enclosed_name().is_none() {
                    return Err(StoreError::NotClassicExport(zip_path.to_owned()));
                }
                let destination = media_dir.join(media_file_name(&asset.id, &asset.name));
                let mut file = File::create(&destination)
                    .map_err(|error| StoreError::io(&destination, error))?;
                io::copy(&mut source, &mut file)
                    .map_err(|error| StoreError::io(&destination, error))?;
                project.media_assets.push(asset);
            }
            self.save(&project)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&dir);
        }
        result.map(|()| project)
    }
}

/// A JSON entry, or `None` when the zip has no entry by that name.
fn read_json(
    archive: &mut Archive,
    name: &str,
    zip_path: &Path,
) -> Result<Option<Value>, StoreError> {
    let entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(source) => {
            return Err(StoreError::Zip {
                path: zip_path.to_owned(),
                source,
            });
        }
    };
    let mut bytes = Vec::new();
    entry
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| StoreError::io(zip_path, source))?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(StoreError::NotClassicExport(zip_path.to_owned()));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| StoreError::json(&zip_path.join(name), source))
}

/// The asset for a `media.json` item: its stored classic metadata, or for
/// files exported without metadata, what the file entry says.
fn media_asset(item: &Value, entry: &str) -> Option<MediaAsset> {
    if let Ok(asset) = serde_json::from_value(item["metadata"].clone()) {
        return Some(asset);
    }
    let file = &item["file"];
    let media_type = match file["mimeType"].as_str()?.split('/').next()? {
        "image" => MediaType::Image,
        "video" => MediaType::Video,
        "audio" => MediaType::Audio,
        _ => return None,
    };
    Some(MediaAsset {
        id: String::new(),
        name: entry.rsplit('/').next().unwrap_or(entry).to_owned(),
        media_type,
        size: file["size"].as_u64().unwrap_or(0),
        last_modified: file["lastModified"].as_i64().unwrap_or(0),
        width: None,
        height: None,
        duration: None,
        fps: None,
        has_audio: None,
        ephemeral: None,
        thumbnail_url: None,
    })
}

/// Rewrites every `mediaId` that `ids` renames.
fn remap_media_ids(value: &mut Value, ids: &HashMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if key == "mediaId"
                    && let Some(new_id) = value.as_str().and_then(|id| ids.get(id))
                {
                    *value = Value::String(new_id.clone());
                } else {
                    remap_media_ids(value, ids);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                remap_media_ids(item, ids);
            }
        }
        _ => {}
    }
}
