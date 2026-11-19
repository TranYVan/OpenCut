//! Exports a project to an MP4: each frame rendered by the compositor's
//! [`Renderer`] (what the preview shows), the sound mixed by the playback
//! [`Mixer`] (what the preview plays), encoded by `opencut-media`'s
//! [`Mp4Writer`] (hardware H.264/HEVC first, AAC audio).
//!
//! Classic reference (tag `pre-rewrite`):
//! `apps/web/src/services/renderer/scene-exporter.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use opencut_compositor::gpu::GpuContext;
use opencut_compositor::{CompositorError, MediaSources, Renderer};
use opencut_media::{AudioFormat, EncodeSettings, MediaError, Mp4Writer, decode_audio};
use opencut_model::time::MediaTime;
use opencut_model::{MediaAsset, Project};
use opencut_playback::{CHANNELS, Mixer, ticks_to_frames};

pub use opencut_media::VideoCodec;

const SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("the project is empty or its frame rate is invalid")]
    Empty,
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error(transparent)]
    Render(#[from] CompositorError),
}

/// Writes the current scene of `project` to `output`, an MP4 at the
/// project's canvas size and frame rate. `resolve_path` maps an asset to
/// its file; as in the preview, media it can't find or decode is left out.
/// Returns FFmpeg's name of the video encoder used.
pub fn export_project(
    context: &GpuContext,
    project: &Project,
    resolve_path: impl Fn(&MediaAsset) -> Option<PathBuf>,
    output: &Path,
    codec: VideoCodec,
) -> Result<&'static str, ExportError> {
    let rate = project.settings.fps;
    let duration = project
        .current_scene()
        .map_or(MediaTime::ZERO, |scene| scene.tracks.duration());
    // Whole frames, as the classic exporter.
    let frames = duration
        .to_frame_floor(rate)
        .filter(|&frames| frames > 0)
        .ok_or(ExportError::Empty)?;

    let mut decoded = HashMap::new();
    let mixer = Mixer::from_project(project, SAMPLE_RATE, |asset| {
        decoded
            .entry(asset.id.clone())
            .or_insert_with(|| {
                let format = AudioFormat::new(SAMPLE_RATE, CHANNELS as u16);
                let audio = decode_audio(resolve_path(asset)?, Some(format)).ok()?;
                Some(Arc::<[f32]>::from(audio.samples))
            })
            .clone()
    });

    let canvas = project.settings.canvas_size;
    let mut writer = Mp4Writer::create(
        output,
        EncodeSettings {
            width: canvas.width,
            height: canvas.height,
            frame_rate: rate,
            codec,
            sample_rate: (!mixer.is_empty()).then_some(SAMPLE_RATE),
        },
    )?;
    let mut renderer = Renderer::new(context);
    let mut sources = MediaSources::new(|asset: &MediaAsset| resolve_path(asset));
    let mut audio = Vec::new();
    let mut mixed = 0;
    for frame in 0..frames {
        let time = MediaTime::from_frame(frame, rate).ok_or(ExportError::Empty)?;
        let image = renderer.render_project(context, project, time, &mut sources)?;
        writer.push_video(&image.data)?;

        if !mixer.is_empty() {
            // The audio up to the end of this frame, so the muxer can
            // interleave as it goes.
            let end = MediaTime::from_frame(frame + 1, rate).ok_or(ExportError::Empty)?;
            let end = ticks_to_frames(end.as_ticks(), SAMPLE_RATE);
            audio.resize((end - mixed) as usize * CHANNELS, 0.0);
            mixer.mix(mixed, &mut audio);
            writer.push_audio(&audio)?;
            mixed = end;
        }
    }
    let encoder = writer.encoder();
    writer.finish()?;
    Ok(encoder)
}
