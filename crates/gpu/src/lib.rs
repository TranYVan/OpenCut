//! The GPU context the compositor (and later effects and masks) renders with.
//!
//! Ported from the classic app's `rust/crates/gpu` (tag `pre-rewrite`),
//! without the browser paths (WebGL canvas fallback, offscreen canvas
//! import): desktop and headless use only native wgpu backends.
//!
//! - [`GpuContext`] owns the device and queue plus what every pass shares:
//!   a fullscreen quad, linear/nearest samplers, the texture+sampler bind
//!   group layout and a blit pipeline.
//! - [`GpuContext::read_texture_rgba`] reads a render texture back as
//!   tightly packed RGBA, for headless rendering, export and tests.
//!
//! Adapter choice: a hardware adapter first, then the platform's software
//! fallback (Vulkan lavapipe/llvmpipe on Linux, WARP on Windows), so
//! headless machines without a GPU can still render. `WGPU_ADAPTER_NAME` and
//! `WGPU_BACKEND` override the choice.

mod context;

use thiserror::Error;

pub use context::{AdapterKind, GpuContext, begin_clear_pass};
pub use wgpu;

/// The vertex stage shared by every fullscreen pass: a quad from
/// [`GpuContext::fullscreen_quad`] with `tex_coord` (0,0) at the top left.
pub const FULLSCREEN_SHADER_SOURCE: &str = include_str!("shaders/fullscreen.wgsl");

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no GPU adapter is available (not even a software fallback)")]
    AdapterUnavailable,
    #[error("failed to request a GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("the output surface does not support the required texture format")]
    UnsupportedSurfaceFormat,
    #[error("the surface texture could not be acquired")]
    SurfaceUnavailable,
    #[error("cannot read back a texture of format {0:?}")]
    UnsupportedReadbackFormat(wgpu::TextureFormat),
    #[error("reading the texture back failed: {0}")]
    Readback(String),
}
