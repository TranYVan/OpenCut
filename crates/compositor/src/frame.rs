//! What the compositor draws: a list of textured quads, bottom to top.
//!
//! Same JSON shape as the classic `FrameDescriptor`
//! (`services/renderer/compositor/types.ts`), minus effect passes, masks and
//! scene effects, which come with the effects and masks ports (P8).

use serde::{Deserialize, Serialize};

use crate::BlendMode;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameDescriptor {
    pub width: u32,
    pub height: u32,
    pub clear: CanvasClearDescriptor,
    pub items: Vec<FrameItemDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasClearDescriptor {
    /// Straight-alpha RGBA, 0–1.
    pub color: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FrameItemDescriptor {
    Layer(LayerDescriptor),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerDescriptor {
    /// A texture registered with [`crate::Compositor::upsert_texture`].
    pub texture_id: String,
    pub transform: QuadTransformDescriptor,
    pub opacity: f32,
    pub blend_mode: BlendMode,
}

/// Where a texture lands on the canvas, in canvas pixels (origin top left).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuadTransformDescriptor {
    pub center_x: f32,
    pub center_y: f32,
    pub width: f32,
    pub height: f32,
    /// Clockwise.
    pub rotation_degrees: f32,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl QuadTransformDescriptor {
    /// Covers a `width`×`height` canvas exactly.
    pub fn full_canvas(width: u32, height: u32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let (width, height) = (width as f32, height as f32);
        Self {
            center_x: width / 2.0,
            center_y: height / 2.0,
            width,
            height,
            rotation_degrees: 0.0,
            flip_x: false,
            flip_y: false,
        }
    }
}
