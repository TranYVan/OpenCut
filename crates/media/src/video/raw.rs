//! Safe owners of the FFmpeg objects video decoding uses, and the
//! YUV→RGBA conversion.

use std::ptr;

use crate::MediaError;
use crate::ffi;

use super::RgbaImage;

/// `AVSEEK_FLAG_BACKWARD`: seek to the keyframe at or before the target.
/// (The `AVSEEK_` macros fall outside the bindgen allowlist.)
pub(super) const AVSEEK_FLAG_BACKWARD: i32 = 1;

/// An owned `AVFrame`.
pub(super) struct Frame(*mut ffi::AVFrame);

// SAFETY: an AVFrame and the refcounted buffers it holds may move between
// threads; `&mut self` guards mutation.
unsafe impl Send for Frame {}

impl Frame {
    pub(super) fn new() -> Result<Self, MediaError> {
        // SAFETY: returns a fresh frame or null.
        let frame = unsafe { ffi::av_frame_alloc() };
        if frame.is_null() {
            return Err(MediaError::ffmpeg("av_frame_alloc", -12));
        }
        Ok(Self(frame))
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut ffi::AVFrame {
        self.0
    }

    pub(super) fn get(&self) -> &ffi::AVFrame {
        // SAFETY: the pointer is valid for self's lifetime.
        unsafe { &*self.0 }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: allocated by av_frame_alloc; unrefs and frees.
        unsafe { ffi::av_frame_free(&mut self.0) };
    }
}

/// An owned `AVPacket`.
pub(super) struct Packet(*mut ffi::AVPacket);

// SAFETY: as for Frame.
unsafe impl Send for Packet {}

impl Packet {
    pub(super) fn new() -> Result<Self, MediaError> {
        // SAFETY: returns a fresh packet or null.
        let packet = unsafe { ffi::av_packet_alloc() };
        if packet.is_null() {
            return Err(MediaError::ffmpeg("av_packet_alloc", -12));
        }
        Ok(Self(packet))
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut ffi::AVPacket {
        self.0
    }

    pub(super) fn unref(&mut self) {
        // SAFETY: the packet is valid; unref of an empty packet is a no-op.
        unsafe { ffi::av_packet_unref(self.0) };
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: allocated by av_packet_alloc.
        unsafe { ffi::av_packet_free(&mut self.0) };
    }
}

/// An owned, opened `AVCodecContext`.
pub(super) struct CodecContext(*mut ffi::AVCodecContext);

// SAFETY: a codec context may move between threads; it is used from one at
// a time (`&mut self`). FFmpeg's own decode threads are internal to it.
unsafe impl Send for CodecContext {}

impl CodecContext {
    /// Opens a decoder for `stream`, multithreaded as FFmpeg sees fit.
    ///
    /// # Safety
    /// `stream` must be a valid stream of an open input.
    pub(super) unsafe fn open_decoder(stream: *const ffi::AVStream) -> Result<Self, MediaError> {
        // SAFETY (whole block): the caller guarantees `stream`; every
        // pointer below is checked or comes from FFmpeg.
        unsafe {
            let parameters = (*stream).codecpar;
            let codec = ffi::avcodec_find_decoder((*parameters).codec_id);
            if codec.is_null() {
                return Err(MediaError::ffmpeg(
                    "avcodec_find_decoder",
                    ffi::AVERROR_DECODER_NOT_FOUND,
                ));
            }
            let context = Self(ffi::avcodec_alloc_context3(codec));
            if context.0.is_null() {
                return Err(MediaError::ffmpeg("avcodec_alloc_context3", -12));
            }
            check(
                "avcodec_parameters_to_context",
                ffi::avcodec_parameters_to_context(context.0, parameters),
            )?;
            (*context.0).pkt_timebase = (*stream).time_base;
            // 0 = one thread per core.
            (*context.0).thread_count = 0;
            (*context.0).thread_type = (ffi::FF_THREAD_FRAME | ffi::FF_THREAD_SLICE) as i32;
            check(
                "avcodec_open2",
                ffi::avcodec_open2(context.0, codec, ptr::null_mut()),
            )?;
            Ok(context)
        }
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut ffi::AVCodecContext {
        self.0
    }

    pub(super) fn codec_name(&self) -> String {
        // SAFETY: an open context has a codec with a static name.
        unsafe {
            std::ffi::CStr::from_ptr((*(*self.0).codec).name)
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        // SAFETY: from avcodec_alloc_context3; closes and frees.
        unsafe { ffi::avcodec_free_context(&mut self.0) };
    }
}

pub(super) fn check(operation: &'static str, code: i32) -> Result<i32, MediaError> {
    if code < 0 {
        Err(MediaError::ffmpeg(operation, code))
    } else {
        Ok(code)
    }
}

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
