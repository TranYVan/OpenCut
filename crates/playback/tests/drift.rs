//! A one-minute project "played" through the mixer, clock and frame
//! scheduler with a simulated audio device: no hardware, but the same code
//! the `cpal` callback runs.

use std::sync::Arc;

use opencut_model::time::{FrameRate, MediaTime};
use opencut_model::{
    AudioElement, AudioSource, Element, ElementBase, MediaAsset, MediaType, ParamValue, Project,
    TrackKind, VideoElement,
};
use opencut_playback::{
    AudioClock, CHANNELS, FrameScheduler, Mixer, Player, db_to_gain, ticks_to_frames,
};
use pretty_assertions::assert_eq;

const RATE: u32 = 48_000;
const FPS: FrameRate = FrameRate::FPS_30;
/// An awkward callback size, so blocks never line up with video frames.
const BLOCK: usize = 1021;

fn secs(seconds: f64) -> MediaTime {
    MediaTime::from_seconds_f64(seconds).unwrap()
}

fn asset(id: &str, media_type: MediaType) -> MediaAsset {
    MediaAsset {
        id: id.to_owned(),
        name: id.to_owned(),
        media_type,
        size: 0,
        last_modified: 0,
        width: None,
        height: None,
        duration: None,
        fps: None,
        has_audio: Some(true),
        ephemeral: None,
        thumbnail_url: None,
    }
}

/// Stereo source of `seconds` that is silent except for a full-scale
/// impulse at its first frame.
fn impulse(seconds: u32) -> Arc<[f32]> {
    let mut samples = vec![0.0; (seconds * RATE) as usize * CHANNELS];
    samples[0] = 1.0;
    samples[1] = 1.0;
    samples.into()
}

fn audio_element(id: &str, start: f64, duration: f64) -> AudioElement {
    AudioElement {
        base: ElementBase::new(id, id, secs(start), secs(duration)),
        source: AudioSource::Upload {
            media_id: id.to_owned(),
        },
        retime: None,
    }
}

/// A 60 s video clip (source audio on) and a 20 s audio clip at 10 s.
fn project() -> Project {
    let mut project = Project::new("Test", chrono::Utc::now());
    project.media_assets = vec![
        asset("video", MediaType::Video),
        asset("music", MediaType::Audio),
    ];
    let scene = project.current_scene_mut().unwrap();
    scene
        .tracks
        .main
        .elements
        .push(Element::Video(VideoElement {
            base: ElementBase::new("video", "video", MediaTime::ZERO, secs(60.0)),
            media_id: "video".to_owned(),
            is_source_audio_enabled: None,
            hidden: None,
            retime: None,
            effects: None,
            masks: None,
        }));
    let mut track = opencut_model::Track::new("audio-1", TrackKind::Audio);
    track
        .elements
        .push(Element::Audio(audio_element("music", 10.0, 20.0)));
    scene.tracks.audio.push(track);
    project
}

fn mixer_for(project: &Project) -> Mixer {
    Mixer::from_project(project, RATE, |asset| {
        Some(impulse(if asset.id == "video" { 60 } else { 20 }))
    })
}

#[test]
fn plays_one_minute_with_no_drift() {
    let mixer = Arc::new(mixer_for(&project()));
    assert_eq!(mixer.len(), 60 * u64::from(RATE));

    let clock = Arc::new(AudioClock::new(RATE));
    // The device buffers two callbacks ahead of what is heard.
    let latency = 2 * BLOCK as u64;
    clock.set_latency(latency);
    let mut player = Player::new(mixer, clock.clone(), MediaTime::ZERO);

    let mut scheduler = FrameScheduler::default();
    let mut shown = 0u64;
    let mut dropped = 0u64;
    let mut last_frame = -1i64;
    let mut impulses = Vec::new();
    let mut produced = 0usize;
    let mut block = vec![0.0f32; BLOCK * CHANNELS];

    // Wall time is when a sample is heard: frame f is heard at f / RATE.
    // Each callback renders `BLOCK` frames; the UI polls every ~16 ms, but
    // stalls for 300 ms once, to force dropped frames.
    let total = 60 * RATE as usize;
    let mut next_poll = 0.0f64;
    let mut max_error = 0.0f64;
    while produced < total + 4 * BLOCK {
        player.fill(&mut block);
        for (i, frame) in block.chunks_exact(CHANNELS).enumerate() {
            if frame[0] != 0.0 {
                impulses.push(produced + i);
            }
        }
        produced += BLOCK;

        // The device has played everything rendered except `latency` frames.
        let heard_seconds = produced.saturating_sub(latency as usize) as f64 / f64::from(RATE);
        while next_poll <= heard_seconds {
            let position = clock.position();
            // Against the true heard time the clock is off by under a block.
            let error = (position.to_seconds_f64() - next_poll).abs();
            max_error = max_error.max(error);
            if let Some(tick) = scheduler.poll(position, FPS) {
                shown += 1;
                dropped += tick.dropped;
                assert!(tick.frame > last_frame);
                last_frame = tick.frame;
            }
            next_poll += if (30.0..30.1).contains(&next_poll) {
                0.3
            } else {
                0.016
            };
        }
    }

    // Every frame was either shown or dropped: the video never fell behind.
    assert_eq!(shown + dropped, last_frame as u64 + 1);
    assert!(dropped > 0, "the stall should have dropped frames");
    // Sound and playhead stay within one video frame all the way.
    assert!(max_error < 1.0 / 30.0, "clock error {max_error}s");

    // The video's impulse at 0 s and the music's at 10 s land on exactly
    // the frame the clock calls 0 s and 10 s: no drift in the mix itself.
    assert_eq!(impulses, vec![0, 10 * RATE as usize]);

    // The clock counts whole frames, so nothing accumulates: after all that
    // it is exactly what the device has rendered minus its latency.
    assert_eq!(clock.position_frames(), produced as u64 - latency);
}

#[test]
fn clips_play_at_their_volume_and_respect_mute() {
    let mut project = project();
    let scene = project.current_scene_mut().unwrap();
    let Element::Audio(music) = &mut scene.tracks.audio[0].elements[0] else {
        unreachable!()
    };
    music
        .base
        .params
        .insert("volume".to_owned(), ParamValue::Number(-6.0));
    // Isolate the music: the video's source audio is off.
    let Element::Video(video) = &mut scene.tracks.main.elements[0] else {
        unreachable!()
    };
    video.is_source_audio_enabled = Some(false);

    let mixer = mixer_for(&project);
    let mut out = vec![0.0f32; 2 * CHANNELS];
    mixer.mix(10 * u64::from(RATE) - 1, &mut out);
    assert_eq!(out[0], 0.0);
    assert!((out[2] - db_to_gain(-6.0)).abs() < 1e-6);
    assert!((out[2] - 0.501_187).abs() < 1e-5);

    // Muting the element, or its track, silences it.
    let mut muted = project.clone();
    let scene = muted.current_scene_mut().unwrap();
    scene.tracks.audio[0].muted = Some(true);
    assert!(mixer_for(&muted).is_empty());
    let mut muted = project;
    let scene = muted.current_scene_mut().unwrap();
    let Element::Audio(music) = &mut scene.tracks.audio[0].elements[0] else {
        unreachable!()
    };
    music
        .base
        .params
        .insert("muted".to_owned(), ParamValue::Bool(true));
    assert!(mixer_for(&muted).is_empty());
}

#[test]
fn trimmed_and_retimed_clips_read_the_right_source_frames() {
    let mut project = project();
    let scene = project.current_scene_mut().unwrap();
    scene.tracks.main.elements.clear();
    let Element::Audio(music) = &mut scene.tracks.audio[0].elements[0] else {
        unreachable!()
    };
    music.base.trim_start = secs(1.0);
    music.retime = Some(opencut_model::RetimeConfig {
        rate: 2.0,
        maintain_pitch: None,
    });
    // Source frame f holds f / 1e6, so the output shows what was read.
    let source: Arc<[f32]> = (0..20 * RATE as usize)
        .flat_map(|f| [f as f32 / 1e6; 2])
        .collect();
    let mixer = Mixer::from_project(&project, RATE, |_| Some(source.clone()));
    assert_eq!(mixer.len(), 30 * u64::from(RATE));
    let mut out = vec![0.0f32; 2 * CHANNELS];
    mixer.mix(10 * u64::from(RATE), &mut out);
    // Trim 1 s = source frame 48000; at 2x the next output frame reads 48002.
    assert!((out[0] - 0.048).abs() < 1e-6);
    assert!((out[2] - 0.048_002).abs() < 1e-6);
    assert_eq!(ticks_to_frames(secs(1.0).as_ticks(), RATE), u64::from(RATE));
}
