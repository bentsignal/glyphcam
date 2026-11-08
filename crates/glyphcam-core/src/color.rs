//! Color presets. Each mode maps a normalized intensity (plus the camera's
//! own color, for true-color mode) to a glyph color, and defines a background.

use crate::settings::ColorMode;

pub type Rgb = [f32; 3];

fn lerp3(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Piecewise-linear ramp through `(position, color)` stops.
fn ramp(stops: &[(f32, Rgb)], t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let (p0, c0) = w[0];
        let (p1, c1) = w[1];
        if t <= p1 {
            return lerp3(c0, c1, ((t - p0) / (p1 - p0).max(1e-6)).clamp(0.0, 1.0));
        }
    }
    stops.last().unwrap().1
}

fn hex(r: u8, g: u8, b: u8) -> Rgb {
    [r as f32, g as f32, b as f32]
}

pub struct Palette {
    pub mode: ColorMode,
    pub bg: Rgb,
    custom_fg: Rgb,
    saturation: f32,
}

impl Palette {
    pub fn new(mode: ColorMode, custom_fg: [u8; 3], custom_bg: [u8; 3], saturation: f32) -> Self {
        let bg = match mode {
            ColorMode::TrueColor | ColorMode::Mono | ColorMode::Thermal => hex(0, 0, 0),
            ColorMode::Matrix => hex(0, 5, 2),
            ColorMode::Amber => hex(8, 4, 0),
            ColorMode::Ice => hex(0, 4, 8),
            ColorMode::Crimson => hex(7, 0, 2),
            ColorMode::Synthwave => hex(9, 0, 18),
            ColorMode::Custom => hex(custom_bg[0], custom_bg[1], custom_bg[2]),
        };
        Palette {
            mode,
            bg,
            custom_fg: hex(custom_fg[0], custom_fg[1], custom_fg[2]),
            saturation,
        }
    }

    /// Glyph color for intensity `i` (0..1). `cam` is the camera color at the
    /// cell (0..255), `y` the vertical position (0 top .. 1 bottom).
    #[inline]
    pub fn color(&self, i: f32, cam: Rgb, y: f32) -> Rgb {
        let i = i.clamp(0.0, 1.0);
        match self.mode {
            ColorMode::TrueColor => {
                let l = (0.299 * cam[0] + 0.587 * cam[1] + 0.114 * cam[2]).max(1.0);
                // Saturation around luma, then rescale so brightness tracks `i`.
                let s = self.saturation;
                let c = [
                    l + (cam[0] - l) * s,
                    l + (cam[1] - l) * s,
                    l + (cam[2] - l) * s,
                ];
                let k = (0.2 + 0.95 * i) * 255.0 / l;
                [
                    (c[0] * k).clamp(0.0, 255.0),
                    (c[1] * k).clamp(0.0, 255.0),
                    (c[2] * k).clamp(0.0, 255.0),
                ]
            }
            ColorMode::Matrix => ramp(
                &[
                    (0.0, hex(0, 40, 10)),
                    (0.35, hex(0, 125, 40)),
                    (0.75, hex(20, 255, 80)),
                    (1.0, hex(205, 255, 215)),
                ],
                i,
            ),
            ColorMode::Mono => ramp(
                &[
                    (0.0, hex(40, 40, 40)),
                    (0.8, hex(220, 220, 220)),
                    (1.0, hex(255, 255, 255)),
                ],
                i,
            ),
            ColorMode::Amber => ramp(
                &[
                    (0.0, hex(50, 22, 0)),
                    (0.7, hex(255, 170, 0)),
                    (1.0, hex(255, 235, 180)),
                ],
                i,
            ),
            ColorMode::Ice => ramp(
                &[
                    (0.0, hex(0, 30, 50)),
                    (0.65, hex(0, 190, 255)),
                    (1.0, hex(225, 250, 255)),
                ],
                i,
            ),
            ColorMode::Crimson => ramp(
                &[
                    (0.0, hex(45, 0, 6)),
                    (0.7, hex(255, 30, 60)),
                    (1.0, hex(255, 205, 210)),
                ],
                i,
            ),
            ColorMode::Synthwave => {
                let hue = lerp3(hex(255, 60, 200), hex(20, 220, 255), y.clamp(0.0, 1.0));
                let base = lerp3(hex(30, 0, 40), hue, (i / 0.8).min(1.0));
                lerp3(base, hex(255, 245, 255), ((i - 0.8) / 0.2).clamp(0.0, 1.0))
            }
            ColorMode::Thermal => ramp(
                &[
                    (0.0, hex(10, 0, 40)),
                    (0.2, hex(40, 0, 140)),
                    (0.4, hex(170, 0, 140)),
                    (0.6, hex(240, 40, 30)),
                    (0.8, hex(255, 170, 0)),
                    (1.0, hex(255, 255, 210)),
                ],
                i,
            ),
            ColorMode::Custom => {
                let dim = lerp3(self.bg, self.custom_fg, 0.18);
                let base = lerp3(dim, self.custom_fg, (i / 0.85).min(1.0));
                lerp3(
                    base,
                    hex(255, 255, 255),
                    ((i - 0.85) / 0.15).clamp(0.0, 1.0) * 0.6,
                )
            }
        }
    }
}
