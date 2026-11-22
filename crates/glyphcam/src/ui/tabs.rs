//! The settings tabs.

use super::view::GlyphUi;
use super::{theme, widgets::*};
use crate::config::{self, BUILD_SHA, CHANNEL};
use crate::updater::{Job, ListState};
use glyphcam_core::glyphs;
use glyphcam_core::settings::{
    Background, ColorMode, DepthMode, GlyphPreset, Scripts, Settings, SourceKind, OUTPUT_SIZES,
};
use gpui::{prelude::FluentBuilder as _, *};
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    progress::Progress,
    Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::Duration;

const SCRIPT_NAMES: [&str; 10] = [
    "Katakana",
    "Latin",
    "Digits",
    "Punctuation",
    "Greek",
    "Cyrillic",
    "Math",
    "Box drawing",
    "Blocks",
    "Braille",
];

fn script_flag(sc: &mut Scripts, i: usize) -> &mut bool {
    match i {
        0 => &mut sc.katakana,
        1 => &mut sc.latin,
        2 => &mut sc.digits,
        3 => &mut sc.punctuation,
        4 => &mut sc.greek,
        5 => &mut sc.cyrillic,
        6 => &mut sc.math,
        7 => &mut sc.box_drawing,
        8 => &mut sc.blocks,
        _ => &mut sc.braille,
    }
}

const BACKGROUNDS: [Background; 4] = [
    Background::Rain,
    Background::Dim,
    Background::Hide,
    Background::Show,
];

impl GlyphUi {
    // ------------------------------------------------------------------
    // Look
    // ------------------------------------------------------------------

    pub fn tab_look(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let st = self.sh.status.lock().clone();

        // --- Style ---
        let colors: Vec<_> = ColorMode::ALL
            .iter()
            .map(|m| (SharedString::from(m.label()), *m == self.s.color_mode))
            .collect();
        let preset = GlyphPreset::matching(&self.s.scripts);
        let mut presets: Vec<_> = GlyphPreset::ALL
            .iter()
            .map(|p| (SharedString::from(p.label()), preset == Some(*p)))
            .collect();
        if preset.is_none() {
            presets.push(("Custom mix".into(), true));
        }
        let mut style = card("Style")
            .child(row(
                "Colour",
                None,
                self.dropdown(
                    "color",
                    self.s.color_mode.label(),
                    colors,
                    Rc::new(|this, i| this.s.color_mode = ColorMode::ALL[i]),
                    cx,
                ),
            ))
            .child(row(
                "Glyphs",
                None,
                self.dropdown(
                    "glyphs",
                    preset.map(|p| p.label()).unwrap_or("Custom mix"),
                    presets,
                    Rc::new(|this, i| {
                        if let Some(p) = GlyphPreset::ALL.get(i) {
                            this.s.scripts = p.scripts();
                        }
                    }),
                    cx,
                ),
            ));
        if self.s.color_mode == ColorMode::TrueColor {
            style = style.child(self.slider(
                SliderSpec::new(
                    "saturation",
                    "Saturation",
                    0.0,
                    3.0,
                    |s| s.saturation,
                    |s, v| s.saturation = v,
                ),
                false,
                window,
                cx,
            ));
        }
        if self.s.color_mode == ColorMode::Custom {
            style = style.child(self.custom_colors(cx));
        }
        style = style.child(
            self.slider(
                SliderSpec::new(
                    "columns",
                    "Density",
                    48.0,
                    240.0,
                    |s| s.columns as f32,
                    |s, v| s.columns = v.round() as u32,
                )
                .step(4.0)
                .fmt(Fmt::Int)
                .hint("Glyph columns across the frame"),
                false,
                window,
                cx,
            ),
        );
        style = style.child(self.glyph_mix(cx));

        // --- Depth camera ---
        let mut depth = card("Depth camera").child(if st.depth {
            status_line(
                theme::accent(),
                format!(
                    "Depth active · your face is {:.0} cm away",
                    st.depth_m * 100.0
                ),
            )
        } else {
            status_line(
                theme::muted(),
                "No depth camera. The head shape is estimated instead.",
            )
        });
        let mode_ix = DepthMode::ALL
            .iter()
            .position(|m| *m == self.s.depth_mode)
            .unwrap_or(0);
        let labels: Vec<&str> = DepthMode::ALL
            .iter()
            .map(|m| match m {
                DepthMode::Relief => "Relief",
                DepthMode::Ridges => "Ridgelines",
                DepthMode::Wrapped => "Wrapped 3D",
            })
            .collect();
        depth = depth.child(self.segmented(
            "depthmode",
            &labels,
            mode_ix,
            |this, i| this.s.depth_mode = DepthMode::ALL[i],
            cx,
        ));
        match self.s.depth_mode {
            DepthMode::Ridges => {
                for spec in [
                    SliderSpec::new(
                        "mesh_detail",
                        "Line density",
                        0.0,
                        1.0,
                        |s| s.mesh_detail,
                        |s, v| s.mesh_detail = v,
                    )
                    .fmt(Fmt::Pct),
                    SliderSpec::new(
                        "depth_fill",
                        "Face glyphs",
                        0.0,
                        1.0,
                        |s| s.depth_fill,
                        |s, v| s.depth_fill = v,
                    )
                    .fmt(Fmt::Pct)
                    .hint("How much of the glyph face shows under the lines"),
                    SliderSpec::new(
                        "holo",
                        "Relief height",
                        0.0,
                        1.0,
                        |s| s.holo,
                        |s, v| s.holo = v,
                    )
                    .fmt(Fmt::Pct)
                    .hint("How far your face lifts the lines"),
                    SliderSpec::new(
                        "glitch",
                        "Glitch",
                        0.0,
                        1.0,
                        |s| s.glitch,
                        |s, v| s.glitch = v,
                    )
                    .fmt(Fmt::Pct),
                ] {
                    depth = depth.child(self.slider(spec, false, window, cx));
                }
            }
            DepthMode::Wrapped => {
                depth = depth.child(
                    self.slider(
                        SliderSpec::new(
                            "wrap_calm",
                            "Calm",
                            0.0,
                            1.0,
                            |s| s.wrap_calm,
                            |s, v| s.wrap_calm = v,
                        )
                        .fmt(Fmt::Pct)
                        .hint("Steadies glyphs on your skin; higher lags a little"),
                        false,
                        window,
                        cx,
                    ),
                );
            }
            DepthMode::Relief => {}
        }

        // --- Background ---
        let bg_ix = BACKGROUNDS
            .iter()
            .position(|b| *b == self.s.background)
            .unwrap_or(0);
        let mut background = card("Background").child(self.segmented(
            "bg",
            &["Rain", "Dim", "Hide", "Camera"],
            bg_ix,
            |this, i| this.s.background = BACKGROUNDS[i],
            cx,
        ));
        match self.s.background {
            Background::Dim => {
                background = background.child(
                    self.slider(
                        SliderSpec::new(
                            "bg_dim",
                            "Background level",
                            0.0,
                            1.0,
                            |s| s.bg_dim,
                            |s, v| s.bg_dim = v,
                        )
                        .fmt(Fmt::Pct),
                        false,
                        window,
                        cx,
                    ),
                )
            }
            Background::Rain => {
                background = background.child(self.slider(
                    SliderSpec::new(
                        "rain_speed",
                        "Rain speed",
                        0.2,
                        3.0,
                        |s| s.rain_speed,
                        |s, v| s.rain_speed = v,
                    ),
                    false,
                    window,
                    cx,
                ))
            }
            _ => {}
        }

        // --- Tone ---
        let mut tone = card("Tone").child(row(
            "Auto exposure",
            Some("Normalise brightness to your face"),
            self.switch(
                "auto_exposure",
                self.s.auto_exposure,
                |s, v| s.auto_exposure = v,
                cx,
            ),
        ));
        for spec in [
            SliderSpec::new(
                "contrast",
                "Contrast",
                0.3,
                2.5,
                |s| s.contrast,
                |s, v| s.contrast = v,
            ),
            SliderSpec::new(
                "brightness",
                "Brightness",
                -0.6,
                0.6,
                |s| s.brightness,
                |s, v| s.brightness = v,
            )
            .fmt(Fmt::Signed),
            SliderSpec::new("gamma", "Gamma", 0.4, 2.0, |s| s.gamma, |s, v| s.gamma = v),
            SliderSpec::new("edge", "Detail", 0.0, 2.0, |s| s.edge, |s, v| s.edge = v)
                .hint("Makes eyes and mouth pop"),
            SliderSpec::new(
                "depth",
                "Depth shading",
                0.0,
                1.0,
                |s| s.depth,
                |s, v| s.depth = v,
            )
            .fmt(Fmt::Pct)
            .hint("How much your face's 3D shape shades the glyphs"),
            SliderSpec::new("glow", "Glow", 0.0, 1.0, |s| s.glow, |s, v| s.glow = v).fmt(Fmt::Pct),
            SliderSpec::new(
                "churn",
                "Shimmer",
                0.0,
                6.0,
                |s| s.churn,
                |s, v| s.churn = v,
            )
            .hint("How often glyphs change"),
        ] {
            tone = tone.child(self.slider(spec, false, window, cx));
        }

        let reset = div().h_flex().justify_between().gap_3().children([
            button("copy-look", "Copy settings")
                .icon(IconName::Copy)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.copy_settings();
                    cx.notify();
                }))
                .into_any_element(),
            Button::new("reset-look")
                .ghost()
                .label("Reset look")
                .icon(IconName::RotateCw)
                .on_click(cx.listener(|this, _, _, cx| {
                    reset_look(&mut this.s);
                    this.commit();
                    cx.notify();
                }))
                .into_any_element(),
        ]);

        div()
            .v_flex()
            .gap_4()
            .child(style)
            .child(depth)
            .child(background)
            .child(tone)
            .child(reset)
    }

    fn custom_colors(&mut self, cx: &mut Context<Self>) -> Div {
        // a small palette of swatches for glyph and background colour
        const FG: [[u8; 3]; 8] = [
            [53, 224, 122],
            [80, 200, 255],
            [255, 64, 200],
            [255, 190, 92],
            [255, 90, 90],
            [190, 140, 255],
            [255, 255, 255],
            [255, 230, 120],
        ];
        const BG: [[u8; 3]; 6] = [
            [0, 0, 0],
            [6, 0, 12],
            [0, 10, 6],
            [4, 8, 20],
            [20, 6, 6],
            [18, 18, 18],
        ];
        let swatches = |id: &'static str,
                        list: &'static [[u8; 3]],
                        cur: [u8; 3],
                        set: fn(&mut Settings, [u8; 3]),
                        cx: &mut Context<Self>| {
            div()
                .h_flex()
                .gap_2()
                .children(list.iter().enumerate().map(|(i, c)| {
                    let c = *c;
                    div()
                        .id((id, i))
                        .size(px(26.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .bg(rgb(((c[0] as u32) << 16)
                            | ((c[1] as u32) << 8)
                            | c[2] as u32))
                        .border_2()
                        .border_color(if cur == c {
                            theme::accent()
                        } else {
                            theme::border()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            set(&mut this.s, c);
                            this.commit();
                            cx.notify();
                        }))
                }))
        };
        div()
            .v_flex()
            .gap_3()
            .child(row(
                "Glyph colour",
                None,
                swatches("fg", &FG, self.s.custom_fg, |s, c| s.custom_fg = c, cx),
            ))
            .child(row(
                "Background",
                None,
                swatches("bgc", &BG, self.s.custom_bg, |s, c| s.custom_bg = c, cx),
            ))
    }

    fn glyph_mix(&mut self, cx: &mut Context<Self>) -> Div {
        let open = self.show_mix;
        let mut d = div().v_flex().gap_3().child(
            div()
                .id("mix-toggle")
                .h_flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .text_size(px(14.))
                .text_color(theme::muted())
                .hover(|d| d.text_color(theme::text()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_mix = !this.show_mix;
                    cx.notify();
                }))
                .child(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .child("Custom glyph mix"),
        );
        if open {
            let mut grid = div().grid().grid_cols(2).gap_x_4().gap_y_2();
            for (i, name) in SCRIPT_NAMES.iter().enumerate() {
                let mut sc = self.s.scripts;
                let on = *script_flag(&mut sc, i);
                grid = grid.child(
                    Checkbox::new(("script", i))
                        .label(*name)
                        .checked(on)
                        .on_click(cx.listener(move |this, v: &bool, _, cx| {
                            *script_flag(&mut this.s.scripts, i) = *v;
                            this.commit();
                            cx.notify();
                        })),
                );
            }
            d = d
                .child(grid)
                .child(
                    div()
                        .p_3()
                        .rounded(px(10.))
                        .bg(theme::bg())
                        .text_size(px(16.))
                        .text_color(theme::accent())
                        .child(glyphs::sample_text(&self.s.scripts, 30)),
                )
                .child(row(
                    "Mirror katakana",
                    Some("Film style"),
                    self.switch(
                        "mirror_kata",
                        self.s.mirror_katakana,
                        |s, v| s.mirror_katakana = v,
                        cx,
                    ),
                ));
        }
        d
    }

    // ------------------------------------------------------------------
    // Framing
    // ------------------------------------------------------------------

    pub fn tab_framing(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let on = self.s.stabilize;
        let mut stab = card("Face stabilisation").child(row(
            "Keep my face centred",
            Some("Your face stays put while you move, with a little sway"),
            self.switch("stabilize", on, |s, v| s.stabilize = v, cx),
        ));
        for spec in [
            SliderSpec::new(
                "face_size",
                "Face size",
                0.2,
                0.85,
                |s| s.face_size,
                |s, v| s.face_size = v,
            )
            .fmt(Fmt::Pct)
            .hint("How much of the frame height your face fills"),
            SliderSpec::new(
                "vertical",
                "Vertical position",
                0.3,
                0.65,
                |s| s.vertical,
                |s, v| s.vertical = v,
            )
            .fmt(Fmt::Pct),
            SliderSpec::new("sway", "Sway", 0.0, 1.0, |s| s.sway, |s, v| s.sway = v)
                .fmt(Fmt::Pct)
                .hint("0 = locked solid, 1 = loose and bouncy"),
            SliderSpec::new(
                "tilt",
                "Tilt into motion",
                0.0,
                1.0,
                |s| s.tilt,
                |s, v| s.tilt = v,
            )
            .fmt(Fmt::Pct),
            SliderSpec::new(
                "idle_bob",
                "Idle drift",
                0.0,
                1.0,
                |s| s.idle_bob,
                |s, v| s.idle_bob = v,
            )
            .fmt(Fmt::Pct),
            SliderSpec::new(
                "roll_lock",
                "Keep upright",
                0.0,
                1.0,
                |s| s.roll_lock,
                |s, v| s.roll_lock = v,
            )
            .fmt(Fmt::Pct)
            .hint("Cancel head tilt"),
        ] {
            stab = stab.child(self.slider(spec, !on, window, cx));
        }
        let view = card("View").child(row(
            "Mirror",
            Some("Selfie view"),
            self.switch("mirror", self.s.mirror, |s, v| s.mirror = v, cx),
        ));
        div().v_flex().gap_4().child(stab).child(view)
    }

    // ------------------------------------------------------------------
    // Camera
    // ------------------------------------------------------------------

    /// A text input whose value is applied on Enter or when it loses focus.
    fn path_input(
        &mut self,
        key: &'static str,
        initial: String,
        placeholder: &'static str,
        apply: fn(&mut GlyphUi, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Input {
        let state = match self.inputs.get(key) {
            Some(s) => s.clone(),
            None => {
                let st = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(initial)
                });
                cx.subscribe(&st, move |this, st, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let v = st.read(cx).value().to_string();
                        apply(this, v.trim().trim_matches('"').to_string());
                        this.commit();
                        cx.notify();
                    }
                })
                .detach();
                self.inputs.insert(key, st.clone());
                st
            }
        };
        Input::new(&state)
    }

    pub fn tab_camera(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cams = self.sh.cameras.lock().clone();
        let st = self.sh.status.lock().clone();
        let kind = match &self.s.source {
            SourceKind::Camera(_) => 0,
            SourceKind::Demo => 1,
            SourceKind::Image(_) => 2,
            SourceKind::DepthRecording(_) => 3,
        };
        let mut src = card("Input").child(self.segmented(
            "source",
            &["Camera", "Demo", "Image", "Recording"],
            kind,
            |this, i| {
                this.s.source = match i {
                    0 => match &this.s.source {
                        SourceKind::Camera(c) => SourceKind::Camera(c.clone()),
                        _ => SourceKind::Camera(None),
                    },
                    1 => SourceKind::Demo,
                    2 => SourceKind::Image(this.image_path.clone()),
                    _ => SourceKind::DepthRecording(this.rec_path.clone()),
                }
            },
            cx,
        ));
        match self.s.source.clone() {
            SourceKind::Camera(sel) => {
                const AUTO: &str = "Automatic (prefer a real webcam)";
                let mut opts = vec![(SharedString::from(AUTO), sel.is_none())];
                opts.extend(cams.iter().map(|c| {
                    (
                        SharedString::from(c.name.clone()),
                        sel.as_deref() == Some(&c.name),
                    )
                }));
                let names: Vec<String> = cams.iter().map(|c| c.name.clone()).collect();
                src = src
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(div().flex_1().child(self.dropdown(
                                "camera",
                                sel.clone().unwrap_or_else(|| AUTO.into()),
                                opts,
                                Rc::new(move |this, i| {
                                    this.s.source = SourceKind::Camera(if i == 0 {
                                        None
                                    } else {
                                        names.get(i - 1).cloned()
                                    })
                                }),
                                cx,
                            )))
                            .child(
                                Button::new("rescan")
                                    .outline()
                                    .icon(IconName::RefreshCw)
                                    .tooltip("Rescan cameras")
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.sh.refresh_cameras.store(true, Ordering::SeqCst);
                                    })),
                            ),
                    )
                    .when(cams.is_empty(), |d| {
                        d.child(status_line(theme::warn(), "No cameras detected"))
                    })
                    .child(
                        div()
                            .v_flex()
                            .gap_2()
                            .child("Quality")
                            .child(self.segmented(
                                "quality",
                                &["Light · 480p", "Sharp · 720p"],
                                if self.s.camera_quality >= 720 { 1 } else { 0 },
                                |this, i| this.s.camera_quality = if i == 1 { 720 } else { 480 },
                                cx,
                            )),
                    );
            }
            SourceKind::Demo => {
                src = src.child(note("An animated test head; no camera needed."));
            }
            SourceKind::Image(_) => {
                let p = self.image_path.clone();
                src = src
                    .child(self.path_input(
                        "image",
                        p,
                        "Path to a photo",
                        |this, v| {
                            this.image_path = v.clone();
                            this.s.source = SourceKind::Image(v);
                        },
                        window,
                        cx,
                    ))
                    .child(note("A photo viewed through a simulated wobbling camera."));
            }
            SourceKind::DepthRecording(_) => {
                let p = self.rec_path.clone();
                src = src
                    .child(self.path_input("rec", p, "Folder with meta.json", |this, v| {
                        this.rec_path = v.clone();
                        this.s.source = SourceKind::DepthRecording(v);
                    }, window, cx))
                    .child(note("A recorded colour + depth clip, looped. Try the depth looks without the camera."));
            }
        }

        let kv = |k: &'static str, v: AnyElement| {
            div()
                .h_flex()
                .justify_between()
                .gap_4()
                .text_size(px(14.))
                .child(div().text_color(theme::muted()).child(k))
                .child(v)
        };
        let good = |b: bool, yes: String, no: &'static str| {
            div()
                .text_color(if b { theme::accent() } else { theme::muted() })
                .child(if b { yes } else { no.to_string() })
                .into_any_element()
        };
        let status = card("Status")
            .child(kv(
                "Source",
                div()
                    .text_right()
                    .child(if st.active {
                        format!("{} · {}", st.source, st.source_detail)
                    } else {
                        "Idle (camera off)".into()
                    })
                    .into_any_element(),
            ))
            .child(kv(
                "Output",
                format!("{:.1} fps", st.fps).into_any_element(),
            ))
            .child(kv("Face", good(st.face, "Tracked".into(), "Not found")))
            .child(kv(
                "Depth",
                good(
                    st.depth,
                    format!("Face at {:.2} m", st.depth_m),
                    "No depth data",
                ),
            ))
            .child(kv(
                "Timings",
                div()
                    .text_color(theme::muted())
                    .child(format!(
                        "detect {:.1} · mask {:.1} · frame {:.1} ms",
                        st.detect_ms, st.segment_ms, st.render_ms
                    ))
                    .into_any_element(),
            ))
            .when_some(st.error.clone(), |d, e| {
                d.child(status_line(theme::danger(), e))
            });
        div().v_flex().gap_4().child(src).child(status)
    }

    // ------------------------------------------------------------------
    // Output
    // ------------------------------------------------------------------

    pub fn tab_output(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let st = self.sh.status.lock().clone();
        let sizes: Vec<_> = OUTPUT_SIZES
            .iter()
            .map(|(w, h)| {
                (
                    SharedString::from(format!("{w} × {h}")),
                    self.s.out_width == *w && self.s.out_height == *h,
                )
            })
            .collect();
        let format = card("Format")
            .child(row(
                "Resolution",
                None,
                self.dropdown(
                    "res",
                    format!("{} × {}", self.s.out_width, self.s.out_height),
                    sizes,
                    Rc::new(|this, i| {
                        let (w, h) = OUTPUT_SIZES[i];
                        this.s.out_width = w;
                        this.s.out_height = h;
                    }),
                    cx,
                ),
            ))
            .child(
                self.slider(
                    SliderSpec::new(
                        "fps",
                        "Frame rate",
                        15.0,
                        60.0,
                        |s| s.fps as f32,
                        |s, v| s.fps = v.round() as u32,
                    )
                    .step(5.0)
                    .fmt(Fmt::Int),
                    false,
                    window,
                    cx,
                ),
            )
            .child(row(
                "Keep the webcam on",
                Some("Instant start, but the light stays on and it uses more power"),
                self.switch(
                    "warm",
                    self.s.keep_camera_warm,
                    |s, v| s.keep_camera_warm = v,
                    cx,
                ),
            ));

        let mut col = div().v_flex().gap_4().child(format);
        #[cfg(windows)]
        {
            col = col
                .child(self.system_camera_card(cx))
                .child(self.classic_camera_card(cx));
        }
        #[cfg(not(windows))]
        {
            col = col.child(
                card("Virtual camera")
                    .child(status_line(
                        theme::warn(),
                        "No macOS system camera in this build",
                    ))
                    .child(note(
                        "macOS only accepts virtual cameras as an Apple-signed Camera Extension. \
                         Until then: add a Browser source in OBS with the stream address below \
                         (1280×720), click Start Virtual Camera, and pick \"OBS Virtual Camera\" \
                         in your app.",
                    )),
            );
        }
        col.child(self.stream_card(&st, window, cx))
    }

    #[cfg(windows)]
    pub fn poll_mf_job(&mut self) {
        let job = self.mf_job.lock().clone();
        if let Some(Some(res)) = job {
            match res {
                Ok(()) => self.toast("Windows 11 camera ready. Restart Discord to see it."),
                Err(e) => self.toast(format!("Camera setup: {e}")),
            }
            *self.mf_job.lock() = None;
            self.mf_status = crate::mfcam::status();
        }
    }

    #[cfg(windows)]
    fn system_camera_card(&mut self, cx: &mut Context<Self>) -> Div {
        let busy = matches!(*self.mf_job.lock(), Some(None));
        let st = self.mf_status.clone();
        let mut c = card("Windows 11 camera");
        c = c.child(note("Discord, Teams, Windows Camera and everything else."));
        if !st.supported {
            return c.child(status_line(
                theme::muted(),
                "Needs Windows 11. On Windows 10, use the classic camera.",
            ));
        }
        c = c.child(if st.up_to_date() {
            status_line(
                theme::accent(),
                format!(
                    "Installed: pick \"{}\" in your app",
                    crate::mfcam::CAMERA_NAME
                ),
            )
        } else if st.installed() {
            status_line(theme::warn(), "An older version is installed; update it")
        } else {
            status_line(
                theme::warn(),
                "Not set up yet. Discord only lists this kind of camera.",
            )
        });
        let label = if busy {
            "Setting up…"
        } else if st.up_to_date() {
            "Repair"
        } else if st.installed() {
            "Update (asks for admin once)"
        } else {
            "Set up (asks for admin once)"
        };
        let waker = self.waker.clone();
        let waker2 = self.waker.clone();
        c.child(
            div()
                .h_flex()
                .gap_2()
                .child({
                    let b = Button::new("mf-go")
                        .label(label)
                        .loading(busy)
                        .disabled(busy);
                    let b = if st.up_to_date() {
                        b.outline()
                    } else {
                        b.primary()
                    };
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        *this.mf_job.lock() = Some(None);
                        let job = this.mf_job.clone();
                        let w = waker.clone();
                        std::thread::spawn(move || {
                            let r = crate::mfcam::setup().map_err(|e| format!("{e:#}"));
                            *job.lock() = Some(Some(r));
                            w.wake();
                        });
                        cx.notify();
                    }))
                })
                .when(st.installed() && !busy, |d| {
                    d.child(button("mf-remove", "Remove").ghost().on_click(cx.listener(
                        move |this, _, _, cx| {
                            *this.mf_job.lock() = Some(None);
                            let job = this.mf_job.clone();
                            let w = waker2.clone();
                            std::thread::spawn(move || {
                                crate::mfcam::teardown();
                                *job.lock() = Some(Some(Ok(())));
                                w.wake();
                            });
                            cx.notify();
                        },
                    )))
                }),
        )
    }

    #[cfg(windows)]
    fn classic_camera_card(&mut self, cx: &mut Context<Self>) -> Div {
        let ok = self.vcam_status.ok();
        card("Classic camera")
            .child(note("OBS, Zoom, browsers and Windows 10."))
            .child(if ok {
                status_line(theme::accent(), "\"glyphcam\" is installed for this user")
            } else {
                status_line(theme::warn(), "Not registered")
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        button("vcam-repair", "Repair")
                            .outline()
                            .on_click(cx.listener(|this, _, _, cx| {
                                match crate::vcam::install::ensure_registered() {
                                    Ok(()) => this.toast("Virtual camera registered"),
                                    Err(e) => this.toast(format!("Registration failed: {e}")),
                                }
                                this.vcam_status = crate::vcam::install::status();
                                cx.notify();
                            })),
                    )
                    .child(
                        button("vcam-remove", "Remove")
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                crate::vcam::install::unregister();
                                this.vcam_status = crate::vcam::install::status();
                                cx.notify();
                            })),
                    ),
            )
            .child(note(
                "Restart apps that were already open so they see a new camera.",
            ))
    }

    fn stream_card(
        &mut self,
        st: &crate::engine::Status,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let on = self.s.stream_server;
        let mut c = card("Web stream").child(row(
            "Serve the output as a web stream",
            Some("For OBS, a browser, or another computer"),
            self.switch("stream", on, |s, v| s.stream_server = v, cx),
        ));
        if !on {
            return c;
        }
        c = c.child(row(
            "Allow other devices on my network",
            Some("Wi-Fi, LAN or Tailscale"),
            self.switch("stream_lan", self.s.stream_lan, |s, v| s.stream_lan = v, cx),
        ));
        if self.s.stream_lan {
            c = c.child(status_line(
                theme::warn(),
                "Anyone on your network can watch while this is on",
            ));
        }
        let port = self.s.stream_port.to_string();
        c = c.child(row(
            "Port",
            None,
            div().w(px(110.)).child(self.path_input(
                "port",
                port,
                "8737",
                |this, v| {
                    if let Ok(p) = v.parse::<u16>() {
                        this.s.stream_port = p;
                    }
                },
                window,
                cx,
            )),
        ));
        let url_row = |id: &'static str, url: String, label: String, cx: &mut Context<Self>| {
            let u = url.clone();
            let u2 = url.clone();
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .p_2()
                .pl_3()
                .rounded(px(10.))
                .bg(theme::bg())
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.5))
                                .text_color(theme::accent())
                                .child(url),
                        )
                        .when(!label.is_empty(), |d| d.child(note(label))),
                )
                .child(
                    Button::new((id, 0usize))
                        .ghost()
                        .small()
                        .icon(IconName::Copy)
                        .tooltip("Copy")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(u.clone()));
                            this.toast("Address copied");
                            cx.notify();
                        })),
                )
                .child(
                    Button::new((id, 1usize))
                        .ghost()
                        .small()
                        .icon(IconName::ExternalLink)
                        .tooltip("Open")
                        .on_click(move |_, _, cx| cx.open_url(&u2)),
                )
        };
        if let Some(url) = st.stream_url.clone() {
            c = c.child(url_row("url-local", url, "This computer".into(), cx));
            if self.s.stream_lan {
                if st.stream_lan_urls.is_empty() {
                    c = c.child(note("Looking for this computer's network address…"));
                }
                for (i, entry) in st.stream_lan_urls.iter().enumerate() {
                    let (u, label) = entry.split_once("  ").unwrap_or((entry.as_str(), ""));
                    let id: &'static str = ["url-0", "url-1", "url-2", "url-3", "url-4", "url-5"]
                        .get(i)
                        .copied()
                        .unwrap_or("url-x");
                    c = c.child(url_row(id, u.to_string(), label.to_string(), cx));
                }
                c = c.child(note(
                    "On the other computer: OBS → + → Browser → paste the address (1280×720), \
                     then Start Virtual Camera. Or just open it in a browser.",
                ));
                #[cfg(windows)]
                {
                    c = c.child(
                        div().child(
                            button("firewall", "Allow through Windows Firewall")
                                .outline()
                                .tooltip(
                                    "If other devices can't connect. Asks for admin permission.",
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let msg =
                                        match crate::platform::allow_firewall(this.s.stream_port) {
                                            Ok(()) => "Firewall rule requested".to_string(),
                                            Err(e) => format!("Firewall: {e}"),
                                        };
                                    this.toast(msg);
                                    cx.notify();
                                })),
                        ),
                    );
                }
            }
            c = c.child(note(format!(
                "{} viewer(s) connected · raw MJPEG at …/stream.mjpg",
                st.stream_clients
            )));
        }
        if let Some(e) = &st.stream_error {
            c = c.child(status_line(theme::danger(), e.clone()));
        }
        c
    }

    // ------------------------------------------------------------------
    // App (+ updates)
    // ------------------------------------------------------------------

    pub fn copy_settings(&mut self) {
        let code = crate::share::export(&self.s);
        let msg = match crate::share::copy_to_clipboard(&code) {
            Ok(()) => "Settings copied. Paste them in glyphcam on your other computer.".to_string(),
            Err(e) => format!("Couldn't copy: {e}"),
        };
        self.toast(msg);
    }

    pub fn tab_app(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let updates = self.updates_card(cx);
        let transfer = card("Move your setup to another computer")
            .child(note(
                "Copy here, then paste in glyphcam on the other machine. Camera choice and app \
                 options stay per computer.",
            ))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        button("copy", "Copy settings")
                            .icon(IconName::Copy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy_settings();
                                cx.notify();
                            })),
                    )
                    .child(
                        button("paste", "Paste settings")
                            .icon(Icon::empty().path("icons/clipboard-paste.svg"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let msg = match crate::share::read_clipboard()
                                    .and_then(|t| crate::share::import(&t, &this.s))
                                {
                                    Ok((new, n)) => {
                                        this.s = new;
                                        this.commit();
                                        format!("Applied {n} settings")
                                    }
                                    Err(e) => e,
                                };
                                this.toast(msg);
                                cx.notify();
                            })),
                    ),
            );

        let auto = *self
            .autostart_state
            .get_or_insert_with(crate::platform::autostart_enabled);
        let startup = card("Startup")
            .child(row(
                "Start glyphcam when I log in",
                None,
                gpui_kit::component::switch::Switch::new("autostart")
                    .checked(auto)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        match crate::platform::set_autostart(*v) {
                            Ok(()) => {
                                this.s.autostart = *v;
                                this.autostart_state = Some(*v);
                                this.commit();
                            }
                            Err(e) => this.toast(format!("Couldn't change login item: {e}")),
                        }
                        cx.notify();
                    })),
            ))
            .child(row(
                "Start hidden",
                Some("Only the tray / menu bar icon"),
                self.switch(
                    "start_min",
                    self.s.start_minimized,
                    |s, v| s.start_minimized = v,
                    cx,
                ),
            ))
            .child(row(
                "Closing the window keeps glyphcam running",
                None,
                self.switch(
                    "close_tray",
                    self.s.close_to_tray,
                    |s, v| s.close_to_tray = v,
                    cx,
                ),
            ));

        let about = card("About")
            .child(
                div()
                    .text_size(px(15.))
                    .child(format!("glyphcam {}", config::version_string())),
            )
            .child(note(format!("Channel {CHANNEL} · build {BUILD_SHA}")))
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button("open-cfg", "Settings folder")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .on_click(|_, _, _| crate::platform::open_path(&config::config_dir())),
                    )
                    .child(
                        button("open-logs", "Logs")
                            .outline()
                            .icon(IconName::FileText)
                            .on_click(|_, _, _| crate::platform::open_path(&config::log_dir())),
                    )
                    .child(
                        button("gh", "GitHub")
                            .outline()
                            .icon(IconName::Github)
                            .on_click(|_, _, cx| {
                                cx.open_url("https://github.com/bentsignal/glyphcam")
                            }),
                    ),
            )
            .child(
                div().child(
                    Button::new("quit")
                        .danger()
                        .label("Quit glyphcam")
                        .on_click(cx.listener(|_, _, _, cx| {
                            let ui = cx.entity();
                            cx.defer(move |cx| GlyphUi::quit(&ui, cx));
                        })),
                ),
            );

        div()
            .v_flex()
            .gap_4()
            .child(updates)
            .child(transfer)
            .child(startup)
            .child(about)
    }

    fn updates_card(&mut self, cx: &mut Context<Self>) -> Div {
        let mut c = card("Updates")
            .child(row(
                format!("Running {}", config::version_string()),
                None,
                button("check", "Check now")
                    .outline()
                    .icon(IconName::RefreshCw)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let w = this.waker.clone();
                        this.updater.refresh(move || w.wake());
                        cx.notify();
                    })),
            ))
            .child(row(
                "Show nightly builds",
                Some("Pre-releases built from every change"),
                self.switch(
                    "prerel",
                    self.s.show_prereleases,
                    |s, v| s.show_prereleases = v,
                    cx,
                ),
            ));
        match self.updater.job() {
            Job::None => {}
            Job::Downloading { tag, got, total } => {
                let f = if total > 0 {
                    got as f32 / total as f32
                } else {
                    0.0
                };
                c = c
                    .child(note(format!("Downloading {tag}…")))
                    .child(Progress::new("dl").value(f * 100.0));
            }
            Job::Installing { tag } => {
                c = c.child(status_line(
                    theme::accent(),
                    format!("Installing {tag}; glyphcam will restart…"),
                ));
            }
            Job::Failed(e) => {
                c = c.child(status_line(theme::danger(), format!("Update failed: {e}")));
            }
        }
        let busy = matches!(
            self.updater.job(),
            Job::Downloading { .. } | Job::Installing { .. }
        );
        match self.updater.list() {
            ListState::Idle => {}
            ListState::Loading => c = c.child(note("Checking GitHub…")),
            ListState::Failed(e) => {
                c = c.child(status_line(
                    theme::danger(),
                    format!("Couldn't reach GitHub: {e}"),
                ))
            }
            ListState::Loaded(list) => {
                let shown: Vec<_> = list
                    .into_iter()
                    .filter(|r| self.s.show_prereleases || !r.prerelease)
                    .take(6)
                    .collect();
                if shown.is_empty() {
                    c = c.child(note("No builds published yet."));
                }
                for (i, rel) in shown.into_iter().enumerate() {
                    let tag = if rel.is_current() {
                        Some(("Running", theme::accent()))
                    } else if rel.is_nightly() {
                        Some(("Nightly", theme::hex(0xb48cff)))
                    } else if rel.prerelease {
                        Some(("Pre-release", theme::warn()))
                    } else {
                        None
                    };
                    let asset = rel.platform_asset().cloned();
                    let html = rel.html_url.clone();
                    let mut action = div().h_flex().gap_2();
                    if let Some(a) = asset {
                        let r = rel.clone();
                        action = action.child(
                            Button::new(("install", i))
                                .small()
                                .label(if rel.is_current() {
                                    "Reinstall"
                                } else {
                                    "Install"
                                })
                                .when(!rel.is_current(), |b| b.primary())
                                .when(rel.is_current(), |b| b.outline())
                                .disabled(busy)
                                .tooltip(format!("{} ({:.1} MB)", a.name, a.size as f64 / 1e6))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let w = this.waker.clone();
                                    let sh = this.sh.clone();
                                    let s = this.s.clone();
                                    let w2 = this.waker.clone();
                                    this.updater.install(
                                        r.clone(),
                                        move || {
                                            config::save(&s);
                                            sh.quit.store(true, Ordering::SeqCst);
                                            std::thread::sleep(Duration::from_millis(400));
                                            w2.wake();
                                            std::process::exit(0);
                                        },
                                        move || w.wake(),
                                    );
                                    cx.notify();
                                })),
                        );
                    } else {
                        action = action.child(note("No build for this platform"));
                    }
                    action = action.child(
                        Button::new(("rel-page", i))
                            .ghost()
                            .small()
                            .icon(IconName::ExternalLink)
                            .tooltip("Release page")
                            .on_click(move |_, _, cx| cx.open_url(&html)),
                    );
                    c = c.child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_3()
                            .p_3()
                            .rounded(px(10.))
                            .bg(theme::bg())
                            .child(
                                div()
                                    .v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(14.5))
                                                    .font_semibold()
                                                    .child(rel.title()),
                                            )
                                            .when_some(tag, |d, (t, col)| {
                                                d.child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .text_color(col)
                                                        .child(t),
                                                )
                                            }),
                                    )
                                    .child(note(format!("{} · {}", rel.tag_name, rel.date()))),
                            )
                            .child(action),
                    );
                }
            }
        }
        c
    }
}

/// Reset everything on the Look tab to defaults.
fn reset_look(s: &mut Settings) {
    let d = Settings::default();
    s.color_mode = d.color_mode;
    s.custom_fg = d.custom_fg;
    s.custom_bg = d.custom_bg;
    s.scripts = d.scripts;
    s.mirror_katakana = d.mirror_katakana;
    s.columns = d.columns;
    s.auto_exposure = d.auto_exposure;
    s.contrast = d.contrast;
    s.brightness = d.brightness;
    s.gamma = d.gamma;
    s.edge = d.edge;
    s.depth = d.depth;
    s.glow = d.glow;
    s.churn = d.churn;
    s.saturation = d.saturation;
    s.background = d.background;
    s.bg_dim = d.bg_dim;
    s.rain_speed = d.rain_speed;
    s.depth_mode = d.depth_mode;
    s.mesh_detail = d.mesh_detail;
    s.depth_fill = d.depth_fill;
    s.holo = d.holo;
    s.glitch = d.glitch;
    s.wrap_calm = d.wrap_calm;
}
