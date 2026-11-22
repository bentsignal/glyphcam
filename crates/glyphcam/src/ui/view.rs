//! The control panel's state and layout: live preview with the two big
//! toggles on the left, tabbed settings on the right.

use super::{open_window, theme, widgets::*, Launch, Waker};
use crate::config;
use crate::engine::Shared;
use crate::tray::{Tray, TrayCmd};
use crate::updater::Updater;
use glyphcam_core::settings::SourceKind;
use glyphcam_core::Settings;
use gpui::{prelude::FluentBuilder as _, *};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::InputState,
    slider::SliderState,
    switch::Switch,
    tab::{Tab as UiTab, TabBar},
    Icon, IconName, Sizable as _, StyledExt as _,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Look,
    Framing,
    Camera,
    Output,
    App,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Look, Tab::Framing, Tab::Camera, Tab::Output, Tab::App];
    fn label(self) -> &'static str {
        match self {
            Tab::Look => "Look",
            Tab::Framing => "Framing",
            Tab::Camera => "Camera",
            Tab::Output => "Output",
            Tab::App => "App",
        }
    }
}

pub struct GlyphUi {
    pub sh: Arc<Shared>,
    pub s: Settings,
    seen_rev: u64,
    dirty_since: Option<Instant>,
    pub window: Option<AnyWindowHandle>,
    pub visible: bool,
    pub quitting: bool,
    tray: Option<Tray>,
    cmd_rx: Receiver<TrayCmd>,
    pub waker: Waker,
    pub updater: Updater,
    update_checked: bool,
    pub autostart_state: Option<bool>,
    pub tab: Tab,
    pub sliders: HashMap<&'static str, Entity<SliderState>>,
    pub inputs: HashMap<&'static str, Entity<InputState>>,
    pub image_path: String,
    pub rec_path: String,
    pub show_mix: bool,
    toast: Option<(String, Instant)>,
    pub close_prompt: bool,
    engine: Option<std::thread::JoinHandle<()>>,
    preview_img: Option<Arc<RenderImage>>,
    preview_seq: u64,
    retired: Vec<Arc<RenderImage>>,
    #[cfg(windows)]
    pub vcam_status: crate::vcam::install::Status,
    #[cfg(windows)]
    pub mf_status: crate::mfcam::Status,
    /// Windows 11 camera setup running in the background: None = idle.
    #[cfg(windows)]
    pub mf_job: Arc<parking_lot::Mutex<Option<Option<Result<(), String>>>>>,
}

impl GlyphUi {
    pub fn new(l: Launch, waker: Waker, _cx: &mut Context<Self>) -> Self {
        let s = l.sh.settings.read().clone();
        let wake = waker.clone();
        let tray = match crate::tray::create(s.vcam_enabled, l.cmd_tx.clone(), move || wake.wake())
        {
            Ok(t) => Some(t),
            Err(e) => {
                log::warn!("tray icon unavailable: {e}");
                None
            }
        };
        let image_path = match &s.source {
            SourceKind::Image(p) => p.clone(),
            _ => String::new(),
        };
        let rec_path = match &s.source {
            SourceKind::DepthRecording(p) => p.clone(),
            _ => String::new(),
        };
        GlyphUi {
            seen_rev: l.sh.settings_rev.load(Ordering::SeqCst),
            sh: l.sh,
            s,
            dirty_since: None,
            window: None,
            visible: false,
            quitting: false,
            tray,
            cmd_rx: l.cmd_rx,
            waker,
            updater: Updater::default(),
            update_checked: false,
            autostart_state: None,
            tab: match l.initial_tab.as_deref() {
                Some("framing") => Tab::Framing,
                Some("camera") => Tab::Camera,
                Some("output") => Tab::Output,
                Some("app") | Some("updates") => Tab::App,
                _ => Tab::Look,
            },
            sliders: HashMap::new(),
            inputs: HashMap::new(),
            image_path,
            rec_path,
            show_mix: false,
            toast: None,
            close_prompt: false,
            engine: Some(l.engine),
            preview_img: None,
            preview_seq: 0,
            retired: Vec::new(),
            #[cfg(windows)]
            vcam_status: crate::vcam::install::status(),
            #[cfg(windows)]
            mf_status: crate::mfcam::status(),
            #[cfg(windows)]
            mf_job: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }

    /// Push local edits to the engine (and schedule a save).
    pub fn commit(&mut self) {
        let cur = self.sh.settings.read().clone();
        if cur != self.s {
            self.sh.update_settings(|x| *x = self.s.clone());
            self.s = self.sh.settings.read().clone();
            self.seen_rev = self.sh.settings_rev.load(Ordering::SeqCst);
            self.dirty_since = Some(Instant::now());
        }
        self.sync_preview_wanted();
    }

    fn sync_preview_wanted(&self) {
        self.sh
            .preview_wanted
            .store(self.visible && self.s.preview, Ordering::Relaxed);
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    // ------------------------------------------------------------------
    // Window lifecycle (static: these open/close windows around the entity)
    // ------------------------------------------------------------------

    pub fn show(ui: &Entity<Self>, cx: &mut App) {
        let existing = ui
            .read(cx)
            .window
            .filter(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()));
        match existing {
            Some(h) => {
                let _ = h.update(cx, |_, window, _| window.activate_window());
            }
            None => {
                let h = open_window(ui, cx);
                ui.update(cx, |this, _| this.window = h);
            }
        }
        ui.update(cx, |this, cx| {
            this.visible = true;
            this.sync_preview_wanted();
            cx.notify();
        });
    }

    pub fn hide(ui: &Entity<Self>, cx: &mut App) {
        let h = ui.update(cx, |this, _| {
            this.visible = false;
            this.preview_img = None;
            this.retired.clear();
            this.sync_preview_wanted();
            this.window.take()
        });
        if let Some(h) = h {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
    }

    /// The window's close button. Returns whether the window may close.
    pub fn on_should_close(ui: &Entity<Self>, cx: &mut App) -> bool {
        let (quitting, to_tray, hint_shown) = {
            let t = ui.read(cx);
            (
                t.quitting,
                t.s.close_to_tray && t.tray.is_some(),
                t.s.close_hint_shown,
            )
        };
        if quitting {
            return true;
        }
        if to_tray {
            if !hint_shown {
                ui.update(cx, |this, cx| {
                    this.close_prompt = true;
                    cx.notify();
                });
                return false;
            }
            ui.update(cx, |this, _| {
                this.visible = false;
                this.window = None;
                this.preview_img = None;
                this.retired.clear();
                this.sync_preview_wanted();
            });
            true
        } else {
            Self::quit(ui, cx);
            true
        }
    }

    /// Ctrl/Cmd-W: same rules as the close button.
    pub fn request_close(ui: &Entity<Self>, cx: &mut App) {
        if ui.read(cx).window.is_none() {
            return;
        }
        let to_tray = {
            let t = ui.read(cx);
            t.s.close_to_tray && t.tray.is_some() && t.s.close_hint_shown
        };
        if to_tray {
            Self::hide(ui, cx);
        } else {
            let _ = Self::on_should_close(ui, cx);
        }
    }

    pub fn quit(ui: &Entity<Self>, cx: &mut App) {
        let engine = ui.update(cx, |this, _| {
            this.quitting = true;
            this.commit();
            config::save(&this.s);
            this.sh.quit.store(true, Ordering::SeqCst);
            this.engine.take()
        });
        if let Some(e) = engine {
            let t = Instant::now();
            while !e.is_finished() && t.elapsed() < Duration::from_millis(1500) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // make sure we really exit even if something holds the loop open
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(1500));
            std::process::exit(0);
        });
        cx.quit();
    }

    /// Something happened on another thread: tray command, new preview frame,
    /// updater progress.
    pub fn on_wake(ui: &Entity<Self>, cx: &mut App) {
        let cmds: Vec<TrayCmd> = ui.update(cx, |this, cx| {
            let cmds: Vec<TrayCmd> = this.cmd_rx.try_iter().collect();
            if this.visible {
                this.refresh_preview();
                cx.notify();
            }
            cmds
        });
        for cmd in cmds {
            match cmd {
                TrayCmd::Show => Self::show(ui, cx),
                TrayCmd::ToggleVcam => ui.update(cx, |this, cx| {
                    this.s.vcam_enabled = !this.s.vcam_enabled;
                    this.commit();
                    cx.notify();
                }),
                TrayCmd::TogglePreview => {
                    let show = ui.update(cx, |this, cx| {
                        this.s.preview = !this.s.preview;
                        this.commit();
                        cx.notify();
                        this.s.preview && !this.visible
                    });
                    if show {
                        Self::show(ui, cx);
                    }
                }
                TrayCmd::Quit => Self::quit(ui, cx),
            }
        }
    }

    /// Twice a second: pick up outside changes, tray tooltip, debounced save.
    pub fn tick(ui: &Entity<Self>, cx: &mut App) {
        ui.update(cx, |this, cx| {
            let rev = this.sh.settings_rev.load(Ordering::SeqCst);
            if rev != this.seen_rev {
                this.s = this.sh.settings.read().clone();
                this.seen_rev = rev;
            }
            if let Some(t) = &this.tray {
                t.set_vcam_checked(this.s.vcam_enabled);
                let st = this.sh.status.lock();
                let tip = if st.vcam_in_use {
                    "glyphcam: live in an app"
                } else if !this.s.vcam_enabled && cfg!(windows) {
                    "glyphcam: virtual camera paused"
                } else if st.active {
                    "glyphcam: camera on"
                } else if st.error.is_some() {
                    "glyphcam: camera problem (open for details)"
                } else {
                    "glyphcam: standing by (camera turns on when an app uses it)"
                };
                t.set_tooltip(tip);
            }
            if let Some(t) = this.dirty_since {
                if t.elapsed() > Duration::from_millis(800) {
                    config::save(&this.s);
                    this.dirty_since = None;
                }
            }
            #[cfg(windows)]
            this.poll_mf_job();
            if !this.update_checked {
                this.update_checked = true;
                let w = this.waker.clone();
                this.updater.refresh(move || w.wake());
            }
            if let Some((_, t)) = &this.toast {
                if t.elapsed() > Duration::from_secs(4) {
                    this.toast = None;
                }
            }
            this.sync_preview_wanted();
            if this.visible {
                cx.notify();
            }
        });
    }

    fn refresh_preview(&mut self) {
        let p = self.sh.preview.lock();
        let Some(p) = p.as_ref() else { return };
        if p.seq == self.preview_seq {
            return;
        }
        self.preview_seq = p.seq;
        let mut bgra = p.rgba.clone();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let Some(buf) = image::RgbaImage::from_raw(p.width, p.height, bgra) else {
            return;
        };
        let img = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buf
        )]));
        if let Some(old) = self.preview_img.replace(img) {
            self.retired.push(old);
        }
    }

    // ------------------------------------------------------------------
    // Layout
    // ------------------------------------------------------------------

    fn header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = self.sh.status.lock().clone();
        let update = self.updater.available_update(self.s.show_prereleases);
        let (dot, label) = if st.vcam_in_use {
            (theme::accent(), "Live in an app".to_string())
        } else if st.active {
            (theme::accent(), format!("Camera on · {}", st.source))
        } else if st.error.is_some() {
            (theme::danger(), "Camera problem".to_string())
        } else {
            (theme::muted(), "Standing by".to_string())
        };
        #[cfg(windows)]
        let needs_mf = self.mf_status.supported && !self.mf_status.up_to_date();
        #[cfg(not(windows))]
        let needs_mf = false;
        div()
            .h_flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .text_size(px(24.))
                    .font_family("Consolas")
                    .font_bold()
                    .text_color(theme::accent())
                    .child("glyphcam"),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(theme::card())
                    .border_1()
                    .border_color(theme::border())
                    .child(div().size(px(8.)).rounded_full().bg(dot))
                    .child(div().text_size(px(13.)).child(label)),
            )
            .child(div().flex_1())
            .when(needs_mf, |d| {
                d.child(
                    Button::new("mf-setup")
                        .warning()
                        .small()
                        .icon(IconName::TriangleAlert)
                        .label("Set up the camera for Discord")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tab = Tab::Output;
                            cx.notify();
                        })),
                )
            })
            .when_some(update, |d, u| {
                d.child(
                    Button::new("update")
                        .primary()
                        .small()
                        .icon(Icon::empty().path("icons/download.svg"))
                        .label(format!("{} available", u.tag_name))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tab = Tab::App;
                            cx.notify();
                        })),
                )
            })
    }

    fn preview(&mut self, window: &mut Window) -> impl IntoElement {
        for old in self.retired.drain(..) {
            let _ = window.drop_image(old);
        }
        let st = self.sh.status.lock().clone();
        let live = self.s.preview && st.active;
        let aspect = self.s.out_width as f32 / self.s.out_height.max(1) as f32;
        let msg: Option<String> = if !self.s.preview {
            Some("Preview is off. The virtual camera still works.".into())
        } else if let Some(e) = &st.error {
            Some(e.clone())
        } else if !st.active || self.preview_img.is_none() {
            Some("Starting camera…".into())
        } else {
            None
        };
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(16.))
                    .overflow_hidden()
                    .bg(gpui::black())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        // keep the output's aspect ratio
                        div()
                            .w_full()
                            .relative()
                            .pb(relative(1.0 / aspect))
                            .when_some(self.preview_img.clone().filter(|_| live), |d, img| {
                                d.child(
                                    gpui::img(ImageSource::Render(img))
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                )
                            })
                            .when_some(msg, |d, m| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .size_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .p_8()
                                        .text_size(px(16.))
                                        .text_color(theme::muted())
                                        .child(m),
                                )
                            }),
                    ),
            )
    }

    fn big_toggle(
        &self,
        id: &'static str,
        icon: Icon,
        title: &'static str,
        sub: String,
        on: bool,
        set: fn(&mut Settings, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex_1()
            .h_flex()
            .items_center()
            .gap_4()
            .px_5()
            .py_4()
            .rounded(px(14.))
            .border_1()
            .cursor_pointer()
            .when(on, |d| {
                d.bg(theme::accent_soft()).border_color(theme::accent())
            })
            .when(!on, |d| d.bg(theme::card()).border_color(theme::border()))
            .hover(|d| d.bg(theme::card_hover()))
            .on_click(cx.listener(move |this, _, _, cx| {
                let v = !on;
                set(&mut this.s, v);
                this.commit();
                cx.notify();
            }))
            .child(
                div()
                    .size(px(40.))
                    .rounded(px(10.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if on { theme::accent() } else { theme::border() })
                    .text_color(if on {
                        theme::hex(0x04140b)
                    } else {
                        theme::muted()
                    })
                    .text_size(px(20.))
                    .child(icon),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_size(px(16.)).font_semibold().child(title))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(if on { theme::accent() } else { theme::muted() })
                            .child(sub),
                    ),
            )
            .child(Switch::new((id, 1usize)).checked(on).on_click(cx.listener(
                move |this, v: &bool, _, cx| {
                    set(&mut this.s, *v);
                    this.commit();
                    cx.notify();
                },
            )))
    }

    fn toggles(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = self.sh.status.lock().clone();
        let vcam_sub = if !cfg!(windows) {
            "Web stream for OBS".to_string()
        } else if !self.s.vcam_enabled {
            "Paused: apps see a placeholder".to_string()
        } else if st.vcam_in_use {
            "Live in an app".to_string()
        } else {
            "Ready: pick \"glyphcam\" in any app".to_string()
        };
        let prev_sub = if self.s.preview {
            format!("{:.0} fps", st.fps)
        } else {
            "Off (saves CPU)".to_string()
        };
        div()
            .h_flex()
            .gap_4()
            .child(self.big_toggle(
                "vcam",
                Icon::empty().path("icons/webcam.svg"),
                if cfg!(windows) {
                    "Virtual camera"
                } else {
                    "Output"
                },
                vcam_sub,
                self.s.vcam_enabled,
                |s, v| s.vcam_enabled = v,
                cx,
            ))
            .child(self.big_toggle(
                "preview",
                Icon::from(IconName::Eye),
                "Preview",
                prev_sub,
                self.s.preview,
                |s, v| s.preview = v,
                cx,
            ))
    }

    fn settings_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sel = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let body = match self.tab {
            Tab::Look => self.tab_look(window, cx),
            Tab::Framing => self.tab_framing(window, cx),
            Tab::Camera => self.tab_camera(window, cx),
            Tab::Output => self.tab_output(window, cx),
            Tab::App => self.tab_app(window, cx),
        };
        div()
            .v_flex()
            .w(px(460.))
            .flex_none()
            .h_full()
            .bg(theme::panel())
            .border_l_1()
            .border_color(theme::border())
            .child(
                div().px_5().pt_5().pb_3().child(
                    TabBar::new("tabs")
                        .segmented()
                        .w_full()
                        .selected_index(sel)
                        .children(
                            Tab::ALL
                                .iter()
                                .map(|t| UiTab::new().label(t.label()).flex_1()),
                        )
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.tab = Tab::ALL[*ix];
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(div().v_flex().gap_4().px_5().pb_8().pt_1().child(body)),
            )
    }

    fn close_prompt_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::hexa(0x000000b0))
            .child(
                div()
                    .v_flex()
                    .gap_4()
                    .w(px(480.))
                    .p_6()
                    .rounded(px(16.))
                    .bg(theme::card())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_size(px(19.))
                            .font_semibold()
                            .text_color(theme::accent())
                            .child("glyphcam keeps running in the tray"),
                    )
                    .child(div().text_size(px(14.5)).child(
                        "The glyphcam camera stays available in Discord, Zoom, OBS and your \
                         browser. Your webcam turns off while nothing is using glyphcam and \
                         turns back on by itself when an app needs it.",
                    ))
                    .child(row(
                        "Keep the webcam on all the time",
                        Some("Instant start, but uses more power. Change it later in Output."),
                        self.switch(
                            "warm-prompt",
                            self.s.keep_camera_warm,
                            |s, v| s.keep_camera_warm = v,
                            cx,
                        ),
                    ))
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_3()
                            .child(button("prompt-quit", "Quit glyphcam").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.close_prompt = false;
                                    this.s.close_hint_shown = true;
                                    this.commit();
                                    let ui = cx.entity();
                                    cx.defer(move |cx| GlyphUi::quit(&ui, cx));
                                },
                            )))
                            .child(
                                Button::new("prompt-hide")
                                    .primary()
                                    .label("Hide to tray")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_prompt = false;
                                        this.s.close_hint_shown = true;
                                        this.commit();
                                        let ui = cx.entity();
                                        cx.defer(move |cx| GlyphUi::hide(&ui, cx));
                                    })),
                            ),
                    ),
            )
    }
}

impl Render for GlyphUi {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toast = self.toast.as_ref().map(|(m, _)| m.clone());
        div()
            .size_full()
            .relative()
            .bg(theme::bg())
            .text_color(theme::text())
            .h_flex()
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap_5()
                    .p_6()
                    .child(self.header(cx))
                    .child(self.preview(window))
                    .child(self.toggles(cx)),
            )
            .child(self.settings_panel(window, cx))
            .when_some(toast, |d, t| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(28.))
                        .left_0()
                        .right(px(460.))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px_5()
                                .py_3()
                                .rounded(px(12.))
                                .bg(theme::hex(0x1d2a24))
                                .border_1()
                                .border_color(theme::accent())
                                .text_size(px(14.))
                                .child(t),
                        ),
                )
            })
            .when(self.close_prompt, |d| {
                d.child(self.close_prompt_overlay(cx))
            })
    }
}
