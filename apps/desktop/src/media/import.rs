//! Bringing files into a project: where they are kept, and turning a file
//! into a [`MediaAsset`]. Blocking I/O; run it off the UI thread.

use std::fs;
use std::path::{Path, PathBuf};

use opencut_editor::model::{MediaAsset, new_id};

/// Where a project's files live on disk: `<app data>/OpenCut/projects/<id>`,
/// with imported media in `media/` and derived data (waveform summaries)
/// in `cache/`.
///
/// TODO(#15): projects aren't saved or reopened yet, so every launch starts
/// a new project folder. Saving and loading should take this over.
#[derive(Clone, Debug)]
pub(crate) struct ProjectFiles {
    root: PathBuf,
}

impl ProjectFiles {
    pub(crate) fn for_project(project_id: &str) -> Self {
        Self {
            root: app_data_dir().join("projects").join(project_id),
        }
    }

    pub(crate) fn media_dir(&self) -> PathBuf {
        self.root.join("media")
    }

    /// Data derived from the media that can be rebuilt, e.g. waveforms.
    pub(crate) fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// The project's copy of an asset's file: `media/<asset id>.<ext>`,
    /// with the extension of the original file name.
    pub(crate) fn media_path(&self, asset: &MediaAsset) -> PathBuf {
        self.media_dir().join(file_name_for(
            &asset.id,
            extension_of(&asset.name).as_deref(),
        ))
    }
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

/// A file's extension, lowercased; `None` unless it is short and
/// alphanumeric, so it's safe to put in a file name.
fn extension_of(name: &str) -> Option<String> {
    let extension = Path::new(name).extension()?.to_str()?;
    (!extension.is_empty()
        && extension.len() <= 10
        && extension.chars().all(|c| c.is_ascii_alphanumeric()))
    .then(|| extension.to_ascii_lowercase())
}

fn file_name_for(id: &str, extension: Option<&str>) -> String {
    match extension {
        Some(extension) => format!("{id}.{extension}"),
        None => id.to_owned(),
    }
}

/// Probes `source` and copies it into the project's media folder. The asset
/// keeps the original file's name, size and modification time.
pub(crate) fn import_file(source: &Path, files: &ProjectFiles) -> Result<MediaAsset, String> {
    let display_name = source.file_name().map_or_else(
        || source.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let fail = |reason: String| format!("{display_name}: {reason}");

    let asset =
        opencut_media::probe_asset(source, new_id()).map_err(|error| fail(error.to_string()))?;
    let destination = files.media_path(&asset);
    fs::create_dir_all(files.media_dir()).map_err(|error| fail(error.to_string()))?;
    if let Err(error) = fs::copy(source, &destination) {
        let _ = fs::remove_file(&destination);
        return Err(fail(format!("can't copy into the project: {error}")));
    }
    Ok(asset)
}

/// Saves pasted image bytes into the project's media folder as
/// `Pasted image.<extension>` and probes them.
pub(crate) fn import_image_bytes(
    bytes: &[u8],
    extension: &str,
    files: &ProjectFiles,
) -> Result<MediaAsset, String> {
    let name = format!("Pasted image.{extension}");
    let fail = |reason: String| format!("{name}: {reason}");

    let id = new_id();
    let destination = files.media_dir().join(file_name_for(&id, Some(extension)));
    fs::create_dir_all(files.media_dir()).map_err(|error| fail(error.to_string()))?;
    fs::write(&destination, bytes).map_err(|error| fail(error.to_string()))?;
    match opencut_media::probe_asset(&destination, id) {
        Ok(mut asset) => {
            asset.name = name;
            Ok(asset)
        }
        Err(error) => {
            let _ = fs::remove_file(&destination);
            Err(fail(error.to_string()))
        }
    }
}

/// File paths in pasted text: one per line, plain or as `file://` URLs
/// (what file managers put on the clipboard as text). Doesn't check that
/// they exist.
pub(crate) fn paths_in_text(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(|line| line.trim().trim_matches('"').trim_matches('\''))
        .filter(|line| !line.is_empty())
        .filter_map(|line| match line.strip_prefix("file://") {
            Some(url) => path_from_file_url(url),
            None => Some(PathBuf::from(line)),
        })
        .filter(|path| path.is_absolute())
        .collect()
}

/// `url` is a `file://` URL without its scheme: `/home/a%20b.mp4`,
/// `localhost/…`, or `/C:/…` on Windows.
fn path_from_file_url(url: &str) -> Option<PathBuf> {
    let path = url.strip_prefix("localhost").unwrap_or(url);
    let decoded = percent_decode(path)?;
    let bytes = decoded.as_bytes();
    // `/C:/x` → `C:/x`.
    let decoded = if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[2] == b':'
        && bytes[1].is_ascii_alphabetic()
    {
        decoded[1..].to_owned()
    } else {
        decoded
    };
    Some(PathBuf::from(decoded))
}

fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = tail.get(..2)?;
            bytes.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            rest = &tail[2..];
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_paths_use_the_asset_id_and_a_safe_extension() {
        let files = ProjectFiles {
            root: PathBuf::from("/projects/p"),
        };
        let mut asset = MediaAsset {
            id: "abc".to_owned(),
            name: "Beach Day.MP4".to_owned(),
            media_type: opencut_editor::model::MediaType::Video,
            size: 0,
            last_modified: 0,
            width: None,
            height: None,
            duration: None,
            fps: None,
            has_audio: None,
            ephemeral: None,
            thumbnail_url: None,
        };
        assert_eq!(
            files.media_path(&asset),
            PathBuf::from("/projects/p/media/abc.mp4")
        );
        asset.name = "no extension".to_owned();
        assert_eq!(
            files.media_path(&asset),
            PathBuf::from("/projects/p/media/abc")
        );
        asset.name = "weird.a/b".to_owned();
        assert_eq!(
            files.media_path(&asset),
            PathBuf::from("/projects/p/media/abc")
        );
    }

    #[cfg(unix)]
    #[test]
    fn reads_plain_paths_and_file_urls_from_text() {
        let text = "/tmp/a.mp4\n  file:///tmp/b%20c.png \nrelative.wav\n\n\"/tmp/d.mp3\"\nfile://localhost/tmp/e.mov";
        assert_eq!(
            paths_in_text(text),
            [
                PathBuf::from("/tmp/a.mp4"),
                PathBuf::from("/tmp/b c.png"),
                PathBuf::from("/tmp/d.mp3"),
                PathBuf::from("/tmp/e.mov"),
            ]
        );
        assert!(paths_in_text("hello world").is_empty());
        assert!(paths_in_text("file:///bad%zz").is_empty());
    }

    #[test]
    fn windows_file_urls_drop_the_leading_slash() {
        assert_eq!(
            path_from_file_url("/C:/Users/me/a%20b.mp4"),
            Some(PathBuf::from("C:/Users/me/a b.mp4"))
        );
    }
}
