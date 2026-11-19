use gpui::{
    App, ClickEvent, ElementId, IntoElement, MouseButton, MouseDownEvent, RenderOnce, SharedString,
    Window, div, prelude::*, px,
};

use crate::components::{Button, ButtonSize, ButtonVariant};
use crate::theme::ActiveTheme;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type ScrubHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

/// A labelled number with `−`/`+` steppers and an optional reset. Pressing
/// the label starts a scrub: the owner follows the pointer (GPUI has no
/// pointer capture, so it needs window-level mouse listeners).
///
/// Deliberately has no typed entry: that needs a real text input (see the
/// note on [`TextField`](super::TextField)).
#[derive(IntoElement)]
pub(crate) struct NumberField {
    id: ElementId,
    label: SharedString,
    value: SharedString,
    on_scrub_start: Option<ScrubHandler>,
    on_decrement: Option<ClickHandler>,
    on_increment: Option<ClickHandler>,
    on_reset: Option<ClickHandler>,
}

impl NumberField {
    /// `value` is the already formatted text (with its unit).
    pub(crate) fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            on_scrub_start: None,
            on_decrement: None,
            on_increment: None,
            on_reset: None,
        }
    }

    pub(crate) fn on_scrub_start(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_scrub_start = Some(Box::new(handler));
        self
    }

    pub(crate) fn on_decrement(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_decrement = Some(Box::new(handler));
        self
    }

    pub(crate) fn on_increment(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_increment = Some(Box::new(handler));
        self
    }

    /// Shows a reset button; pass it only while the value isn't the default.
    pub(crate) fn on_reset(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_reset = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for NumberField {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let theme = window.theme();
        let colors = theme.colors;
        let id = self.id.to_string();
        let stepper = |suffix: &str, label: &'static str, handler: Option<ClickHandler>| {
            let button = Button::new(SharedString::from(format!("{id}-{suffix}")), label)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSmall);
            match handler {
                Some(handler) => button.on_click(handler),
                None => button.disabled(true),
            }
        };

        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .h(px(28.0))
            .child(
                div()
                    .id(self.id.clone())
                    .w(px(84.0))
                    .flex_none()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .cursor_ew_resize()
                    .when_some(self.on_scrub_start, |this, handler| {
                        this.on_mouse_down(MouseButton::Left, handler)
                    })
                    .child(self.label),
            )
            .child(stepper("dec", "−", self.on_decrement))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .items_center()
                    .h(px(24.0))
                    .rounded(theme.radius)
                    .bg(colors.input.opacity(0.3))
                    .text_xs()
                    .text_color(colors.foreground)
                    .whitespace_nowrap()
                    .child(self.value),
            )
            .child(stepper("inc", "+", self.on_increment))
            .child(
                div()
                    .flex_none()
                    .w(px(24.0))
                    .when_some(self.on_reset, |this, handler| {
                        this.child(
                            Button::new(SharedString::from(format!("{id}-reset")), "↺")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::IconSmall)
                                .on_click(handler),
                        )
                    }),
            )
    }
}
