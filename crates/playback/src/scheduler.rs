use opencut_model::time::{FrameRate, MediaTime};

/// A video frame that should be on screen now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    pub frame: i64,
    /// Frames skipped since the last one shown because the caller polled
    /// late: they are dropped, never shown late.
    pub dropped: u64,
}

/// Picks the video frame for the clock's position. The frame is a pure
/// function of the clock (`floor(position * fps)`), so a slow poll skips
/// frames instead of falling behind the audio.
#[derive(Debug, Default)]
pub struct FrameScheduler {
    last: Option<i64>,
}

impl FrameScheduler {
    /// `Some` when the frame at `position` differs from the last one returned.
    pub fn poll(&mut self, position: MediaTime, rate: FrameRate) -> Option<Tick> {
        let frame = position.to_frame_floor(rate)?;
        if self.last == Some(frame) {
            return None;
        }
        let dropped = self
            .last
            .map_or(0, |last| u64::try_from(frame - last - 1).unwrap_or(0));
        self.last = Some(frame);
        Some(Tick { frame, dropped })
    }

    /// Forget the last frame (after a seek) so the next poll always shows.
    pub fn reset(&mut self) {
        self.last = None;
    }
}
