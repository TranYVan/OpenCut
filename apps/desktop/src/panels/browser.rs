use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, ExternalPaths, FocusHandle, Hsla, MouseButton, MouseDownEvent,
    ObjectFit, RenderImage, SharedString, Subscription, Window, actions, div, img, prelude::*, px,
    relative, transparent_black,
};
use opencut_editor::model::{MediaAsset, MediaType};

use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, ContextMenu, ContextMenuItem,
    ContextMenuItemVariant, Label,
};
use crate::media::{THUMBNAIL_EDGE, Thumbnail, ThumbnailProvider};
use crate::theme::ActiveTheme;
use crate::workspace::Workspace;

actions!(media_browser, [RemoveSelectedMedia]);

/// Key context of the Browser panel, for its key bindings.
pub(crate) const KEY_CONTEXT: &str = "MediaBrowser";

const CARD_WIDTH: f32 = 120.0;
const TILE_HEIGHT: f32 = 68.0;

enum ThumbnailState {
    Loading,
    Ready(Arc<RenderImage>),
    /// Bar heights of an audio waveform, `0.0..=1.0`.
    Waveform(Arc<[f32]>),
    /// Nothing to show: the placeholder tile.
    Missing,
}

/// The media bin: the project's imported assets as a grid of cards.
pub(crate) struct Browser {
    workspace: Entity<Workspace>,
    thumbnail_provider: Arc<dyn ThumbnailProvider>,
    /// Per asset id. Kept after an asset is removed, so undo shows it again
    /// without reloading.
    thumbnails: HashMap<String, ThumbnailState>,
    /// Selected asset ids, in project order.
    selected: Vec<String>,
    /// Where a shift-click range starts.
    anchor: Option<String>,
    focus_handle: FocusHandle,
    context_menu: Entity<ContextMenu>,
    _workspace_changed: Subscription,
}

impl Browser {
    pub(crate) fn new(
        workspace: Entity<Workspace>,
        thumbnail_provider: Arc<dyn ThumbnailProvider>,
        cx: &mut Context<Self>,
    ) -> Self {
        let browser = cx.entity().downgrade();
        let import_workspace = workspace.clone();
        let context_menu = cx.new(|_| {
            ContextMenu::new([
                ContextMenuItem::entry("Import media…").on_select(move |_, cx| {
                    import_workspace.update(cx, |workspace, cx| workspace.prompt_import(cx));
                }),
                ContextMenuItem::separator(),
                ContextMenuItem::entry("Remove")
                    .shortcut("⌫")
                    .variant(ContextMenuItemVariant::Destructive)
                    .on_select(move |_, cx| {
                        let _ = browser.update(cx, |browser, cx| browser.remove_selected(cx));
                    }),
            ])
        });
        let workspace_changed = cx.observe(&workspace, |this, _, cx| this.sync(cx));

        let mut this = Self {
            workspace,
            thumbnail_provider,
            thumbnails: HashMap::new(),
            selected: Vec::new(),
            anchor: None,
            focus_handle: cx.focus_handle(),
            context_menu,
            _workspace_changed: workspace_changed,
        };
        this.sync(cx);
        this
    }

    /// Follows project changes: drops selected assets that are gone and
    /// starts loading thumbnails for new ones.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        let assets = &workspace.editor().project().media_assets;
        self.selected
            .retain(|id| assets.iter().any(|asset| &asset.id == id));
        let pending: Vec<(MediaAsset, std::path::PathBuf)> = assets
            .iter()
            .filter(|asset| !self.thumbnails.contains_key(&asset.id))
            .map(|asset| (asset.clone(), workspace.files().media_path(asset)))
            .collect();

        for (asset, path) in pending {
            self.thumbnails
                .insert(asset.id.clone(), ThumbnailState::Loading);
            let provider = Arc::clone(&self.thumbnail_provider);
            let id = asset.id.clone();
            let work = cx.background_executor().spawn(async move {
                provider
                    .thumbnail(&path, &asset, THUMBNAIL_EDGE)
                    .and_then(thumbnail_state)
            });
            cx.spawn(async move |this, cx| {
                let image = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.thumbnails
                        .insert(id, image.unwrap_or(ThumbnailState::Missing));
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn assets<'a>(&self, cx: &'a Context<Self>) -> &'a [MediaAsset] {
        &self.workspace.read(cx).editor().project().media_assets
    }

    /// Click: select only this. Cmd/Ctrl-click: toggle. Shift-click: the
    /// range from the last clicked asset.
    fn click_asset(
        &mut self,
        asset_id: &str,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        let modifiers = event.modifiers;
        if modifiers.shift
            && let Some(anchor) = self.anchor.clone()
        {
            let ids: Vec<&str> = self
                .assets(cx)
                .iter()
                .map(|asset| asset.id.as_str())
                .collect();
            let from = ids.iter().position(|id| *id == anchor);
            let to = ids.iter().position(|id| *id == asset_id);
            if let (Some(from), Some(to)) = (from, to) {
                let (start, end) = (from.min(to), from.max(to));
                self.selected = ids[start..=end].iter().map(|id| (*id).to_owned()).collect();
                cx.notify();
                return;
            }
        }
        if modifiers.secondary() {
            if let Some(index) = self.selected.iter().position(|id| id == asset_id) {
                self.selected.remove(index);
            } else {
                self.selected.push(asset_id.to_owned());
            }
        } else {
            self.selected = vec![asset_id.to_owned()];
        }
        self.anchor = Some(asset_id.to_owned());
        cx.notify();
    }

    /// Right-click selects the asset unless it's already part of the
    /// selection, then opens the menu for the selection.
    fn open_menu(
        &mut self,
        asset_id: Option<&str>,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        match asset_id {
            Some(id) if !self.selected.iter().any(|selected| selected == id) => {
                self.selected = vec![id.to_owned()];
                self.anchor = Some(id.to_owned());
            }
            Some(_) => {}
            None => self.selected.clear(),
        }
        self.context_menu
            .update(cx, |menu, cx| menu.open(event.position, cx));
        cx.notify();
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let ids = std::mem::take(&mut self.selected);
        self.workspace
            .update(cx, |workspace, cx| workspace.remove_assets(ids, cx));
    }

    fn render_card(
        &self,
        index: usize,
        asset: &MediaAsset,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = window.theme();
        let colors = theme.colors;
        let selected = self.selected.iter().any(|id| id == &asset.id);
        let accent = type_color(asset.media_type, window);
        let click_id = asset.id.clone();
        let menu_id = asset.id.clone();

        let preview = match self.thumbnails.get(&asset.id) {
            Some(ThumbnailState::Ready(image)) => img(Arc::clone(image))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            Some(ThumbnailState::Waveform(bars)) => waveform(bars, accent).into_any_element(),
            Some(ThumbnailState::Loading) => div().into_any_element(),
            Some(ThumbnailState::Missing) | None => div()
                .text_color(accent)
                .text_size(px(10.0))
                .child(type_label(asset.media_type))
                .into_any_element(),
        };

        let tile = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(TILE_HEIGHT))
            .rounded(theme.radius)
            .overflow_hidden()
            .bg(accent.opacity(0.15))
            .child(preview)
            .when_some(asset.duration.map(format_duration), |this, duration| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(4.0))
                        .right(px(4.0))
                        .child(Badge::new(duration).variant(BadgeVariant::Secondary)),
                )
            });

        div()
            .id(("media-asset", index))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .w(px(CARD_WIDTH))
            .p(px(4.0))
            .rounded(theme.radius)
            .border_1()
            .border_color(if selected {
                colors.sidebar_ring
            } else {
                transparent_black()
            })
            .when(selected, |this| this.bg(colors.sidebar_accent))
            .hover(|style| style.bg(colors.sidebar_accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    cx.stop_propagation();
                    this.click_asset(&click_id, event, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    cx.stop_propagation();
                    this.open_menu(Some(&menu_id), event, window, cx);
                }),
            )
            .on_drag(
                DraggedAsset {
                    id: asset.id.clone(),
                    name: asset.name.clone().into(),
                },
                |asset, _, _, cx| cx.new(|_| asset.clone()),
            )
            .child(tile)
            .child(
                div()
                    .text_xs()
                    .text_color(colors.sidebar_foreground)
                    .truncate()
                    .child(asset.name.clone()),
            )
            .into_any_element()
    }
}

/// An asset being dragged out of the Browser; the Timeline takes the drop.
#[derive(Clone)]
pub(crate) struct DraggedAsset {
    pub(crate) id: String,
    name: SharedString,
}

impl Render for DraggedAsset {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = window.theme();
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.popover)
            .text_xs()
            .text_color(theme.colors.popover_foreground)
            .child(self.name.clone())
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;
        let workspace = self.workspace.read(cx);
        let assets = workspace.editor().project().media_assets.clone();
        let importing = workspace.importing();
        let notice = workspace.notice().cloned();

        let cards: Vec<AnyElement> = assets
            .iter()
            .enumerate()
            .map(|(index, asset)| self.render_card(index, asset, window, cx))
            .collect();

        let import_workspace = self.workspace.clone();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(40.0))
            .px(px(12.0))
            .border_b_1()
            .border_color(colors.sidebar_border)
            .child(Label::new("Media"))
            .when(!assets.is_empty(), |this| {
                this.child(Badge::new(assets.len().to_string()).variant(BadgeVariant::Secondary))
            })
            .child(div().flex_1())
            .child(
                Button::new("import-media", "Import")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Small)
                    .on_click(move |_, _, cx| {
                        import_workspace.update(cx, |workspace, cx| workspace.prompt_import(cx));
                    }),
            );

        let body = if assets.is_empty() && importing == 0 {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .p(px(16.0))
                .child(Label::new("No media yet"))
                .child(Label::new("Drop files here, paste, or click Import").muted())
                .into_any_element()
        } else {
            div()
                .id("media-assets")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .content_start()
                        .gap(px(4.0))
                        .p(px(8.0))
                        .children(cards),
                )
                .into_any_element()
        };

        let status = (importing > 0).then(|| {
            let files = if importing == 1 { "file" } else { "files" };
            Label::new(format!("Importing {importing} {files}…")).muted()
        });

        div()
            .id("browser")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &RemoveSelectedMedia, _, cx| this.remove_selected(cx)))
            .flex()
            .flex_col()
            .w_1_4()
            .h_full()
            .border_r_1()
            .border_color(colors.sidebar_border)
            .bg(colors.sidebar)
            .text_color(colors.sidebar_foreground)
            .drag_over::<ExternalPaths>(|style, _, window, _| {
                let colors = window.theme().colors;
                style
                    .bg(colors.sidebar_accent)
                    .border_color(colors.sidebar_ring)
            })
            // Clicking empty space clears the selection.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle);
                    this.selected.clear();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event, window, cx| this.open_menu(None, event, window, cx)),
            )
            .child(header)
            .child(body)
            .when_some(status, |this, status| {
                this.child(div().flex_none().px(px(12.0)).py(px(6.0)).child(status))
            })
            .when_some(notice, |this, notice| {
                this.child(
                    div()
                        .flex_none()
                        .px(px(12.0))
                        .py(px(6.0))
                        .border_t_1()
                        .border_color(colors.sidebar_border)
                        .text_xs()
                        .text_color(colors.destructive)
                        .child(notice),
                )
            })
            .child(self.context_menu.clone())
    }
}

/// Turns a provider's thumbnail into what the card draws. Runs on the
/// background executor.
fn thumbnail_state(thumbnail: Thumbnail) -> Option<ThumbnailState> {
    match thumbnail {
        Thumbnail::Picture {
            width,
            height,
            bgra,
        } => {
            // `RenderImage` takes BGRA bytes in an `RgbaImage` container.
            let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
            let image = RenderImage::new(vec![image::Frame::new(buffer)]);
            Some(ThumbnailState::Ready(Arc::new(image)))
        }
        Thumbnail::Waveform(bars) if !bars.is_empty() => {
            Some(ThumbnailState::Waveform(bars.into()))
        }
        Thumbnail::Waveform(_) => None,
    }
}

/// Centered bars, one per amplitude, in the audio type color.
fn waveform(bars: &[f32], color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(1.0))
        .size_full()
        .px(px(6.0))
        .py(px(8.0))
        .children(bars.iter().map(|amplitude| {
            div()
                .flex_1()
                .min_h(px(1.0))
                .h(relative(amplitude.clamp(0.0, 1.0)))
                .rounded(px(1.0))
                .bg(color)
        }))
}

fn type_color(media_type: MediaType, window: &Window) -> Hsla {
    let colors = window.theme().colors;
    match media_type {
        MediaType::Video => colors.chart_1,
        MediaType::Image => colors.chart_2,
        MediaType::Audio => colors.chart_3,
    }
}

fn type_label(media_type: MediaType) -> &'static str {
    match media_type {
        MediaType::Video => "VIDEO",
        MediaType::Image => "IMAGE",
        MediaType::Audio => "AUDIO",
    }
}

/// `m:ss`, or `h:mm:ss` from an hour up.
fn format_duration(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds.round() as u64
    } else {
        0
    };
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn durations_read_like_a_clock() {
        assert_eq!(format_duration(0.4), "0:00");
        assert_eq!(format_duration(9.6), "0:10");
        assert_eq!(format_duration(125.0), "2:05");
        assert_eq!(format_duration(3_725.0), "1:02:05");
        assert_eq!(format_duration(f64::NAN), "0:00");
    }
}
