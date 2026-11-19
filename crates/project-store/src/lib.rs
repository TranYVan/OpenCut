//! OpenCut projects on disk.
//!
//! Every project is a folder named after its id under one root:
//!
//! ```text
//! <root>/<project id>/project.json   the opencut-model v31 document
//! <root>/<project id>/media/         imported media files
//! <root>/<project id>/cache/         derived data that can be rebuilt
//! ```
//!
//! `project.json` is the classic app's storage v31 JSON, so a classic project
//! exported to that shape opens directly. Writes are atomic: a temp file in
//! the same folder, then a rename over the old file, so a crash mid-save
//! leaves the previous version intact.
//!
//! Plain blocking I/O and no UI: the desktop app runs it off its UI thread,
//! and tests run it against a temp dir. Ported from the classic app's
//! `core/managers/project-manager.ts` (tag `pre-rewrite`).

mod classic;
mod error;
mod sort;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use opencut_model::{PROJECT_VERSION, Project, ProjectMetadata};
use serde::Deserialize;

pub use error::StoreError;
pub use sort::{SortKey, SortOrder, sort_projects};

pub use opencut_model as model;

/// The project document inside a project folder.
pub const PROJECT_FILE: &str = "project.json";
/// Imported media inside a project folder.
pub const MEDIA_DIR: &str = "media";
/// Rebuildable derived data (waveforms, …) inside a project folder.
pub const CACHE_DIR: &str = "cache";
/// What a project created from the projects list is called, as in classic.
pub const DEFAULT_PROJECT_NAME: &str = "New project";

/// The projects found by [`ProjectStore::list`].
#[derive(Debug, Default)]
pub struct ProjectList {
    pub projects: Vec<ProjectMetadata>,
    /// Folders that look like projects but couldn't be read, with why.
    /// Folders without a `project.json` aren't projects and aren't listed.
    pub unreadable: Vec<(PathBuf, StoreError)>,
}

/// A folder of projects. Cheap to clone; holds only the root path.
#[derive(Clone, Debug)]
pub struct ProjectStore {
    root: PathBuf,
}

impl ProjectStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store in the OS's per-user app data folder:
    /// `<app data>/OpenCut/projects`.
    pub fn in_app_data() -> Self {
        Self::new(app_data_dir().join("projects"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of project `id`. Fails for ids that aren't a plain file
    /// name, so a hand-edited `project.json` can't point outside the store.
    pub fn project_dir(&self, id: &str) -> Result<PathBuf, StoreError> {
        if !is_valid_id(id) {
            return Err(StoreError::InvalidId(id.to_owned()));
        }
        Ok(self.root.join(id))
    }

    /// Every readable project, unsorted (see [`sort_projects`]). A missing
    /// root is an empty store.
    pub fn list(&self) -> Result<ProjectList, StoreError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ProjectList::default());
            }
            Err(source) => return Err(StoreError::io(&self.root, source)),
        };

        let mut list = ProjectList::default();
        for entry in entries {
            let entry = entry.map_err(|source| StoreError::io(&self.root, source))?;
            let dir = entry.path();
            let file = dir.join(PROJECT_FILE);
            if !dir.is_dir() || !file.is_file() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            match read_header(&file, &dir_name) {
                Ok(metadata) => list.projects.push(metadata),
                Err(error) => list.unreadable.push((dir, error)),
            }
        }
        Ok(list)
    }

    /// Reads and validates project `id`.
    pub fn load(&self, id: &str) -> Result<Project, StoreError> {
        let file = self.project_dir(id)?.join(PROJECT_FILE);
        let bytes = fs::read(&file).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => StoreError::NotFound(id.to_owned()),
            _ => StoreError::io(&file, source),
        })?;
        let project: Project =
            serde_json::from_slice(&bytes).map_err(|source| StoreError::json(&file, source))?;
        check_version(project.version)?;
        if project.metadata.id != id {
            return Err(StoreError::IdMismatch {
                folder: id.to_owned(),
                file: project.metadata.id,
            });
        }
        project.validate().map_err(StoreError::Invalid)?;
        Ok(project)
    }

    /// Writes `project` to its folder (creating it), replacing the old
    /// `project.json` atomically.
    pub fn save(&self, project: &Project) -> Result<(), StoreError> {
        let dir = self.project_dir(&project.metadata.id)?;
        fs::create_dir_all(&dir).map_err(|source| StoreError::io(&dir, source))?;
        let bytes =
            serde_json::to_vec_pretty(project).map_err(|source| StoreError::json(&dir, source))?;
        let file = dir.join(PROJECT_FILE);
        write_atomic(&file, &bytes).map_err(|source| StoreError::io(&file, source))
    }

    /// Creates and saves a new empty project.
    pub fn create(&self, name: &str, now: DateTime<Utc>) -> Result<Project, StoreError> {
        let project = Project::new(clean_name(name)?, now);
        self.save(&project)?;
        Ok(project)
    }

    /// Renames project `id` on disk and bumps its modification time, as
    /// classic did. Returns the new metadata.
    pub fn rename(
        &self,
        id: &str,
        name: &str,
        now: DateTime<Utc>,
    ) -> Result<ProjectMetadata, StoreError> {
        let name = clean_name(name)?;
        let mut project = self.load(id)?;
        project.metadata.name = name;
        project.metadata.updated_at = truncate_to_millis(now);
        self.save(&project)?;
        Ok(project.metadata)
    }

    /// Deletes project `id` and all its media.
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        let dir = self.project_dir(id)?;
        if !dir.join(PROJECT_FILE).is_file() {
            return Err(StoreError::NotFound(id.to_owned()));
        }
        fs::remove_dir_all(&dir).map_err(|source| StoreError::io(&dir, source))
    }
}

/// Writes `bytes` to `path` all-or-nothing: into a temp file next to it,
/// flushed to disk, then renamed over `path`. Readers see the old file or
/// the new one, never a partial write.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy());
    let temp = dir.join(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        // Replaces an existing file on every platform std supports.
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Only the fields the projects list needs; the rest of the file is skipped.
#[derive(Deserialize)]
struct Header {
    metadata: ProjectMetadata,
    version: u32,
}

fn read_header(file: &Path, dir_name: &str) -> Result<ProjectMetadata, StoreError> {
    let bytes = fs::read(file).map_err(|source| StoreError::io(file, source))?;
    let header: Header =
        serde_json::from_slice(&bytes).map_err(|source| StoreError::json(file, source))?;
    check_version(header.version)?;
    if header.metadata.id != dir_name {
        return Err(StoreError::IdMismatch {
            folder: dir_name.to_owned(),
            file: header.metadata.id,
        });
    }
    Ok(header.metadata)
}

/// Only v31 is read; older classic projects come in through
/// [`ProjectStore::import_classic_zip`], which migrates them.
fn check_version(version: u32) -> Result<(), StoreError> {
    if version == PROJECT_VERSION {
        Ok(())
    } else {
        Err(StoreError::UnsupportedVersion(version))
    }
}

fn clean_name(name: &str) -> Result<String, StoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StoreError::EmptyName);
    }
    Ok(name.to_owned())
}

/// What the file stores: `toISOString` has millisecond precision.
fn truncate_to_millis(time: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::SubsecRound;
    time.trunc_subsecs(3)
}

/// The file name of an asset's copy in a project's `media/` folder:
/// `<asset id>.<ext>`, with the original name's extension when it is short
/// and alphanumeric (lowercased), so it's always a safe file name.
pub fn media_file_name(id: &str, original_name: &str) -> String {
    let extension = Path::new(original_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 10
                && extension.chars().all(|c| c.is_ascii_alphanumeric())
        });
    match extension {
        Some(extension) => format!("{id}.{}", extension.to_ascii_lowercase()),
        None => id.to_owned(),
    }
}

/// Ids become folder names: no separators, no `.`/`..`, nothing hidden.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// The OS's per-user app data folder, `OpenCut` inside it.
fn app_data_dir() -> PathBuf {
    let env_dir = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let base = if cfg!(target_os = "macos") {
        env_dir("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else if cfg!(target_os = "windows") {
        env_dir("APPDATA").map(PathBuf::from)
    } else {
        env_dir("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env_dir("HOME").map(|home| PathBuf::from(home).join(".local/share")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("OpenCut")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_must_be_plain_file_names() {
        assert!(is_valid_id("1b4e28ba-2fa1-11d2-883f-0016d3cca427"));
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", ".hidden", "a b"] {
            assert!(!is_valid_id(bad), "{bad:?}");
        }
    }
}
