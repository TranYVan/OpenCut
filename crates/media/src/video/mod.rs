//! Video decoding: frames at a source time as RGBA, a memory-bounded frame
//! cache, poster frames and thumbnail strips, and a worker thread that does
//! all of it off the UI thread.
//!
//! - [`VideoDecoder`] seeks to a [`MediaTime`] (keyframe seek, then decode
//!   forward) or plays frames in order.
//! - [`FrameCache`] is an LRU of decoded frames keyed by media id and frame
//!   index, bounded by bytes.
//! - [`poster_frame`] and [`thumbnail_strip`] make thumbnails for the media
//!   browser and timeline clips.
//! - [`DecodeWorker`] runs requests for any of these on its own thread.

mod cache;
mod decoder;
mod raw;
#[cfg(test)]
mod test_clip;
#[cfg(test)]
mod tests;
mod thumbnail;
mod worker;

use std::fmt;

use opencut_time::MediaTime;

pub use cache::{FrameCache, SharedFrameCache};
pub use decoder::{VideoDecoder, VideoInfo};
pub use thumbnail::{POSTER_TIME, Thumbnail, fit_size, poster_frame, thumbnail_strip};
pub use worker::{DecodeOutput, DecodeResponse, DecodeWorker, RequestId};

/// An 8-bit RGBA image, straight (not premultiplied) alpha, rows packed
/// top to bottom with no padding (`width * 4` bytes each).
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// The pixel at (`x`, `y`) as `[r, g, b, a]`.
    ///
    /// # Panics
    /// When the point is outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is outside the image"
        );
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        let mut pixel = [0; 4];
        pixel.copy_from_slice(&self.data[offset..offset + 4]);
        pixel
    }

    /// Bytes of pixel data.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }
}

impl fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// A decoded video frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    /// Index of the frame in its source: its time at the stream's frame
    /// rate, rounded. The first frame is 0.
    pub index: i64,
    /// When the frame starts showing, from the start of the source.
    pub time: MediaTime,
    /// How long it shows.
    pub duration: MediaTime,
    pub image: RgbaImage,
}
