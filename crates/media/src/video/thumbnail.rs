//! Thumbnails: a poster frame for the media browser (classic
//! `media/thumbnail.ts` and `readVideoFile`) and evenly spaced frames for
//! timeline clips.

use std::path::Path;

use opencut_time::{MediaTime, TICKS_PER_SECOND};

use crate::MediaError;

use super::{RgbaImage, VideoDecoder};

/// Where the poster frame comes from: 1 s in, as in the classic app (or
/// halfway through anything shorter than 2 s).
pub const POSTER_TIME: MediaTime = MediaTime::from_ticks(TICKS_PER_SECOND);

/// One frame of a thumbnail strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    /// The source time it was taken at.
    pub time: MediaTime,
    pub image: RgbaImage,
}

/// `width`×`height` shrunk, keeping its aspect ratio, to fit within
/// `max_width`×`max_height`. Never enlarged, never below 1×1.
pub fn fit_size(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1, 1);
    }
    let (width_f, height_f) = (f64::from(width), f64::from(height));
    let scale = (f64::from(max_width) / width_f)
        .min(f64::from(max_height) / height_f)
        .min(1.0);
    if scale >= 1.0 {
        return (width, height);
    }
    let fitted = |side: f64| ((side * scale).round() as u32).max(1);
    (fitted(width_f), fitted(height_f))
}

/// A poster frame for `path` (video or still image), fitted within
/// `max_width`×`max_height`. The classic app used 1280×720.
pub fn poster_frame(
    path: impl AsRef<Path>,
    max_width: u32,
    max_height: u32,
) -> Result<RgbaImage, MediaError> {
    let path = path.as_ref();
    let mut decoder = VideoDecoder::open(path)?;
    let info = decoder.info().clone();
    decoder.set_output_size(Some(fit_size(
        info.display_width,
        info.display_height,
        max_width,
        max_height,
    )));
    let time = match info.duration {
        Some(duration) if duration < POSTER_TIME * 2 => duration / 2,
        _ => POSTER_TIME,
    };
    decoder
        .frame_at(time)?
        .map(|frame| frame.image)
        .ok_or_else(|| MediaError::NoVideo {
            path: path.to_owned(),
        })
}

/// `count` frames spread evenly over `path`, each from the middle of its
/// slice of the duration, `tile_height` pixels tall (or the video's own
/// height if smaller) — the tiles of a timeline clip. A file with no known
/// duration gives its first frame `count` times.
pub fn thumbnail_strip(
    path: impl AsRef<Path>,
    count: usize,
    tile_height: u32,
) -> Result<Vec<Thumbnail>, MediaError> {
    let path = path.as_ref();
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut decoder = VideoDecoder::open(path)?;
    let info = decoder.info().clone();
    decoder.set_output_size(Some(fit_size(
        info.display_width,
        info.display_height,
        u32::MAX,
        tile_height,
    )));
    let duration = info.duration.unwrap_or(MediaTime::ZERO);
    let slots = i64::try_from(count).unwrap_or(i64::MAX);

    let mut strip = Vec::with_capacity(count);
    for slot in 0..slots {
        // Middle of slot `slot` of `slots`: duration * (2 slot + 1) / 2 slots.
        let ticks =
            i128::from(duration.as_ticks()) * i128::from(2 * slot + 1) / i128::from(2 * slots);
        let time = MediaTime::from_ticks(i64::try_from(ticks).unwrap_or(i64::MAX));
        let frame = decoder.frame_at(time)?.ok_or_else(|| MediaError::NoVideo {
            path: path.to_owned(),
        })?;
        strip.push(Thumbnail {
            time,
            image: frame.image,
        });
    }
    Ok(strip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_shrinks_keeping_aspect() {
        assert_eq!(fit_size(1920, 1080, 1280, 720), (1280, 720));
        assert_eq!(fit_size(1080, 1920, 1280, 720), (405, 720));
        assert_eq!(fit_size(4000, 1000, 1280, 720), (1280, 320));
        assert_eq!(fit_size(64, 48, 1280, 720), (64, 48));
        assert_eq!(fit_size(1920, 1080, u32::MAX, 54), (96, 54));
        assert_eq!(fit_size(10_000, 1, 100, 100), (100, 1));
        assert_eq!(fit_size(0, 0, 100, 100), (1, 1));
    }
}
