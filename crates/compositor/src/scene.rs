//! Project + time → the layers visible at that time, bottom to top.
//!
//! Port of the classic `services/renderer/scene-builder.ts` plus the parts
//! of `resolve.ts` and `compositor/frame-descriptor.ts` that do not need
//! pixels. Pure: decoding happens later, in [`crate::Renderer`].
//!
//! Covered: video and image elements, the colour background, transform
//! (`transform.positionX/Y`, `transform.scaleX/Y`, `transform.rotate`),
//! `opacity`, `blendMode`, hidden tracks and elements, trim and playback
//! rate. Not yet: text, stickers, graphics (P7), effects and effect layers,
//! masks and the blur background (P8). Keyframed params use their value at
//! the frame's time.

use opencut_model::{
    Background, CanvasSize, Element, MediaAsset, MediaType, ParamValues, Project, ProjectSettings,
    RetimeConfig, Scene, Track,
};
use opencut_time::MediaTime;

use crate::BlendMode;
use crate::frame::QuadTransformDescriptor;

/// Everything to draw for one frame: layers bottom to top over `clear`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenePlan {
    pub canvas: CanvasSize,
    /// Straight-alpha RGBA, 0–1, under every layer. Opaque black, as in
    /// the classic renderer.
    pub clear: [f32; 4],
    pub layers: Vec<SceneLayer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneLayer {
    /// The element id, or `"background"`.
    pub id: String,
    pub source: LayerSource,
    pub transform: ElementTransform,
    /// 0–1.
    pub opacity: f32,
    pub blend_mode: BlendMode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayerSource {
    /// A solid straight-alpha RGBA colour filling the canvas.
    Color([f32; 4]),
    /// A video frame at `source_time` from the start of the media.
    Video {
        asset: MediaAsset,
        source_time: MediaTime,
    },
    Image {
        asset: MediaAsset,
    },
}

/// An element's transform params. Position is in canvas pixels from the
/// canvas centre; scale is relative to the source contained in the canvas;
/// negative scale flips. Rotation is in degrees, clockwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElementTransform {
    pub position_x: f64,
    pub position_y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotate: f64,
}

impl Default for ElementTransform {
    fn default() -> Self {
        Self {
            position_x: 0.0,
            position_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotate: 0.0,
        }
    }
}

impl ElementTransform {
    /// Reads `transform.*` params, defaulting missing or non-numeric ones
    /// (classic `buildTransformFromParams`).
    pub fn from_params(params: &ParamValues) -> Self {
        let defaults = Self::default();
        Self {
            position_x: number_param(params, "transform.positionX").unwrap_or(defaults.position_x),
            position_y: number_param(params, "transform.positionY").unwrap_or(defaults.position_y),
            scale_x: number_param(params, "transform.scaleX").unwrap_or(defaults.scale_x),
            scale_y: number_param(params, "transform.scaleY").unwrap_or(defaults.scale_y),
            rotate: number_param(params, "transform.rotate").unwrap_or(defaults.rotate),
        }
    }

    /// Where a `source_width`×`source_height` picture lands on the canvas:
    /// contained in it, then scaled, moved and rotated (classic
    /// `computeVisualTransform`).
    #[allow(clippy::cast_possible_truncation)]
    pub fn quad(
        &self,
        canvas: CanvasSize,
        source_width: u32,
        source_height: u32,
    ) -> QuadTransformDescriptor {
        let (canvas_width, canvas_height) = (f64::from(canvas.width), f64::from(canvas.height));
        let (source_width, source_height) = (f64::from(source_width), f64::from(source_height));
        let contain = (canvas_width / source_width).min(canvas_height / source_height);
        let scaled_width = source_width * contain * self.scale_x;
        let scaled_height = source_height * contain * self.scale_y;
        QuadTransformDescriptor {
            center_x: (canvas_width / 2.0 + self.position_x) as f32,
            center_y: (canvas_height / 2.0 + self.position_y) as f32,
            width: scaled_width.abs() as f32,
            height: scaled_height.abs() as f32,
            rotation_degrees: self.rotate as f32,
            flip_x: scaled_width < 0.0,
            flip_y: scaled_height < 0.0,
        }
    }
}

/// The current scene of `project` at `time`. A project whose current scene
/// is missing gives an empty plan.
pub fn build_scene(project: &Project, time: MediaTime) -> ScenePlan {
    match project.current_scene() {
        Some(scene) => build_scene_from(scene, &project.settings, &project.media_assets, time),
        None => ScenePlan {
            canvas: project.settings.canvas_size,
            clear: CLEAR_COLOR,
            layers: Vec::new(),
        },
    }
}

const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// `scene` at `time`, with media looked up in `assets`.
///
/// Track order follows the timeline: the main track at the bottom, overlay
/// tracks above it with the first overlay on top. Within a track, elements
/// draw in start order (ties by id). Hidden tracks and elements are skipped;
/// muted only silences audio, so muted tracks still draw.
pub fn build_scene_from(
    scene: &Scene,
    settings: &ProjectSettings,
    assets: &[MediaAsset],
    time: MediaTime,
) -> ScenePlan {
    let mut layers = Vec::new();
    if let Some(color) = background_color(&settings.background) {
        layers.push(SceneLayer {
            id: "background".to_owned(),
            source: LayerSource::Color(color),
            transform: ElementTransform::default(),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        });
    }

    let tracks_bottom_to_top = std::iter::once(&scene.tracks.main)
        .chain(scene.tracks.overlay.iter().rev())
        .filter(|track| !track.is_hidden());
    for track in tracks_bottom_to_top {
        for element in visible_sorted_elements(track) {
            if let Some(layer) = element_layer(element, assets, time) {
                layers.push(layer);
            }
        }
    }

    ScenePlan {
        canvas: settings.canvas_size,
        clear: CLEAR_COLOR,
        layers,
    }
}

fn visible_sorted_elements(track: &Track) -> Vec<&Element> {
    let mut elements: Vec<&Element> = track
        .elements
        .iter()
        .filter(|element| element.is_hidden() != Some(true))
        .collect();
    elements.sort_by(|a, b| {
        a.start_time()
            .cmp(&b.start_time())
            .then_with(|| a.id().cmp(b.id()))
    });
    elements
}

fn element_layer(element: &Element, assets: &[MediaAsset], time: MediaTime) -> Option<SceneLayer> {
    let base = element.base();
    let clip_time = time - base.start_time;
    if clip_time < MediaTime::ZERO || clip_time >= base.duration {
        return None;
    }
    let asset_of = |media_id: &str, media_type: MediaType| {
        assets
            .iter()
            .find(|asset| asset.id == media_id && asset.media_type == media_type)
            .cloned()
    };
    let source = match element {
        Element::Video(video) => {
            let rate = RetimeConfig::safe_rate(video.retime.as_ref());
            LayerSource::Video {
                asset: asset_of(&video.media_id, MediaType::Video)?,
                source_time: base.trim_start + clip_time.mul_f64_round(rate)?,
            }
        }
        Element::Image(image) => LayerSource::Image {
            asset: asset_of(&image.media_id, MediaType::Image)?,
        },
        // TODO(P7, P8): text, stickers, graphics and effect layers.
        _ => return None,
    };
    let params = base.params_at(clip_time);
    #[allow(clippy::cast_possible_truncation)]
    let opacity = number_param(&params, "opacity")
        .unwrap_or(1.0)
        .clamp(0.0, 1.0) as f32;
    let blend_mode = params
        .get("blendMode")
        .and_then(|value| value.as_str()?.parse().ok())
        .unwrap_or_default();
    Some(SceneLayer {
        id: base.id.clone(),
        source,
        transform: ElementTransform::from_params(&params),
        opacity,
        blend_mode,
    })
}

fn number_param(params: &ParamValues, key: &str) -> Option<f64> {
    params
        .get(key)
        .and_then(opencut_model::ParamValue::as_f64)
        .filter(|value| value.is_finite())
}

/// The fill of a colour background; `None` for `transparent`, the blur
/// background (P8) and colours this parser does not read (CSS gradients and
/// named colours other than `transparent`).
fn background_color(background: &Background) -> Option<[f32; 4]> {
    match background {
        Background::Color { color } => parse_css_color(color).filter(|color| color[3] > 0.0),
        Background::Blur { .. } => None,
    }
}

/// Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` and `transparent` to
/// straight-alpha RGBA, 0–1.
pub fn parse_css_color(css: &str) -> Option<[f32; 4]> {
    let css = css.trim();
    if css.eq_ignore_ascii_case("transparent") {
        return Some([0.0; 4]);
    }
    let hex = css.strip_prefix('#')?;
    if !hex.is_ascii() {
        return None;
    }
    let channel = |digits: &str| u8::from_str_radix(digits, 16).ok();
    let bytes: Vec<u8> = match hex.len() {
        3 | 4 => hex
            .chars()
            .map(|digit| channel(&digit.to_string()).map(|value| value * 17))
            .collect::<Option<_>>()?,
        6 | 8 => (0..hex.len())
            .step_by(2)
            .map(|index| channel(&hex[index..index + 2]))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let alpha = bytes.get(3).copied().unwrap_or(u8::MAX);
    Some([bytes[0], bytes[1], bytes[2], alpha].map(|value| f32::from(value) / 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_css_color("#fff"), Some([1.0; 4]));
        assert_eq!(parse_css_color("#000000"), Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(
            parse_css_color("#ff000080").map(|c| c[3]),
            Some(128.0 / 255.0)
        );
        assert_eq!(
            parse_css_color(" #0f08 "),
            Some([0.0, 1.0, 0.0, 136.0 / 255.0])
        );
        assert_eq!(parse_css_color("transparent"), Some([0.0; 4]));
        assert_eq!(parse_css_color("#12345"), None);
        assert_eq!(parse_css_color("#gggggg"), None);
        assert_eq!(parse_css_color("red"), None);
        assert_eq!(parse_css_color("linear-gradient(red, blue)"), None);
    }

    #[test]
    fn transform_contains_source_then_scales() {
        let canvas = CanvasSize {
            width: 1920,
            height: 1080,
        };
        let quad = ElementTransform::default().quad(canvas, 1280, 720);
        assert_eq!((quad.width, quad.height), (1920.0, 1080.0));
        assert_eq!((quad.center_x, quad.center_y), (960.0, 540.0));

        // A square source is letterboxed to the canvas height.
        let quad = ElementTransform {
            position_x: 100.0,
            position_y: -50.0,
            scale_x: -0.5,
            scale_y: 0.5,
            rotate: 45.0,
        }
        .quad(canvas, 500, 500);
        assert_eq!((quad.width, quad.height), (540.0, 540.0));
        assert_eq!((quad.center_x, quad.center_y), (1060.0, 490.0));
        assert!(quad.flip_x && !quad.flip_y);
        assert_eq!(quad.rotation_degrees, 45.0);
    }
}
