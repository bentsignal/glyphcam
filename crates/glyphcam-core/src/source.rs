//! Synthetic frame sources for running without a camera (demo mode,
//! headless tests and CI).

use crate::image::RgbImage;
use crate::track::{Face, Mask};
use anyhow::{Context, Result};
use std::sync::Arc;

pub struct SourceFrame {
    pub image: Arc<RgbImage>,
    /// Ground-truth face for synthetic sources (skips detection).
    pub face_hint: Option<Face>,
    /// Ground-truth person mask for synthetic sources.
    pub mask_hint: Option<Arc<Mask>>,
}

/// Procedural animated head: moves around, blinks and talks. Provides
/// ground-truth tracking so stabilization can be seen without a camera.
pub struct DemoSource {
    w: u32,
    h: u32,
}

impl DemoSource {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }

    pub fn frame(&self, t: f64) -> SourceFrame {
        let (w, h) = (self.w as f32, self.h as f32);
        let t = t as f32;
        // Head path: slow drift plus occasional quick moves.
        let jerk = ((t * 0.35).floor() as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32;
        let cx = w * (0.5 + 0.22 * (t * 0.7).sin() + 0.08 * (jerk - 0.5));
        let cy = h * (0.48 + 0.1 * (t * 1.1).sin());
        let r = h * (0.2 + 0.03 * (t * 0.5).sin());
        let roll = 0.18 * (t * 0.8).sin();
        let blink = ((t * 0.9).sin() > 0.97) as u8 as f32;
        let mouth_open =
            0.25 + 0.75 * ((t * 7.0).sin() * 0.5 + 0.5) * ((t * 0.6).sin() > -0.2) as u8 as f32;

        let mut img = RgbImage::new(self.w, self.h);
        let mw = 128usize;
        let mh = 72usize;
        let mut mask = vec![0.0f32; mw * mh];
        let (sr, crl) = roll.sin_cos();
        let ry = r * 1.25;
        for y in 0..self.h {
            for x in 0..self.w {
                let fx = x as f32;
                let fy = y as f32;
                // room: vertical gradient + soft lamp blobs
                let mut c = [
                    30.0 + 40.0 * fy / h,
                    34.0 + 30.0 * fy / h,
                    48.0 + 20.0 * fy / h,
                ];
                let lamp = (-(((fx - w * 0.85).powi(2) + (fy - h * 0.2).powi(2))
                    / (2.0 * (w * 0.12).powi(2))))
                .exp();
                c[0] += 150.0 * lamp;
                c[1] += 130.0 * lamp;
                c[2] += 90.0 * lamp;
                // head-local coords
                let dx = fx - cx;
                let dy = fy - cy;
                let lx = dx * crl + dy * sr;
                let ly = -dx * sr + dy * crl;
                let e = (lx / r).powi(2) + (ly / ry).powi(2);
                // shoulders / torso
                let torso = fy > cy + ry * 0.9 && (dx.abs() < r * (1.3 + (fy - cy - ry) / h * 3.0));
                let mut person = torso;
                if torso {
                    c = [60.0, 70.0, 120.0];
                }
                if e < 1.0 {
                    person = true;
                    let z = (1.0 - e).sqrt();
                    let shade =
                        (0.35 + 0.65 * (z * 0.8 - lx / r * 0.25 - ly / ry * 0.3)).clamp(0.1, 1.0);
                    c = [235.0 * shade, 185.0 * shade, 150.0 * shade];
                    // hair
                    if ly < -ry * 0.55 + (lx / r).powi(2) * ry * 0.25 {
                        c = [55.0 * shade, 38.0 * shade, 28.0 * shade];
                    }
                    // eyes
                    for side in [-1.0f32, 1.0] {
                        let ex = lx - side * r * 0.38;
                        let ey = ly + ry * 0.12;
                        let eh = r * 0.11 * (1.0 - blink * 0.9);
                        if (ex / (r * 0.17)).powi(2) + (ey / eh.max(0.5)).powi(2) < 1.0 {
                            c = [240.0, 240.0, 240.0];
                            if ex.abs() < r * 0.07 && ey.abs() < eh * 0.9 {
                                c = [25.0, 20.0, 18.0];
                            }
                        }
                        // brows
                        if (ex / (r * 0.2)).powi(2) + ((ly + ry * 0.33) / (r * 0.04)).powi(2) < 1.0
                        {
                            c = [50.0, 35.0, 25.0];
                        }
                    }
                    // nose
                    if (lx / (r * 0.08)).powi(2) + ((ly - ry * 0.12) / (r * 0.12)).powi(2) < 1.0 {
                        c = [c[0] * 0.8, c[1] * 0.75, c[2] * 0.75];
                    }
                    // mouth
                    let my = ly - ry * 0.45;
                    if (lx / (r * 0.3)).powi(2) + (my / (r * 0.12 * mouth_open + 1.0)).powi(2) < 1.0
                    {
                        c = [90.0, 20.0, 30.0];
                    }
                }
                let i = ((y * self.w + x) * 3) as usize;
                img.data[i] = c[0].clamp(0.0, 255.0) as u8;
                img.data[i + 1] = c[1].clamp(0.0, 255.0) as u8;
                img.data[i + 2] = c[2].clamp(0.0, 255.0) as u8;
                if person {
                    let mi = (y as usize * mh / self.h as usize) * mw
                        + (x as usize * mw / self.w as usize);
                    mask[mi] = 1.0;
                }
            }
        }
        let rot = |lx: f32, ly: f32| [cx + lx * crl - ly * sr, cy + lx * sr + ly * crl];
        let face = Face {
            cx: rot(0.0, ry * 0.15)[0],
            cy: rot(0.0, ry * 0.15)[1],
            w: r * 1.3,
            h: r * 1.3,
            score: 1.0,
            kps: [
                rot(-r * 0.38, -ry * 0.12),
                rot(r * 0.38, -ry * 0.12),
                rot(0.0, ry * 0.12),
                rot(0.0, ry * 0.45),
                rot(-r * 0.95, 0.0),
                rot(r * 0.95, 0.0),
            ],
        };
        SourceFrame {
            image: Arc::new(img),
            face_hint: Some(face),
            mask_hint: Some(Arc::new(Mask {
                w: mw,
                h: mh,
                data: mask,
            })),
        }
    }
}

/// A still photo viewed through a simulated wobbling camera, so the face
/// moves around the frame like someone shifting in their chair. Uses the
/// real tracker (no hints).
pub struct ImageSource {
    img: RgbImage,
    w: u32,
    h: u32,
}

impl ImageSource {
    pub fn open(path: &str, w: u32, h: u32) -> Result<Self> {
        let im = image::open(path)
            .with_context(|| format!("opening {path}"))?
            .to_rgb8();
        let (iw, ih) = im.dimensions();
        Ok(Self {
            img: RgbImage::from_raw(iw, ih, im.into_raw()).unwrap(),
            w,
            h,
        })
    }

    pub fn from_image(img: RgbImage, w: u32, h: u32) -> Self {
        Self { img, w, h }
    }

    /// `motion` scales the simulated movement (0 = static).
    pub fn frame(&self, t: f64, motion: f32) -> SourceFrame {
        let t = t as f32;
        let (iw, ih) = (self.img.width as f32, self.img.height as f32);
        let base = (iw / self.w as f32).max(ih / self.h as f32);
        let zoom = base * (1.0 + 0.12 * motion * (t * 0.43).sin());
        let ox =
            iw * 0.5 + iw * 0.12 * motion * (t * 0.9).sin() + iw * 0.03 * motion * (t * 5.3).sin();
        let oy = ih * 0.5 + ih * 0.08 * motion * (t * 1.3).cos();
        let ang = 0.08 * motion * (t * 0.7).sin();
        let (s, c) = ang.sin_cos();
        let mut out = RgbImage::new(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                let dx = (x as f32 - self.w as f32 * 0.5) * zoom;
                let dy = (y as f32 - self.h as f32 * 0.5) * zoom;
                let sx = (ox + dx * c - dy * s).clamp(0.5, iw - 0.5);
                let sy = (oy + dx * s + dy * c).clamp(0.5, ih - 0.5);
                let v = self.img.sample(sx, sy).unwrap_or([0.0; 3]);
                let i = ((y * self.w + x) * 3) as usize;
                out.data[i] = v[0] as u8;
                out.data[i + 1] = v[1] as u8;
                out.data[i + 2] = v[2] as u8;
            }
        }
        SourceFrame {
            image: Arc::new(out),
            face_hint: None,
            mask_hint: None,
        }
    }
}
