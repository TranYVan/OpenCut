//! Real-time playback of the open project: the audio device's clock drives
//! the playhead, and video frames follow the clock (a late frame is dropped,
//! never shown late). The headless pieces live in `opencut-playback`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Action, Context, Task};
use opencut_editor::model::time::MediaTime;
use opencut_media::audio::{AudioFormat, decode_audio};
use opencut_playback::{FrameScheduler, Mixer};

use super::Workspace;
use crate::audio::AudioOutput;

/// How often the playhead checks the clock: well under a frame at 60 fps.
const TICK: Duration = Duration::from_millis(8);
/// Classic `seek-forward` / `jump-forward` distances.
const SEEK: MediaTime = MediaTime::from_ticks(opencut_editor::model::time::TICKS_PER_SECOND);
const JUMP: MediaTime = MediaTime::from_ticks(5 * opencut_editor::model::time::TICKS_PER_SECOND);

/// The playback actions of the classic app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Action)]
#[action(namespace = playback, no_json)]
pub(crate) enum Transport {
    TogglePlay,
    StopPlayback,
    SeekForward,
    SeekBackward,
    FrameStepForward,
    FrameStepBackward,
    JumpForward,
    JumpBackward,
    GotoStart,
    GotoEnd,
}

#[derive(Default)]
pub(super) struct Engine {
    /// Opened on the first play; the error is kept so a missing device
    /// isn't retried (and reported) on every play.
    output: Option<Result<AudioOutput, String>>,
    /// Decoded audio per asset id, stereo at the device rate. Empty when
    /// the file has no audio.
    // shortcut: whole files decoded up front and kept in memory (~11 MB per
    // minute); upgrade to streaming `AudioDecoder` blocks for long media.
    sources: HashMap<String, Arc<[f32]>>,
    scheduler: FrameScheduler,
    /// Without an audio device the playhead runs off the wall clock.
    wall: Option<(Instant, MediaTime)>,
    active: bool,
    /// Decoding the project's audio before the session can begin.
    decoding: Option<Task<()>>,
    ticker: Option<Task<()>>,
}

impl Workspace {
    pub(crate) fn run_transport(&mut self, action: Transport, cx: &mut Context<Self>) {
        let before = self.editor.playback().position;
        match action {
            Transport::TogglePlay if self.editor.playback().playing => self.pause(cx),
            Transport::TogglePlay => self.play(cx),
            Transport::StopPlayback => {
                self.editor.stop();
                self.end_session(cx);
                self.position_moved(before, cx);
            }
            Transport::SeekForward => self.seek_by(SEEK, cx),
            Transport::SeekBackward => self.seek_by(-SEEK, cx),
            Transport::JumpForward => self.seek_by(JUMP, cx),
            Transport::JumpBackward => self.seek_by(-JUMP, cx),
            Transport::FrameStepForward => self.step_frames(1, cx),
            Transport::FrameStepBackward => self.step_frames(-1, cx),
            Transport::GotoStart => self.seek(MediaTime::ZERO, cx),
            Transport::GotoEnd => self.seek(self.editor.duration(), cx),
        }
    }

    pub(crate) fn play(&mut self, cx: &mut Context<Self>) {
        self.editor.play();
        if self.editor.playback().playing {
            self.start_session(cx);
        }
        cx.notify();
    }

    pub(crate) fn pause(&mut self, cx: &mut Context<Self>) {
        self.editor.pause();
        self.end_session(cx);
        cx.notify();
    }

    fn seek_by(&mut self, delta: MediaTime, cx: &mut Context<Self>) {
        self.seek(self.editor.playback().position + delta, cx);
    }

    fn step_frames(&mut self, frames: i64, cx: &mut Context<Self>) {
        let before = self.editor.playback().position;
        self.editor.step_frames(frames);
        self.position_moved(before, cx);
    }

    /// (Re)starts audio and the clock at the playhead, e.g. after a seek or
    /// an edit. Decodes any audio not yet decoded first, off the UI thread.
    pub(super) fn start_session(&mut self, cx: &mut Context<Self>) {
        if self.engine.decoding.is_some() {
            // Its completion starts from wherever the playhead is by then.
            return;
        }
        let engine = &mut self.engine;
        let output = engine
            .output
            .get_or_insert_with(|| {
                AudioOutput::open()
                    .inspect_err(|error| eprintln!("no audio, playing silent: {error}"))
            })
            .as_ref();
        let rate = output.map_or(0, |output| output.sample_rate().min(u32::MAX));
        let files = &self.files;
        let mut missing: Vec<(String, PathBuf)> = Vec::new();
        if rate > 0 {
            let project = self.editor.project();
            for element in project
                .current_scene()
                .into_iter()
                .flat_map(|scene| scene.tracks.iter())
                .flat_map(|track| &track.elements)
            {
                let asset = element
                    .media_id()
                    .and_then(|id| project.media_assets.iter().find(|asset| asset.id == id));
                if let Some(asset) = asset
                    && asset.has_audio != Some(false)
                    && !engine.sources.contains_key(&asset.id)
                    && !missing.iter().any(|(id, _)| *id == asset.id)
                {
                    missing.push((asset.id.clone(), files.media_path(asset)));
                }
            }
        }
        if missing.is_empty() {
            self.begin(cx);
            return;
        }
        let work = cx.background_executor().spawn(async move {
            missing
                .into_iter()
                .map(|(id, path)| {
                    let samples = decode_audio(&path, Some(AudioFormat::new(rate, 2)))
                        .map(|audio| audio.samples)
                        .unwrap_or_default();
                    (id, Arc::<[f32]>::from(samples))
                })
                .collect::<Vec<_>>()
        });
        self.engine.decoding = Some(cx.spawn(async move |this, cx| {
            let decoded = work.await;
            let _ = this.update(cx, |this, cx| {
                this.engine.decoding = None;
                this.engine.sources.extend(decoded);
                if this.editor.playback().playing {
                    this.begin(cx);
                }
            });
        }));
    }

    /// Starts the mix and the clock at the playhead.
    fn begin(&mut self, cx: &mut Context<Self>) {
        let position = self.editor.playback().position;
        let engine = &mut self.engine;
        match engine.output.as_ref() {
            Some(Ok(output)) => {
                let sources = &engine.sources;
                let mixer =
                    Mixer::from_project(self.editor.project(), output.sample_rate(), |asset| {
                        sources.get(&asset.id).cloned()
                    });
                output.start(Arc::new(mixer), position);
                engine.wall = None;
            }
            _ => engine.wall = Some((Instant::now(), position)),
        }
        engine.scheduler.reset();
        engine.active = true;
        engine.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if !this.update(cx, |this, cx| this.tick(cx)).unwrap_or(false) {
                    break;
                }
            }
        }));
    }

    /// Moves the playhead to the clock's frame. Returns whether to keep
    /// ticking.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let engine = &mut self.engine;
        if !engine.active {
            return false;
        }
        let position = match (engine.wall, &engine.output) {
            (Some((start, origin)), _) => {
                origin
                    + MediaTime::from_seconds_f64(start.elapsed().as_secs_f64()).unwrap_or_default()
            }
            (None, Some(Ok(output))) => output.position(),
            _ => return false,
        };
        let rate = self.editor.project().settings.fps;
        // Same frame as last tick: nothing to redraw. A slow tick jumps
        // several frames at once, which drops the ones in between.
        if engine.scheduler.poll(position, rate).is_none() {
            return true;
        }
        let running = self.editor.advance_to(position);
        if !running {
            self.end_session(cx);
        }
        cx.notify();
        running
    }

    /// Silences the output and stops the clock; saves the playhead.
    fn end_session(&mut self, cx: &mut Context<Self>) {
        let engine = &mut self.engine;
        if let Some(Ok(output)) = &engine.output {
            output.stop();
        }
        engine.active = false;
        engine.wall = None;
        engine.ticker = None;
        self.autosave.view_changed = true;
        self.schedule_save(cx);
    }
}
