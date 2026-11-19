//! The timeline panel: the current scene's tracks and elements under a
//! time ruler, with a playhead. Zoom with Cmd/Ctrl + wheel or the toolbar,
//! scroll with the wheel (Shift for horizontal), click or drag on the ruler
//! to seek.
//!
//! Zoom and scroll are view state and live here; the project, the playhead
//! and the track toggles live in the editor and change through
//! [`Workspace`].

mod layout;
mod ruler;
mod scale;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Context, Entity, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    ObjectFit, Pixels, RenderImage, ScrollWheelEvent, Subscription, Window, canvas, div, fill, img,
    point, prelude::*, px, size,
};
use opencut_editor::commands::{ToggleTrackMute, ToggleTrackVisibility};
use opencut_editor::model::time::{FrameRate, MediaTime, TimeCodeFormat, format_timecode};
use opencut_editor::model::{
    Element, ElementKind, ElementRef, MediaAsset, MediaType, RetimeConfig, Track, TrackKind,
};
use opencut_media::{WaveformCache, WaveformSummary};

use crate::components::{Button, ButtonSize, ButtonVariant, Label};
use crate::media::{Thumbnail, ThumbnailProvider};
use crate::theme::{ActiveTheme, SemanticColors};
use crate::workspace::Workspace;

use layout::{
    HEADER_WIDTH, RULER_HEIGHT, RowLayout, VisibleSpan, source_range, tile_offsets, track_rows,
    visible_span,
};
use ruler::{format_label, ruler_config, ruler_ticks};
use scale::{
    DEFAULT_ZOOM, FIT_SHARE, ZOOM_STEP, anchored_scroll, clamp_scroll, clamp_zoom, content_width,
    min_zoom, pixels_per_second, px_to_time, slider_to_zoom, time_to_px, wheel_zoom_factor,
    zoom_to_fit, zoom_to_slider,
};

/// Longest edge of the poster frames drawn on video and image elements.
const POSTER_EDGE: u32 = 160;
/// Pixels drawn past each viewport edge, so element borders just off
/// screen stay hidden.
const OVERSCAN: f64 = 4.0;
/// Width of one waveform bar and the gap after it.
const WAVEFORM_BAR: f64 = 2.0;
const WAVEFORM_GAP: f64 = 1.0;
/// Pixels per "line" of a line-based (non-precise) scroll wheel.
const WHEEL_LINE_HEIGHT: f32 = 20.0;
const TOOLBAR_HEIGHT: f32 = 36.0;

/// What a media-backed element draws, per asset.
enum MediaPreview {
    Loading,
    /// A poster frame and its width / height.
    Poster(Arc<RenderImage>, f64),
    /// The asset's whole waveform; elements draw their trimmed part.
    Waveform(Arc<WaveformSummary>),
    Missing,
}

/// The timeline panel.
pub(crate) struct Timeline {
    workspace: Entity<Workspace>,
    thumbnail_provider: Arc<dyn ThumbnailProvider>,
    /// Per asset id, kept across removals so undo doesn't reload.
    previews: HashMap<String, MediaPreview>,
    /// Zoom level: 1.0 is 50 px per second.
    zoom: f64,
    /// Horizontal scroll, content pixels from the timeline's start.
    scroll_x: f64,
    /// Vertical scroll of the track rows.
    scroll_y: f64,
    /// Window bounds of the ruler + tracks area, from the last paint.
    viewport: Bounds<Pixels>,
    /// Dragging on the ruler moves the playhead.
    scrubbing: bool,
    /// What the panel last drew, to re-render only when it changes.
    seen_project: Option<String>,
    seen_revision: u64,
    seen_position: MediaTime,
    _workspace_changed: Subscription,
}

impl Timeline {
    pub(crate) fn new(
        workspace: Entity<Workspace>,
        thumbnail_provider: Arc<dyn ThumbnailProvider>,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace_changed = cx.observe(&workspace, |this, _, cx| this.sync(cx));
        let mut this = Self {
            workspace,
            thumbnail_provider,
            previews: HashMap::new(),
            zoom: DEFAULT_ZOOM,
            scroll_x: 0.0,
            scroll_y: 0.0,
            viewport: Bounds::default(),
            scrubbing: false,
            seen_project: None,
            seen_revision: 0,
            seen_position: MediaTime::ZERO,
            _workspace_changed: workspace_changed,
        };
        this.sync(cx);
        this
    }

    /// Follows the workspace: re-renders when the project's revision or the
    /// playhead changes, restores the saved view when another project
    /// opens, and loads previews for media new to the timeline.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let editor = self.workspace.read(cx).editor();
        let project = editor.project();
        let revision = editor.revision();
        let position = editor.playback().position;
        let opened = self.seen_project.as_deref() != Some(project.metadata.id.as_str());
        if !opened && revision == self.seen_revision && position == self.seen_position {
            return;
        }
        self.seen_project = Some(project.metadata.id.clone());
        self.seen_revision = revision;
        self.seen_position = position;

        // The workspace restores the playhead when it opens the project.
        if opened {
            let view = self.workspace.read(cx).timeline_view();
            self.zoom = view.map_or(DEFAULT_ZOOM, |view| view.zoom_level);
            self.scroll_x = view.map_or(0.0, |view| view.scroll_left);
            self.scroll_y = 0.0;
        }
        self.load_previews(cx);
        cx.notify();
    }

    /// Starts loading a poster or waveform for each asset an element uses
    /// that has none yet.
    fn load_previews(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        let editor = workspace.editor();
        let project = editor.project();
        let files = workspace.files();
        let waveform_dir = files.cache_dir().join("waveforms");
        let mut pending: Vec<(MediaAsset, PathBuf)> = Vec::new();
        for track in editor.tracks().iter() {
            for element in &track.elements {
                let Some(asset) = element.media_id().and_then(|id| project.media_asset(id)) else {
                    continue;
                };
                if self.previews.contains_key(&asset.id)
                    || pending.iter().any(|(queued, _)| queued.id == asset.id)
                {
                    continue;
                }
                pending.push((asset.clone(), files.media_path(asset)));
            }
        }

        for (asset, path) in pending {
            self.previews
                .insert(asset.id.clone(), MediaPreview::Loading);
            let provider = Arc::clone(&self.thumbnail_provider);
            let waveform_dir = waveform_dir.clone();
            let id = asset.id.clone();
            let work = cx
                .background_executor()
                .spawn(async move { load_preview(&*provider, &asset, &path, waveform_dir) });
            cx.spawn(async move |this, cx| {
                let preview = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.previews.insert(id, preview);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn viewport_width(&self) -> f64 {
        f64::from(self.viewport.size.width)
    }

    fn tracks_height(&self) -> f64 {
        (f64::from(self.viewport.size.height) - RULER_HEIGHT).max(0.0)
    }

    fn duration(&self, cx: &Context<Self>) -> MediaTime {
        self.workspace.read(cx).editor().duration()
    }

    fn min_zoom(&self, cx: &Context<Self>) -> f64 {
        min_zoom(self.duration(cx), self.viewport_width())
    }

    /// Sets the zoom, keeping the time at viewport pixel `anchor_x` still.
    fn set_zoom(&mut self, zoom: f64, anchor_x: f64, cx: &mut Context<Self>) {
        let zoom = clamp_zoom(zoom, self.min_zoom(cx));
        if (zoom - self.zoom).abs() < f64::EPSILON {
            return;
        }
        let anchor_x = anchor_x.clamp(0.0, self.viewport_width());
        self.scroll_x = anchored_scroll(anchor_x, self.scroll_x, self.zoom, zoom);
        self.zoom = zoom;
        self.clamp_scroll(cx);
        self.save_view(cx);
        cx.notify();
    }

    /// Hands zoom and horizontal scroll to the workspace, which saves them
    /// with the project (debounced, not undoable).
    fn save_view(&self, cx: &mut Context<Self>) {
        let (zoom, scroll_x) = (self.zoom, self.scroll_x);
        self.workspace.update(cx, |workspace, cx| {
            workspace.set_timeline_view(zoom, scroll_x, cx);
        });
    }

    /// Toolbar zoom: moves `step` (a share of the whole range) along the
    /// log-scale zoom slider, keeping the playhead still when it's on
    /// screen, else the middle of the view.
    fn zoom_step(&mut self, step: f64, cx: &mut Context<Self>) {
        let min = self.min_zoom(cx);
        let zoom = slider_to_zoom(zoom_to_slider(self.zoom, min) + step, min);
        let playhead = time_to_px(self.seen_position, self.zoom) - self.scroll_x;
        let anchor = if (0.0..=self.viewport_width()).contains(&playhead) {
            playhead
        } else {
            self.viewport_width() / 2.0
        };
        self.set_zoom(zoom, anchor, cx);
    }

    fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        let zoom = zoom_to_fit(self.duration(cx), self.viewport_width(), FIT_SHARE);
        self.zoom = clamp_zoom(zoom, self.min_zoom(cx));
        self.scroll_x = 0.0;
        self.save_view(cx);
        cx.notify();
    }

    fn clamp_scroll(&mut self, cx: &Context<Self>) {
        // Before the first paint the size is unknown; don't clobber a
        // restored scroll position with it.
        if self.viewport.size.width <= px(0.0) {
            return;
        }
        let width = self.viewport_width();
        let content = content_width(self.duration(cx), width, self.zoom, self.min_zoom(cx));
        self.scroll_x = clamp_scroll(self.scroll_x, content, width);
        let (_, content_height) = track_rows(self.track_kinds(cx));
        self.scroll_y = clamp_scroll(self.scroll_y, content_height, self.tracks_height());
    }

    fn track_kinds(&self, cx: &Context<Self>) -> Vec<TrackKind> {
        self.workspace
            .read(cx)
            .editor()
            .tracks()
            .iter()
            .map(|track| track.kind)
            .collect()
    }

    /// Wheel: Cmd/Ctrl zooms around the pointer, Shift (or a sideways
    /// swipe) scrolls horizontally, otherwise scrolls both ways.
    fn on_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(WHEEL_LINE_HEIGHT));
        let (dx, dy) = (f64::from(delta.x), f64::from(delta.y));
        if event.modifiers.control || event.modifiers.platform {
            let anchor = f64::from(event.position.x - self.viewport.origin.x);
            // Wheel deltas are positive when scrolling up, which zooms in.
            self.set_zoom(self.zoom * wheel_zoom_factor(-dy), anchor, cx);
            return;
        }
        if event.modifiers.shift && dx == 0.0 {
            self.scroll_x -= dy;
        } else {
            self.scroll_x -= dx;
            self.scroll_y -= dy;
        }
        self.clamp_scroll(cx);
        self.save_view(cx);
        cx.notify();
    }

    /// Moves the playhead to the time under window x `x`.
    fn seek_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let content_x = f64::from(x - self.viewport.origin.x) + self.scroll_x;
        let time = px_to_time(content_x, self.zoom);
        self.workspace
            .update(cx, |workspace, cx| workspace.seek(time, cx));
    }

    fn render_toolbar(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;
        let editor = self.workspace.read(cx).editor();
        let fps = editor.project().settings.fps;
        let timecode =
            |time| format_timecode(time, TimeCodeFormat::HhMmSsFf, Some(fps)).unwrap_or_default();
        let position = timecode(editor.playback().position);
        let duration = timecode(editor.duration());
        let zoom_percent = zoom_to_slider(self.zoom, self.min_zoom(cx)) * 100.0;
        let this = cx.entity().downgrade();
        let zoom_button = |id: &'static str, label: &'static str, step: f64| {
            let this = this.clone();
            Button::new(id, label)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSmall)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.zoom_step(step, cx));
                })
        };
        let fit = this.clone();

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .h(px(TOOLBAR_HEIGHT))
            .px(px(12.0))
            .border_b_1()
            .border_color(colors.border)
            .child(Label::new("Timeline"))
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(format!("{position} / {duration}")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(format!("Zoom {zoom_percent:.0}%")),
            )
            .child(zoom_button("timeline-zoom-out", "−", -ZOOM_STEP))
            .child(zoom_button("timeline-zoom-in", "+", ZOOM_STEP))
            .child(
                Button::new("timeline-zoom-fit", "Fit")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Small)
                    .on_click(move |_, _, cx| {
                        let _ = fit.update(cx, |this, cx| this.zoom_to_fit(cx));
                    }),
            )
    }

    fn render_ruler(&self, fps: FrameRate, colors: &SemanticColors) -> AnyElement {
        let width = self.viewport_width();
        let config = ruler_config(self.zoom, fps);
        let start = px_to_time(self.scroll_x - OVERSCAN, self.zoom);
        let end = px_to_time(self.scroll_x + width + OVERSCAN, self.zoom);
        let marks = ruler_ticks(start, end, config).into_iter().map(|tick| {
            let x = time_to_px(tick.time, self.zoom) - self.scroll_x;
            let height = if tick.labeled { 10.0 } else { 5.0 };
            div()
                .absolute()
                .left(f(x))
                .bottom_0()
                .w(px(1.0))
                .h(px(height))
                .bg(colors.border)
                .when(tick.labeled, |this| {
                    this.child(
                        div()
                            .absolute()
                            .bottom(px(height))
                            .left(px(3.0))
                            .text_size(px(10.0))
                            .text_color(colors.muted_foreground)
                            .whitespace_nowrap()
                            .child(format_label(tick.time, fps)),
                    )
                })
        });

        div()
            .relative()
            .flex_none()
            .h(f(RULER_HEIGHT))
            .w_full()
            .overflow_hidden()
            .border_b_1()
            .border_color(colors.border)
            .children(marks)
            .into_any_element()
    }

    fn render_bookmarks(&self, colors: &SemanticColors, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(scene) = self.workspace.read(cx).editor().project().current_scene() else {
            return Vec::new();
        };
        let width = self.viewport_width();
        scene
            .bookmarks
            .iter()
            .filter_map(|bookmark| {
                let start = time_to_px(bookmark.time, self.zoom);
                let length = bookmark
                    .duration
                    .map_or(0.0, |duration| time_to_px(duration, self.zoom));
                let span = visible_span(start, start + length.max(4.0), self.scroll_x, width, 0.0)?;
                let color = bookmark
                    .color
                    .as_deref()
                    .and_then(parse_hex_color)
                    .unwrap_or(colors.chart_4);
                Some(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(f(span.left))
                        .w(f(span.width))
                        .h(px(4.0))
                        .rounded_b(px(2.0))
                        .bg(color)
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_headers(
        &self,
        tracks: &[&Track],
        rows: &[RowLayout],
        colors: &SemanticColors,
    ) -> Vec<AnyElement> {
        tracks
            .iter()
            .zip(rows)
            .enumerate()
            .filter(|(_, (_, row))| self.row_visible(**row))
            .map(|(index, (track, row))| {
                let mut toggles = Vec::new();
                if track.kind.can_have_audio() {
                    toggles.push(self.toggle_button(
                        ("timeline-mute", index),
                        "M",
                        track.is_muted(),
                        TrackToggle::Mute(track.id.clone()),
                    ));
                }
                if track.kind.can_be_hidden() {
                    toggles.push(self.toggle_button(
                        ("timeline-hide", index),
                        "H",
                        track.is_hidden(),
                        TrackToggle::Hide(track.id.clone()),
                    ));
                }
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(f(row.top - self.scroll_y))
                    .h(f(row.height))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .px(px(8.0))
                    .child(
                        div()
                            .w(px(3.0))
                            .h(px(12.0))
                            .rounded(px(1.0))
                            .bg(kind_color(track.kind, None, colors)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(4.0))
                            .text_xs()
                            .truncate()
                            .text_color(colors.foreground)
                            .child(track.name.clone()),
                    )
                    .children(toggles)
                    .into_any_element()
            })
            .collect()
    }

    fn toggle_button(
        &self,
        id: (&'static str, usize),
        label: &'static str,
        on: bool,
        toggle: TrackToggle,
    ) -> AnyElement {
        let workspace = self.workspace.clone();
        let button = Button::new(id, label)
            .variant(if on {
                ButtonVariant::Secondary
            } else {
                ButtonVariant::Ghost
            })
            .size(ButtonSize::IconXSmall)
            .on_click(move |_, _, cx| {
                workspace.update(cx, |workspace, cx| match &toggle {
                    TrackToggle::Mute(track_id) => workspace.execute(
                        &mut ToggleTrackMute {
                            track_id: track_id.clone(),
                        },
                        cx,
                    ),
                    TrackToggle::Hide(track_id) => workspace.execute(
                        &mut ToggleTrackVisibility {
                            track_id: track_id.clone(),
                        },
                        cx,
                    ),
                });
            });
        div()
            // Keep clicks on the toggle from reaching the header row.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .opacity(if on { 1.0 } else { 0.6 })
            .child(button)
            .into_any_element()
    }

    fn row_visible(&self, row: RowLayout) -> bool {
        row.bottom() > self.scroll_y && row.top < self.scroll_y + self.tracks_height()
    }

    fn render_rows(
        &self,
        tracks: &[&Track],
        rows: &[RowLayout],
        window: &Window,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = window.theme();
        let colors = &theme.colors;
        let editor = self.workspace.read(cx).editor();
        let project = editor.project();
        let selection = editor.selection();
        let width = self.viewport_width();
        let mut children = Vec::new();

        for (track, row) in tracks.iter().zip(rows) {
            if !self.row_visible(*row) {
                continue;
            }
            let top = row.top - self.scroll_y;
            children.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(f(top))
                    .h(f(row.height))
                    .bg(colors.muted.opacity(0.35))
                    .into_any_element(),
            );
            for element in &track.elements {
                let start = time_to_px(element.start_time(), self.zoom);
                let end = time_to_px(element.end_time(), self.zoom);
                let Some(span) = visible_span(start, end, self.scroll_x, width, OVERSCAN) else {
                    continue;
                };
                let asset = element.media_id().and_then(|id| project.media_asset(id));
                let selected = selection.contains(&ElementRef::new(&track.id, element.id()));
                children.push(self.render_element(
                    track, element, asset, span, top, row.height, selected, colors,
                ));
            }
        }
        children
    }

    #[allow(clippy::too_many_arguments)]
    fn render_element(
        &self,
        track: &Track,
        element: &Element,
        asset: Option<&MediaAsset>,
        span: VisibleSpan,
        top: f64,
        height: f64,
        selected: bool,
        colors: &SemanticColors,
    ) -> AnyElement {
        let kind = element.kind();
        let accent = kind_color(track.kind, Some(kind), colors);
        let dimmed = track.is_hidden() || element.is_hidden() == Some(true);
        let preview = asset.and_then(|asset| self.previews.get(&asset.id));
        let inner_height = height - 2.0;

        let content: Option<AnyElement> = match preview {
            Some(MediaPreview::Poster(image, aspect)) if kind != ElementKind::Audio => {
                let tile_width = (inner_height * aspect).max(8.0);
                let tiles = tile_offsets(span, tile_width).into_iter().map(|left| {
                    img(Arc::clone(image))
                        .absolute()
                        .top_0()
                        .left(f(left))
                        .w(f(tile_width))
                        .h_full()
                        .object_fit(ObjectFit::Cover)
                });
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .children(tiles)
                        .into_any_element(),
                )
            }
            Some(MediaPreview::Waveform(summary)) => {
                let base = element.base();
                let rate = RetimeConfig::safe_rate(element.retime());
                let (from, to) = source_range(
                    base.trim_start.to_seconds_f64(),
                    rate,
                    span,
                    pixels_per_second(self.zoom),
                );
                let color = if track.is_muted() {
                    colors.muted_foreground.opacity(0.5)
                } else {
                    accent
                };
                Some(waveform(Arc::clone(summary), from, to, color).into_any_element())
            }
            _ => None,
        };

        div()
            .absolute()
            .left(f(span.left))
            .top(f(top + 1.0))
            .w(f(span.width))
            .h(f(inner_height))
            .overflow_hidden()
            .rounded(px(4.0))
            .border_1()
            .border_color(if selected { colors.ring } else { accent })
            .bg(accent.opacity(0.3))
            .when(dimmed, |this| this.opacity(0.45))
            .children(content)
            .child(
                div()
                    .absolute()
                    .top(px(2.0))
                    .left(px(4.0))
                    .max_w(f((span.width - 8.0).max(0.0)))
                    .px(px(3.0))
                    .rounded(px(2.0))
                    .bg(colors.background.opacity(0.6))
                    .text_size(px(11.0))
                    .text_color(colors.foreground)
                    .truncate()
                    .child(element.base().name.clone()),
            )
            .into_any_element()
    }
}

/// A header button's command.
enum TrackToggle {
    Mute(String),
    Hide(String),
}

impl Render for Timeline {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.zoom = clamp_zoom(self.zoom, self.min_zoom(cx));
        self.clamp_scroll(cx);

        let colors = window.theme().colors;
        let toolbar = self.render_toolbar(window, cx);
        let workspace = self.workspace.read(cx);
        let editor = workspace.editor();
        let fps = editor.project().settings.fps;
        let position = editor.playback().position;
        let tracks: Vec<&Track> = editor.tracks().iter().collect();
        let (rows, _) = track_rows(tracks.iter().map(|track| track.kind));

        let ruler = self.render_ruler(fps, &colors);
        let bookmarks = self.render_bookmarks(&colors, cx);
        let track_rows = self.render_rows(&tracks, &rows, window, cx);
        let headers = self.render_headers(&tracks, &rows, &colors);

        let playhead_x = time_to_px(position, self.zoom) - self.scroll_x;
        let playhead =
            (playhead_x >= -1.0 && playhead_x <= self.viewport_width() + 1.0).then(|| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(f(playhead_x - 1.0))
                    .w(px(2.0))
                    .bg(colors.primary)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(px(-4.0))
                            .w(px(10.0))
                            .h(px(8.0))
                            .rounded_b(px(3.0))
                            .bg(colors.primary),
                    )
            });

        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.viewport != bounds {
                        this.viewport = bounds;
                        cx.notify();
                    }
                });
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size_full();

        let header_column = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(f(HEADER_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex_none()
                    .h(f(RULER_HEIGHT))
                    .border_b_1()
                    .border_color(colors.border),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .children(headers),
            );

        let timeline_column = div()
            .id("timeline-area")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(measure)
            .child(
                div()
                    .id("timeline-ruler")
                    .relative()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.scrubbing = true;
                            this.seek_to(event.position.x, cx);
                        }),
                    )
                    .child(ruler)
                    .children(bookmarks),
            )
            .child(
                div()
                    .id("timeline-tracks")
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.seek_to(event.position.x, cx);
                        }),
                    )
                    .children(track_rows),
            )
            .children(playhead);

        div()
            .id("timeline")
            .flex()
            .flex_col()
            .h_1_3()
            .bg(colors.card)
            .text_color(colors.card_foreground)
            .child(toolbar)
            .child(
                div()
                    .id("timeline-body")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        this.on_wheel(event, cx);
                    }))
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if !this.scrubbing {
                            return;
                        }
                        if event.dragging() {
                            this.seek_to(event.position.x, cx);
                        } else {
                            this.scrubbing = false;
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.scrubbing = false),
                    )
                    .child(header_column)
                    .child(timeline_column),
            )
    }
}

/// Loads what an element of `asset` draws. Blocking; runs on the
/// background executor.
fn load_preview(
    provider: &dyn ThumbnailProvider,
    asset: &MediaAsset,
    path: &std::path::Path,
    waveform_dir: PathBuf,
) -> MediaPreview {
    match asset.media_type {
        MediaType::Audio => WaveformCache::new()
            .with_directory(waveform_dir)
            .get(path)
            .map_or(MediaPreview::Missing, MediaPreview::Waveform),
        MediaType::Video | MediaType::Image => match provider.thumbnail(path, asset, POSTER_EDGE) {
            Some(Thumbnail::Picture {
                width,
                height,
                bgra,
            }) if height > 0 => image::RgbaImage::from_raw(width, height, bgra).map_or(
                MediaPreview::Missing,
                |buffer| {
                    // `RenderImage` takes BGRA bytes in an `RgbaImage`.
                    let image = RenderImage::new(vec![image::Frame::new(buffer)]);
                    MediaPreview::Poster(Arc::new(image), f64::from(width) / f64::from(height))
                },
            ),
            _ => MediaPreview::Missing,
        },
    }
}

/// Bars of `summary`'s peaks between `from` and `to` source seconds,
/// filling the element, centered vertically.
fn waveform(summary: Arc<WaveformSummary>, from: f64, to: f64, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let width = f64::from(bounds.size.width);
            let height = f64::from(bounds.size.height);
            let step = WAVEFORM_BAR + WAVEFORM_GAP;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let count = (width / step).floor().max(0.0) as usize;
            if count == 0 || to <= from {
                return;
            }
            let peaks = summary.peaks_between(from, to, count);
            for (index, peak) in peaks.into_iter().enumerate() {
                let amplitude = f64::from(peak.amplitude().clamp(0.0, 1.0));
                let bar = (amplitude * height * 0.85).max(1.0);
                #[allow(clippy::cast_precision_loss)]
                let x = index as f64 * step;
                let origin = point(
                    bounds.origin.x + f(x),
                    bounds.origin.y + f((height - bar) / 2.0),
                );
                window.paint_quad(fill(
                    Bounds::new(origin, size(f(WAVEFORM_BAR), f(bar))),
                    color,
                ));
            }
        },
    )
    .absolute()
    .inset_0()
}

/// Theme color for an element (or, with `None`, a track) of `track` kind.
fn kind_color(track: TrackKind, element: Option<ElementKind>, colors: &SemanticColors) -> Hsla {
    match (track, element) {
        (_, Some(ElementKind::Image)) => colors.chart_2,
        (TrackKind::Video, _) => colors.chart_1,
        (TrackKind::Audio, _) => colors.chart_3,
        (TrackKind::Text, _) => colors.chart_4,
        (TrackKind::Graphic, _) => colors.chart_5,
        (TrackKind::Effect, _) => colors.ring,
    }
}

/// `#rrggbb` (a bookmark's stored color) as a color.
fn parse_hex_color(hex: &str) -> Option<Hsla> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    u32::from_str_radix(digits, 16)
        .ok()
        .map(|value| gpui::rgb(value).into())
}

/// Pixels from an `f64` layout value.
#[allow(clippy::cast_possible_truncation)]
fn f(value: f64) -> Pixels {
    px(value as f32)
}

#[cfg(test)]
mod tests {
    use opencut_editor::model::Project;

    use super::layout::{track_rows, visible_span};
    use super::parse_hex_color;
    use super::scale::{FIT_SHARE, time_to_px, zoom_to_fit};

    const FIXTURE: &str = include_str!("../../../../crates/model/tests/fixtures/classic-v31.json");

    /// The classic fixture lays out one row per track, and at "Fit" zoom
    /// every element is on screen, in time order, inside the fitted share.
    #[test]
    fn classic_fixture_fits_on_screen() {
        let project: Project = serde_json::from_str(FIXTURE).expect("fixture parses");
        let tracks = &project.current_scene().expect("scene").tracks;
        let (rows, height) = track_rows(tracks.iter().map(|track| track.kind));
        assert_eq!(rows.len(), tracks.iter().count());
        assert!(rows.windows(2).all(|pair| pair[0].bottom() < pair[1].top));
        assert!(height > rows.last().expect("rows").bottom());

        let width = 1200.0;
        let zoom = zoom_to_fit(tracks.duration(), width, FIT_SHARE);
        for track in tracks.iter() {
            for element in &track.elements {
                let start = time_to_px(element.start_time(), zoom);
                let end = time_to_px(element.end_time(), zoom);
                let span = visible_span(start, end, 0.0, width, 0.0)
                    .unwrap_or_else(|| panic!("{} is on screen", element.id()));
                assert!((span.left - start).abs() < 1e-9);
                assert!(span.left + span.width <= width * FIT_SHARE + 1e-6);
                assert_eq!(span.offset, 0.0);
            }
        }
    }

    #[test]
    fn bookmark_colors_parse_from_hex() {
        assert!(parse_hex_color("#ff0000").is_some());
        assert!(parse_hex_color("ff0000").is_none());
        assert!(parse_hex_color("#fff").is_none());
        assert!(parse_hex_color("#gggggg").is_none());
    }
}
