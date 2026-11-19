//! YUV→RGBA conversion for video decoding. The codec context, packet and
//! frame are in `crate::raw`.

use std::ptr;

use crate::MediaError;
use crate::ffi;
use crate::raw::check;

use super::RgbaImage;

/// What a `SwsContext` was made for; a frame that differs needs a new one.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ScalerKey {
    width: i32,
    height: i32,
    format: i32,
    colorspace: ffi::AVColorSpace,
    full_range: bool,
    out_width: u32,
    out_height: u32,
}

/// Converts decoded frames to RGBA, scaling on the way when asked to.
pub(super) struct Scaler {
    context: *mut ffi::SwsContext,
    key: Option<ScalerKey>,
}

// SAFETY: a SwsContext may move between threads; used via `&mut self`.
unsafe impl Send for Scaler {}

impl Scaler {
    pub(super) fn new() -> Self {
        Self {
            context: ptr::null_mut(),
            key: None,
        }
    }

    /// `frame` as straight RGBA8 at `out_width`×`out_height`.
    pub(super) fn convert(
        &mut self,
        frame: &ffi::AVFrame,
        out_width: u32,
        out_height: u32,
    ) -> Result<RgbaImage, MediaError> {
        let key = ScalerKey {
            width: frame.width,
            height: frame.height,
            format: frame.format,
            colorspace: frame.colorspace,
            full_range: is_full_range(frame),
            out_width,
            out_height,
        };
        if self.key != Some(key) {
            self.rebuild(key)?;
        }

        let stride = out_width as usize * 4;
        let length = stride * out_height as usize;
        // Slack past the end: some SIMD paths write a little beyond the last
        // pixel of a row when the stride isn't aligned.
        let mut data = vec![0u8; length + 64];
        let destination = [
            data.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ];
        let strides = [stride as i32, 0, 0, 0];
        // SAFETY: the context matches the frame (key above); the frame's
        // planes are valid for its height; `data` holds out_height rows of
        // `stride` bytes.
        let rows = unsafe {
            ffi::sws_scale(
                self.context,
                frame.data.as_ptr().cast(),
                frame.linesize.as_ptr(),
                0,
                frame.height,
                destination.as_ptr(),
                strides.as_ptr(),
            )
        };
        check("sws_scale", rows)?;
        data.truncate(length);
        Ok(RgbaImage {
            width: out_width,
            height: out_height,
            data,
        })
    }

    fn rebuild(&mut self, key: ScalerKey) -> Result<(), MediaError> {
        self.free();
        let downscale = (key.out_width as i32) < key.width || (key.out_height as i32) < key.height;
        let flags = if downscale {
            ffi::SwsFlags_SWS_AREA
        } else {
            ffi::SwsFlags_SWS_BILINEAR
        };
        // SAFETY: plain values; returns a context or null.
        self.context = unsafe {
            ffi::sws_getContext(
                key.width,
                key.height,
                key.format,
                key.out_width as i32,
                key.out_height as i32,
                ffi::AVPixelFormat_AV_PIX_FMT_RGBA,
                flags as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if self.context.is_null() {
            return Err(MediaError::ffmpeg(
                "sws_getContext",
                ffi::AVERROR_INVALIDDATA,
            ));
        }

        // Untagged video: HD and up is almost always BT.709, SD BT.601.
        let colorspace = match key.colorspace {
            ffi::AVColorSpace_AVCOL_SPC_UNSPECIFIED | ffi::AVColorSpace_AVCOL_SPC_RESERVED => {
                if key.height >= 720 {
                    ffi::SWS_CS_ITU709 as i32
                } else {
                    ffi::SWS_CS_DEFAULT as i32
                }
            }
            // sws_getCoefficients is indexed by AVColorSpace and falls back
            // to the default for values it doesn't know.
            other => other as i32,
        };
        // SAFETY: the context is valid; the coefficient tables are static.
        // Fails harmlessly (and is ignored) for RGB sources.
        unsafe {
            ffi::sws_setColorspaceDetails(
                self.context,
                ffi::sws_getCoefficients(colorspace),
                i32::from(key.full_range),
                ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT as i32),
                1,
                0,
                1 << 16,
                1 << 16,
            );
        }
        self.key = Some(key);
        Ok(())
    }

    fn free(&mut self) {
        if !self.context.is_null() {
            // SAFETY: from sws_getContext.
            unsafe { ffi::sws_freeContext(self.context) };
            self.context = ptr::null_mut();
        }
        self.key = None;
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        self.free();
    }
}

/// Full ("JPEG") range: tagged so, or one of the deprecated `yuvj` formats
/// MJPEG and some H.264 streams still decode to.
fn is_full_range(frame: &ffi::AVFrame) -> bool {
    frame.color_range == ffi::AVColorRange_AVCOL_RANGE_JPEG
        || matches!(
            frame.format,
            ffi::AVPixelFormat_AV_PIX_FMT_YUVJ420P
                | ffi::AVPixelFormat_AV_PIX_FMT_YUVJ422P
                | ffi::AVPixelFormat_AV_PIX_FMT_YUVJ444P
                | ffi::AVPixelFormat_AV_PIX_FMT_YUVJ440P
                | ffi::AVPixelFormat_AV_PIX_FMT_YUVJ411P
        )
}
