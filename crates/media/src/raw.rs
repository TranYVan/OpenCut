//! Safe owners of the FFmpeg objects every decoder uses: codec context,
//! packet and frame, plus FFmpeg error-code checking. Media-specific
//! wrappers live with their media (`ChannelLayout` in `audio`, `Scaler` in
//! `video`).

use std::ptr;

use crate::MediaError;
use crate::ffi;

/// `Err` for FFmpeg's negative error codes, the code itself otherwise.
pub(crate) fn check(operation: &'static str, code: i32) -> Result<i32, MediaError> {
    if code < 0 {
        Err(MediaError::ffmpeg(operation, code))
    } else {
        Ok(code)
    }
}

/// How many threads a decoder may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Threads {
    /// FFmpeg's default: decode on the caller's thread.
    Single,
    /// Frame and slice threads, one per core.
    PerCore,
}

/// An owned, opened `AVCodecContext`.
pub(crate) struct CodecContext(*mut ffi::AVCodecContext);

// SAFETY: a codec context may move between threads; it is used from one at
// a time (`&mut self`). FFmpeg's own decode threads are internal to it.
unsafe impl Send for CodecContext {}

impl CodecContext {
    /// Opens a decoder for `stream`.
    ///
    /// # Safety
    /// `stream` must be a valid stream of an open input.
    pub(crate) unsafe fn open_decoder(
        stream: *const ffi::AVStream,
        threads: Threads,
    ) -> Result<Self, MediaError> {
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
            // The time base lets the decoder interpret timestamps and
            // skip-samples side data.
            (*context.0).pkt_timebase = (*stream).time_base;
            if threads == Threads::PerCore {
                // 0 = one thread per core.
                (*context.0).thread_count = 0;
                (*context.0).thread_type = (ffi::FF_THREAD_FRAME | ffi::FF_THREAD_SLICE) as i32;
            }
            check(
                "avcodec_open2",
                ffi::avcodec_open2(context.0, codec, ptr::null_mut()),
            )?;
            Ok(context)
        }
    }

    /// A fresh context for `codec`, to configure and open (encoders).
    pub(crate) fn alloc(codec: *const ffi::AVCodec) -> Result<Self, MediaError> {
        // SAFETY: returns a context with defaults for `codec`, or null.
        let context = unsafe { ffi::avcodec_alloc_context3(codec) };
        if context.is_null() {
            return Err(MediaError::ffmpeg("avcodec_alloc_context3", -12));
        }
        Ok(Self(context))
    }

    pub(crate) fn as_ptr(&self) -> *const ffi::AVCodecContext {
        self.0
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut ffi::AVCodecContext {
        self.0
    }

    /// The decoder's short name, e.g. `h264`.
    pub(crate) fn codec_name(&self) -> String {
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
        // SAFETY: from avcodec_alloc_context3; closes, frees and nulls the
        // pointer (null is fine).
        unsafe { ffi::avcodec_free_context(&mut self.0) };
    }
}

/// An owned `AVPacket`.
pub(crate) struct Packet(*mut ffi::AVPacket);

// SAFETY: as for CodecContext.
unsafe impl Send for Packet {}

impl Packet {
    pub(crate) fn new() -> Result<Self, MediaError> {
        // SAFETY: returns a fresh packet or null.
        let packet = unsafe { ffi::av_packet_alloc() };
        if packet.is_null() {
            return Err(MediaError::ffmpeg("av_packet_alloc", -12));
        }
        Ok(Self(packet))
    }

    pub(crate) fn as_ptr(&self) -> *const ffi::AVPacket {
        self.0
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut ffi::AVPacket {
        self.0
    }

    pub(crate) fn unref(&mut self) {
        // SAFETY: the packet is valid; unref of an empty packet is a no-op.
        unsafe { ffi::av_packet_unref(self.0) };
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: allocated by av_packet_alloc; frees its data too and
        // nulls the pointer.
        unsafe { ffi::av_packet_free(&mut self.0) };
    }
}

/// An owned `AVFrame`.
pub(crate) struct Frame(*mut ffi::AVFrame);

// SAFETY: an AVFrame and the refcounted buffers it holds may move between
// threads; `&mut self` guards mutation.
unsafe impl Send for Frame {}

impl Frame {
    pub(crate) fn new() -> Result<Self, MediaError> {
        // SAFETY: returns a fresh frame or null.
        let frame = unsafe { ffi::av_frame_alloc() };
        if frame.is_null() {
            return Err(MediaError::ffmpeg("av_frame_alloc", -12));
        }
        Ok(Self(frame))
    }

    pub(crate) fn as_ptr(&self) -> *const ffi::AVFrame {
        self.0
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut ffi::AVFrame {
        self.0
    }

    pub(crate) fn get(&self) -> &ffi::AVFrame {
        // SAFETY: the pointer is valid for self's lifetime.
        unsafe { &*self.0 }
    }

    pub(crate) fn unref(&mut self) {
        // SAFETY: the frame is allocated.
        unsafe { ffi::av_frame_unref(self.0) };
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: allocated by av_frame_alloc; unrefs, frees and nulls the
        // pointer.
        unsafe { ffi::av_frame_free(&mut self.0) };
    }
}
