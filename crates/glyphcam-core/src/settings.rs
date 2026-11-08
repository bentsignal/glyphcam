//! User-tweakable settings. Serialized to JSON by the app; every field has a
//! default so older/newer settings files keep loading.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    TrueColor,
    Matrix,
    Mono,
    Amber,
    Ice,
    Crimson,
    Synthwave,
    Thermal,
    Custom,
}

impl ColorMode {
    pub const ALL: [ColorMode; 9] = [
        ColorMode::Matrix,
        ColorMode::TrueColor,
        ColorMode::Mono,
        ColorMode::Amber,
        ColorMode::Ice,
        ColorMode::Crimson,
        ColorMode::Synthwave,
        ColorMode::Thermal,
        ColorMode::Custom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ColorMode::TrueColor => "True color",
            ColorMode::Matrix => "Matrix green",
            ColorMode::Mono => "Monochrome",
            ColorMode::Amber => "Amber terminal",
            ColorMode::Ice => "Ice",
            ColorMode::Crimson => "Crimson",
            ColorMode::Synthwave => "Synthwave",
            ColorMode::Thermal => "Thermal",
            ColorMode::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Background {
    /// Render the whole camera image as glyphs.
    Show,
    /// Dim everything that isn't you.
    Dim,
    /// Only you; the background is empty.
    Hide,
    /// Only you, with falling digital rain behind.
    Rain,
}

impl Background {
    pub const ALL: [Background; 4] = [
        Background::Rain,
        Background::Dim,
        Background::Hide,
        Background::Show,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Background::Show => "Show camera background",
            Background::Dim => "Dim background",
            Background::Hide => "Hide background",
            Background::Rain => "Digital rain",
        }
    }
    pub fn needs_mask(self) -> bool {
        !matches!(self, Background::Show)
    }
}

/// Which character families feed the glyph ramp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scripts {
    pub latin: bool,
    pub digits: bool,
    pub punctuation: bool,
    pub katakana: bool,
    pub greek: bool,
    pub cyrillic: bool,
    pub box_drawing: bool,
    pub blocks: bool,
    pub math: bool,
    pub braille: bool,
}

impl Default for Scripts {
    fn default() -> Self {
        GlyphPreset::Matrix.scripts()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlyphPreset {
    Matrix,
    Polyglot,
    Ascii,
    Blocks,
    GreekMath,
    Cyrillic,
}

impl GlyphPreset {
    pub const ALL: [GlyphPreset; 6] = [
        GlyphPreset::Matrix,
        GlyphPreset::Polyglot,
        GlyphPreset::Ascii,
        GlyphPreset::Blocks,
        GlyphPreset::GreekMath,
        GlyphPreset::Cyrillic,
    ];
    pub fn label(self) -> &'static str {
        match self {
            GlyphPreset::Matrix => "Matrix (katakana + digits)",
            GlyphPreset::Polyglot => "Polyglot (everything)",
            GlyphPreset::Ascii => "Classic ASCII",
            GlyphPreset::Blocks => "Blocks & braille",
            GlyphPreset::GreekMath => "Greek & math",
            GlyphPreset::Cyrillic => "Cyrillic",
        }
    }
    pub fn scripts(self) -> Scripts {
        let none = Scripts {
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
        };
        match self {
            GlyphPreset::Matrix => Scripts {
                katakana: true,
                digits: true,
                math: true,
                ..none
            },
            GlyphPreset::Polyglot => Scripts {
                latin: true,
                digits: true,
                punctuation: true,
                katakana: true,
                greek: true,
                cyrillic: true,
                box_drawing: true,
                blocks: false,
                math: true,
                braille: false,
            },
            GlyphPreset::Ascii => Scripts {
                latin: true,
                digits: true,
                punctuation: true,
                ..none
            },
            GlyphPreset::Blocks => Scripts {
                blocks: true,
                braille: true,
                box_drawing: true,
                ..none
            },
            GlyphPreset::GreekMath => Scripts {
                greek: true,
                math: true,
                digits: true,
                ..none
            },
            GlyphPreset::Cyrillic => Scripts {
                cyrillic: true,
                digits: true,
                punctuation: true,
                ..none
            },
        }
    }
    pub fn matching(scripts: &Scripts) -> Option<GlyphPreset> {
        GlyphPreset::ALL
            .into_iter()
            .find(|p| p.scripts() == *scripts)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SourceKind {
    /// A physical camera. `None` selects the first available one.
    Camera(Option<String>),
    /// Procedural animated test pattern (no camera needed).
    Demo,
    /// A still image with simulated head motion (for testing tracking).
    Image(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // --- input ---
    pub source: SourceKind,
    /// Mirror horizontally (selfie view).
    pub mirror: bool,
    /// Preferred capture height (480 = light, 720 = sharper).
    pub camera_quality: u32,

    // --- output ---
    pub out_width: u32,
    pub out_height: u32,
    pub fps: u32,

    // --- look ---
    pub color_mode: ColorMode,
    pub custom_fg: [u8; 3],
    pub custom_bg: [u8; 3],
    pub scripts: Scripts,
    pub mirror_katakana: bool,
    /// Glyph columns across the output (density).
    pub columns: u32,
    pub auto_exposure: bool,
    pub contrast: f32,
    pub brightness: f32,
    pub gamma: f32,
    /// Local-contrast boost that makes eyes and mouth pop.
    pub edge: f32,
    /// How much the 3D head shape modulates shading (0..1).
    pub depth: f32,
    /// Soft halo behind bright glyphs (0..1).
    pub glow: f32,
    /// How often glyphs re-roll, in changes per second per cell.
    pub churn: f32,
    /// Saturation multiplier for true-color mode.
    pub saturation: f32,
    pub background: Background,
    pub bg_dim: f32,
    pub rain_speed: f32,

    // --- framing ---
    pub stabilize: bool,
    /// Face height as a fraction of output height.
    pub face_size: f32,
    /// 0 = locked solid, 1 = loose and bouncy.
    pub sway: f32,
    /// View-model style tilt when you move sideways.
    pub tilt: f32,
    /// Gentle idle drift.
    pub idle_bob: f32,
    /// 0 = keep your head tilt, 1 = always upright.
    pub roll_lock: f32,
    /// Vertical position of the face (0 top .. 1 bottom).
    pub vertical: f32,

    // --- app ---
    pub vcam_enabled: bool,
    pub preview: bool,
    pub stream_server: bool,
    pub stream_port: u16,
    pub keep_camera_warm: bool,
    pub start_minimized: bool,
    pub autostart: bool,
    pub close_to_tray: bool,
    pub show_prereleases: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: SourceKind::Camera(None),
            mirror: true,
            camera_quality: 480,
            out_width: 1280,
            out_height: 720,
            fps: 30,
            color_mode: ColorMode::Matrix,
            custom_fg: [255, 64, 200],
            custom_bg: [6, 0, 12],
            scripts: Scripts::default(),
            mirror_katakana: true,
            columns: 128,
            auto_exposure: true,
            contrast: 1.15,
            brightness: 0.0,
            gamma: 0.9,
            edge: 0.6,
            depth: 0.55,
            glow: 0.45,
            churn: 0.6,
            saturation: 1.3,
            background: Background::Rain,
            bg_dim: 0.25,
            rain_speed: 1.0,
            stabilize: true,
            face_size: 0.5,
            sway: 0.45,
            tilt: 0.35,
            idle_bob: 0.25,
            roll_lock: 0.0,
            vertical: 0.46,
            vcam_enabled: true,
            preview: true,
            stream_server: true,
            stream_port: 8737,
            keep_camera_warm: false,
            start_minimized: false,
            autostart: false,
            close_to_tray: true,
            show_prereleases: true,
        }
    }
}

impl Settings {
    /// Clamp everything into sane ranges (settings files are user-editable).
    pub fn sanitize(&mut self) {
        let (w, h) = OUTPUT_SIZES
            .iter()
            .copied()
            .find(|&(w, h)| w == self.out_width && h == self.out_height)
            .unwrap_or((1280, 720));
        self.out_width = w;
        self.out_height = h;
        self.fps = self.fps.clamp(10, 60);
        self.camera_quality = if self.camera_quality >= 720 { 720 } else { 480 };
        self.columns = self.columns.clamp(32, 320);
        self.contrast = self.contrast.clamp(0.2, 3.0);
        self.brightness = self.brightness.clamp(-1.0, 1.0);
        self.gamma = self.gamma.clamp(0.3, 3.0);
        self.edge = self.edge.clamp(0.0, 2.0);
        self.depth = self.depth.clamp(0.0, 1.0);
        self.glow = self.glow.clamp(0.0, 1.0);
        self.churn = self.churn.clamp(0.0, 20.0);
        self.saturation = self.saturation.clamp(0.0, 3.0);
        self.bg_dim = self.bg_dim.clamp(0.0, 1.0);
        self.rain_speed = self.rain_speed.clamp(0.1, 4.0);
        self.face_size = self.face_size.clamp(0.15, 0.9);
        self.sway = self.sway.clamp(0.0, 1.0);
        self.tilt = self.tilt.clamp(0.0, 1.0);
        self.idle_bob = self.idle_bob.clamp(0.0, 1.0);
        self.roll_lock = self.roll_lock.clamp(0.0, 1.0);
        self.vertical = self.vertical.clamp(0.25, 0.75);
        if self.stream_port < 1024 {
            self.stream_port = 8737;
        }
    }
}

/// Output resolutions offered everywhere (the Windows virtual camera
/// advertises the same list).
pub const OUTPUT_SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (960, 540), (640, 360)];
