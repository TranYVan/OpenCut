//! Upload, blit and read back. Skips (with a message) on machines without
//! any adapter, unless `OPENCUT_REQUIRE_GPU=1`.

use opencut_gpu::{GpuContext, wgpu};

fn gpu() -> Option<GpuContext> {
    match GpuContext::new_blocking() {
        Ok(context) => Some(context),
        Err(error) if std::env::var_os("OPENCUT_REQUIRE_GPU").is_some() => {
            panic!("OPENCUT_REQUIRE_GPU is set but no adapter is available: {error}")
        }
        Err(error) => {
            eprintln!("SKIPPED: {error}");
            None
        }
    }
}

/// 5×3 (odd width, so rows need padding to 256 bytes) of distinct pixels.
fn pattern() -> Vec<u8> {
    (0..15u8)
        .flat_map(|index| [index * 16, 255 - index * 8, index, 128 + index])
        .collect()
}

#[test]
fn blit_then_read_back_is_lossless() {
    let Some(context) = gpu() else { return };
    let source = context.upload_rgba_texture(5, 3, &pattern(), "source");
    let target = context.create_render_texture(5, 3, "target");
    let mut encoder = context
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    context.encode_texture_blit_to_view(&mut encoder, &source, &view, "blit");
    context.queue().submit([encoder.finish()]);

    assert_eq!(context.read_texture_rgba(&target).unwrap(), pattern());
}

#[test]
fn rejects_unsupported_readback_formats() {
    let Some(context) = gpu() else { return };
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    assert!(context.read_texture_rgba(&texture).is_err());
}
