use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use opencut_gpu::{GpuContext, begin_clear_pass, wgpu};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::BlendMode;
use crate::frame::{FrameDescriptor, FrameItemDescriptor, LayerDescriptor};
use crate::texture_pool::TexturePool;

const LAYER_SHADER_SOURCE: &str = include_str!("shaders/layer.wgsl");
const BLEND_SHADER_SOURCE: &str = include_str!("shaders/blend.wgsl");

#[derive(Debug, Error)]
pub enum CompositorError {
    #[error("texture '{texture_id}' is not available")]
    MissingTexture { texture_id: String },
    #[error("cannot render an empty {width}x{height} frame")]
    EmptyFrame { width: u32, height: u32 },
    #[error(transparent)]
    Gpu(#[from] opencut_gpu::GpuError),
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LayerUniformBuffer {
    resolution: [f32; 2],
    center: [f32; 2],
    size: [f32; 2],
    rotation_radians: f32,
    opacity: f32,
    flip_x: f32,
    flip_y: f32,
    // Uniform buffers must be a multiple of 16 bytes on WebGL (40 -> 48).
    _padding: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlendUniformBuffer {
    blend_mode: u32,
    _padding: [u32; 3],
}

/// Composites a [`FrameDescriptor`] on the GPU: each layer is drawn into a
/// canvas-sized texture (transform, flip, opacity), then blended onto the
/// scene so far with its blend mode.
///
/// Ported from the classic `rust/crates/compositor`. Effect passes, masks and
/// scene effects are not ported yet (P8).
pub struct Compositor {
    textures: HashMap<String, wgpu::Texture>,
    texture_pool: TexturePool,
    uniform_bind_group_layout: wgpu::BindGroupLayout,
    layer_pipeline: wgpu::RenderPipeline,
    blend_pipeline: wgpu::RenderPipeline,
}

impl Compositor {
    pub fn new(context: &GpuContext) -> Self {
        let device = context.device();
        let layer_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor-layer-shader"),
            source: wgpu::ShaderSource::Wgsl(LAYER_SHADER_SOURCE.into()),
        });
        let blend_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor-blend-shader"),
            source: wgpu::ShaderSource::Wgsl(BLEND_SHADER_SOURCE.into()),
        });
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("compositor-uniform-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let texture_layout = context.texture_sampler_bind_group_layout();
        let layer_pipeline = context.fullscreen_pipeline(
            "compositor-layer-pipeline",
            &[texture_layout, &uniform_bind_group_layout],
            &layer_shader,
        );
        let blend_pipeline = context.fullscreen_pipeline(
            "compositor-blend-pipeline",
            &[texture_layout, texture_layout, &uniform_bind_group_layout],
            &blend_shader,
        );

        Self {
            textures: HashMap::new(),
            texture_pool: TexturePool::default(),
            uniform_bind_group_layout,
            layer_pipeline,
            blend_pipeline,
        }
    }

    /// Registers (or replaces) a source texture layers can refer to.
    pub fn upsert_texture(&mut self, id: impl Into<String>, texture: wgpu::Texture) {
        self.textures.insert(id.into(), texture);
    }

    pub fn has_texture(&self, id: &str) -> bool {
        self.textures.contains_key(id)
    }

    pub fn release_texture(&mut self, id: &str) {
        self.textures.remove(id);
    }

    /// Drops every source texture `keep` returns `false` for.
    pub fn retain_textures(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.textures.retain(|id, _| keep(id));
    }

    /// Composites the frame into a texture of [`GpuContext::texture_format`]
    /// and returns it. It stays valid until the next render.
    pub fn render_frame_to_texture(
        &mut self,
        context: &GpuContext,
        frame: &FrameDescriptor,
    ) -> Result<wgpu::Texture, CompositorError> {
        if frame.width == 0 || frame.height == 0 {
            return Err(CompositorError::EmptyFrame {
                width: frame.width,
                height: frame.height,
            });
        }
        self.texture_pool.recycle_frame();
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor-frame-encoder"),
                });
        let mut scene = self.create_cleared_texture(context, &mut encoder, frame);

        for item in &frame.items {
            match item {
                FrameItemDescriptor::Layer(layer) => {
                    let layer_texture = self.render_layer(context, &mut encoder, frame, layer)?;
                    scene = self.blend_texture(
                        context,
                        &mut encoder,
                        &scene,
                        &layer_texture,
                        layer.blend_mode,
                        frame,
                    );
                }
            }
        }

        context.queue().submit([encoder.finish()]);
        Ok(scene)
    }

    /// Composites the frame and presents it on a configured `surface`.
    pub fn render_frame_to_surface(
        &mut self,
        context: &GpuContext,
        frame: &FrameDescriptor,
        surface: &wgpu::Surface<'_>,
    ) -> Result<(), CompositorError> {
        let scene = self.render_frame_to_texture(context, frame)?;
        context.present_texture_to_surface(&scene, surface)?;
        Ok(())
    }

    /// Composites the frame and reads it back as straight-alpha RGBA.
    pub fn render_frame_to_rgba(
        &mut self,
        context: &GpuContext,
        frame: &FrameDescriptor,
    ) -> Result<Vec<u8>, CompositorError> {
        let scene = self.render_frame_to_texture(context, frame)?;
        Ok(context.read_texture_rgba(&scene)?)
    }

    fn render_layer(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        frame: &FrameDescriptor,
        layer: &LayerDescriptor,
    ) -> Result<wgpu::Texture, CompositorError> {
        let source = self.textures.get(&layer.texture_id).ok_or_else(|| {
            CompositorError::MissingTexture {
                texture_id: layer.texture_id.clone(),
            }
        })?;
        let source_bind_group = context.texture_bind_group(
            source,
            context.linear_sampler(),
            "compositor-layer-source-bind-group",
        );
        #[allow(clippy::cast_precision_loss)]
        let uniforms = LayerUniformBuffer {
            resolution: [frame.width as f32, frame.height as f32],
            center: [layer.transform.center_x, layer.transform.center_y],
            size: [layer.transform.width, layer.transform.height],
            rotation_radians: layer.transform.rotation_degrees.to_radians(),
            opacity: layer.opacity,
            flip_x: if layer.transform.flip_x { 1.0 } else { 0.0 },
            flip_y: if layer.transform.flip_y { 1.0 } else { 0.0 },
            _padding: [0.0; 2],
        };
        let uniform_bind_group = self.uniform_bind_group(context, bytemuck::bytes_of(&uniforms));

        let target =
            self.texture_pool
                .acquire(context, frame.width, frame.height, "compositor-layer");
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = begin_clear_pass(encoder, &target_view, "compositor-layer-pass");
            pass.set_pipeline(&self.layer_pipeline);
            pass.set_vertex_buffer(0, context.fullscreen_quad().slice(..));
            pass.set_bind_group(0, &source_bind_group, &[]);
            pass.set_bind_group(1, &uniform_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        Ok(target)
    }

    fn create_cleared_texture(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        frame: &FrameDescriptor,
    ) -> wgpu::Texture {
        let texture = self.texture_pool.acquire(
            context,
            frame.width,
            frame.height,
            "compositor-cleared-texture",
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let [r, g, b, a] = frame.clear.color.map(f64::from);
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compositor-clear-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        texture
    }

    fn blend_texture(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        base: &wgpu::Texture,
        layer: &wgpu::Texture,
        blend_mode: BlendMode,
        frame: &FrameDescriptor,
    ) -> wgpu::Texture {
        let base_bind_group = context.texture_bind_group(
            base,
            context.linear_sampler(),
            "compositor-base-bind-group",
        );
        let layer_bind_group = context.texture_bind_group(
            layer,
            context.linear_sampler(),
            "compositor-layer-bind-group",
        );
        let uniforms = BlendUniformBuffer {
            blend_mode: blend_mode.shader_code(),
            _padding: [0; 3],
        };
        let uniform_bind_group = self.uniform_bind_group(context, bytemuck::bytes_of(&uniforms));

        let target = self.texture_pool.acquire(
            context,
            frame.width,
            frame.height,
            "compositor-blended-texture",
        );
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = begin_clear_pass(encoder, &target_view, "compositor-blend-pass");
            pass.set_pipeline(&self.blend_pipeline);
            pass.set_vertex_buffer(0, context.fullscreen_quad().slice(..));
            pass.set_bind_group(0, &base_bind_group, &[]);
            pass.set_bind_group(1, &layer_bind_group, &[]);
            pass.set_bind_group(2, &uniform_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        target
    }

    fn uniform_bind_group(&self, context: &GpuContext, contents: &[u8]) -> wgpu::BindGroup {
        let buffer = context
            .device()
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("compositor-uniform-buffer"),
                contents,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("compositor-uniform-bind-group"),
                layout: &self.uniform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            })
    }
}
