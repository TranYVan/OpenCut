use std::sync::mpsc;

use wgpu::util::DeviceExt;

use crate::{FULLSCREEN_SHADER_SOURCE, GpuError};

const BLIT_SHADER_SOURCE: &str = include_str!("shaders/blit.wgsl");

const FULLSCREEN_QUAD_POSITIONS: [[f32; 2]; 6] = [
    [-1.0, -1.0],
    [1.0, -1.0],
    [-1.0, 1.0],
    [-1.0, 1.0],
    [1.0, -1.0],
    [1.0, 1.0],
];

/// What kind of adapter the context runs on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterKind {
    /// A real GPU (integrated, discrete or virtual).
    Hardware,
    /// A CPU rasterizer: lavapipe/llvmpipe, WARP, SwiftShader.
    Software,
}

pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture_format: wgpu::TextureFormat,
    fullscreen_quad: wgpu::Buffer,
    linear_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    texture_sampler_bind_group_layout: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
}

impl GpuContext {
    /// A context on the best adapter: hardware first, then the platform's
    /// software fallback. See the crate docs for the environment overrides.
    pub async fn new() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = Self::request_adapter(&instance).await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("gpu-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            })
            .await?;
        Ok(Self::from_parts(instance, adapter, device, queue))
    }

    /// [`Self::new`], blocking the current thread.
    pub fn new_blocking() -> Result<Self, GpuError> {
        pollster::block_on(Self::new())
    }

    async fn request_adapter(instance: &wgpu::Instance) -> Result<wgpu::Adapter, GpuError> {
        if std::env::var_os("WGPU_ADAPTER_NAME").is_some() {
            return wgpu::util::initialize_adapter_from_env(instance, None)
                .await
                .map_err(|_| GpuError::AdapterUnavailable);
        }
        for force_fallback_adapter in [false, true] {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter,
                })
                .await;
            if let Ok(adapter) = adapter {
                return Ok(adapter);
            }
        }
        Err(GpuError::AdapterUnavailable)
    }

    fn from_parts(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Self {
        // Bgra8Unorm is what window surfaces support everywhere but GL.
        let texture_format = if adapter.get_info().backend == wgpu::Backend::Gl {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Bgra8Unorm
        };
        let fullscreen_quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpu-fullscreen-quad-buffer"),
            contents: bytemuck::cast_slice(&FULLSCREEN_QUAD_POSITIONS),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let sampler = |label, filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        };
        let linear_sampler = sampler("gpu-linear-sampler", wgpu::FilterMode::Linear);
        let nearest_sampler = sampler("gpu-nearest-sampler", wgpu::FilterMode::Nearest);
        let texture_sampler_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gpu-texture-sampler-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let blit_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu-blit-shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER_SOURCE.into()),
        });
        let blit_pipeline = Self::build_fullscreen_pipeline(
            &device,
            texture_format,
            "gpu-blit",
            &[&texture_sampler_bind_group_layout],
            &blit_shader_module,
        );

        Self {
            instance,
            adapter,
            device,
            queue,
            texture_format,
            fullscreen_quad,
            linear_sampler,
            nearest_sampler,
            texture_sampler_bind_group_layout,
            blit_pipeline,
        }
    }

    /// A render pipeline drawing [`Self::fullscreen_quad`] with
    /// [`FULLSCREEN_SHADER_SOURCE`] as the vertex stage and `fragment`'s
    /// `fragment_main`, writing `format` with no blending.
    pub fn fullscreen_pipeline(
        &self,
        label: &str,
        bind_group_layouts: &[&wgpu::BindGroupLayout],
        fragment: &wgpu::ShaderModule,
    ) -> wgpu::RenderPipeline {
        Self::build_fullscreen_pipeline(
            &self.device,
            self.texture_format,
            label,
            bind_group_layouts,
            fragment,
        )
    }

    fn build_fullscreen_pipeline(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        label: &str,
        bind_group_layouts: &[&wgpu::BindGroupLayout],
        fragment: &wgpu::ShaderModule,
    ) -> wgpu::RenderPipeline {
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu-fullscreen-shader"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_SHADER_SOURCE.into()),
        });
        let layouts: Vec<Option<&wgpu::BindGroupLayout>> =
            bind_group_layouts.iter().copied().map(Some).collect();
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some("vertex_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 2]>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: fragment,
                entry_point: Some("fragment_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    /// A texture in [`Self::texture_format`] that passes render into, sample
    /// from and copy to and from.
    pub fn create_render_texture(&self, width: u32, height: u32, label: &str) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    /// Uploads straight-alpha RGBA pixels (`width * 4` bytes per row, no
    /// padding) as a sampleable `Rgba8Unorm` texture.
    ///
    /// # Panics
    /// When `rgba` is not `width * height * 4` bytes.
    pub fn upload_rgba_texture(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        label: &str,
    ) -> wgpu::Texture {
        assert_eq!(
            rgba.len(),
            width as usize * height as usize * 4,
            "RGBA data does not match {width}x{height}"
        );
        self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: extent(width, height),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        )
    }

    /// Reads a texture back as RGBA (`width * 4` bytes per row, no padding),
    /// waiting for the GPU. The texture needs `COPY_SRC` and must be
    /// `Rgba8Unorm` or `Bgra8Unorm`.
    pub fn read_texture_rgba(&self, texture: &wgpu::Texture) -> Result<Vec<u8>, GpuError> {
        let format = texture.format();
        let swap_red_blue = match format {
            wgpu::TextureFormat::Rgba8Unorm => false,
            wgpu::TextureFormat::Bgra8Unorm => true,
            other => return Err(GpuError::UnsupportedReadbackFormat(other)),
        };
        let (width, height) = (texture.width(), texture.height());
        let row_bytes = width * 4;
        let padded_row_bytes = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-readback-buffer"),
            size: u64::from(padded_row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu-readback-encoder"),
            });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height),
                },
            },
            extent(width, height),
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| GpuError::Readback(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| GpuError::Readback(error.to_string()))?
            .map_err(|error| GpuError::Readback(error.to_string()))?;

        let mut pixels = Vec::with_capacity(row_bytes as usize * height as usize);
        {
            let mapped = slice.get_mapped_range();
            for row in mapped.chunks_exact(padded_row_bytes as usize) {
                pixels.extend_from_slice(&row[..row_bytes as usize]);
            }
        }
        buffer.unmap();
        if swap_red_blue {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Ok(pixels)
    }

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// Whether this runs on a GPU or a CPU rasterizer.
    pub fn adapter_kind(&self) -> AdapterKind {
        if self.adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            AdapterKind::Software
        } else {
            AdapterKind::Hardware
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The format of render textures (and of surfaces this context presents
    /// to): `Bgra8Unorm`, or `Rgba8Unorm` on GL.
    pub fn texture_format(&self) -> wgpu::TextureFormat {
        self.texture_format
    }

    pub fn fullscreen_quad(&self) -> &wgpu::Buffer {
        &self.fullscreen_quad
    }

    pub fn linear_sampler(&self) -> &wgpu::Sampler {
        &self.linear_sampler
    }

    pub fn nearest_sampler(&self) -> &wgpu::Sampler {
        &self.nearest_sampler
    }

    /// Group layout of one filterable 2D texture (binding 0) and a sampler
    /// (binding 1), used by every pass that samples a texture.
    pub fn texture_sampler_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.texture_sampler_bind_group_layout
    }

    /// A bind group for [`Self::texture_sampler_bind_group_layout`].
    pub fn texture_bind_group(
        &self,
        texture: &wgpu::Texture,
        sampler: &wgpu::Sampler,
        label: &str,
    ) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.texture_sampler_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    pub fn blit_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.blit_pipeline
    }

    /// Configures `surface` for presenting `width`×`height` frames.
    pub fn configure_surface(
        &self,
        surface: &wgpu::Surface<'_>,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        let caps = surface.get_capabilities(&self.adapter);
        if !caps.formats.contains(&self.texture_format) {
            return Err(GpuError::UnsupportedSurfaceFormat);
        }
        surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.texture_format,
                width,
                height,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: caps
                    .alpha_modes
                    .first()
                    .copied()
                    .unwrap_or(wgpu::CompositeAlphaMode::Auto),
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
        Ok(())
    }

    pub fn acquire_surface_texture(
        &self,
        surface: &wgpu::Surface<'_>,
    ) -> Result<wgpu::SurfaceTexture, GpuError> {
        match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(surface_texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(surface_texture) => Ok(surface_texture),
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost
            | wgpu::CurrentSurfaceTexture::Validation => Err(GpuError::SurfaceUnavailable),
        }
    }

    /// Blits `texture` to a configured `surface` and presents it.
    pub fn present_texture_to_surface(
        &self,
        texture: &wgpu::Texture,
        surface: &wgpu::Surface<'_>,
    ) -> Result<(), GpuError> {
        let surface_texture = self.acquire_surface_texture(surface)?;
        let target_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu-surface-blit-encoder"),
            });
        self.encode_texture_blit_to_view(&mut encoder, texture, &target_view, "gpu-surface-blit");
        self.queue.submit([encoder.finish()]);
        surface_texture.present();
        Ok(())
    }

    /// Records a pass copying `texture` onto `target_view`, scaling with
    /// linear filtering when the sizes differ.
    pub fn encode_texture_blit_to_view(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        target_view: &wgpu::TextureView,
        label: &str,
    ) {
        let bind_group =
            self.texture_bind_group(texture, &self.linear_sampler, "gpu-blit-bind-group");
        let mut render_pass = begin_clear_pass(encoder, target_view, label);
        render_pass.set_pipeline(&self.blit_pipeline);
        render_pass.set_vertex_buffer(0, self.fullscreen_quad.slice(..));
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}

/// Starts a render pass onto `view` that first clears it to transparent.
pub fn begin_clear_pass<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'encoder> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        occlusion_query_set: None,
        timestamp_writes: None,
        multiview_mask: None,
    })
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}
