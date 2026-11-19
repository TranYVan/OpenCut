use std::io;
use std::path::{Path, PathBuf};

use opencut_model::{PROJECT_VERSION, ValidationError};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("project {0} not found")]
    NotFound(String),
    #[error("{0:?} is not a valid project id")]
    InvalidId(String),
    #[error("a project name can't be empty")]
    EmptyName,
    #[error("project file version {0} isn't supported (expected {PROJECT_VERSION})")]
    UnsupportedVersion(u32),
    #[error("project folder {folder} holds project {file}")]
    IdMismatch { folder: String, file: String },
    #[error("invalid project: {0}")]
    Invalid(ValidationError),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{}: {source}", path.display())]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl StoreError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }

    pub(crate) fn json(path: &Path, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.to_owned(),
            source,
        }
    }
}
