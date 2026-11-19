use std::fmt;
use std::sync::Arc;

use opencut_model::MediaAsset;
use opencut_time::MediaTime;

/// An 8-bit RGBA image, straight (not premultiplied) alpha, rows packed top
/// to bottom with no padding (`width * 4` bytes each).
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// # Panics
    /// When `data` is not `width * height * 4` bytes.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len(),
            width as usize * height as usize * 4,
            "RGBA data does not match {width}x{height}"
        );
        Self {
            width,
            height,
            data,
        }
    }

    /// An image whose pixel at (`x`, `y`) is `pixel(x, y)`.
    pub fn from_fn(width: u32, height: u32, mut pixel: impl FnMut(u32, u32) -> [u8; 4]) -> Self {
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&pixel(x, y));
            }
        }
        Self::new(width, height, data)
    }

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

/// A decoded source picture handed to the renderer.
#[derive(Clone, Debug)]
pub struct SourceFrame {
    /// Identifies these pixels: the renderer re-uploads a texture only when
    /// the key changes, so it must change whenever the pixels do (e.g.
    /// `"<media id>#<frame index>"`) and must be unique across media.
    pub key: String,
    pub image: Arc<RgbaImage>,
}

/// Where the renderer gets pixels for media elements. The app implements it
/// over `opencut-media` decoders (see `MediaSources` behind the `media`
/// feature); tests implement it with synthetic images.
///
/// Returning `None` leaves the element out of the frame, as the classic
/// renderer did for media it could not load.
pub trait FrameSource {
    /// The frame of video `asset` showing at `source_time` (from the start
    /// of the source, trim and playback rate already applied).
    fn video_frame(&mut self, asset: &MediaAsset, source_time: MediaTime) -> Option<SourceFrame>;

    /// The picture of image `asset`.
    fn image(&mut self, asset: &MediaAsset) -> Option<SourceFrame>;
}
