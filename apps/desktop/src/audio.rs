//! The `cpal` output stream. Thin on purpose: the mixing and the clock live
//! in `opencut-playback`, this only moves their samples to the device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, OutputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig,
};
use opencut_editor::model::time::MediaTime;
use opencut_playback::{AudioClock, CHANNELS, Mixer, Player};

/// The default output device, running for the life of the project window.
/// Silent until [`start`](Self::start) hands it a mix.
pub(crate) struct AudioOutput {
    clock: Arc<AudioClock>,
    player: Arc<Mutex<Option<Player>>>,
    /// Outputs silence while the clock keeps running.
    muted: Arc<AtomicBool>,
    _stream: Stream,
}

impl AudioOutput {
    pub(crate) fn open() -> Result<Self, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no audio output device")?;
        let supported = device
            .default_output_config()
            .map_err(|error| error.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let clock = Arc::new(AudioClock::new(config.sample_rate));
        let player = Arc::new(Mutex::new(None));
        let muted = Arc::new(AtomicBool::new(false));
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, &clock, &player, &muted),
            SampleFormat::I16 => build::<i16>(&device, config, &clock, &player, &muted),
            SampleFormat::U16 => build::<u16>(&device, config, &clock, &player, &muted),
            other => Err(format!("unsupported audio sample format {other}")),
        }?;
        stream.play().map_err(|error| error.to_string())?;
        Ok(Self {
            clock,
            player,
            muted,
            _stream: stream,
        })
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.clock.sample_rate()
    }

    /// The playhead according to the device.
    pub(crate) fn position(&self) -> MediaTime {
        self.clock.position()
    }

    /// Plays `mixer` from `at`, replacing whatever was playing.
    pub(crate) fn start(&self, mixer: Arc<Mixer>, at: MediaTime) {
        let mut player = self.player.lock().unwrap_or_else(|e| e.into_inner());
        *player = Some(Player::new(mixer, self.clock.clone(), at));
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub(crate) fn stop(&self) {
        *self.player.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn build<T>(
    device: &Device,
    config: StreamConfig,
    clock: &Arc<AudioClock>,
    player: &Arc<Mutex<Option<Player>>>,
    muted: &Arc<AtomicBool>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let rate = f64::from(config.sample_rate);
    let (clock, player, muted) = (clock.clone(), player.clone(), muted.clone());
    let mut mix = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &OutputCallbackInfo| {
                mix.resize(data.len() / channels * CHANNELS, 0.0);
                // A lock held by the UI thread (starting or stopping) means
                // silence for this one block, never a wait.
                let filled = match player.try_lock().as_deref_mut() {
                    Ok(Some(player)) => {
                        let stamp = info.timestamp();
                        let ahead = stamp.playback.duration_since(stamp.callback);
                        clock.set_latency((ahead.as_secs_f64() * rate) as u64);
                        player.fill(&mut mix);
                        true
                    }
                    _ => false,
                };
                if !filled || muted.load(Ordering::Relaxed) {
                    mix.fill(0.0);
                }
                for (out, frame) in data.chunks_mut(channels).zip(mix.chunks_exact(CHANNELS)) {
                    // Mono gets the average; more than stereo gets L and R
                    // on the first two channels and silence after.
                    for (index, sample) in out.iter_mut().enumerate() {
                        let value = match (channels, index) {
                            (1, _) => (frame[0] + frame[1]) / 2.0,
                            (_, 0 | 1) => frame[index],
                            _ => 0.0,
                        };
                        *sample = T::from_sample(value);
                    }
                }
            },
            |error| eprintln!("audio output error: {error}"),
            None,
        )
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_device_clock_advances_while_playing() {
        let Ok(output) = AudioOutput::open() else {
            eprintln!("skipped: no audio output device");
            return;
        };
        let start = MediaTime::from_ticks(120_000);
        output.start(Arc::new(Mixer::default()), start);
        std::thread::sleep(Duration::from_millis(500));
        let played = output.position() - start;
        output.stop();
        // Real time, minus the device's start-up latency. The upper bound
        // allows for a loaded CI runner oversleeping; a clock running at
        // double speed would still reach 1.0 s.
        assert!(played > MediaTime::ZERO, "clock did not advance");
        assert!(played.to_seconds_f64() < 0.9, "clock ran fast: {played:?}");
    }
}
