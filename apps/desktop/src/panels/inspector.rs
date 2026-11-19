use gpui::{
    AnyElement, Context, DispatchPhase, Entity, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, SharedString, Subscription, Window, canvas, div, prelude::*, px,
};
use opencut_editor::Command;
use opencut_editor::commands::{SetElementParam, SetElementRetime, ToggleSourceAudioSeparation};
use opencut_editor::model::{
    Element, ElementKind, ElementRef, MediaAsset, ParamDefinition, ParamKind, ParamValue,
    ParamValues, RetimeConfig, Unit, param_value, visible_params,
};

use crate::components::{Button, ButtonSize, ButtonVariant, Label, NumberField, Separator};
use crate::theme::ActiveTheme;
use crate::workspace::Workspace;

/// Speed shown to the user: 0.01x steps, presets for the common cases.
const SPEED_STEP: f64 = 0.01;
const SPEED_PRESETS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
/// A stepper button moves a number this many steps.
const COARSE_STEPS: f64 = 10.0;

/// What a scrub is changing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    Param(&'static str),
    Speed,
}

/// A label being dragged sideways: one pixel is one `step`.
struct Scrub {
    element: ElementRef,
    field: Field,
    start_x: Pixels,
    start_value: f64,
    step: f64,
}

/// The selected element, copied out of the editor for one render.
struct Selected {
    element: ElementRef,
    name: String,
    kind: ElementKind,
    params: ParamValues,
    retime: Option<RetimeConfig>,
    source_audio: Option<SourceAudio>,
}

#[derive(Clone, Copy)]
enum SourceAudio {
    Extract,
    Recover,
}

/// Edits the selected element: transform, blending, audio and speed. Every
/// change is an editor command, so it is undoable and the preview follows;
/// a scrub drag folds into one undo step.
pub(crate) struct Inspector {
    workspace: Entity<Workspace>,
    scrub: Option<Scrub>,
    _workspace_changed: Subscription,
}

impl Inspector {
    pub(crate) fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let workspace_changed = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self {
            workspace,
            scrub: None,
            _workspace_changed: workspace_changed,
        }
    }

    /// The single selected element; `Err` carries what to say instead.
    fn selected(&self, cx: &Context<Self>) -> Result<Selected, String> {
        let editor = self.workspace.read(cx).editor();
        let refs = editor.selection().elements();
        let [element_ref] = refs else {
            return Err(match refs.len() {
                0 => "Select a clip to edit its properties".to_owned(),
                count => format!("{count} elements selected"),
            });
        };
        let element = editor
            .tracks()
            .element(element_ref)
            .ok_or("Select a clip to edit its properties")?;
        let source_audio = match element {
            Element::Video(video) if video.is_source_audio_enabled == Some(false) => {
                Some(SourceAudio::Recover)
            }
            Element::Video(video) => editor
                .project()
                .media_asset(&video.media_id)
                .filter(|asset: &&MediaAsset| asset.has_audio != Some(false))
                .map(|_| SourceAudio::Extract),
            _ => None,
        };
        Ok(Selected {
            element: element_ref.clone(),
            name: element.base().name.clone(),
            kind: element.kind(),
            params: element.base().params.clone(),
            retime: element.retime().cloned(),
            source_audio,
        })
    }

    fn run(&mut self, command: &mut dyn Command, coalesce: Option<String>, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| match coalesce {
            Some(key) => workspace.execute_coalescing(command, &key, cx),
            None => workspace.execute(command, cx),
        });
    }

    fn set_param(
        &mut self,
        element: &ElementRef,
        key: &str,
        value: impl Into<ParamValue>,
        coalesce: bool,
        cx: &mut Context<Self>,
    ) {
        let mut command = SetElementParam::new(element.clone(), key, value);
        let key = coalesce.then(|| command.coalesce_key());
        self.run(&mut command, key, cx);
    }

    fn set_speed(
        &mut self,
        element: &ElementRef,
        rate: f64,
        maintain_pitch: bool,
        coalesce: bool,
        cx: &mut Context<Self>,
    ) {
        let mut command = SetElementRetime {
            element: element.clone(),
            rate,
            maintain_pitch,
        };
        let key = coalesce.then(|| command.coalesce_key());
        self.run(&mut command, key, cx);
    }

    fn set_field(
        &mut self,
        element: &ElementRef,
        field: Field,
        value: f64,
        coalesce: bool,
        cx: &mut Context<Self>,
    ) {
        match field {
            Field::Param(key) => self.set_param(element, key, value, coalesce, cx),
            Field::Speed => {
                let pitch = self.maintain_pitch(element, cx);
                self.set_speed(element, value, pitch, coalesce, cx);
            }
        }
    }

    fn maintain_pitch(&self, element: &ElementRef, cx: &Context<Self>) -> bool {
        self.workspace
            .read(cx)
            .editor()
            .tracks()
            .element(element)
            .and_then(Element::retime)
            .and_then(|retime| retime.maintain_pitch)
            .unwrap_or(false)
    }

    fn scrub_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(scrub) = &self.scrub else {
            return;
        };
        let moved = f64::from(f32::from(x - scrub.start_x));
        let (element, field, value) = (
            scrub.element.clone(),
            scrub.field,
            scrub.start_value + moved * scrub.step,
        );
        self.set_field(&element, field, value, true, cx);
    }

    fn end_scrub(&mut self, cx: &mut Context<Self>) {
        if self.scrub.take().is_some() {
            self.workspace
                .update(cx, |workspace, _| workspace.end_coalescing());
        }
    }

    fn number_row(
        &self,
        selected: &Selected,
        id: SharedString,
        label: &'static str,
        field: Field,
        value: f64,
        (step, default): (f64, f64),
        text: String,
        cx: &mut Context<Self>,
    ) -> NumberField {
        let element = selected.element.clone();
        let coarse = step * COARSE_STEPS;
        let stepper = |direction: f64| {
            let element = element.clone();
            cx.listener(move |this, _, _, cx| {
                this.set_field(&element, field, value + direction * coarse, false, cx);
            })
        };
        let scrub_element = element.clone();
        let mut row = NumberField::new(id, label, text)
            .on_scrub_start(cx.listener(move |this, event: &MouseDownEvent, _, _| {
                this.scrub = Some(Scrub {
                    element: scrub_element.clone(),
                    field,
                    start_x: event.position.x,
                    start_value: value,
                    step,
                });
            }))
            .on_decrement(stepper(-1.0))
            .on_increment(stepper(1.0));
        if (value - default).abs() > f64::EPSILON {
            row = row.on_reset(cx.listener(move |this, _, _, cx| {
                this.set_field(&element, field, default, false, cx);
            }));
        }
        row
    }

    fn param_row(
        &self,
        selected: &Selected,
        param: &ParamDefinition,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element = selected.element.clone();
        let key = param.key;
        match (&param.kind, param_value(param, &selected.params)) {
            (
                ParamKind::Number {
                    step,
                    default,
                    unit,
                    ..
                },
                ParamValue::Number(value),
            ) => self
                .number_row(
                    selected,
                    format!("param-{key}").into(),
                    param.label,
                    Field::Param(key),
                    value,
                    (*step, *default),
                    format_number(value, *unit, *step),
                    cx,
                )
                .into_any_element(),
            (ParamKind::Boolean { .. }, ParamValue::Bool(on)) => toggle_row(
                param.label,
                Button::new(
                    SharedString::from(format!("param-{key}")),
                    if on { "On" } else { "Off" },
                )
                .variant(if on {
                    ButtonVariant::Default
                } else {
                    ButtonVariant::Outline
                })
                .size(ButtonSize::Small)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_param(&element, key, !on, false, cx);
                })),
            ),
            (ParamKind::Select { options, .. }, ParamValue::String(current)) => {
                let index = options
                    .iter()
                    .position(|(value, _)| *value == current)
                    .unwrap_or(0);
                let cycle = |offset: usize| {
                    let element = element.clone();
                    let options = *options;
                    cx.listener(move |this, _, _, cx| {
                        let next = options[(index + offset) % options.len()].0;
                        this.set_param(&element, key, next, false, cx);
                    })
                };
                toggle_row(
                    param.label,
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            Button::new(SharedString::from(format!("param-{key}-prev")), "‹")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::IconSmall)
                                .on_click(cycle(options.len() - 1)),
                        )
                        .child(div().w(px(84.0)).text_xs().child(options[index].1))
                        .child(
                            Button::new(SharedString::from(format!("param-{key}-next")), "›")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::IconSmall)
                                .on_click(cycle(1)),
                        ),
                )
            }
            // A stored value of the wrong type: show nothing editable.
            _ => div().into_any_element(),
        }
    }

    fn speed_section(&self, selected: &Selected, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rate = RetimeConfig::safe_rate(selected.retime.as_ref());
        let pitch = selected
            .retime
            .as_ref()
            .and_then(|retime| retime.maintain_pitch)
            .unwrap_or(false);
        let presets =
            SPEED_PRESETS
                .iter()
                .fold(div().flex().gap(px(4.0)).pl(px(88.0)), |row, preset| {
                    let element = selected.element.clone();
                    let preset = *preset;
                    row.child(
                        Button::new(
                            SharedString::from(format!("speed-{preset}")),
                            format!("{preset}x"),
                        )
                        .variant(if (rate - preset).abs() < 1e-9 {
                            ButtonVariant::Default
                        } else {
                            ButtonVariant::Outline
                        })
                        .size(ButtonSize::XSmall)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_speed(&element, preset, pitch, false, cx);
                        })),
                    )
                });
        let element = selected.element.clone();
        vec![
            section_header("Speed"),
            self.number_row(
                selected,
                "speed".into(),
                "Speed",
                Field::Speed,
                rate,
                (SPEED_STEP, RetimeConfig::DEFAULT_RATE),
                format!("{rate:.2}x"),
                cx,
            )
            .into_any_element(),
            presets.into_any_element(),
            toggle_row(
                "Keep pitch",
                Button::new("keep-pitch", if pitch { "On" } else { "Off" })
                    .variant(if pitch {
                        ButtonVariant::Default
                    } else {
                        ButtonVariant::Outline
                    })
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_speed(&element, rate, !pitch, false, cx);
                    })),
            ),
        ]
    }

    fn render_properties(&self, selected: &Selected, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let mut section = "";
        for param in visible_params(selected.kind, &selected.params) {
            let next = section_of(param.key);
            if next != section {
                section = next;
                rows.push(section_header(section));
            }
            rows.push(self.param_row(selected, &param, cx));
        }
        if let Some(action) = selected.source_audio {
            let element = selected.element.clone();
            rows.push(
                div()
                    .pt(px(4.0))
                    .child(
                        Button::new(
                            "source-audio",
                            match action {
                                SourceAudio::Extract => "Extract audio",
                                SourceAudio::Recover => "Recover audio",
                            },
                        )
                        .variant(ButtonVariant::Secondary)
                        .size(ButtonSize::Small)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.run(
                                &mut ToggleSourceAudioSeparation {
                                    element: element.clone(),
                                },
                                None,
                                cx,
                            );
                        })),
                    )
                    .into_any_element(),
            );
        }
        if selected.kind.is_retimable() {
            rows.extend(self.speed_section(selected, cx));
        }
        rows
    }
}

fn section_of(key: &str) -> &'static str {
    match key {
        _ if key.starts_with("transform.") => "Transform",
        "opacity" | "blendMode" => "Blending",
        _ => "Audio",
    }
}

fn section_header(title: &'static str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .pt(px(8.0))
        .child(Separator::horizontal())
        .child(Label::new(title))
        .into_any_element()
}

fn toggle_row(label: &'static str, control: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .h(px(28.0))
        .child(
            div()
                .w(px(84.0))
                .flex_none()
                .text_xs()
                .child(Label::new(label).muted()),
        )
        .child(control)
        .into_any_element()
}

/// `value` as shown: scaled by the unit, with just enough decimals to show
/// one `step`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn format_number(value: f64, unit: Unit, step: f64) -> String {
    let shown_step = step * unit.multiplier();
    let decimals = if shown_step >= 1.0 {
        0
    } else {
        (-shown_step.log10()).ceil() as usize
    };
    format!("{:.decimals$}{}", value * unit.multiplier(), unit.suffix())
}

impl Render for Inspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;
        let selected = self.selected(cx);

        let body = match &selected {
            Ok(selected) => div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(selected.name.clone()),
                )
                .children(self.render_properties(selected, cx)),
            Err(message) => div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child(message.clone()),
        };

        // While a scrub is on, follow the pointer anywhere in the window
        // (the panel's own listeners only fire while it's hovered).
        let this = cx.entity().downgrade();
        let scrubbing = self.scrub.is_some();
        let follow = canvas(
            move |_, _, _| this,
            move |_, this, window, _| {
                if !scrubbing {
                    return;
                }
                let moved = this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = moved.update(cx, |this, cx| this.scrub_to(event.position.x, cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let _ = this.update(cx, |this, cx| this.end_scrub(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();

        div()
            .id("inspector")
            .relative()
            .flex()
            .flex_col()
            .w_1_4()
            .h_full()
            .min_h_0()
            .p(px(12.0))
            .overflow_y_scroll()
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.card)
            .text_color(colors.card_foreground)
            .child(follow)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_show_their_unit_and_step_precision() {
        assert_eq!(format_number(0.75, Unit::Percent, 0.01), "75%");
        assert_eq!(format_number(-6.0, Unit::Decibels, 0.1), "-6.0 dB");
        assert_eq!(format_number(30.0, Unit::Degrees, 1.0), "30°");
        assert_eq!(format_number(-12.0, Unit::Pixels, 1.0), "-12 px");
    }
}
