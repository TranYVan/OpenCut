use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    Bounds, Context, Entity, ObjectFit, Pixels, RenderImage, SharedString, Subscription, Window,
    canvas, div, fill, img, point, prelude::*, px, size,
};
use opencut_compositor::gpu::GpuContext;
use opencut_compositor::{MediaSources, Renderer, ScenePlan, build_scene};
use opencut_editor::model::{CanvasSize, MediaAsset};

use crate::components::{Button, ButtonSize, ButtonVariant};
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
    /// Draws the rule-of-thirds guide over the frame (classic grid guide).
    show_grid: bool,
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
            show_grid: false,
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;
        let canvas_size = self
            .workspace
            .read(cx)
            .editor()
            .project()
            .settings
            .canvas_size;
        let line_color = colors.foreground.opacity(0.35);
        let grid = self.show_grid.then(|| {
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    for line in grid_lines(bounds, canvas_size, 3) {
                        window.paint_quad(fill(line, line_color));
                    }
                },
            )
            .absolute()
            .size_full()
        });

        div()
            .relative()
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
            .children(grid)
            .child(
                div().absolute().top(px(8.0)).right(px(8.0)).child(
                    Button::new("preview-grid", "Grid")
                        .variant(if self.show_grid {
                            ButtonVariant::Secondary
                        } else {
                            ButtonVariant::Ghost
                        })
                        .size(ButtonSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_grid = !this.show_grid;
                            cx.notify();
                        })),
                ),
            )
    }
}

/// The 1px lines splitting the canvas, fitted inside `bounds` like the
/// frame (`ObjectFit::Contain`), into `cells` × `cells`.
fn grid_lines(bounds: Bounds<Pixels>, canvas: CanvasSize, cells: u32) -> Vec<Bounds<Pixels>> {
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let scale = (width / canvas.width as f32).min(height / canvas.height as f32);
    let (fit_w, fit_h) = (canvas.width as f32 * scale, canvas.height as f32 * scale);
    let left = f32::from(bounds.origin.x) + (width - fit_w) / 2.0;
    let top = f32::from(bounds.origin.y) + (height - fit_h) / 2.0;
    (1..cells)
        .flat_map(|i| {
            let t = i as f32 / cells as f32;
            [
                Bounds::new(
                    point(px(left + fit_w * t), px(top)),
                    size(px(1.0), px(fit_h)),
                ),
                Bounds::new(
                    point(px(left), px(top + fit_h * t)),
                    size(px(fit_w), px(1.0)),
                ),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_fits_the_canvas_inside_the_panel() {
        // A 16:9 canvas in a square panel is letterboxed top and bottom.
        let panel = Bounds::new(point(px(0.0), px(0.0)), size(px(160.0), px(160.0)));
        let canvas = CanvasSize {
            width: 1600,
            height: 900,
        };
        let lines = grid_lines(panel, canvas, 3);
        assert_eq!(lines.len(), 4);
        let first_vertical = lines[0];
        assert_eq!(first_vertical.origin.y, px(35.0));
        assert_eq!(first_vertical.size.height, px(90.0));
        let first_horizontal = lines[1];
        assert_eq!(first_horizontal.origin.y, px(65.0));
        assert_eq!(first_horizontal.size.width, px(160.0));
    }

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
