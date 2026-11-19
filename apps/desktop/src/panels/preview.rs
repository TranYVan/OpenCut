use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    Context, Entity, ObjectFit, RenderImage, SharedString, Subscription, Window, div, img,
    prelude::*,
};
use opencut_compositor::gpu::GpuContext;
use opencut_compositor::{MediaSources, Renderer, ScenePlan, build_scene};
use opencut_editor::model::MediaAsset;

use crate::media::ProjectFiles;
use crate::theme::ActiveTheme;
use crate::workspace::Workspace;

type ResolvePath = Box<dyn FnMut(&MediaAsset) -> Option<PathBuf> + Send>;

/// The GPU and the open decoders. Lives on the background executor while a
/// frame renders, and is handed back to the panel between renders.
struct FrameRenderer {
    gpu: GpuContext,
    renderer: Renderer,
    sources: MediaSources<ResolvePath>,
}

impl FrameRenderer {
    fn new(files: ProjectFiles) -> Result<Self, String> {
        let gpu = GpuContext::new_blocking().map_err(|error| error.to_string())?;
        let resolve: ResolvePath = Box::new(move |asset| Some(files.media_path(asset)));
        Ok(Self {
            renderer: Renderer::new(&gpu),
            sources: MediaSources::new(resolve),
            gpu,
        })
    }

    /// Composites `plan` on the GPU and reads it back as an image GPUI can
    /// draw (GPUI 0.2.2 can't draw wgpu textures).
    // shortcut: full-canvas readback + CPU copy per frame; upgrade to shared
    // textures if 1080p scrubbing measures too slow.
    fn render(&mut self, plan: &ScenePlan) -> Result<Arc<RenderImage>, String> {
        let frame = self
            .renderer
            .render_rgba(&self.gpu, plan, &mut self.sources)
            .map_err(|error| error.to_string())?;
        let mut bgra = frame.data;
        // `RenderImage` takes BGRA bytes in an `RgbaImage` container.
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, bgra)
            .ok_or("the rendered frame has the wrong size")?;
        Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
    }
}

/// The composited frame at the playhead, fitted to the panel.
///
/// One render runs at a time, off the UI thread. Changes made while it runs
/// (scrubbing, edits) are coalesced: when it finishes, only the latest
/// project state is rendered next, so stale frames are never queued.
pub(crate) struct Preview {
    workspace: Entity<Workspace>,
    /// `None` before the first render and while one is running.
    renderer: Option<FrameRenderer>,
    rendering: bool,
    /// The scene of the frame shown or being rendered.
    plan: Option<ScenePlan>,
    frame: Option<Arc<RenderImage>>,
    error: Option<SharedString>,
    _workspace_changed: Subscription,
}

impl Preview {
    pub(crate) fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let workspace_changed = cx.observe(&workspace, |this, _, cx| this.update_frame(cx));
        let mut this = Self {
            workspace,
            renderer: None,
            rendering: false,
            plan: None,
            frame: None,
            error: None,
            _workspace_changed: workspace_changed,
        };
        this.update_frame(cx);
        this
    }

    /// Renders the frame at the playhead unless it's already shown or a
    /// render is running (which calls this again when it finishes).
    fn update_frame(&mut self, cx: &mut Context<Self>) {
        if self.rendering {
            return;
        }
        let workspace = self.workspace.read(cx);
        let editor = workspace.editor();
        // The background colour comes with the scene.
        // shortcut: blur backgrounds draw as the plain clear colour because
        // the compositor has no blur yet (P8); they show once it's ported.
        let plan = build_scene(editor.project(), editor.playback().position);
        if self.plan.as_ref() == Some(&plan) {
            return;
        }
        self.plan = Some(plan.clone());
        self.rendering = true;
        let renderer = self.renderer.take();
        let files = workspace.files().clone();

        let work = cx.background_executor().spawn(async move {
            let mut renderer = match renderer {
                Some(renderer) => renderer,
                None => match FrameRenderer::new(files) {
                    Ok(renderer) => renderer,
                    Err(error) => return (None, Err(error)),
                },
            };
            let frame = renderer.render(&plan);
            (Some(renderer), frame)
        });
        cx.spawn(async move |this, cx| {
            let (renderer, frame) = work.await;
            let _ = this.update(cx, |this, cx| {
                this.rendering = false;
                this.renderer = renderer;
                match frame {
                    Ok(frame) => {
                        // Free the old frame's GPU atlas slot; it's never drawn again.
                        if let Some(old) = this.frame.replace(frame) {
                            cx.drop_image(old, None);
                        }
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
                this.update_frame(cx);
            });
        })
        .detach();
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;

        div()
            .flex()
            .w_1_2()
            .h_full()
            .min_h_0()
            .items_center()
            .justify_center()
            .bg(colors.background)
            .map(|panel| match (&self.frame, &self.error) {
                (_, Some(error)) => panel.text_color(colors.destructive).child(error.clone()),
                (Some(frame), None) => panel.child(
                    img(Arc::clone(frame))
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                ),
                (None, None) => panel,
            })
    }
}

#[cfg(test)]
mod tests {
    use opencut_editor::model::CanvasSize;

    use super::*;

    #[test]
    fn renders_the_scene_as_bgra() {
        let Ok(mut renderer) = FrameRenderer::new(ProjectFiles::new(PathBuf::new())) else {
            eprintln!("skipped: no GPU adapter");
            return;
        };
        let plan = ScenePlan {
            canvas: CanvasSize {
                width: 4,
                height: 2,
            },
            clear: [1.0, 0.0, 0.0, 1.0],
            layers: Vec::new(),
        };
        let frame = renderer.render(&plan).unwrap();
        let bytes = frame.as_bytes(0).unwrap();
        assert_eq!(bytes.len(), 4 * 2 * 4);
        // Red, as BGRA.
        assert_eq!(&bytes[..4], &[0, 0, 255, 255]);
    }
}
