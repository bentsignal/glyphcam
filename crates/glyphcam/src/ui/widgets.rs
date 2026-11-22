//! Small building blocks shared by the tabs: cards, labelled rows, sliders,
//! switches, segmented choices and dropdowns, all wired to `GlyphUi.s`.

use super::theme;
use super::view::GlyphUi;
use glyphcam_core::Settings;
use gpui::{prelude::FluentBuilder as _, *};
use gpui_kit::component::{
    button::Button,
    menu::{DropdownMenu as _, PopupMenuItem},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    IconName, StyledExt as _,
};
use std::rc::Rc;

/// Applies the option picked from a dropdown.
pub type Pick = Rc<dyn Fn(&mut GlyphUi, usize)>;

/// A titled group of settings.
pub fn card(title: impl Into<SharedString>) -> Div {
    div()
        .v_flex()
        .gap_4()
        .p_5()
        .rounded(px(14.))
        .bg(theme::card())
        .border_1()
        .border_color(theme::border())
        .child(
            div()
                .text_size(px(13.))
                .font_semibold()
                .text_color(theme::muted())
                .child(title.into().to_uppercase()),
        )
}

/// Label (and optional one-line explanation) on the left, control on the right.
pub fn row(label: impl Into<SharedString>, desc: Option<&str>, control: impl IntoElement) -> Div {
    div()
        .h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .min_h(px(36.))
        .child(
            div()
                .v_flex()
                .gap_0p5()
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(15.)).child(label.into()))
                .when_some(desc, |d, desc| {
                    d.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme::muted())
                            .child(SharedString::from(desc.to_string())),
                    )
                }),
        )
        .child(div().flex_none().child(control))
}

/// Muted helper text.
pub fn note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.5))
        .text_color(theme::muted())
        .child(text.into())
}

/// A small coloured status line with a dot.
pub fn status_line(color: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .h_flex()
        .gap_2()
        .items_center()
        .text_size(px(13.5))
        .child(div().size(px(8.)).rounded_full().bg(color))
        .child(div().text_color(color).child(text.into()))
}

#[derive(Clone, Copy)]
pub enum Fmt {
    /// 0..1 shown as a percentage.
    Pct,
    /// Two decimals.
    Dec,
    /// Whole number.
    Int,
    /// Signed, two decimals.
    Signed,
}

impl Fmt {
    fn show(self, v: f32) -> String {
        match self {
            Fmt::Pct => format!("{:.0}%", v * 100.0),
            Fmt::Dec => format!("{v:.2}"),
            Fmt::Int => format!("{v:.0}"),
            Fmt::Signed => format!("{v:+.2}"),
        }
    }
}

/// One slider bound to a settings field.
#[derive(Clone, Copy)]
pub struct SliderSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: Option<&'static str>,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub get: fn(&Settings) -> f32,
    pub set: fn(&mut Settings, f32),
    pub fmt: Fmt,
}

impl SliderSpec {
    pub const fn new(
        key: &'static str,
        label: &'static str,
        min: f32,
        max: f32,
        get: fn(&Settings) -> f32,
        set: fn(&mut Settings, f32),
    ) -> Self {
        Self {
            key,
            label,
            hint: None,
            min,
            max,
            step: 0.01,
            get,
            set,
            fmt: Fmt::Dec,
        }
    }
    pub const fn hint(mut self, h: &'static str) -> Self {
        self.hint = Some(h);
        self
    }
    pub const fn step(mut self, s: f32) -> Self {
        self.step = s;
        self
    }
    pub const fn fmt(mut self, f: Fmt) -> Self {
        self.fmt = f;
        self
    }
}

impl GlyphUi {
    /// A labelled slider. Its state lives across frames, keyed by `spec.key`,
    /// and follows the setting when it changes elsewhere (reset, paste, tray).
    pub fn slider(
        &mut self,
        spec: SliderSpec,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let v = (spec.get)(&self.s);
        let state = match self.sliders.get(spec.key) {
            Some(s) => s.clone(),
            None => {
                let st = cx.new(|_| {
                    SliderState::new()
                        .min(spec.min)
                        .max(spec.max)
                        .step(spec.step)
                        .default_value(v)
                });
                let set = spec.set;
                cx.subscribe(&st, move |this, _, ev: &SliderEvent, cx| {
                    let (SliderEvent::Change(val) | SliderEvent::Release(val)) = ev;
                    set(&mut this.s, val.start());
                    this.commit();
                    cx.notify();
                })
                .detach();
                self.sliders.insert(spec.key, st.clone());
                st
            }
        };
        let cur = state.read(cx).value().start();
        if (cur - v).abs() > spec.step * 0.5 {
            state.update(cx, |st, cx| st.set_value(v, window, cx));
        }
        div()
            .v_flex()
            .gap_2()
            .when(disabled, |d| d.opacity(0.45))
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_end()
                    .child(
                        div()
                            .v_flex()
                            .gap_0p5()
                            .child(div().text_size(px(15.)).child(spec.label))
                            .when_some(spec.hint, |d, h| {
                                d.child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(theme::muted())
                                        .child(h),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .text_color(theme::muted())
                            .child(spec.fmt.show(v)),
                    ),
            )
            .child(Slider::new(&state).disabled(disabled))
    }

    /// A switch bound to a settings flag.
    pub fn switch(
        &self,
        id: &'static str,
        on: bool,
        set: fn(&mut Settings, bool),
        cx: &mut Context<Self>,
    ) -> Switch {
        Switch::new(id)
            .checked(on)
            .on_click(cx.listener(move |this, v: &bool, _, cx| {
                set(&mut this.s, *v);
                this.commit();
                cx.notify();
            }))
    }

    /// Full-width segmented choice; `pick(index)` applies the selection.
    pub fn segmented(
        &self,
        id: &'static str,
        labels: &[&str],
        selected: usize,
        pick: fn(&mut GlyphUi, usize),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .h_flex()
            .w_full()
            .p_1()
            .gap_1()
            .rounded(px(10.))
            .bg(theme::bg())
            .border_1()
            .border_color(theme::border())
            .children(labels.iter().enumerate().map(|(i, l)| {
                let on = i == selected;
                div()
                    .id((id, i))
                    .flex_1()
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .text_size(px(14.))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(theme::accent_soft())
                            .text_color(theme::accent())
                            .font_semibold()
                            .border_1()
                            .border_color(theme::hexa(0x35e07a80))
                    })
                    .when(!on, |d| {
                        d.text_color(theme::muted())
                            .hover(|d| d.bg(theme::card_hover()).text_color(theme::text()))
                    })
                    .child(SharedString::from(l.to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pick(this, i);
                        this.commit();
                        cx.notify();
                    }))
            }))
    }

    /// A dropdown button listing `options`; `pick(index)` applies the choice.
    pub fn dropdown(
        &self,
        id: &'static str,
        current: impl Into<SharedString>,
        options: Vec<(SharedString, bool)>,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let options = Rc::new(options);
        Button::new(id)
            .outline()
            .label(current.into())
            .icon(IconName::ChevronsUpDown)
            .min_w(px(210.))
            .dropdown_menu(move |mut menu, _, _| {
                for (i, (label, checked)) in options.iter().enumerate() {
                    let weak = weak.clone();
                    let pick = pick.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(*checked)
                            .on_click(move |_, _, cx| {
                                if let Some(e) = weak.upgrade() {
                                    e.update(cx, |this, cx| {
                                        pick(this, i);
                                        this.commit();
                                        cx.notify();
                                    });
                                }
                            }),
                    );
                }
                menu.min_w(px(240.)).scrollable(true).max_h(px(420.))
            })
    }
}

/// A plain secondary button.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).label(label.into())
}
