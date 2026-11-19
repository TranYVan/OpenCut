use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use opencut_model::time::{MediaTime, TICKS_PER_SECOND};

use crate::mixer::{CHANNELS, Mixer, ticks_to_frames};

/// The playhead, measured by the audio device: it advances only by the
/// frames the device callback has rendered (minus the device's output
/// latency), so audio and playhead can't drift apart. The UI thread reads
/// it with [`position`](Self::position); the audio thread advances it.
#[derive(Debug)]
pub struct AudioClock {
    sample_rate: u32,
    /// Output frame the current play session started at.
    origin: AtomicU64,
    /// Frames rendered since `origin`.
    rendered: AtomicU64,
    /// Frames between rendering a sample and hearing it.
    latency: AtomicU64,
}

impl AudioClock {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            origin: AtomicU64::new(0),
            rendered: AtomicU64::new(0),
            latency: AtomicU64::new(0),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Starts a session at output frame `origin`.
    pub fn reset(&self, origin: u64) {
        self.origin.store(origin, Ordering::Relaxed);
        self.rendered.store(0, Ordering::Relaxed);
    }

    /// Called by the audio thread after rendering `frames`.
    pub fn advance(&self, frames: u64) {
        self.rendered.fetch_add(frames, Ordering::Relaxed);
    }

    pub fn set_latency(&self, frames: u64) {
        self.latency.store(frames, Ordering::Relaxed);
    }

    /// The output frame being heard right now.
    pub fn position_frames(&self) -> u64 {
        let heard = self
            .rendered
            .load(Ordering::Relaxed)
            .saturating_sub(self.latency.load(Ordering::Relaxed));
        self.origin.load(Ordering::Relaxed) + heard
    }

    pub fn position(&self) -> MediaTime {
        let ticks = (i128::from(self.position_frames()) * i128::from(TICKS_PER_SECOND)
            + i128::from(self.sample_rate) / 2)
            / i128::from(self.sample_rate);
        MediaTime::from_ticks(i64::try_from(ticks).unwrap_or(i64::MAX))
    }
}

/// One play session as the audio callback sees it: a mixer, a cursor and
/// the clock it advances.
pub struct Player {
    mixer: Arc<Mixer>,
    cursor: u64,
    clock: Arc<AudioClock>,
}

impl Player {
    /// Starts at `start`; resets the clock to it.
    pub fn new(mixer: Arc<Mixer>, clock: Arc<AudioClock>, start: MediaTime) -> Self {
        let cursor = ticks_to_frames(start.as_ticks(), clock.sample_rate());
        clock.reset(cursor);
        Self {
            mixer,
            cursor,
            clock,
        }
    }

    /// Renders the next block (interleaved stereo) and advances the clock.
    /// Silent past the end of the mix.
    pub fn fill(&mut self, out: &mut [f32]) {
        self.mixer.mix(self.cursor, out);
        let frames = (out.len() / CHANNELS) as u64;
        self.cursor += frames;
        self.clock.advance(frames);
    }
}
