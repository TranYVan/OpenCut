//! Owners of the FFmpeg objects the audio decoder uses: codec context,
//! packet, frame, and a channel layout.

use std::ptr;

use crate::MediaError;
use crate::ffi;

/// An opened decoder (`AVCodecContext`).
pub(super) struct CodecContext(*mut ffi::AVCodecContext);

// SAFETY: a codec context may move between threads; it's only used through
// `&mut self` on its owner.
unsafe impl Send for CodecContext {}

impl CodecContext {
    /// Opens a decoder for `stream`.
    ///
    /// # Safety
    /// `stream` must be a valid stream of an open format context.
    pub(super) unsafe fn open_decoder(stream: *const ffi::AVStream) -> Result<Self, MediaError> {
        // SAFETY: the caller guarantees stream (and so codecpar) is valid.
        let parameters = unsafe { (*stream).codecpar };
        let codec = unsafe { ffi::avcodec_find_decoder((*parameters).codec_id) };
        if codec.is_null() {
            return Err(MediaError::ffmpeg(
                "avcodec_find_decoder",
                ffi::AVERROR_DECODER_NOT_FOUND,
            ));
        }
        // SAFETY: codec is a registered decoder.
        let context = Self(unsafe { ffi::avcodec_alloc_context3(codec) });
        if context.0.is_null() {
            return Err(MediaError::ffmpeg("avcodec_alloc_context3", -12));
        }
        // SAFETY: context is allocated and parameters valid.
        let code = unsafe { ffi::avcodec_parameters_to_context(context.0, parameters) };
        if code < 0 {
            return Err(MediaError::ffmpeg("avcodec_parameters_to_context", code));
        }
        // SAFETY: as above; the time base lets the decoder interpret
        // timestamps and skip-samples side data.
        unsafe { (*context.0).pkt_timebase = (*stream).time_base };
        let code = unsafe { ffi::avcodec_open2(context.0, codec, ptr::null_mut()) };
        if code < 0 {
            return Err(MediaError::ffmpeg("avcodec_open2", code));
        }
        Ok(context)
    }

    pub(super) fn as_ptr(&self) -> *const ffi::AVCodecContext {
        self.0
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut ffi::AVCodecContext {
        self.0
    }
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        // SAFETY: frees the context (null is fine) and nulls the pointer.
        unsafe { ffi::avcodec_free_context(&mut self.0) };
    }
}

/// An `AVPacket`.
pub(super) struct Packet(*mut ffi::AVPacket);

// SAFETY: as for CodecContext.
unsafe impl Send for Packet {}

impl Packet {
    pub(super) fn new() -> Result<Self, MediaError> {
        // SAFETY: plain allocation.
        let packet = unsafe { ffi::av_packet_alloc() };
        if packet.is_null() {
            return Err(MediaError::ffmpeg("av_packet_alloc", -12));
        }
        Ok(Self(packet))
    }

    pub(super) fn as_ptr(&self) -> *const ffi::AVPacket {
        self.0
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut ffi::AVPacket {
        self.0
    }

    pub(super) fn unref(&mut self) {
        // SAFETY: packet is allocated.
        unsafe { ffi::av_packet_unref(self.0) };
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: frees the packet and its data, and nulls the pointer.
        unsafe { ffi::av_packet_free(&mut self.0) };
    }
}

/// An `AVFrame`.
pub(super) struct Frame(*mut ffi::AVFrame);

// SAFETY: as for CodecContext.
unsafe impl Send for Frame {}

impl Frame {
    pub(super) fn new() -> Result<Self, MediaError> {
        // SAFETY: plain allocation.
        let frame = unsafe { ffi::av_frame_alloc() };
        if frame.is_null() {
            return Err(MediaError::ffmpeg("av_frame_alloc", -12));
        }
        Ok(Self(frame))
    }

    pub(super) fn as_ptr(&self) -> *const ffi::AVFrame {
        self.0
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut ffi::AVFrame {
        self.0
    }

    pub(super) fn unref(&mut self) {
        // SAFETY: frame is allocated.
        unsafe { ffi::av_frame_unref(self.0) };
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: frees the frame and its buffers, and nulls the pointer.
        unsafe { ffi::av_frame_free(&mut self.0) };
    }
}

/// An owned `AVChannelLayout`.
pub(super) struct ChannelLayout(pub(super) ffi::AVChannelLayout);

impl ChannelLayout {
    /// FFmpeg's default layout for `channels` (mono, stereo, 5.1, …).
    pub(super) fn default_for(channels: u16) -> Self {
        let mut layout = ffi::AVChannelLayout::default();
        // SAFETY: writes a native or unspecified layout into `layout`.
        unsafe { ffi::av_channel_layout_default(&mut layout, i32::from(channels)) };
        Self(layout)
    }

    /// A copy of `layout`, with an unspecified order replaced by the
    /// default layout for its channel count (swresample needs to know
    /// which channel is which to remix).
    pub(super) fn copy_of(layout: &ffi::AVChannelLayout) -> Result<Self, MediaError> {
        if layout.order == ffi::AVChannelOrder_AV_CHANNEL_ORDER_UNSPEC || layout.nb_channels <= 0 {
            let channels = u16::try_from(layout.nb_channels.max(1)).unwrap_or(u16::MAX);
            return Ok(Self::default_for(channels));
        }
        let mut copy = ffi::AVChannelLayout::default();
        // SAFETY: copy is zeroed (uninitialized as far as FFmpeg cares),
        // layout is valid.
        let code = unsafe { ffi::av_channel_layout_copy(&mut copy, layout) };
        if code < 0 {
            return Err(MediaError::ffmpeg("av_channel_layout_copy", code));
        }
        Ok(Self(copy))
    }

    pub(super) fn channels(&self) -> u16 {
        u16::try_from(self.0.nb_channels).unwrap_or(0)
    }

    pub(super) fn same_as(&self, other: &ffi::AVChannelLayout) -> bool {
        // SAFETY: both layouts are valid; 0 means equal.
        unsafe { ffi::av_channel_layout_compare(&self.0, other) == 0 }
    }
}

impl Drop for ChannelLayout {
    fn drop(&mut self) {
        // SAFETY: frees a custom map, if any, and zeroes the layout.
        unsafe { ffi::av_channel_layout_uninit(&mut self.0) };
    }
}
