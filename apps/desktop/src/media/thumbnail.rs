//! Thumbnails for the Browser panel.
//!
//! [`ThumbnailProvider`] is the seam between the UI and whatever can make a
//! picture of a media file. The Browser calls it on the background executor
//! and caches the result per asset; `None` means "show the placeholder
//! tile".

use std::path::Path;

use opencut_editor::model::{MediaAsset, MediaType};

/// Longest edge, in pixels, of the thumbnails the Browser asks for.
pub(crate) const THUMBNAIL_EDGE: u32 = 320;

/// A decoded thumbnail: straight (non-premultiplied) RGBA8, row-major.
pub(crate) struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Makes thumbnails. Called off the UI thread, possibly for several assets
/// at once, so implementations may block but must be `Send + Sync`.
pub(crate) trait ThumbnailProvider: Send + Sync {
    /// A picture of the media at `path` no larger than `max_edge` on either
    /// side, or `None` when there's nothing to show.
    fn thumbnail(&self, path: &Path, asset: &MediaAsset, max_edge: u32) -> Option<Thumbnail>;
}

/// Until `opencut-media` can decode: images are read directly, video and
/// audio get the placeholder tile.
pub(crate) struct PlaceholderThumbnails;

impl ThumbnailProvider for PlaceholderThumbnails {
    fn thumbnail(&self, path: &Path, asset: &MediaAsset, max_edge: u32) -> Option<Thumbnail> {
        match asset.media_type {
            MediaType::Image => image_thumbnail(path, max_edge),
            // TODO(#3): use opencut-media's poster frame (frame decoding and
            // thumbnails) for video once it merges.
            MediaType::Video => None,
            // TODO(#4): draw opencut-media's waveform summary for audio.
            MediaType::Audio => None,
        }
    }
}

/// Decodes an image file and scales it to fit `max_edge`. Formats the
/// `image` crate can't read (e.g. HEIC) get no thumbnail.
fn image_thumbnail(path: &Path, max_edge: u32) -> Option<Thumbnail> {
    let image = image::open(path).ok()?;
    let scaled = if image.width() > max_edge || image.height() > max_edge {
        image.thumbnail(max_edge, max_edge)
    } else {
        image
    };
    let rgba = scaled.into_rgba8();
    Some(Thumbnail {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}
