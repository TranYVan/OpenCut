//! An owned `AVChannelLayout`, for the audio decoder and resampler. The
//! codec context, packet and frame are in `crate::raw`.

use crate::MediaError;
use crate::ffi;

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
