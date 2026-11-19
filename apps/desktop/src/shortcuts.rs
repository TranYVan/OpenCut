use gpui::{App, FontWeight, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px};

use crate::actions::{SHORTCUTS, display_keys};
use crate::components::{Button, ButtonSize, ButtonVariant, Label};
use crate::theme::ActiveTheme;

type CloseHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// Modal list of the keyboard shortcuts, grouped by category. Drawn over
/// the whole window; clicking outside the card or its button closes it.
#[derive(IntoElement)]
pub(crate) struct ShortcutsDialog {
    on_close: Option<CloseHandler>,
}

impl ShortcutsDialog {
    pub(crate) fn new() -> Self {
        Self { on_close: None }
    }

    pub(crate) fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for ShortcutsDialog {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let theme = window.theme();
        let colors = theme.colors;
        let on_close = self.on_close.map(std::rc::Rc::new);

        let mut categories: Vec<&str> = Vec::new();
        for shortcut in SHORTCUTS {
            if !categories.contains(&shortcut.category) {
                categories.push(shortcut.category);
            }
        }
        let sections = categories.into_iter().map(|category| {
            let rows = SHORTCUTS
                .iter()
                .filter(|shortcut| shortcut.category == category)
                .map(|shortcut| {
                    let keys = shortcut
                        .keys
                        .iter()
                        .map(|keys| display_keys(keys))
                        .collect::<Vec<_>>()
                        .join(" / ");
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(24.0))
                        .py(px(3.0))
                        .text_sm()
                        .child(shortcut.description)
                        .child(div().text_color(colors.muted_foreground).child(keys))
                });
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .pt(px(8.0))
                        .pb(px(2.0))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.muted_foreground)
                        .child(category),
                )
                .children(rows)
        });

        let close_on_scrim = on_close.clone();
        let close_on_button = on_close;
        div()
            .id("shortcuts-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors.background.opacity(0.6))
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(close) = &close_on_scrim {
                    close(window, cx);
                }
            })
            .child(
                div()
                    .id("shortcuts-card")
                    .flex()
                    .flex_col()
                    .w(px(420.0))
                    .max_h(px(520.0))
                    .p(px(16.0))
                    .gap(px(4.0))
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.popover)
                    .text_color(colors.popover_foreground)
                    .overflow_y_scroll()
                    // Clicks inside the card must not reach the scrim.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(Label::new("Keyboard shortcuts"))
                            .child(
                                Button::new("shortcuts-close", "Close")
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Small)
                                    .on_click(move |_, window, cx| {
                                        if let Some(close) = &close_on_button {
                                            close(window, cx);
                                        }
                                    }),
                            ),
                    )
                    .children(sections),
            )
    }
}
