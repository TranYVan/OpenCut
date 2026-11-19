use std::collections::HashSet;

use opencut_gpu::{GpuContext, wgpu};
use opencut_model::Project;
use opencut_time::MediaTime;

use crate::compositor::{Compositor, CompositorError};
use crate::frame::{
    CanvasClearDescriptor, FrameDescriptor, FrameItemDescriptor, LayerDescriptor,
    QuadTransformDescriptor,
};
use crate::scene::{LayerSource, ScenePlan, build_scene};
use crate::source::{FrameSource, RgbaImage, SourceFrame};

/// Renders a [`ScenePlan`]: fetches each layer's pixels from a
/// [`FrameSource`], uploads them (only when their key changed), and
/// composites the frame on the GPU.
///
/// Keep one per output (preview, export) so uploads are reused frame to
/// frame. Textures not used by a frame are released after it.
pub struct Renderer {
    compositor: Compositor,
}

impl Renderer {
    pub fn new(context: &GpuContext) -> Self {
        Self {
            compositor: Compositor::new(context),
        }
    }

    pub fn compositor(&mut self) -> &mut Compositor {
        &mut self.compositor
    }

    /// The current scene of `project` at `time`, read back as RGBA at the
    /// project's canvas size.
    pub fn render_project(
        &mut self,
        context: &GpuContext,
        project: &Project,
        time: MediaTime,
        sources: &mut dyn FrameSource,
    ) -> Result<RgbaImage, CompositorError> {
        self.render_rgba(context, &build_scene(project, time), sources)
    }

    /// `plan`, read back as RGBA at the plan's canvas size.
    pub fn render_rgba(
        &mut self,
        context: &GpuContext,
        plan: &ScenePlan,
        sources: &mut dyn FrameSource,
    ) -> Result<RgbaImage, CompositorError> {
        let frame = self.prepare(context, plan, sources);
        let data = self.compositor.render_frame_to_rgba(context, &frame)?;
        Ok(RgbaImage::new(frame.width, frame.height, data))
    }

    /// `plan` as a texture in [`GpuContext::texture_format`], valid until
    /// the next render (for presenting to a surface).
    pub fn render_texture(
        &mut self,
        context: &GpuContext,
        plan: &ScenePlan,
        sources: &mut dyn FrameSource,
    ) -> Result<wgpu::Texture, CompositorError> {
        let frame = self.prepare(context, plan, sources);
        self.compositor.render_frame_to_texture(context, &frame)
    }

    /// Fetches and uploads the plan's sources and lays out its layers.
    /// Layers whose source gives no frame are left out.
    pub fn prepare(
        &mut self,
        context: &GpuContext,
        plan: &ScenePlan,
        sources: &mut dyn FrameSource,
    ) -> FrameDescriptor {
        let canvas = plan.canvas;
        let mut used = HashSet::new();
        let mut items = Vec::with_capacity(plan.layers.len());
        for layer in &plan.layers {
            let (texture_id, transform) = match &layer.source {
                LayerSource::Color(color) => {
                    let texture_id = self.upload_color(context, *color);
                    let transform =
                        QuadTransformDescriptor::full_canvas(canvas.width, canvas.height);
                    (texture_id, transform)
                }
                LayerSource::Video { asset, source_time } => {
                    let Some(frame) = non_empty(sources.video_frame(asset, *source_time)) else {
                        continue;
                    };
                    self.upload_source(context, frame, |image| {
                        layer.transform.quad(canvas, image.width, image.height)
                    })
                }
                LayerSource::Image { asset } => {
                    let Some(frame) = non_empty(sources.image(asset)) else {
                        continue;
                    };
                    self.upload_source(context, frame, |image| {
                        layer.transform.quad(canvas, image.width, image.height)
                    })
                }
            };
            used.insert(texture_id.clone());
            items.push(FrameItemDescriptor::Layer(LayerDescriptor {
                texture_id,
                transform,
                opacity: layer.opacity,
                blend_mode: layer.blend_mode,
            }));
        }
        self.compositor.retain_textures(|id| used.contains(id));
        FrameDescriptor {
            width: canvas.width,
            height: canvas.height,
            clear: CanvasClearDescriptor { color: plan.clear },
            items,
        }
    }

    fn upload_color(&mut self, context: &GpuContext, color: [f32; 4]) -> String {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rgba = color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
        let texture_id = format!(
            "color:{:02x}{:02x}{:02x}{:02x}",
            rgba[0], rgba[1], rgba[2], rgba[3]
        );
        if !self.compositor.has_texture(&texture_id) {
            let texture = context.upload_rgba_texture(1, 1, &rgba, "compositor-color");
            self.compositor.upsert_texture(texture_id.clone(), texture);
        }
        texture_id
    }

    fn upload_source(
        &mut self,
        context: &GpuContext,
        frame: SourceFrame,
        transform: impl FnOnce(&RgbaImage) -> QuadTransformDescriptor,
    ) -> (String, QuadTransformDescriptor) {
        let texture_id = format!("source:{}", frame.key);
        if !self.compositor.has_texture(&texture_id) {
            let image = &frame.image;
            let texture = context.upload_rgba_texture(
                image.width,
                image.height,
                &image.data,
                "compositor-source",
            );
            self.compositor.upsert_texture(texture_id.clone(), texture);
        }
        (texture_id, transform(&frame.image))
    }
}

fn non_empty(frame: Option<SourceFrame>) -> Option<SourceFrame> {
    frame.filter(|frame| frame.image.width > 0 && frame.image.height > 0)
}
