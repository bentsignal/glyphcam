//! The control panel window (egui). The window is *not* the camera output:
//! it toggles the virtual camera and preview, edits settings and manages
//! builds. Closing it hides to the tray / menu bar; the engine keeps running.

use crate::config::{self, BUILD_SHA, CHANNEL};
use crate::engine::Shared;
use crate::tray::{Tray, TrayCmd};
use crate::updater::{Job, ListState, Updater};
use eframe::egui::{
    self, Color32, ColorImage, ComboBox, CornerRadius, FontData, FontDefinitions, FontFamily,
    RichText, Slider, TextureHandle, TextureOptions, ViewportCommand,
};
use glyphcam_core::glyphs;
use glyphcam_core::settings::{
    Background, ColorMode, GlyphPreset, Scripts, Settings, SourceKind, OUTPUT_SIZES,
};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(40, 230, 110);
const ACCENT_DIM: Color32 = Color32::from_rgb(20, 90, 50);
const BG: Color32 = Color32::from_rgb(10, 14, 12);
const PANEL: Color32 = Color32::from_rgb(16, 22, 19);
const MUTED: Color32 = Color32::from_rgb(130, 150, 140);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Look,
    Framing,
    Camera,
    Output,
    App,
    Updates,
}

pub struct GlyphApp {
    sh: Arc<Shared>,
    s: Settings,
    seen_rev: u64,
    dirty_since: Option<Instant>,
    tex: Option<TextureHandle>,
    tex_seq: u64,
    tab: Tab,
    visible: bool,
    quitting: bool,
    tray: Option<Tray>,
    cmd_rx: Receiver<TrayCmd>,
    updater: Updater,
    update_checked: bool,
    autostart_state: Option<bool>,
    image_path: String,
    toast: Option<(String, Instant)>,
    close_prompt: bool,
    engine: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    vcam_status: crate::vcam::install::Status,
}

pub struct Launch {
    pub sh: Arc<Shared>,
    pub engine: std::thread::JoinHandle<()>,
    pub start_hidden: bool,
    /// Initial settings tab (dev/screenshot helper: --tab updates).
    pub initial_tab: Option<String>,
    pub cmd_tx: Sender<TrayCmd>,
    pub cmd_rx: Receiver<TrayCmd>,
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in glyphs::font_bytes() {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
        for fam in [FontFamily::Monospace, FontFamily::Proportional] {
            if let Some(list) = fonts.families.get_mut(&fam) {
                list.push(name.to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke.color = ACCENT;
    v.hyperlink_color = ACCENT;
    v.widgets.hovered.bg_stroke.color = ACCENT_DIM;
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.slider_width = 170.0;
    });
}

impl GlyphApp {
    pub fn new(cc: &eframe::CreationContext<'_>, l: Launch) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        *l.sh.repaint.lock() = Some(Box::new(move || ctx.request_repaint()));

        let s = l.sh.settings.read().clone();
        let wake_ctx = cc.egui_ctx.clone();
        let tray = match crate::tray::create(s.vcam_enabled, l.cmd_tx.clone(), move || {
            wake_ctx.request_repaint()
        }) {
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
        let mut app = GlyphApp {
            seen_rev: l.sh.settings_rev.load(Ordering::SeqCst),
            sh: l.sh,
            s,
            dirty_since: None,
            tex: None,
            tex_seq: 0,
            tab: match l.initial_tab.as_deref() {
                Some("framing") => Tab::Framing,
                Some("camera") => Tab::Camera,
                Some("output") => Tab::Output,
                Some("app") => Tab::App,
                Some("updates") => Tab::Updates,
                _ => Tab::Look,
            },
            visible: true,
            quitting: false,
            tray,
            cmd_rx: l.cmd_rx,
            updater: Updater::default(),
            update_checked: false,
            autostart_state: None,
            image_path,
            toast: None,
            close_prompt: false,
            engine: Some(l.engine),
            #[cfg(windows)]
            vcam_status: crate::vcam::install::status(),
        };
        if l.start_hidden && app.tray.is_some() {
            app.hide(&cc.egui_ctx);
        }
        app
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        self.sh.preview_wanted.store(false, Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        crate::platform::macos_hide();
        #[cfg(not(target_os = "macos"))]
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        let _ = ctx;
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.visible = true;
        #[cfg(target_os = "macos")]
        crate::platform::macos_show();
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.commit_settings();
        config::save(&self.s);
        self.sh.quit.store(true, Ordering::SeqCst);
        if let Some(e) = self.engine.take() {
            let t = Instant::now();
            while !e.is_finished() && t.elapsed() < Duration::from_millis(1500) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        ctx.send_viewport_cmd(ViewportCommand::Close);
        // Make sure we really exit even if the window is hidden.
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(800));
            std::process::exit(0);
        });
    }

    fn commit_settings(&mut self) {
        let cur = self.sh.settings.read().clone();
        if cur != self.s {
            self.sh.update_settings(|x| *x = self.s.clone());
            self.s = self.sh.settings.read().clone();
            self.seen_rev = self.sh.settings_rev.load(Ordering::SeqCst);
            self.dirty_since = Some(Instant::now());
        }
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn handle_cmds(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                TrayCmd::Show => self.show(ctx),
                TrayCmd::ToggleVcam => {
                    self.s.vcam_enabled = !self.s.vcam_enabled;
                    self.commit_settings();
                }
                TrayCmd::TogglePreview => {
                    self.s.preview = !self.s.preview;
                    self.commit_settings();
                    if self.s.preview && !self.visible {
                        self.show(ctx);
                    }
                }
                TrayCmd::Quit => self.quit(ctx),
            }
        }
    }

    // ------------------------------------------------------------------
    // Widgets
    // ------------------------------------------------------------------

    fn big_toggle(ui: &mut egui::Ui, on: bool, title: &str, sub: &str) -> bool {
        let (fill, stroke, text) = if on {
            (ACCENT_DIM, ACCENT, Color32::WHITE)
        } else {
            (
                Color32::from_rgb(28, 34, 31),
                Color32::from_rgb(60, 70, 65),
                MUTED,
            )
        };
        let label = format!("{title}: {}", if on { "ON" } else { "OFF" });
        let btn = egui::Button::new(
            RichText::new(format!("{label}\n{sub}"))
                .color(text)
                .size(14.0),
        )
        .fill(fill)
        .stroke(egui::Stroke::new(1.5, stroke))
        .corner_radius(CornerRadius::same(10))
        .min_size(egui::vec2(200.0, 52.0));
        ui.add(btn).clicked()
    }

    fn section(ui: &mut egui::Ui, title: &str) {
        ui.add_space(6.0);
        ui.label(RichText::new(title).color(ACCENT).strong().size(13.5));
    }

    fn slider(
        ui: &mut egui::Ui,
        v: &mut f32,
        r: std::ops::RangeInclusive<f32>,
        label: &str,
        hint: &str,
    ) {
        ui.horizontal(|ui| {
            ui.add(Slider::new(v, r).text(label)).on_hover_text(hint);
        });
    }

    // ------------------------------------------------------------------
    // Tabs
    // ------------------------------------------------------------------

    fn tab_look(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.s;
        Self::section(ui, "Color");
        ComboBox::from_id_salt("color")
            .selected_text(s.color_mode.label())
            .width(220.0)
            .show_ui(ui, |ui| {
                for m in ColorMode::ALL {
                    ui.selectable_value(&mut s.color_mode, m, m.label());
                }
            });
        if s.color_mode == ColorMode::Custom {
            ui.horizontal(|ui| {
                ui.label("Glyphs");
                ui.color_edit_button_srgb(&mut s.custom_fg);
                ui.label("Background");
                ui.color_edit_button_srgb(&mut s.custom_bg);
            });
        }
        if s.color_mode == ColorMode::TrueColor {
            Self::slider(
                ui,
                &mut s.saturation,
                0.0..=3.0,
                "Saturation",
                "Color intensity in true-color mode",
            );
        }

        Self::section(ui, "Glyphs");
        let preset = GlyphPreset::matching(&s.scripts);
        ComboBox::from_id_salt("glyphs")
            .selected_text(preset.map(|p| p.label()).unwrap_or("Custom mix"))
            .width(220.0)
            .show_ui(ui, |ui| {
                for p in GlyphPreset::ALL {
                    if ui.selectable_label(preset == Some(p), p.label()).clicked() {
                        s.scripts = p.scripts();
                    }
                }
            });
        let sc = &mut s.scripts;
        egui::Grid::new("scripts")
            .num_columns(2)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                let items: [(&mut bool, &str, Scripts); 10] = [
                    (
                        &mut sc.katakana,
                        "Katakana",
                        Scripts {
                            katakana: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.latin,
                        "Latin",
                        Scripts {
                            latin: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.digits,
                        "Digits",
                        Scripts {
                            digits: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.punctuation,
                        "Punctuation",
                        Scripts {
                            punctuation: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.greek,
                        "Greek",
                        Scripts {
                            greek: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.cyrillic,
                        "Cyrillic",
                        Scripts {
                            cyrillic: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.math,
                        "Math",
                        Scripts {
                            math: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.box_drawing,
                        "Box drawing",
                        Scripts {
                            box_drawing: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.blocks,
                        "Blocks",
                        Scripts {
                            blocks: true,
                            ..none()
                        },
                    ),
                    (
                        &mut sc.braille,
                        "Braille",
                        Scripts {
                            braille: true,
                            ..none()
                        },
                    ),
                ];
                for (i, (flag, name, only)) in items.into_iter().enumerate() {
                    ui.checkbox(flag, name)
                        .on_hover_text(glyphs::sample_text(&only, 12));
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        ui.label(
            RichText::new(glyphs::sample_text(&s.scripts, 34))
                .monospace()
                .color(ACCENT)
                .size(15.0),
        );
        ui.checkbox(&mut s.mirror_katakana, "Mirror katakana (film style)");
        ui.horizontal(|ui| {
            let mut cols = s.columns as f32;
            ui.add(
                Slider::new(&mut cols, 48.0..=240.0)
                    .text("Density")
                    .step_by(4.0),
            )
            .on_hover_text("Glyph columns across the frame. Higher = finer detail.");
            s.columns = cols as u32;
        });

        Self::section(ui, "Tone");
        ui.checkbox(
            &mut s.auto_exposure,
            "Auto exposure (normalize to your face)",
        );
        Self::slider(ui, &mut s.contrast, 0.3..=2.5, "Contrast", "");
        Self::slider(ui, &mut s.brightness, -0.6..=0.6, "Brightness", "");
        Self::slider(
            ui,
            &mut s.gamma,
            0.4..=2.0,
            "Gamma",
            "Lower = brighter midtones",
        );
        Self::slider(
            ui,
            &mut s.edge,
            0.0..=2.0,
            "Detail",
            "Local contrast: makes eyes and mouth pop",
        );
        Self::slider(
            ui,
            &mut s.depth,
            0.0..=1.0,
            "Depth shading",
            "How much the 3D shape of your head shades the glyphs",
        );
        Self::slider(
            ui,
            &mut s.glow,
            0.0..=1.0,
            "Glow",
            "Soft halo behind bright glyphs",
        );
        Self::slider(
            ui,
            &mut s.churn,
            0.0..=6.0,
            "Glyph shimmer",
            "How often characters re-roll",
        );

        Self::section(ui, "Background");
        ComboBox::from_id_salt("bg")
            .selected_text(s.background.label())
            .width(220.0)
            .show_ui(ui, |ui| {
                for b in Background::ALL {
                    ui.selectable_value(&mut s.background, b, b.label());
                }
            });
        match s.background {
            Background::Dim => Self::slider(ui, &mut s.bg_dim, 0.0..=1.0, "Background level", ""),
            Background::Rain => Self::slider(ui, &mut s.rain_speed, 0.2..=3.0, "Rain speed", ""),
            _ => {}
        }
        ui.add_space(8.0);
        Self::section(ui, "Share");
        self.transfer_ui(ui);
        let s = &mut self.s;
        ui.add_space(4.0);
        if ui.button("Reset look to defaults").clicked() {
            let d = Settings::default();
            s.color_mode = d.color_mode;
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
        }
    }

    fn tab_framing(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.s;
        Self::section(ui, "Face stabilization");
        ui.checkbox(&mut s.stabilize, "Keep my face centered");
        ui.label(
            RichText::new("Your face stays put while you move, with a little game-style sway. It never leaves the frame.")
                .color(MUTED)
                .size(12.0),
        );
        ui.add_enabled_ui(s.stabilize, |ui| {
            Self::slider(
                ui,
                &mut s.face_size,
                0.2..=0.85,
                "Face size",
                "How much of the frame height your face fills",
            );
            Self::slider(ui, &mut s.vertical, 0.3..=0.65, "Vertical position", "");
            Self::slider(
                ui,
                &mut s.sway,
                0.0..=1.0,
                "Sway",
                "0 = locked solid, 1 = loose and bouncy",
            );
            Self::slider(
                ui,
                &mut s.tilt,
                0.0..=1.0,
                "Tilt into motion",
                "Lean like a view model when you move sideways",
            );
            Self::slider(
                ui,
                &mut s.idle_bob,
                0.0..=1.0,
                "Idle drift",
                "Gentle breathing motion",
            );
            Self::slider(
                ui,
                &mut s.roll_lock,
                0.0..=1.0,
                "Keep upright",
                "Cancel head tilt (0 = follow your head)",
            );
        });
        Self::section(ui, "View");
        ui.checkbox(&mut s.mirror, "Mirror (selfie view)");
    }

    fn tab_camera(&mut self, ui: &mut egui::Ui) {
        let cams = self.sh.cameras.lock().clone();
        let st = self.sh.status.lock().clone();
        let s = &mut self.s;
        Self::section(ui, "Input");
        let kind = match &s.source {
            SourceKind::Camera(_) => 0,
            SourceKind::Demo => 1,
            SourceKind::Image(_) => 2,
        };
        ui.horizontal(|ui| {
            if ui.selectable_label(kind == 0, "Camera").clicked() && kind != 0 {
                s.source = SourceKind::Camera(None);
            }
            if ui.selectable_label(kind == 1, "Demo").clicked() {
                s.source = SourceKind::Demo;
            }
            if ui.selectable_label(kind == 2, "Image file").clicked() && kind != 2 {
                s.source = SourceKind::Image(self.image_path.clone());
            }
        });
        match &mut s.source {
            SourceKind::Camera(sel) => {
                let label = sel
                    .clone()
                    .unwrap_or_else(|| "Automatic (prefer real webcam)".into());
                ui.horizontal(|ui| {
                    ComboBox::from_id_salt("cam")
                        .selected_text(label)
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(sel.is_none(), "Automatic (prefer real webcam)")
                                .clicked()
                            {
                                *sel = None;
                            }
                            for c in &cams {
                                if ui
                                    .selectable_label(sel.as_deref() == Some(&c.name), &c.name)
                                    .clicked()
                                {
                                    *sel = Some(c.name.clone());
                                }
                            }
                        });
                    if ui.button("⟳").on_hover_text("Rescan cameras").clicked() {
                        self.sh.refresh_cameras.store(true, Ordering::SeqCst);
                    }
                });
                if cams.is_empty() {
                    ui.label(
                        RichText::new("No cameras detected.")
                            .color(Color32::from_rgb(255, 170, 80)),
                    );
                }
                ui.horizontal(|ui| {
                    ui.label("Quality");
                    ui.selectable_value(&mut s.camera_quality, 480, "Light (480p)");
                    ui.selectable_value(&mut s.camera_quality, 720, "Sharp (720p)");
                });
            }
            SourceKind::Demo => {
                ui.label(RichText::new("Animated test head; no camera needed.").color(MUTED));
            }
            SourceKind::Image(p) => {
                ui.horizontal(|ui| {
                    ui.label("Path");
                    if ui.text_edit_singleline(&mut self.image_path).lost_focus() {
                        *p = self.image_path.trim().trim_matches('"').to_string();
                    }
                });
                ui.label(
                    RichText::new(
                        "A photo viewed through a simulated wobbling camera (tests tracking).",
                    )
                    .color(MUTED)
                    .size(12.0),
                );
            }
        }

        Self::section(ui, "Status");
        egui::Grid::new("camstatus").num_columns(2).show(ui, |ui| {
            ui.label("Source");
            ui.label(if st.active {
                format!("{} — {}", st.source, st.source_detail)
            } else {
                "idle (camera off)".into()
            });
            ui.end_row();
            ui.label("Output fps");
            ui.label(format!("{:.1}", st.fps));
            ui.end_row();
            ui.label("Face");
            ui.label(if st.face {
                RichText::new("tracked").color(ACCENT)
            } else {
                RichText::new("not found").color(MUTED)
            });
            ui.end_row();
            ui.label("Timings");
            ui.label(format!(
                "detect {:.1} ms · mask {:.1} ms · frame {:.1} ms",
                st.detect_ms, st.segment_ms, st.render_ms
            ));
            ui.end_row();
        });
        if let Some(e) = &st.error {
            ui.label(RichText::new(e).color(Color32::from_rgb(255, 120, 90)));
        }
    }

    fn tab_output(&mut self, ui: &mut egui::Ui) {
        let st = self.sh.status.lock().clone();
        let s = &mut self.s;
        Self::section(ui, "Format");
        ComboBox::from_id_salt("res")
            .selected_text(format!("{} × {}", s.out_width, s.out_height))
            .show_ui(ui, |ui| {
                for (w, h) in OUTPUT_SIZES {
                    if ui
                        .selectable_label(
                            s.out_width == w && s.out_height == h,
                            format!("{w} × {h}"),
                        )
                        .clicked()
                    {
                        s.out_width = w;
                        s.out_height = h;
                    }
                }
            });
        ui.horizontal(|ui| {
            let mut fps = s.fps as f32;
            ui.add(Slider::new(&mut fps, 15.0..=60.0).step_by(5.0).text("FPS"));
            s.fps = fps as u32;
        });
        ui.checkbox(
            &mut s.keep_camera_warm,
            "Keep the webcam on even when nothing is using glyphcam",
        )
        .on_hover_text("Instant start, but the webcam light stays on and it uses more power.");

        Self::section(ui, "Virtual camera");
        #[cfg(windows)]
        {
            ui.label("Pick \"glyphcam\" as the camera in Discord, Zoom, OBS (Video Capture Device), Teams, Chrome, Edge, Firefox…");
            let vs = &self.vcam_status;
            if vs.ok() {
                ui.label(RichText::new("✔ Installed for this user").color(ACCENT));
            } else {
                ui.label(RichText::new("✖ Not registered").color(Color32::from_rgb(255, 150, 80)));
            }
            ui.horizontal(|ui| {
                if ui.button("Repair registration").clicked() {
                    match crate::vcam::install::ensure_registered() {
                        Ok(()) => {
                            self.toast = Some(("Virtual camera registered".into(), Instant::now()))
                        }
                        Err(e) => {
                            self.toast = Some((format!("Registration failed: {e}"), Instant::now()))
                        }
                    }
                    self.vcam_status = crate::vcam::install::status();
                }
                if ui.button("Remove from system").clicked() {
                    crate::vcam::install::unregister();
                    self.vcam_status = crate::vcam::install::status();
                }
            });
            ui.label(RichText::new("Apps that were already open may need a restart to see a newly installed camera.").color(MUTED).size(12.0));
        }
        #[cfg(not(windows))]
        {
            ui.label(RichText::new("macOS: a system camera needs a signed Camera Extension (coming once the app is signed).").color(Color32::from_rgb(255, 190, 90)));
            ui.label("For now, use OBS's built-in virtual camera:");
            ui.label("1. In OBS add a Browser source with the URL below (1280×720).");
            ui.label("2. Click Start Virtual Camera in OBS.");
            ui.label("3. Choose \"OBS Virtual Camera\" in Discord, Zoom, FaceTime, Chrome…");
        }

        Self::section(ui, "Web stream (OBS · browser · other devices)");
        ui.checkbox(&mut s.stream_server, "Serve the output as a web stream");
        ui.add_enabled_ui(s.stream_server, |ui| {
            ui.checkbox(&mut s.stream_lan, "Allow other devices on my network")
                .on_hover_text(
                    "Lets another computer on your Wi-Fi/LAN (or Tailscale) watch the stream.",
                );
            if s.stream_lan {
                ui.label(
                    RichText::new("Anyone on your network can watch while this is on.")
                        .color(Color32::from_rgb(255, 190, 90))
                        .size(12.0),
                );
            }
            ui.horizontal(|ui| {
                let mut port = s.stream_port.to_string();
                ui.label("Port");
                if ui
                    .add(egui::TextEdit::singleline(&mut port).desired_width(60.0))
                    .changed()
                {
                    if let Ok(p) = port.parse::<u16>() {
                        s.stream_port = p;
                    }
                }
            });
        });
        let mut copied: Option<String> = None;
        if let Some(url) = &st.stream_url {
            ui.label(RichText::new("This computer").color(MUTED).size(12.0));
            ui.horizontal(|ui| {
                ui.monospace(url);
                if ui.button("Copy").clicked() {
                    copied = Some(url.clone());
                }
                if ui.button("Open").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            });
            if s.stream_lan {
                ui.label(
                    RichText::new("Other devices on your network")
                        .color(MUTED)
                        .size(12.0),
                );
                if st.stream_lan_urls.is_empty() {
                    ui.label(
                        RichText::new("Looking for this computer's network address…").color(MUTED),
                    );
                }
                for entry in &st.stream_lan_urls {
                    let (u, label) = entry.split_once("  ").unwrap_or((entry.as_str(), ""));
                    ui.horizontal(|ui| {
                        ui.monospace(RichText::new(u).color(ACCENT));
                        if ui.button("Copy").clicked() {
                            copied = Some(u.to_string());
                        }
                        ui.label(RichText::new(label).color(MUTED).size(12.0));
                    });
                }
                ui.label(
                    RichText::new(
                        "On the other computer: OBS → + → Browser → paste the address (1280×720), then Start Virtual Camera to use it in Discord/Zoom. Or just open it in a browser.",
                    )
                    .color(MUTED)
                    .size(12.0),
                );
                #[cfg(windows)]
                if ui
                    .button("Allow through Windows Firewall")
                    .on_hover_text("Use this if other devices can't connect. Asks for administrator permission.")
                    .clicked()
                {
                    let msg = match crate::platform::allow_firewall(s.stream_port) {
                        Ok(()) => "Firewall rule requested".to_string(),
                        Err(e) => format!("Firewall: {e}"),
                    };
                    self.toast = Some((msg, Instant::now()));
                }
            }
            ui.label(
                RichText::new(format!(
                    "{} viewer(s) connected · raw MJPEG at …/stream.mjpg",
                    st.stream_clients
                ))
                .color(MUTED)
                .size(12.0),
            );
        }
        if let Some(u) = copied {
            ui.ctx().copy_text(u);
            self.toast = Some(("Address copied".into(), Instant::now()));
        }
        if let Some(e) = &st.stream_error {
            ui.label(RichText::new(e).color(Color32::from_rgb(255, 120, 90)));
        }
    }

    /// Copy/paste the whole look between machines.
    fn transfer_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("📋 Copy settings")
                .on_hover_text("Copies your look, framing and output format as a code you can paste on another computer.")
                .clicked()
            {
                let code = crate::share::export(&self.s);
                let msg = match crate::share::copy_to_clipboard(&code) {
                    Ok(()) => "Settings copied. Paste them in glyphcam on your other computer.".to_string(),
                    Err(e) => format!("Couldn't copy: {e}"),
                };
                self.toast = Some((msg, Instant::now()));
            }
            if ui.button("📥 Paste settings").on_hover_text("Applies settings copied from another glyphcam.").clicked() {
                let msg = match crate::share::read_clipboard().and_then(|t| crate::share::import(&t, &self.s)) {
                    Ok((new, n)) => {
                        self.s = new;
                        format!("Applied {n} settings")
                    }
                    Err(e) => e,
                };
                self.toast = Some((msg, Instant::now()));
            }
        });
    }

    fn tab_app(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "Move your setup to another computer");
        ui.label(
            RichText::new("Copy here, then Paste in glyphcam on the other machine (send the code via chat, notes…). Camera choice and app options stay per-computer.")
                .color(MUTED)
                .size(12.0),
        );
        self.transfer_ui(ui);
        Self::section(ui, "Background behavior");
        let auto = *self
            .autostart_state
            .get_or_insert_with(crate::platform::autostart_enabled);
        let mut want = auto;
        if ui
            .checkbox(&mut want, "Start glyphcam when I log in")
            .changed()
        {
            match crate::platform::set_autostart(want) {
                Ok(()) => {
                    self.s.autostart = want;
                    self.autostart_state = Some(want);
                }
                Err(e) => self.toast(format!("Couldn't change login item: {e}")),
            }
        }
        let s = &mut self.s;
        ui.checkbox(
            &mut s.start_minimized,
            "Start hidden in the tray / menu bar",
        );
        ui.checkbox(
            &mut s.close_to_tray,
            "Closing the window keeps glyphcam running",
        );
        Self::section(ui, "Files");
        ui.horizontal(|ui| {
            if ui.button("Open settings folder").clicked() {
                crate::platform::open_path(&config::config_dir());
            }
            if ui.button("Open logs").clicked() {
                crate::platform::open_path(&config::log_dir());
            }
        });
        Self::section(ui, "About");
        ui.label(format!("glyphcam {}", config::version_string()));
        ui.label(RichText::new(format!("channel {CHANNEL} · build {BUILD_SHA}")).color(MUTED));
        ui.hyperlink_to(
            "github.com/bentsignal/glyphcam",
            "https://github.com/bentsignal/glyphcam",
        );
        ui.add_space(10.0);
        if ui
            .button(RichText::new("Quit glyphcam").color(Color32::from_rgb(255, 140, 120)))
            .clicked()
        {
            let ctx = ui.ctx().clone();
            self.quit(&ctx);
        }
    }

    fn tab_updates(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        Self::section(ui, "Builds");
        ui.label(format!("Running {}", config::version_string()));
        ui.horizontal(|ui| {
            if ui.button("Check now").clicked() {
                let c = ctx.clone();
                self.updater.refresh(move || c.request_repaint());
            }
            ui.checkbox(&mut self.s.show_prereleases, "Show pre-releases & nightly");
        });
        match self.updater.job() {
            Job::None => {}
            Job::Downloading { tag, got, total } => {
                let f = if total > 0 {
                    got as f32 / total as f32
                } else {
                    0.0
                };
                ui.label(format!("Downloading {tag}…"));
                ui.add(egui::ProgressBar::new(f).show_percentage());
            }
            Job::Installing { tag } => {
                ui.label(
                    RichText::new(format!("Installing {tag}; glyphcam will restart…"))
                        .color(ACCENT),
                );
            }
            Job::Failed(e) => {
                ui.label(
                    RichText::new(format!("Update failed: {e}"))
                        .color(Color32::from_rgb(255, 120, 90)),
                );
            }
        }
        let busy = matches!(
            self.updater.job(),
            Job::Downloading { .. } | Job::Installing { .. }
        );
        match self.updater.list() {
            ListState::Idle => {
                ui.label("Press \"Check now\" to list builds from GitHub.");
            }
            ListState::Loading => {
                ui.spinner();
            }
            ListState::Failed(e) => {
                ui.label(
                    RichText::new(format!("Couldn't reach GitHub: {e}"))
                        .color(Color32::from_rgb(255, 120, 90)),
                );
            }
            ListState::Loaded(list) => {
                let shown: Vec<_> = list
                    .into_iter()
                    .filter(|r| self.s.show_prereleases || !r.prerelease)
                    .collect();
                if shown.is_empty() {
                    ui.label("No builds published yet.");
                }
                for rel in shown {
                    egui::Frame::new()
                        .fill(BG)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(rel.title()).strong());
                                if rel.is_current() {
                                    ui.label(RichText::new("● running").color(ACCENT));
                                } else if rel.is_nightly() {
                                    ui.label(
                                        RichText::new("nightly")
                                            .color(Color32::from_rgb(180, 140, 255)),
                                    );
                                } else if rel.prerelease {
                                    ui.label(
                                        RichText::new("pre-release")
                                            .color(Color32::from_rgb(255, 190, 90)),
                                    );
                                }
                            });
                            ui.label(
                                RichText::new(format!("{} · {}", rel.tag_name, rel.date()))
                                    .color(MUTED)
                                    .size(12.0),
                            );
                            ui.horizontal(|ui| {
                                match rel.platform_asset() {
                                    Some(a) => {
                                        let label = if rel.is_current() {
                                            "Reinstall"
                                        } else {
                                            "Install"
                                        };
                                        if ui
                                            .add_enabled(!busy, egui::Button::new(label))
                                            .on_hover_text(format!(
                                                "{} ({:.1} MB)",
                                                a.name,
                                                a.size as f64 / 1e6
                                            ))
                                            .clicked()
                                        {
                                            let c = ctx.clone();
                                            let c2 = ctx.clone();
                                            let sh = self.sh.clone();
                                            let s = self.s.clone();
                                            self.updater.install(
                                                rel.clone(),
                                                move || {
                                                    config::save(&s);
                                                    sh.quit.store(true, Ordering::SeqCst);
                                                    std::thread::sleep(Duration::from_millis(400));
                                                    c2.request_repaint();
                                                    std::process::exit(0);
                                                },
                                                move || c.request_repaint(),
                                            );
                                        }
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new("no build for this platform")
                                                .color(MUTED),
                                        );
                                    }
                                }
                                ui.hyperlink_to("release page", &rel.html_url);
                            });
                            if let Some(body) = rel.body.as_deref().filter(|b| !b.trim().is_empty())
                            {
                                egui::CollapsingHeader::new("Notes")
                                    .id_salt(&rel.tag_name)
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(body).size(12.0));
                                    });
                            }
                        });
                    ui.add_space(4.0);
                }
            }
        }
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let st = self.sh.status.lock().clone();
        // Upload the latest preview frame.
        if let Some(p) = self.sh.preview.lock().as_ref() {
            if p.seq != self.tex_seq {
                self.tex_seq = p.seq;
                let img = ColorImage::from_rgba_unmultiplied(
                    [p.width as usize, p.height as usize],
                    &p.rgba,
                );
                match &mut self.tex {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => {
                        self.tex = Some(ui.ctx().load_texture(
                            "preview",
                            img,
                            TextureOptions::LINEAR,
                        ))
                    }
                }
            }
        }
        let avail = ui.available_size();
        let aspect = self.s.out_width as f32 / self.s.out_height as f32;
        let w = avail.x.min((avail.y - 90.0).max(120.0) * aspect);
        let size = egui::vec2(w, w / aspect);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, CornerRadius::same(10), Color32::BLACK);
            let live = self.s.preview && st.active && self.tex.is_some();
            if live {
                if let Some(t) = &self.tex {
                    egui::Image::new((t.id(), size))
                        .corner_radius(CornerRadius::same(10))
                        .paint_at(ui, rect);
                }
            } else {
                let msg = if !self.s.preview {
                    "Preview is off — the virtual camera still works"
                } else if let Some(e) = &st.error {
                    e.as_str()
                } else {
                    "Starting camera…"
                };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    msg,
                    egui::FontId::proportional(15.0),
                    MUTED,
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let total = 2.0 * 200.0 + 12.0;
                ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                let vcam_sub = if !cfg!(windows) {
                    "stream for OBS".to_string()
                } else if !self.s.vcam_enabled {
                    "apps see a paused card".to_string()
                } else if st.vcam_in_use {
                    "● live in an app".to_string()
                } else {
                    "ready: pick \"glyphcam\" in any app".to_string()
                };
                if Self::big_toggle(ui, self.s.vcam_enabled, "Virtual camera", &vcam_sub) {
                    self.s.vcam_enabled = !self.s.vcam_enabled;
                }
                ui.add_space(12.0);
                let prev_sub = if self.s.preview {
                    format!("{:.0} fps", st.fps)
                } else {
                    "saves CPU".into()
                };
                if Self::big_toggle(ui, self.s.preview, "Preview", &prev_sub) {
                    self.s.preview = !self.s.preview;
                }
            });
        });
    }
}

fn none() -> Scripts {
    Scripts {
        latin: false,
        digits: false,
        punctuation: false,
        katakana: false,
        greek: false,
        cyrillic: false,
        box_drawing: false,
        blocks: false,
        math: false,
        braille: false,
    }
}

impl eframe::App for GlyphApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_cmds(ctx);

        // Pick up external settings changes (tray menu).
        let rev = self.sh.settings_rev.load(Ordering::SeqCst);
        if rev != self.seen_rev {
            self.s = self.sh.settings.read().clone();
            self.seen_rev = rev;
        }
        if let Some(t) = &self.tray {
            t.set_vcam_checked(self.s.vcam_enabled);
            let st = self.sh.status.lock();
            let tip = if st.vcam_in_use {
                "glyphcam: live in an app"
            } else if !self.s.vcam_enabled && cfg!(windows) {
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

        // Close button hides to tray.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.s.close_to_tray && self.tray.is_some() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                if self.s.close_hint_shown {
                    self.hide(ctx);
                } else {
                    // First time: explain what "closing" means.
                    self.close_prompt = true;
                    ctx.request_repaint();
                }
            } else {
                self.quit(ctx);
            }
        }

        // On macOS "hidden" is app-level (Cmd-H / Dock click unhides it).
        #[cfg(target_os = "macos")]
        {
            self.visible = !crate::platform::macos_is_hidden();
        }
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        let wanted = self.visible && !minimized && self.s.preview;
        self.sh.preview_wanted.store(wanted, Ordering::Relaxed);

        // Debounced save.
        if let Some(t) = self.dirty_since {
            if t.elapsed() > Duration::from_millis(800) {
                config::save(&self.s);
                self.dirty_since = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(900));
            }
        }

        if !self.update_checked {
            self.update_checked = true;
            let c = ctx.clone();
            self.updater.refresh(move || c.request_repaint());
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let update = self.updater.available_update(self.s.show_prereleases);

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("glyphcam")
                        .monospace()
                        .size(22.0)
                        .color(ACCENT)
                        .strong(),
                );
                ui.label(
                    RichText::new(config::version_string())
                        .color(MUTED)
                        .size(12.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let st = self.sh.status.lock().clone();
                    if let Some(u) = &update {
                        if ui
                            .button(
                                RichText::new(format!("⬆ {} available", u.tag_name)).color(ACCENT),
                            )
                            .clicked()
                        {
                            self.tab = Tab::Updates;
                        }
                    }
                    let cam = if st.active {
                        format!("● {}", st.source)
                    } else {
                        "○ camera idle".into()
                    };
                    ui.label(
                        RichText::new(cam)
                            .color(if st.active { ACCENT } else { MUTED })
                            .size(12.0),
                    );
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::right("settings")
            .resizable(true)
            .default_size(380.0)
            .min_size(320.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    for (t, name) in [
                        (Tab::Look, "Look"),
                        (Tab::Framing, "Framing"),
                        (Tab::Camera, "Camera"),
                        (Tab::Output, "Output"),
                        (Tab::App, "App"),
                        (
                            Tab::Updates,
                            if update.is_some() {
                                "Updates •"
                            } else {
                                "Updates"
                            },
                        ),
                    ] {
                        ui.selectable_value(&mut self.tab, t, name);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.tab {
                            Tab::Look => self.tab_look(ui),
                            Tab::Framing => self.tab_framing(ui),
                            Tab::Camera => self.tab_camera(ui),
                            Tab::Output => self.tab_output(ui),
                            Tab::App => self.tab_app(ui),
                            Tab::Updates => self.tab_updates(ui),
                        }
                        ui.add_space(20.0);
                    });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(8.0);
            self.preview_panel(ui);
            if let Some((msg, t)) = &self.toast {
                if t.elapsed() < Duration::from_secs(4) {
                    ui.add_space(6.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new(msg).color(ACCENT)));
                    ctx.request_repaint_after(Duration::from_millis(500));
                } else {
                    self.toast = None;
                }
            }
        });

        if self.close_prompt {
            let mut choice: Option<bool> = None;
            egui::Modal::new(egui::Id::unique("close-hint")).show(&ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(RichText::new("glyphcam keeps running in the tray").strong().size(16.0).color(ACCENT));
                ui.add_space(6.0);
                ui.label(
                    "The \"glyphcam\" camera stays available in Discord, Zoom, OBS and your browser. To save power and for privacy, your webcam turns off while nothing is using glyphcam, and turns back on by itself the moment an app (or this preview) needs it.",
                );
                ui.add_space(6.0);
                ui.checkbox(&mut self.s.keep_camera_warm, "Keep the webcam on all the time instead (instant, uses more power)");
                ui.label(RichText::new("You can change this later in Output.").color(MUTED).size(12.0));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Hide to tray").strong()).clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Quit glyphcam").clicked() {
                        choice = Some(false);
                    }
                });
            });
            match choice {
                Some(true) => {
                    self.close_prompt = false;
                    self.s.close_hint_shown = true;
                    self.commit_settings();
                    self.hide(&ctx);
                }
                Some(false) => {
                    self.close_prompt = false;
                    self.s.close_hint_shown = true;
                    self.quit(&ctx);
                }
                None => {}
            }
        }

        self.commit_settings();
        // Keep status text fresh without burning CPU.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.commit_settings();
        config::save(&self.s);
        self.sh.quit.store(true, Ordering::SeqCst);
    }
}
