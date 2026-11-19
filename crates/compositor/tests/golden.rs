//! Golden images: the golden project rendered at several times must match
//! `tests/golden/*.png` on every OS and adapter, within a small tolerance
//! (GPUs differ slightly in filtering precision and on exact edges).
//!
//! Without any GPU adapter (not even lavapipe/WARP) these tests print
//! `SKIPPED` and pass, unless `OPENCUT_REQUIRE_GPU=1`.
//!
//! Regenerate after an intended change with
//! `UPDATE_GOLDEN=1 cargo test -p opencut-compositor --test golden`, and look
//! at the images before committing them.

mod common;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use common::{CANVAS, SyntheticSources, golden_project, gpu, seconds};
use opencut_compositor::{
    BlendMode, CanvasClearDescriptor, Compositor, FrameDescriptor, FrameItemDescriptor,
    LayerDescriptor, QuadTransformDescriptor, Renderer, RgbaImage,
};

/// Times (ms) covering each layer combination of the golden project.
const TIMES_MS: [u32; 6] = [250, 1000, 1750, 2500, 3750, 4500];

/// A pixel counts as different when a channel is off by more than this.
const CHANNEL_TOLERANCE: u8 = 4;
/// At most this share of pixels may differ (anti-aliasing-free edges can
/// flip either way on a different rasterizer).
const MAX_DIFFERENT_SHARE: f64 = 0.01;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn read_png(path: &Path) -> RgbaImage {
    let decoder = png::Decoder::new(std::io::BufReader::new(File::open(path).unwrap_or_else(
        |error| {
            panic!(
                "{}: {error}. Run with UPDATE_GOLDEN=1 to create it.",
                path.display()
            )
        },
    )));
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut data).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgba, png::BitDepth::Eight),
        "{} must be 8-bit RGBA",
        path.display()
    );
    data.truncate(info.buffer_size());
    RgbaImage::new(info.width, info.height, data)
}

fn write_png(path: &Path, image: &RgbaImage) {
    let file = BufWriter::new(File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&image.data)
        .unwrap();
}

/// (pixels differing by more than the tolerance, largest channel difference)
fn compare(actual: &RgbaImage, expected: &RgbaImage) -> (usize, u8) {
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height)
    );
    let mut different = 0;
    let mut largest = 0;
    for (a, e) in actual
        .data
        .chunks_exact(4)
        .zip(expected.data.chunks_exact(4))
    {
        let difference = a
            .iter()
            .zip(e)
            .map(|(a, e)| a.abs_diff(*e))
            .max()
            .unwrap_or(0);
        largest = largest.max(difference);
        if difference > CHANNEL_TOLERANCE {
            different += 1;
        }
    }
    (different, largest)
}

#[test]
fn golden_project_matches_reference_frames() {
    let Some(context) = gpu() else { return };
    let project = golden_project();
    let mut renderer = Renderer::new(&context);
    let mut sources = SyntheticSources::default();
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();

    let mut failures = Vec::new();
    for ms in TIMES_MS {
        let time = seconds(f64::from(ms) / 1000.0);
        let frame = renderer
            .render_project(&context, &project, time, &mut sources)
            .unwrap();
        assert_eq!((frame.width, frame.height), (CANVAS.width, CANVAS.height));

        let path = golden_dir().join(format!("frame-{ms:05}ms.png"));
        if update {
            write_png(&path, &frame);
            continue;
        }
        let expected = read_png(&path);
        let (different, largest) = compare(&frame, &expected);
        let pixels = frame.data.len() / 4;
        #[allow(clippy::cast_precision_loss)]
        let share = different as f64 / pixels as f64;
        eprintln!("{ms} ms: {different}/{pixels} pixels differ, largest channel diff {largest}");
        if share > MAX_DIFFERENT_SHARE {
            let actual_path =
                Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("frame-{ms:05}ms.actual.png"));
            write_png(&actual_path, &frame);
            failures.push(format!(
                "{ms} ms: {different} of {pixels} pixels differ by more than \
                 {CHANNEL_TOLERANCE} (largest {largest}); actual frame at {}",
                actual_path.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn rendering_is_repeatable_and_order_independent() {
    let Some(context) = gpu() else { return };
    let project = golden_project();
    let mut sources = SyntheticSources::default();

    // One renderer playing forward, one jumping around: same frames, whatever
    // textures each has cached.
    let mut forward = Renderer::new(&context);
    let mut shuffled = Renderer::new(&context);
    let frames: Vec<RgbaImage> = TIMES_MS
        .iter()
        .map(|&ms| {
            forward
                .render_project(
                    &context,
                    &project,
                    seconds(f64::from(ms) / 1000.0),
                    &mut sources,
                )
                .unwrap()
        })
        .collect();
    for index in [3, 0, 5, 1, 4, 2, 3] {
        let ms = TIMES_MS[index];
        let frame = shuffled
            .render_project(
                &context,
                &project,
                seconds(f64::from(ms) / 1000.0),
                &mut sources,
            )
            .unwrap();
        assert!(frame == frames[index], "{ms} ms differs between renders");
    }
}

#[test]
fn background_only_frame_is_the_exact_colour() {
    let Some(context) = gpu() else { return };
    let mut renderer = Renderer::new(&context);
    let frame = renderer
        .render_project(
            &context,
            &golden_project(),
            seconds(4.5),
            &mut SyntheticSources::default(),
        )
        .unwrap();
    assert!(
        frame
            .data
            .chunks_exact(4)
            .all(|pixel| pixel == [0x20, 0x30, 0x40, 0xff]),
        "every pixel is #203040"
    );
}

#[test]
fn full_canvas_video_lands_pixel_for_pixel() {
    let Some(context) = gpu() else { return };
    let mut renderer = Renderer::new(&context);
    let mut sources = SyntheticSources::default();
    // At 0.25 s only the main video shows: 64×36 contained in 160×90 is a
    // 2.5× upscale filling the canvas, trimmed 0.5 s in, so frame 22.
    let frame = renderer
        .render_project(&context, &golden_project(), seconds(0.25), &mut sources)
        .unwrap();
    assert_eq!(
        sources.video_requests,
        vec![("vid".to_owned(), seconds(0.75))]
    );
    let source = common::video_frame(22);
    // Away from the bar and from texel edges, the colour is the texel's.
    for (x, y) in [(5u32, 5u32), (150, 80), (100, 40)] {
        // Canvas pixel centre (x + 0.5) maps to texel (x + 0.5) / 2.5.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let texel = |value: u32| ((f64::from(value) + 0.5) / 2.5) as u32;
        let expected = source.pixel(texel(x), texel(y));
        let actual = frame.pixel(x, y);
        for channel in 0..4 {
            assert!(
                actual[channel].abs_diff(expected[channel]) <= 20,
                "({x}, {y}): {actual:?} vs texel {expected:?}"
            );
        }
    }
}

/// Every blend mode on one layer: a smoke test that the shader covers all
/// modes and normal/multiply/screen give their textbook results.
#[test]
fn blend_modes_match_formulas_on_flat_colours() {
    let Some(context) = gpu() else { return };
    let mut compositor = Compositor::new(&context);
    compositor.upsert_texture(
        "layer",
        context.upload_rgba_texture(1, 1, &[204, 102, 51, 255], "layer"),
    );
    let base = [0.4f32, 0.6, 0.8, 1.0];
    for mode in BlendMode::ALL {
        let frame = FrameDescriptor {
            width: 4,
            height: 4,
            clear: CanvasClearDescriptor { color: base },
            items: vec![FrameItemDescriptor::Layer(LayerDescriptor {
                texture_id: "layer".to_owned(),
                transform: QuadTransformDescriptor::full_canvas(4, 4),
                opacity: 1.0,
                blend_mode: mode,
            })],
        };
        let pixels = compositor.render_frame_to_rgba(&context, &frame).unwrap();
        let pixel = &pixels[..4];
        let layer = [0.8f32, 0.4, 0.2];
        let expected: Option<[f32; 3]> = match mode {
            BlendMode::Normal => Some(layer),
            BlendMode::Multiply => Some([0.32, 0.24, 0.16]),
            BlendMode::Screen => Some([0.88, 0.76, 0.84]),
            BlendMode::Darken => Some([0.4, 0.4, 0.2]),
            BlendMode::Difference => Some([0.4, 0.2, 0.6]),
            _ => None,
        };
        assert_eq!(pixel[3], 255, "{mode:?} keeps the frame opaque");
        if let Some(expected) = expected {
            for channel in 0..3 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let expected = (expected[channel] * 255.0).round() as u8;
                assert!(
                    pixel[channel].abs_diff(expected) <= 2,
                    "{mode:?}: {pixel:?}, expected channel {channel} = {expected}"
                );
            }
        }
    }
}
