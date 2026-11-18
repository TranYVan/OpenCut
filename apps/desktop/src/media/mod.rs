//! Media import and thumbnails for the desktop app. The project document
//! and its edits live in `opencut-editor`; this is the file-system side.

mod import;
mod thumbnail;

pub(crate) use import::{ProjectFiles, import_file, import_image_bytes, paths_in_text};
pub(crate) use thumbnail::{PlaceholderThumbnails, THUMBNAIL_EDGE, Thumbnail, ThumbnailProvider};
