use gpui::{
    AnyElement, Context, DispatchPhase, Entity, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Point, SharedString, Subscription, Window, canvas, div,
    fill, point, prelude::*, px,
};
use opencut_editor::animation::{EASING_PRESETS, Easing, has_keyframes, segment_easing};
use opencut_editor::commands::{
    RemoveKeyframe, SetElementParam, SetElementRetime, SetKeyframeEasing,
    ToggleSourceAudioSeparation, UpsertKeyframe,
};
use opencut_editor::model::animation::{Channel, ChannelData, ScalarChannel};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{
    Animations, Element, ElementKind, ElementRef, MediaAsset, ParamDefinition, ParamKind,
    ParamValue, ParamValues, RetimeConfig, Unit, param_value, visible_params,
};
use opencut_editor::{Command, Editor};

use crate::components::{Button, ButtonSize, ButtonVariant, Label, NumberField, Separator};
use crate::theme::ActiveTheme;
use crate::workspace::Workspace;

/// Speed shown to the user: 0.01x steps, presets for the common cases.
const SPEED_STEP: f64 = 0.01;
const SPEED_PRESETS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
/// A stepper button moves a number this many steps.
const COARSE_STEPS: f64 = 10.0;
/// The curve graph's size; its height spans easing values -0.5 to 1.5.
const GRAPH_SIZE: f32 = 120.0;
const GRAPH_VALUE_RANGE: f64 = 2.0;

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

/// A curve handle being dragged in the graph editor.
struct CurveDrag {
    target: SetKeyframeEasing,
    /// 0: the handle leaving the first key, 1: the one entering the next.
    handle: usize,
    start: Point<Pixels>,
    start_easing: Easing,
}

/// The selected element, copied out of the editor for one render.
struct Selected {
    element: ElementRef,
    name: String,
    kind: ElementKind,
    /// The params at the playhead, keyframes applied.
    params: ParamValues,
    animations: Option<Animations>,
    /// The playhead, relative to the element and clamped into it.
    local_time: MediaTime,
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
///
/// Values show at the playhead. The diamond after a param keys it there
/// (or removes that key); once a param has keys, edits set a key at the
/// playhead. Keyed numbers get a curve graph for the segment under the
/// playhead, with easing presets and draggable handles.
pub(crate) struct Inspector {
    workspace: Entity<Workspace>,
    scrub: Option<Scrub>,
    curve_drag: Option<CurveDrag>,
    _workspace_changed: Subscription,
}

impl Inspector {
    pub(crate) fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let workspace_changed = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self {
            workspace,
            scrub: None,
            curve_drag: None,
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
        let local_time = local_time(editor, element);
        Ok(Selected {
            element: element_ref.clone(),
            name: element.base().name.clone(),
            kind: element.kind(),
            params: element.base().params_at(local_time),
            animations: element.base().animations.clone(),
            local_time,
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
        let editor = self.workspace.read(cx).editor();
        let animated_at = editor.tracks().element(element).and_then(|target| {
            has_keyframes(target.base().animations.as_ref(), key)
                .then(|| local_time(editor, target))
        });
        if let Some(time) = animated_at {
            let mut command = UpsertKeyframe::new(element.clone(), key, time, value);
            let key = coalesce.then(|| command.coalesce_key());
            self.run(&mut command, key, cx);
        } else {
            let mut command = SetElementParam::new(element.clone(), key, value);
            let key = coalesce.then(|| command.coalesce_key());
            self.run(&mut command, key, cx);
        }
    }

    /// Keys `key` at the playhead, or removes the key there.
    fn toggle_keyframe(&mut self, selected: &Selected, key: &'static str, cx: &mut Context<Self>) {
        let value = selected.params.get(key).cloned();
        match key_at(selected, key) {
            Some(keyframe_id) => self.run(
                &mut RemoveKeyframe {
                    element: selected.element.clone(),
                    path: key.to_owned(),
                    keyframe_id,
                    value_at_playhead: value,
                },
                None,
                cx,
            ),
            None => {
                let Some(value) = value else {
                    return;
                };
                let mut command =
                    UpsertKeyframe::new(selected.element.clone(), key, selected.local_time, value);
                self.run(&mut command, None, cx);
            }
        }
    }

    fn curve_drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.curve_drag else {
            return;
        };
        let dx = f64::from(f32::from(position.x - drag.start.x) / GRAPH_SIZE);
        let dy = -f64::from(f32::from(position.y - drag.start.y) / GRAPH_SIZE) * GRAPH_VALUE_RANGE;
        let mut easing = drag.start_easing;
        let (x, y) = (drag.handle * 2, drag.handle * 2 + 1);
        easing[x] = (easing[x] + dx).clamp(0.0, 1.0);
        easing[y] += dy;
        drag.target.easing = easing;
        let key = drag.target.coalesce_key();
        self.workspace.update(cx, |workspace, cx| {
            workspace.execute_coalescing(&mut drag.target, &key, cx);
        });
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
        if self.scrub.take().is_some() | self.curve_drag.take().is_some() {
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

    /// A param's control followed by its keyframe toggle.
    fn param_row(
        &self,
        selected: &Selected,
        param: &ParamDefinition,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = param.key;
        let keyed = key_at(selected, key).is_some();
        let animated = has_keyframes(selected.animations.as_ref(), key);
        let element = selected.element.clone();
        let toggle = Button::new(
            SharedString::from(format!("key-{key}")),
            if keyed { "◆" } else { "◇" },
        )
        .variant(if animated {
            ButtonVariant::Secondary
        } else {
            ButtonVariant::Ghost
        })
        .size(ButtonSize::IconXSmall)
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Ok(selected) = this.selected(cx)
                && selected.element == element
            {
                this.toggle_keyframe(&selected, key, cx);
            }
        }));
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.param_control(selected, param, cx)),
            )
            .child(toggle)
            .into_any_element()
    }

    fn param_control(
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
        rows.extend(self.curve_section(selected, cx));
        if selected.kind.is_retimable() {
            rows.extend(self.speed_section(selected, cx));
        }
        rows
    }

    /// The graph editor: per keyed number, the segment under the playhead.
    fn curve_section(&self, selected: &Selected, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        for (path, data) in selected.animations.iter().flatten() {
            let ChannelData::Leaf(Channel::Scalar(channel)) = data else {
                continue;
            };
            let Some(keyframe_id) = segment_at(channel, selected.local_time) else {
                continue;
            };
            if rows.is_empty() {
                rows.push(section_header("Curves"));
            }
            let target = |easing: Easing| SetKeyframeEasing {
                element: selected.element.clone(),
                path: path.clone(),
                component: "value".to_owned(),
                keyframe_id: keyframe_id.clone(),
                easing,
            };
            let easing = segment_easing(channel, &keyframe_id);
            let presets = EASING_PRESETS.iter().enumerate().fold(
                div().flex().flex_wrap().gap(px(4.0)),
                |row, (index, (label, preset))| {
                    let command = target(*preset);
                    let active = easing.is_some_and(|easing| {
                        easing.iter().zip(preset).all(|(a, b)| (a - b).abs() < 0.02)
                    });
                    row.child(
                        Button::new(SharedString::from(format!("ease-{path}-{index}")), *label)
                            .variant(if active {
                                ButtonVariant::Default
                            } else {
                                ButtonVariant::Outline
                            })
                            .size(ButtonSize::XSmall)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run(&mut command.clone(), None, cx);
                            })),
                    )
                },
            );
            rows.push(
                div()
                    .pt(px(4.0))
                    .text_xs()
                    .child(Label::new(path.clone()).muted())
                    .into_any_element(),
            );
            if let Some(easing) = easing {
                rows.push(self.curve_graph(easing, &target, cx));
            }
            rows.push(presets.into_any_element());
        }
        rows
    }

    /// The easing curve with its two handles; drag a handle to reshape it.
    fn curve_graph(
        &self,
        easing: Easing,
        target: &dyn Fn(Easing) -> SetKeyframeEasing,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let [x1, y1, x2, y2] = easing;
        let handles = [(x1, y1), (x2, y2)]
            .into_iter()
            .enumerate()
            .map(|(index, (x, y))| {
                let (left, top) = graph_point(x, y);
                let target = target(easing);
                div()
                    .absolute()
                    .left(left - px(5.0))
                    .top(top - px(5.0))
                    .size(px(10.0))
                    .rounded_full()
                    .bg(gpui::white())
                    .border_1()
                    .border_color(gpui::black())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.curve_drag = Some(CurveDrag {
                                target: target.clone(),
                                handle: index,
                                start: event.position,
                                start_easing: easing,
                            });
                            // Re-render so the window-level drag listeners attach.
                            cx.notify();
                        }),
                    )
            });
        let graph = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let colors = window.theme().colors;
                window.paint_quad(fill(bounds, colors.background));
                let at = |x: f64, y: f64| {
                    let (left, top) = graph_point(x, y);
                    point(bounds.origin.x + left, bounds.origin.y + top)
                };
                let mut lines = PathBuilder::stroke(px(1.0));
                lines.move_to(at(0.0, 0.0));
                lines.line_to(at(x1, y1));
                lines.move_to(at(1.0, 1.0));
                lines.line_to(at(x2, y2));
                if let Ok(path) = lines.build() {
                    window.paint_path(path, colors.muted_foreground);
                }
                let mut curve = PathBuilder::stroke(px(2.0));
                curve.move_to(at(0.0, 0.0));
                curve.cubic_bezier_to(at(1.0, 1.0), at(x1, y1), at(x2, y2));
                if let Ok(path) = curve.build() {
                    window.paint_path(path, colors.primary);
                }
            },
        )
        .absolute()
        .inset_0();
        div()
            .relative()
            .size(px(GRAPH_SIZE))
            .my(px(4.0))
            .child(graph)
            .children(handles)
            .into_any_element()
    }
}

/// An easing point `(x, y)` in graph pixels from the top left.
#[allow(clippy::cast_possible_truncation)]
fn graph_point(x: f64, y: f64) -> (Pixels, Pixels) {
    (
        px(x as f32 * GRAPH_SIZE),
        px((1.0 - (y + 0.5) / GRAPH_VALUE_RANGE) as f32 * GRAPH_SIZE),
    )
}

/// `element`'s playhead, relative to it and clamped into it.
fn local_time(editor: &Editor, element: &Element) -> MediaTime {
    (editor.playback().position - element.start_time())
        .clamp(MediaTime::ZERO, element.base().duration)
}

/// The id of `key`'s keyframe at the playhead, if there is one.
fn key_at(selected: &Selected, key: &str) -> Option<String> {
    let data = selected.animations.as_ref()?.get(key)?;
    data.channels().into_iter().find_map(|(_, channel)| {
        channel
            .key_times()
            .into_iter()
            .find(|(_, time)| *time == selected.local_time)
            .map(|(id, _)| id.to_owned())
    })
}

/// The first key of the segment under `time`: the last segment past the
/// end, the first before the start. `None` with fewer than two keys.
fn segment_at(channel: &ScalarChannel, time: MediaTime) -> Option<String> {
    let channel = channel.normalized();
    let mut found = None;
    for pair in channel.keys.windows(2) {
        found = Some(pair[0].id.clone());
        if time < pair[1].time {
            break;
        }
    }
    found
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
        let scrubbing = self.scrub.is_some() || self.curve_drag.is_some();
        let follow = canvas(
            move |_, _, _| this,
            move |_, this, window, _| {
                if !scrubbing {
                    return;
                }
                let moved = this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = moved.update(cx, |this, cx| {
                            if this.curve_drag.is_some() {
                                this.curve_drag_to(event.position, cx);
                            } else {
                                this.scrub_to(event.position.x, cx);
                            }
                        });
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
