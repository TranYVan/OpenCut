use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    Context, Entity, ObjectFit, RenderImage, SharedString, Subscription, Window, div, img,
    prelude::*, px,
};
use opencut_compositor::gpu::GpuContext;
use opencut_compositor::{MediaSources, Renderer, ScenePlan, build_scene};
use opencut_editor::commands::UpdateProjectSettings;
use opencut_editor::model::time::{TimeCodeFormat, format_timecode};
use opencut_editor::model::{CanvasSize, MediaAsset};

use crate::components::{Button, ButtonSize, ButtonVariant};
use crate::media::ProjectFiles;
use crate::theme::ActiveTheme;
use crate::workspace::{Transport, Workspace};

/// Zoom steps the zoom button cycles through; `None` fits the panel.
const ZOOMS: [Option<f32>; 4] = [None, Some(0.5), Some(1.0), Some(2.0)];

/// Canvas presets the ratio button cycles through (classic's common sizes).
const RATIOS: [(&str, CanvasSize); 4] = [
    (
        "16:9",
        CanvasSize {
            width: 1920,
            height: 1080,
        },
    ),
    (
        "9:16",
        CanvasSize {
            width: 1080,
            height: 1920,
        },
    ),
    (
        "1:1",
        CanvasSize {
            width: 1080,
            height: 1080,
        },
    ),
    (
        "4:3",
        CanvasSize {
            width: 1440,
            height: 1080,
        },
    ),
];

/// The preset after the one matching `size` by aspect ratio; the first one
/// for a custom size.
fn next_ratio(size: CanvasSize) -> CanvasSize {
    let current = RATIOS.iter().position(|(_, preset)| {
        u64::from(preset.width) * u64::from(size.height)
            == u64::from(size.width) * u64::from(preset.height)
    });
    current.map_or(RATIOS[0].1, |index| RATIOS[(index + 1) % RATIOS.len()].1)
}

fn ratio_label(size: CanvasSize) -> &'static str {
    RATIOS
        .iter()
        .find(|(_, preset)| {
            u64::from(preset.width) * u64::from(size.height)
                == u64::from(size.width) * u64::from(preset.height)
        })
        .map_or("Custom", |(label, _)| label)
}

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
    /// Index into [`ZOOMS`].
    zoom: usize,
    /// Filling the window (the shell hides the other panels).
    fullscreen: bool,
    _workspace_changed: Subscription,
}

impl Preview {
    pub(crate) fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let workspace_changed = cx.observe(&workspace, |this, _, cx| {
            this.update_frame(cx);
            // The time and play state show even when the frame is unchanged.
            cx.notify();
        });
        let mut this = Self {
            workspace,
            renderer: None,
            rendering: false,
            plan: None,
            frame: None,
            error: None,
            zoom: 0,
            fullscreen: false,
            _workspace_changed: workspace_changed,
        };
        this.update_frame(cx);
        this
    }

    pub(crate) fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Fills the window with the preview, in OS full screen.
    pub(crate) fn toggle_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fullscreen = !self.fullscreen;
        if window.is_fullscreen() != self.fullscreen {
            window.toggle_fullscreen();
        }
        // The shell lays out the panels, so the whole window redraws.
        window.refresh();
        cx.notify();
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.workspace.read(cx);
        let editor = workspace.editor();
        let fps = editor.project().settings.fps;
        let timecode =
            |time| format_timecode(time, TimeCodeFormat::HhMmSsFf, Some(fps)).unwrap_or_default();
        let time = format!(
            "{} / {}",
            timecode(editor.playback().position),
            timecode(editor.duration())
        );
        let playing = editor.playback().playing;
        let muted = workspace.muted();
        let canvas = editor.project().settings.canvas_size;
        let zoom_label = ZOOMS[self.zoom].map_or("Fit".to_owned(), |zoom| {
            format!("{}%", (zoom * 100.0).round())
        });
        let button = |id: &'static str, label: SharedString| {
            Button::new(id, label)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Small)
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .h(px(36.0))
            .px(px(8.0))
            .child(
                button(
                    "preview-play",
                    if playing { "Pause" } else { "Play" }.into(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.run_transport(Transport::TogglePlay, cx);
                    });
                })),
            )
            .child(
                button("preview-mute", if muted { "Unmute" } else { "Mute" }.into()).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.workspace
                            .update(cx, |workspace, cx| workspace.toggle_muted(cx));
                    }),
                ),
            )
            .child(div().text_xs().whitespace_nowrap().child(time))
            .child(div().flex_1())
            .child(
                button("preview-zoom", zoom_label.into()).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.zoom = (this.zoom + 1) % ZOOMS.len();
                        cx.notify();
                    },
                )),
            )
            .child(
                button("preview-ratio", ratio_label(canvas).into()).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let mut command = UpdateProjectSettings {
                            canvas_size: Some(next_ratio(canvas)),
                            ..Default::default()
                        };
                        this.workspace
                            .update(cx, |workspace, cx| workspace.execute(&mut command, cx));
                    },
                )),
            )
            .child(
                button(
                    "preview-fullscreen",
                    if self.fullscreen {
                        "Exit full screen"
                    } else {
                        "Full screen"
                    }
                    .into(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_fullscreen(window, cx))),
            )
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
        let zoom = ZOOMS[self.zoom];
        let canvas = self
            .workspace
            .read(cx)
            .editor()
            .project()
            .settings
            .canvas_size;

        // A zoomed frame scrolls; auto margins centre it while it's smaller
        // than the viewport without clipping its left/top edge when larger.
        let viewport = div()
            .id("preview-viewport")
            .flex()
            .flex_1()
            .min_h_0()
            .overflow_scroll()
            .map(|viewport| match (&self.frame, &self.error) {
                (_, Some(error)) => viewport
                    .items_center()
                    .justify_center()
                    .text_color(colors.destructive)
                    .child(error.clone()),
                (Some(frame), None) => {
                    let image = img(Arc::clone(frame)).object_fit(ObjectFit::Contain);
                    viewport.child(match zoom {
                        None => image.size_full(),
                        Some(zoom) => image
                            .flex_none()
                            .m_auto()
                            .w(px(canvas.width as f32 * zoom))
                            .h(px(canvas.height as f32 * zoom)),
                    })
                }
                (None, None) => viewport,
            });

        div()
            .flex()
            .flex_col()
            .when(self.fullscreen, |panel| panel.size_full())
            .when(!self.fullscreen, |panel| panel.w_1_2().h_full())
            .min_h_0()
            .bg(colors.background)
            .child(viewport)
            .child(self.render_controls(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_cycles_through_presets() {
        let size = |width, height| CanvasSize { width, height };
        assert_eq!(ratio_label(size(3840, 2160)), "16:9");
        assert_eq!(next_ratio(size(3840, 2160)), size(1080, 1920));
        assert_eq!(next_ratio(size(1440, 1080)), size(1920, 1080));
        assert_eq!(ratio_label(size(1000, 700)), "Custom");
        assert_eq!(next_ratio(size(1000, 700)), size(1920, 1080));
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
