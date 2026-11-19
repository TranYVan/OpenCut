//! Renders a project at a given time into an image.
//!
//! 1. [`build_scene`] turns a [`opencut_model::Project`] and a time into a
//!    [`ScenePlan`]: the visible video/image layers bottom to top, with
//!    transform, opacity and blend mode. Pure, no GPU.
//! 2. [`Renderer`] fetches each layer's pixels from a [`FrameSource`]
//!    (decoded video frames, images), uploads them, and composites the frame
//!    on the GPU with [`Compositor`], returning an [`RgbaImage`] (headless,
//!    export) or a texture (preview).
//!
//! The compositor and its shaders are ported from the classic app's
//! `rust/crates/compositor` (tag `pre-rewrite`); the scene builder from
//! `apps/web/src/services/renderer`. Effects, masks, text, stickers,
//! graphics, the blur background and keyframes are not ported yet: see
//! [`scene`].
//!
//! With the `media` feature, `MediaSources` implements [`FrameSource`] over
//! `opencut-media` decoders.

mod blend_mode;
mod compositor;
mod frame;
#[cfg(feature = "media")]
mod media_sources;
mod renderer;
pub mod scene;
mod source;
mod texture_pool;

pub use blend_mode::{BlendMode, UnknownBlendMode};
pub use compositor::{Compositor, CompositorError};
pub use frame::{
    CanvasClearDescriptor, FrameDescriptor, FrameItemDescriptor, LayerDescriptor,
    QuadTransformDescriptor,
};
#[cfg(feature = "media")]
pub use media_sources::MediaSources;
pub use renderer::Renderer;
pub use scene::{
    ElementTransform, LayerSource, SceneLayer, ScenePlan, build_scene, build_scene_from,
};
pub use source::{FrameSource, RgbaImage, SourceFrame};

pub use opencut_gpu as gpu;
