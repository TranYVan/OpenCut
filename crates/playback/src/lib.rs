//! Playback logic that needs no audio device: the audio-driven clock, the
//! mixer and the video frame scheduler. The desktop app owns the `cpal`
//! stream and calls [`Player::fill`] from its callback; everything here is
//! plain data so it can be tested (and its drift measured) offline.
//!
//! Classic reference (tag `pre-rewrite`): `playback-manager.ts`,
//! `audio-manager.ts`, `media/audio.ts`.

mod clock;
mod mixer;
mod scheduler;

pub use clock::{AudioClock, Player};
pub use mixer::{CHANNELS, Clip, Mixer, db_to_gain, ticks_to_frames};
pub use scheduler::{FrameScheduler, Tick};
