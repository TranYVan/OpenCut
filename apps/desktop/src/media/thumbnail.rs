//! Thumbnails for the Browser panel.
//!
//! [`ThumbnailProvider`] is the seam between the UI and whatever can make a
//! picture of a media file. The Browser calls it on the background executor
//! and caches the result per asset; `None` means "show the placeholder
//! tile".

use std::path::{Path, PathBuf};

use opencut_editor::model::{MediaAsset, MediaType};
use opencut_media::WaveformCache;
use opencut_media::video::poster_frame;

/// Longest edge, in pixels, of the thumbnails the Browser asks for.
pub(crate) const THUMBNAIL_EDGE: u32 = 320;

/// Bars in an audio asset's waveform tile.
pub(crate) const WAVEFORM_BARS: usize = 48;

/// What the Browser shows on an asset's tile.
pub(crate) enum Thumbnail {
    /// A picture: straight (non-premultiplied) BGRA8, row-major, no
    /// padding. BGRA because that's what GPUI's `RenderImage` draws.
    Picture {
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    },
    /// An audio waveform: one amplitude per bar, `0.0..=1.0`, scaled so the
    /// loudest bar is `1.0`. The UI draws the bars in theme colors, so they
    /// follow light/dark switches without re-decoding.
    Waveform(Vec<f32>),
}

/// Makes thumbnails. Called off the UI thread, possibly for several assets
/// at once, so implementations may block but must be `Send + Sync`.
pub(crate) trait ThumbnailProvider: Send + Sync {
    /// A picture of the media at `path` no larger than `max_edge` on either
    /// side, or `None` when there's nothing to show.
    fn thumbnail(&self, path: &Path, asset: &MediaAsset, max_edge: u32) -> Option<Thumbnail>;
}

/// Thumbnails from `opencut-media`: a poster frame for video and images,
/// a waveform for audio.
pub(crate) struct MediaThumbnails {
    waveforms: WaveformCache,
}

impl MediaThumbnails {
    /// Waveform summaries are kept in `waveform_dir` across launches.
    pub(crate) fn new(waveform_dir: PathBuf) -> Self {
        Self {
            waveforms: WaveformCache::new().with_directory(waveform_dir),
        }
    }

    fn waveform(&self, path: &Path) -> Option<Thumbnail> {
        let summary = self.waveforms.get(path).ok()?;
        let duration = summary.duration();
        if !(duration.is_finite() && duration > 0.0) {
            return None;
        }
        let amplitudes: Vec<f32> = summary
            .peaks_between(0.0, duration, WAVEFORM_BARS)
            .into_iter()
            .map(|peak| peak.amplitude())
            .collect();
        Some(Thumbnail::Waveform(normalized(amplitudes)))
    }
}

impl ThumbnailProvider for MediaThumbnails {
    fn thumbnail(&self, path: &Path, asset: &MediaAsset, max_edge: u32) -> Option<Thumbnail> {
        match asset.media_type {
            // `poster_frame` decodes stills too (FFmpeg's image demuxers),
            // so images and video share one path.
            MediaType::Video | MediaType::Image => {
                let image = poster_frame(path, max_edge, max_edge).ok()?;
                Some(Thumbnail::Picture {
                    width: image.width,
                    height: image.height,
                    bgra: rgba_to_bgra(image.data),
                })
            }
            MediaType::Audio => self.waveform(path),
        }
    }
}

/// Swaps the red and blue channel of every pixel, in place.
fn rgba_to_bgra(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    pixels
}

/// Scales amplitudes so the largest is 1.0 (silence stays all zeros).
fn normalized(mut amplitudes: Vec<f32>) -> Vec<f32> {
    let loudest = amplitudes
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold(0.0_f32, f32::max);
    for value in &mut amplitudes {
        *value = if loudest > 0.0 && value.is_finite() {
            (*value / loudest).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    amplitudes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/media/tests/fixtures")
            .join(name)
    }

    fn provider() -> (MediaThumbnails, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "opencut-desktop-thumbnail-test-{}-{}",
            std::process::id(),
            opencut_editor::model::new_id()
        ));
        (MediaThumbnails::new(dir.clone()), dir)
    }

    /// A picture of `name` fits `max_edge`, keeps the aspect ratio of the
    /// probed source, and is BGRA (the source pixel with red/blue swapped).
    fn assert_picture(name: &str, max_edge: u32) {
        let path = fixture(name);
        let asset = opencut_media::probe_asset(&path, "a".to_owned()).expect("fixture probes");
        let (provider, dir) = provider();
        let Some(Thumbnail::Picture {
            width,
            height,
            bgra,
        }) = provider.thumbnail(&path, &asset, max_edge)
        else {
            panic!("{name}: expected a picture");
        };
        let (source_w, source_h) = (asset.width.expect("width"), asset.height.expect("height"));
        assert_eq!(
            (width, height),
            opencut_media::video::fit_size(source_w, source_h, max_edge, max_edge),
            "{name}: size"
        );
        assert!(width <= max_edge && height <= max_edge);
        assert_eq!(bgra.len(), (width * height * 4) as usize, "{name}: bytes");

        let rgba = poster_frame(&path, max_edge, max_edge).expect("poster frame");
        let (x, y) = (width / 2, height / 2);
        let [r, g, b, a] = rgba.pixel(x, y);
        let offset = ((y * width + x) * 4) as usize;
        assert_eq!(
            &bgra[offset..offset + 4],
            &[b, g, r, a],
            "{name}: BGRA order"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn video_gets_a_bgra_poster_frame_that_fits() {
        assert_picture("video.mp4", THUMBNAIL_EDGE);
        assert_picture("video.mp4", 16);
    }

    #[test]
    fn images_get_a_bgra_picture_that_fits() {
        assert_picture("image.png", THUMBNAIL_EDGE);
        assert_picture("image.jpg", THUMBNAIL_EDGE);
    }

    #[test]
    fn audio_gets_a_normalized_waveform_cached_on_disk() {
        let path = fixture("audio.wav");
        let asset = opencut_media::probe_asset(&path, "a".to_owned()).expect("fixture probes");
        let (provider, dir) = provider();
        let Some(Thumbnail::Waveform(bars)) = provider.thumbnail(&path, &asset, THUMBNAIL_EDGE)
        else {
            panic!("expected a waveform");
        };
        assert_eq!(bars.len(), WAVEFORM_BARS);
        assert!(bars.iter().all(|bar| (0.0..=1.0).contains(bar)));
        assert!(bars.iter().any(|bar| (*bar - 1.0).abs() < f32::EPSILON));
        assert!(
            std::fs::read_dir(&dir).is_ok_and(|mut entries| entries.next().is_some()),
            "summary written to the cache dir"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unreadable_files_get_no_thumbnail() {
        let path = fixture("make-video.swift");
        let (provider, dir) = provider();
        for media_type in [MediaType::Video, MediaType::Image, MediaType::Audio] {
            let mut asset =
                opencut_media::probe_asset(fixture("video.mp4"), "a".to_owned()).expect("probe");
            asset.media_type = media_type;
            assert!(provider.thumbnail(&path, &asset, THUMBNAIL_EDGE).is_none());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn swaps_red_and_blue() {
        assert_eq!(
            rgba_to_bgra(vec![1, 2, 3, 4, 5, 6, 7, 8]),
            [3, 2, 1, 4, 7, 6, 5, 8]
        );
    }

    #[test]
    fn normalizes_to_the_loudest_bar() {
        assert_eq!(normalized(vec![0.25, 0.5, 0.0]), [0.5, 1.0, 0.0]);
        assert_eq!(normalized(vec![0.0, 0.0]), [0.0, 0.0]);
        assert_eq!(normalized(vec![f32::NAN, 0.5]), [0.0, 1.0]);
    }
}
