use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use opencut_media::MediaError;
use opencut_media::video::VideoDecoder;
use opencut_model::MediaAsset;
use opencut_time::MediaTime;

use crate::source::{FrameSource, RgbaImage, SourceFrame};

/// A [`FrameSource`] decoding with `opencut-media`: one [`VideoDecoder`] per
/// video asset (kept open so playback decodes forward instead of seeking),
/// and each image decoded once.
///
/// `resolve_path` maps an asset to its file; assets it returns `None` for
/// are left out of the frame. Decode failures also leave the element out;
/// the latest one is kept in [`Self::last_error`].
pub struct MediaSources<F> {
    resolve_path: F,
    decoders: HashMap<String, VideoDecoder>,
    images: HashMap<String, Arc<RgbaImage>>,
    last_error: Option<MediaError>,
}

impl<F: FnMut(&MediaAsset) -> Option<PathBuf>> MediaSources<F> {
    pub fn new(resolve_path: F) -> Self {
        Self {
            resolve_path,
            decoders: HashMap::new(),
            images: HashMap::new(),
            last_error: None,
        }
    }

    /// The most recent decode error, if any.
    pub fn last_error(&self) -> Option<&MediaError> {
        self.last_error.as_ref()
    }

    /// Closes the decoder and drops the cached image of `asset_id`.
    pub fn release(&mut self, asset_id: &str) {
        self.decoders.remove(asset_id);
        self.images.remove(asset_id);
    }

    fn decoder(&mut self, asset: &MediaAsset) -> Option<&mut VideoDecoder> {
        if !self.decoders.contains_key(&asset.id) {
            let path = (self.resolve_path)(asset)?;
            let decoder = self.record(VideoDecoder::open(path))?;
            self.decoders.insert(asset.id.clone(), decoder);
        }
        self.decoders.get_mut(&asset.id)
    }

    fn record<T>(&mut self, result: Result<T, MediaError>) -> Option<T> {
        result.map_err(|error| self.last_error = Some(error)).ok()
    }
}

impl<F: FnMut(&MediaAsset) -> Option<PathBuf>> FrameSource for MediaSources<F> {
    fn video_frame(&mut self, asset: &MediaAsset, source_time: MediaTime) -> Option<SourceFrame> {
        let result = self.decoder(asset)?.frame_at(source_time);
        let frame = self.record(result)??;
        Some(SourceFrame {
            key: format!("{}#{}", asset.id, frame.index),
            image: Arc::new(convert(frame.image)),
        })
    }

    fn image(&mut self, asset: &MediaAsset) -> Option<SourceFrame> {
        if !self.images.contains_key(&asset.id) {
            let path = (self.resolve_path)(asset)?;
            let result =
                VideoDecoder::open(path).and_then(|mut decoder| decoder.frame_at(MediaTime::ZERO));
            let frame = self.record(result)??;
            self.images
                .insert(asset.id.clone(), Arc::new(convert(frame.image)));
        }
        Some(SourceFrame {
            key: asset.id.clone(),
            image: Arc::clone(self.images.get(&asset.id)?),
        })
    }
}

fn convert(image: opencut_media::video::RgbaImage) -> RgbaImage {
    RgbaImage::new(image.width, image.height, image.data)
}
