//! Depth camera support: a raw depth image aligned to the colour frame
//! ([`DepthMap`]), and a cleaned-up, coarse 3D surface built from it each
//! frame ([`DepthField`]). The field fills holes (glasses, IR dropouts),
//! smooths the sensor noise, keeps a little temporal memory, and decides which
//! pixels belong to the person in front of the camera.
//!
//! Who is the person comes from depth *and* the colour segmentation mask.
//! Dark fabric, hair and headphones reflect the sensor's infrared poorly and
//! often have no depth at all, so where depth is missing the mask decides
//! (and the depth is filled in from the body around it). Where depth does
//! read something far behind you, the mask is overruled, which keeps the
//! mask's halos and false hits on the background out of the cutout.

use crate::image::RgbImage;
use crate::track::{Face, Mask};

/// Pinhole intrinsics of the colour stream the depth is aligned to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Intrinsics {
    pub fx: f32,
    pub fy: f32,
    pub ppx: f32,
    pub ppy: f32,
}

/// Raw depth aligned pixel-for-pixel to the colour image.
#[derive(Clone, Debug)]
pub struct DepthMap {
    pub width: u32,
    pub height: u32,
    /// Raw sensor units; 0 means "no reading".
    pub data: Vec<u16>,
    /// Metres per unit.
    pub scale: f32,
    pub intr: Intrinsics,
}

/// Coarse grid cell size in source pixels.
const CELL: usize = 4;
/// How far behind the face the person extends (shoulders, chest).
const BEHIND: f32 = 0.45;
/// How far in front of the face still counts (hands, a raised phone).
const IN_FRONT: f32 = 0.3;

#[derive(Default)]
pub struct DepthField {
    gw: usize,
    gh: usize,
    /// Source size the grid was built for.
    sw: u32,
    sh: u32,
    intr: Option<Intrinsics>,
    /// Smoothed depth in metres (0 = none).
    z: Vec<f32>,
    /// Person membership 0..1.
    subject: Vec<f32>,
    /// `subject` softened over neighbours: what callers read.
    sub_s: Vec<f32>,
    /// Segmentation mask at each cell this frame (0 without a mask).
    seg: Vec<f32>,
    /// `seg` averaged over time where nothing moves (a monitor the network
    /// can't make its mind up about); wherever the image or the depth
    /// changes it follows at once, so a moving person is never late.
    seg_avg: Vec<f32>,
    /// Image brightness per cell last frame, for spotting motion.
    lum: Vec<f32>,
    /// How much each cell is moving this frame (0..1).
    mov: Vec<f32>,
    /// How often each cell has had a depth reading lately (0..1). Dark
    /// fabric flickers between a noisy reading and none; such spots are
    /// filled in from steadier neighbours instead.
    rel: Vec<f32>,
    /// Per-cell reading this frame: see `NONE`, `IN_BAND`, `AWAY`, `FILLED`.
    kind: Vec<u8>,
    raw: Vec<f32>,
    w: Vec<f32>,
    /// Surface coordinates in metres (arc length along the skin from the
    /// face centre); NaN where unknown. Built by [`DepthField::build_uv`].
    su: Vec<f32>,
    sv: Vec<f32>,
    /// Smoothed curvature part of (su, sv): surface coordinates minus the
    /// flat, rigid-with-the-anchor part. Kept across frames.
    ru: Vec<f32>,
    rv: Vec<f32>,
    /// Extra-smoothed depth used only for surface coordinates.
    uz: Vec<f32>,
    uw: Vec<f32>,
    /// Smoothed anchor (source pixels) and depth scale for `build_uv`.
    anchor: [f32; 2],
    /// Smoothed anchor speed (px/s), for the speed-aware anchor filter.
    anchor_v: f32,
    uv_zref: f32,
    /// `ru`/`rv`/`anchor` hold a previous frame.
    uv_init: bool,
    tmp_a: Vec<f32>,
    tmp_b: Vec<f32>,
    /// Connected-piece labels for dropping stray bits of the cutout.
    label: Vec<u32>,
    stack: Vec<usize>,
    /// Depth of the face (or nearest surface), metres.
    pub z_ref: f32,
    pub valid: bool,
    /// Someone is in front of the camera (0..1). Drops only after the face
    /// detector has seen nobody for [`AWAY_AFTER`] seconds.
    pub present: f32,
    /// Seconds since the face detector last reported a face.
    lost_t: f32,
}

/// Seconds without a detected face before the person is treated as gone,
/// so the whole frame gets the background treatment instead of the nearest
/// chair or desk standing in for you.
const AWAY_AFTER: f32 = 3.0;

/// No depth reading.
const NONE: u8 = 0;
/// Depth within the person's band.
const IN_BAND: u8 = 1;
/// Depth well behind (or in front of) the person: not you, whatever the
/// mask says.
const AWAY: u8 = 2;
/// No reading, but the mask says it's you; depth filled in from around it.
const FILLED: u8 = 3;

/// Sampled surface at a source pixel.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    /// Metres from the camera.
    pub z: f32,
    /// Unit normal in camera space (x right, y down, z away from camera);
    /// a surface facing the camera has n.z < 0.
    pub n: [f32; 3],
    /// Person membership 0..1.
    pub subject: f32,
}

impl DepthField {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intrinsics(&self) -> Option<Intrinsics> {
        self.intr
    }

    /// Size of the source image the field was built from.
    pub fn source_size(&self) -> (f32, f32) {
        (self.sw as f32, self.sh as f32)
    }

    pub fn clear(&mut self) {
        self.valid = false;
        self.uv_init = false;
        self.z.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Rebuild the field from a new depth frame. `face` (in source pixels)
    /// anchors which depth counts as "you"; `mask` (the colour segmentation
    /// for the same frame) fills in where depth has no reading; `img` (the
    /// colour frame) tells still parts of the scene from moving ones.
    pub fn update(
        &mut self,
        d: &DepthMap,
        face: Option<&Face>,
        mask: Option<&Mask>,
        img: Option<&RgbImage>,
        dt: f32,
    ) {
        let gw = (d.width as usize).div_ceil(CELL);
        let gh = (d.height as usize).div_ceil(CELL);
        let n = gw * gh;
        if gw != self.gw || gh != self.gh || self.sw != d.width || self.sh != d.height {
            *self = DepthField {
                gw,
                gh,
                sw: d.width,
                sh: d.height,
                z: vec![0.0; n],
                subject: vec![0.0; n],
                sub_s: vec![0.0; n],
                seg: vec![0.0; n],
                seg_avg: vec![0.0; n],
                lum: vec![-1.0; n],
                mov: vec![1.0; n],
                rel: vec![1.0; n],
                kind: vec![NONE; n],
                raw: vec![0.0; n],
                w: vec![0.0; n],
                su: vec![f32::NAN; n],
                sv: vec![f32::NAN; n],
                ru: vec![0.0; n],
                rv: vec![0.0; n],
                uz: vec![0.0; n],
                uw: vec![0.0; n],
                tmp_a: vec![0.0; n],
                tmp_b: vec![0.0; n],
                ..Default::default()
            };
        }
        self.intr = Some(d.intr);

        // 1. block-average valid samples into the coarse grid
        let width = d.width as usize;
        for gy in 0..gh {
            for gx in 0..gw {
                let (mut acc, mut cnt) = (0.0f32, 0u32);
                for y in gy * CELL..((gy + 1) * CELL).min(d.height as usize) {
                    let row = &d.data[y * width..(y + 1) * width];
                    for &v in &row[gx * CELL..((gx + 1) * CELL).min(width)] {
                        if v != 0 {
                            acc += v as f32;
                            cnt += 1;
                        }
                    }
                }
                let i = gy * gw + gx;
                // need a few hits to trust a block (kills speckle)
                if cnt >= 3 {
                    self.raw[i] = acc / cnt as f32 * d.scale;
                    self.w[i] = 1.0;
                } else {
                    self.raw[i] = 0.0;
                    self.w[i] = 0.0;
                }
            }
        }

        // the mask at each cell, steadied where nothing moves
        let (sw, sh) = (d.width as f32, d.height as f32);
        let img = img.filter(|i| i.width == d.width && i.height == d.height);
        let slow = (dt / 0.3).clamp(0.0, 1.0);
        for gy in 0..gh {
            for gx in 0..gw {
                let i = gy * gw + gx;
                let cx = (gx * CELL) as f32 + CELL as f32 * 0.5;
                let cy = (gy * CELL) as f32 + CELL as f32 * 0.5;
                self.seg[i] = mask.map_or(0.0, |m| m.sample(cx / sw, cy / sh));
                // motion: the picture changed here, or the depth jumped
                let mut moving = 1.0f32;
                if let Some(im) = img {
                    let (x0, y0) = ((gx * CELL) as u32, (gy * CELL) as u32);
                    let mut l = 0.0;
                    for (dx, dy) in [(0, 0), (2, 0), (0, 2), (2, 2)] {
                        let px = im.get((x0 + dx).min(im.width - 1), (y0 + dy).min(im.height - 1));
                        l += px[0] as f32 + 2.0 * px[1] as f32 + px[2] as f32;
                    }
                    l /= 4.0 * 4.0 * 255.0;
                    let prev = self.lum[i];
                    self.lum[i] = l;
                    if prev >= 0.0 {
                        moving = (((l - prev).abs() - 0.02) / 0.05).clamp(0.0, 1.0);
                    }
                }
                let (v, p) = (self.raw[i], self.z[i]);
                // (losing a reading isn't motion: dark fabric does that)
                if v > 0.0 && (p <= 0.0 || (v - p).abs() > 0.04) {
                    moving = 1.0;
                }
                let k = slow + (1.0 - slow) * moving;
                self.seg_avg[i] += (self.seg[i] - self.seg_avg[i]) * k;
                self.mov[i] = moving;
                let got = if v > 0.0 { 1.0 } else { 0.0 };
                self.rel[i] += (got - self.rel[i]) * (dt / 0.4).clamp(0.0, 1.0);
            }
        }

        // 2. reference depth: the face if we know where it is, else what
        // the mask calls you, else the nearest sizeable surface
        let mut zs: Vec<f32> = Vec::new();
        if let Some(f) = face {
            let (x0, x1) = (f.cx - f.w * 0.4, f.cx + f.w * 0.4);
            let (y0, y1) = (f.cy - f.h * 0.4, f.cy + f.h * 0.4);
            for gy in 0..gh {
                let cy = (gy * CELL) as f32 + CELL as f32 * 0.5;
                if cy < y0 || cy > y1 {
                    continue;
                }
                for gx in 0..gw {
                    let cx = (gx * CELL) as f32 + CELL as f32 * 0.5;
                    let v = self.raw[gy * gw + gx];
                    if cx >= x0 && cx <= x1 && v > 0.0 {
                        zs.push(v);
                    }
                }
            }
        }
        // presence: you only count as gone after a long run without a face
        if face.is_some() {
            self.lost_t = 0.0;
        } else {
            self.lost_t += dt;
        }
        self.present = if self.lost_t < AWAY_AFTER {
            1.0
        } else {
            (self.present - dt / 0.6).max(0.0)
        };
        let mut by_mask = None;
        if zs.len() < 8 && mask.is_some() && self.present >= 1.0 {
            let mut ms: Vec<f32> = (0..n)
                .filter(|&i| self.raw[i] > 0.0 && self.seg[i] > 0.6)
                .map(|i| self.raw[i])
                .collect();
            if ms.len() >= 30 {
                // the mask can spill onto the wall behind you: lean near
                by_mask = Some(percentile(&mut ms, 0.3));
            }
        }
        let z_now = if zs.len() >= 8 {
            percentile(&mut zs, 0.5)
        } else if let Some(z) = by_mask {
            z
        } else {
            zs = self.raw.iter().copied().filter(|&v| v > 0.0).collect();
            if zs.len() < 30 {
                if self.present < 1.0 {
                    // nobody there and nothing in range: an empty field, so
                    // the render shows background, not the raw camera
                    self.z.iter_mut().for_each(|v| *v = 0.0);
                    self.subject.iter_mut().for_each(|v| *v = 0.0);
                    self.sub_s.iter_mut().for_each(|v| *v = 0.0);
                    self.kind.iter_mut().for_each(|v| *v = NONE);
                    self.valid = true;
                } else {
                    self.valid = false;
                }
                return;
            }
            percentile(&mut zs, 0.04)
        };
        let k = if self.z_ref > 0.0 {
            (dt * 6.0).clamp(0.05, 1.0)
        } else {
            1.0
        };
        self.z_ref += (z_now - self.z_ref) * k;
        let (near, far) = (self.z_ref - IN_FRONT, self.z_ref + BEHIND);

        // 3. keep only the person, fill small holes (glasses, dropouts)
        for i in 0..n {
            let v = self.raw[i];
            self.kind[i] = if v <= 0.0 {
                NONE
            } else if v < near || v > far {
                self.raw[i] = 0.0;
                self.w[i] = 0.0;
                AWAY
            } else {
                IN_BAND
            };
        }
        if mask.is_some() {
            // readings on dark fabric are sparse, noisy and sometimes plain
            // wrong: drop a reading where the spot rarely has one, or that
            // disagrees with its neighbours, and let the fill below cover it
            self.tmp_a.copy_from_slice(&self.raw);
            for gy in 0..gh {
                for gx in 0..gw {
                    let i = gy * gw + gx;
                    if self.kind[i] != IN_BAND {
                        continue;
                    }
                    let mut nb = [0.0f32; 8];
                    let mut c = 0;
                    for dy in -1isize..=1 {
                        for dx in -1isize..=1 {
                            let (x, y) = (gx as isize + dx, gy as isize + dy);
                            if (dx, dy) == (0, 0)
                                || x < 0
                                || y < 0
                                || x >= gw as isize
                                || y >= gh as isize
                            {
                                continue;
                            }
                            let z = self.tmp_a[y as usize * gw + x as usize];
                            if z > 0.0 {
                                nb[c] = z;
                                c += 1;
                            }
                        }
                    }
                    let odd = if c >= 3 {
                        let m = percentile(&mut nb[..c], 0.5);
                        (self.tmp_a[i] - m).abs() > 0.03
                    } else {
                        // a lone reading with no support around it
                        true
                    };
                    if odd || self.rel[i] < 0.6 {
                        self.raw[i] = 0.0;
                        self.w[i] = 0.0;
                        self.kind[i] = NONE;
                    }
                }
            }
        }
        for _ in 0..3 {
            normalized_blur(
                &mut self.raw,
                &mut self.w,
                &mut self.tmp_a,
                &mut self.tmp_b,
                gw,
                gh,
                true,
            );
        }
        // 4. smooth the noise (normalized, so edges don't bleed into the void)
        normalized_blur(
            &mut self.raw,
            &mut self.w,
            &mut self.tmp_a,
            &mut self.tmp_b,
            gw,
            gh,
            false,
        );

        // 4b. no reading but the mask says it's you (dark clothes, hair):
        // grow depth in from the body around it
        if mask.is_some() {
            let want = |i: usize, kind: &[u8], seg: &[f32], w: &[f32]| {
                w[i] == 0.0 && kind[i] == NONE && seg[i] > 0.5
            };
            for _ in 0..24 {
                let mut grew = false;
                for gy in 0..gh {
                    for gx in 0..gw {
                        let i = gy * gw + gx;
                        self.tmp_b[i] = 0.0;
                        if !want(i, &self.kind, &self.seg_avg, &self.w) {
                            continue;
                        }
                        let (mut a, mut b) = (0.0, 0.0);
                        for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                            let x = gx as isize + dx;
                            let y = gy as isize + dy;
                            if x < 0 || y < 0 || x >= gw as isize || y >= gh as isize {
                                continue;
                            }
                            let j = y as usize * gw + x as usize;
                            if self.w[j] > 0.0 {
                                a += self.raw[j];
                                b += 1.0;
                            }
                        }
                        if b > 0.0 {
                            self.tmp_a[i] = a / b;
                            self.tmp_b[i] = 1.0;
                            grew = true;
                        }
                    }
                }
                for i in 0..n {
                    if self.tmp_b[i] > 0.0 {
                        self.raw[i] = self.tmp_a[i];
                        self.w[i] = 1.0;
                        self.kind[i] = FILLED;
                    }
                }
                if !grew {
                    break;
                }
            }
            // anything the mask wants that's still empty (a whole dark
            // torso with no depth around it): just behind the face
            for i in 0..n {
                if want(i, &self.kind, &self.seg_avg, &self.w) {
                    self.raw[i] = self.z_ref + 0.08;
                    self.w[i] = 1.0;
                    self.kind[i] = FILLED;
                }
            }
            // the grown-in depth is streaky: smooth it (only it)
            for _ in 0..3 {
                for gy in 0..gh {
                    for gx in 0..gw {
                        let i = gy * gw + gx;
                        self.tmp_a[i] = self.raw[i];
                        if self.kind[i] != FILLED {
                            continue;
                        }
                        let (mut acc, mut cnt) = (0.0, 0.0);
                        for (dx, dy) in [(0isize, 0isize), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                            let (x, y) = (gx as isize + dx, gy as isize + dy);
                            if x < 0 || y < 0 || x >= gw as isize || y >= gh as isize {
                                continue;
                            }
                            let z = self.raw[y as usize * gw + x as usize];
                            if z > 0.0 {
                                acc += z;
                                cnt += 1.0;
                            }
                        }
                        self.tmp_a[i] = acc / cnt;
                    }
                }
                self.raw.copy_from_slice(&self.tmp_a);
            }
        }

        // 5. temporal blend; jumps (you moved) reset immediately. The
        // cutout follows within a frame or two either way, so it never
        // leaves a trail behind you.
        let a = (dt * 14.0).clamp(0.2, 1.0);
        // where nothing moves, depth noise is averaged over ~0.2 s
        let a_still = (dt / 0.2).clamp(0.0, 1.0);
        let k = (dt * 20.0).clamp(0.5, 1.0);
        for i in 0..n {
            let v = self.raw[i];
            let p = self.z[i];
            if v <= 0.0 {
                self.z[i] = 0.0;
                self.subject[i] = 0.0;
                continue;
            }
            let mv = self.mov[i];
            let jump = p <= 0.0 || (v - p).abs() > 0.15 || ((v - p).abs() > 0.04 && mv > 0.3);
            self.z[i] = if jump {
                v
            } else {
                p + (v - p) * (a_still + (a - a_still) * mv)
            };
            let target = match self.kind[i] {
                // only a confident mask fills in (the network is unsure
                // over screens and shiny things)
                FILLED => ((self.seg_avg[i] - 0.45) / 0.35).clamp(0.0, 1.0),
                // a depth reading: you, fading out toward the back of the
                // band so the cutout isn't a hard wall; with a mask, things
                // at your depth that clearly aren't a person (a monitor, a
                // chair arm) are left out
                _ => {
                    let edge = ((far - v) / 0.08).clamp(0.0, 1.0);
                    if mask.is_some() {
                        edge * ((self.seg_avg[i] - 0.3) / 0.3).clamp(0.0, 1.0)
                    } else {
                        edge
                    }
                }
            } * self.present;
            let cur = self.subject[i];
            self.subject[i] = if p <= 0.0 {
                target
            } else {
                cur + (target - cur) * k
            };
        }
        // 5b. drop stray pieces (a lamp or a laptop corner at your depth):
        // anything much smaller than the biggest piece, which is you
        drop_small_pieces(&mut self.subject, &mut self.label, &mut self.stack, gw, gh);

        // 6. soften the cutout edge over neighbours (read-only copy, so the
        // blur doesn't build up over time)
        for gy in 0..gh {
            for gx in 0..gw {
                let (mut acc, mut wsum) = (0.0, 0.0);
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let x = (gx as isize + dx).clamp(0, gw as isize - 1) as usize;
                        let y = (gy as isize + dy).clamp(0, gh as isize - 1) as usize;
                        let w = if dx == 0 && dy == 0 { 4.0 } else { 1.0 };
                        acc += self.subject[y * gw + x] * w;
                        wsum += w;
                    }
                }
                self.sub_s[gy * gw + gx] = acc / wsum;
            }
        }
        self.valid = true;
    }

    #[inline]
    fn at(&self, gx: isize, gy: isize) -> f32 {
        let x = gx.clamp(0, self.gw as isize - 1) as usize;
        let y = gy.clamp(0, self.gh as isize - 1) as usize;
        self.z[y * self.gw + x]
    }

    /// Depth (metres) at a source pixel, bilinear over valid neighbours.
    pub fn depth(&self, x: f32, y: f32) -> Option<f32> {
        if !self.valid || x < 0.0 || y < 0.0 || x >= self.sw as f32 || y >= self.sh as f32 {
            return None;
        }
        let fx = x / CELL as f32 - 0.5;
        let fy = y / CELL as f32 - 0.5;
        let x0 = fx.floor() as isize;
        let y0 = fy.floor() as isize;
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let mut acc = 0.0;
        let mut wsum = 0.0;
        for (dx, dy, w) in [
            (0, 0, (1.0 - tx) * (1.0 - ty)),
            (1, 0, tx * (1.0 - ty)),
            (0, 1, (1.0 - tx) * ty),
            (1, 1, tx * ty),
        ] {
            let v = self.at(x0 + dx, y0 + dy);
            if v > 0.0 && w > 0.0 {
                acc += v * w;
                wsum += w;
            }
        }
        if wsum < 0.3 {
            None
        } else {
            Some(acc / wsum)
        }
    }

    /// Surface (depth, normal, membership) at a source pixel.
    pub fn surface(&self, x: f32, y: f32) -> Option<Surface> {
        let z = self.depth(x, y)?;
        let intr = self.intr?;
        let h = CELL as f32 * 1.5;
        let zx0 = self.depth(x - h, y).unwrap_or(z);
        let zx1 = self.depth(x + h, y).unwrap_or(z);
        let zy0 = self.depth(x, y - h).unwrap_or(z);
        let zy1 = self.depth(x, y + h).unwrap_or(z);
        // metres per pixel at this depth
        let mx = z / intr.fx;
        let my = z / intr.fy;
        let dzdx = (zx1 - zx0) / (2.0 * h * mx);
        let dzdy = (zy1 - zy0) / (2.0 * h * my);
        let len = (dzdx * dzdx + dzdy * dzdy + 1.0).sqrt();
        let n = [dzdx / len, dzdy / len, -1.0 / len];
        let gx = ((x / CELL as f32) as usize).min(self.gw - 1);
        let gy = ((y / CELL as f32) as usize).min(self.gh - 1);
        Some(Surface {
            z,
            n,
            subject: self.sub_s[gy * self.gw + gx],
        })
    }

    /// Build surface coordinates: `u` is the arc length along each grid row
    /// (through x and depth) from the anchor column, `v` the arc length down
    /// each column from the anchor row. Laying glyphs out in (u, v) paints
    /// them onto the skin: they foreshorten where the surface turns away and
    /// stretch over the nose. `(cx, cy)` is the anchor in source pixels.
    ///
    /// Arc length integrates depth noise, so the result is split into a flat
    /// part that moves rigidly with the (lightly smoothed) anchor and a
    /// curvature part that is smoothed in space and time. `calm` (0..1) sets
    /// how much smoothing; `dt` is the frame time in seconds.
    pub fn build_uv(&mut self, cx: f32, cy: f32, calm: f32, dt: f32) {
        let (gw, gh) = (self.gw, self.gh);
        if !self.valid || gw == 0 {
            return;
        }
        let n = gw * gh;
        let calm = calm.clamp(0.0, 1.0);
        let dt = dt.clamp(0.0, 0.25);
        let ease = |tau: f32| 1.0 - (-dt / tau.max(1e-4)).exp();
        let init = self.uv_init;

        // anchor and depth scale, smoothed (a big jump means a new face: snap)
        // The face box shakes a few pixels every frame, and every glyph is
        // laid out from this anchor, so any shake moves them all. A
        // speed-aware filter (one-euro style): heavy smoothing while you're
        // still, light while you move so glyphs don't lag behind you.
        let [px, py] = self.anchor;
        let jump = (cx - px).hypot(cy - py) > 60.0;
        let (ax, ay) = if init && !jump && dt > 0.0 {
            let speed = (cx - px).hypot(cy - py) / dt;
            self.anchor_v += (speed - self.anchor_v) * ease(0.1);
            // still: ~0.15-0.8 s; moving fast (>200 px/s): ~20 ms
            let tau_still = 0.15 + 0.65 * calm;
            let fast = (self.anchor_v / 200.0).clamp(0.0, 1.0);
            let k = ease(tau_still + (0.02 - tau_still) * fast);
            (px + (cx - px) * k, py + (cy - py) * k)
        } else {
            self.anchor_v = 0.0;
            (cx, cy)
        };
        self.anchor = [ax, ay];
        let zr = if init && self.uv_zref > 0.0 {
            self.uv_zref + (self.z_ref - self.uv_zref) * ease(0.1 + 0.6 * calm)
        } else {
            self.z_ref
        };
        self.uv_zref = zr;

        // depth smoothed some more, just for the coordinates
        self.uz.copy_from_slice(&self.z);
        for i in 0..n {
            self.uw[i] = if self.z[i] > 0.0 { 1.0 } else { 0.0 };
        }
        let passes = 1 + (calm * 3.0).round() as usize;
        for _ in 0..passes {
            normalized_blur(
                &mut self.uz,
                &mut self.uw,
                &mut self.tmp_a,
                &mut self.tmp_b,
                gw,
                gh,
                false,
            );
        }

        let (fx, fy) = self.intr.map(|i| (i.fx, i.fy)).unwrap_or((600.0, 600.0));
        let (mx, my) = (CELL as f32 / fx, CELL as f32 / fy);
        // one grid step along the skin: lateral size at this depth plus the
        // depth change (capped so a gap doesn't tear the coordinates); off
        // the surface, the flat step at face depth
        let step = |za: f32, zb: f32, m: f32| -> f32 {
            if za > 0.0 && zb > 0.0 {
                let d = m * 0.5 * (za + zb);
                let dz = (zb - za).clamp(-0.03, 0.03);
                (d * d + dz * dz).sqrt()
            } else {
                m * zr
            }
        };
        let c0 = ((ax / CELL as f32).max(0.0) as usize).min(gw - 1);
        let r0 = ((ay / CELL as f32).max(0.0) as usize).min(gh - 1);
        let centre = |g: usize| (g * CELL) as f32 + CELL as f32 * 0.5;
        // flat coordinates: rigid with the anchor
        let flat_u = |gx: usize| (centre(gx) - ax) * zr / fx;
        let flat_v = |gy: usize| (centre(gy) - ay) * zr / fy;

        let uz = &self.uz;
        for gy in 0..gh {
            let row = gy * gw;
            self.su[row + c0] = flat_u(c0);
            for gx in c0 + 1..gw {
                self.su[row + gx] =
                    self.su[row + gx - 1] + step(uz[row + gx - 1], uz[row + gx], mx);
            }
            for gx in (0..c0).rev() {
                self.su[row + gx] =
                    self.su[row + gx + 1] - step(uz[row + gx + 1], uz[row + gx], mx);
            }
        }
        for gx in 0..gw {
            self.sv[r0 * gw + gx] = flat_v(r0);
            for gy in r0 + 1..gh {
                let (a, b) = ((gy - 1) * gw + gx, gy * gw + gx);
                self.sv[b] = self.sv[a] + step(uz[a], uz[b], my);
            }
            for gy in (0..r0).rev() {
                let (a, b) = ((gy + 1) * gw + gx, gy * gw + gx);
                self.sv[b] = self.sv[a] - step(uz[a], uz[b], my);
            }
        }

        // smooth the curvature part over time; a large change (you turned
        // your head) follows faster so glyphs don't swim
        // where the picture is still, hold the shape much longer
        let k_move = ease(0.03 + 0.4 * calm);
        let k_still = ease(0.1 + 1.4 * calm);
        for gy in 0..gh {
            for gx in 0..gw {
                let i = gy * gw + gx;
                let k = k_still + (k_move - k_still) * self.mov[i];
                let (fu, fv) = (flat_u(gx), flat_v(gy));
                let nu = self.su[i] - fu;
                let nv = self.sv[i] - fv;
                if init {
                    let du = nu - self.ru[i];
                    let dv = nv - self.rv[i];
                    let big = (du.abs().max(dv.abs()) / 0.03).min(1.0);
                    let kk = k + (1.0 - k) * big * big * 0.6;
                    self.ru[i] += du * kk;
                    self.rv[i] += dv * kk;
                } else {
                    self.ru[i] = nu;
                    self.rv[i] = nv;
                }
                self.su[i] = fu + self.ru[i];
                self.sv[i] = fv + self.rv[i];
            }
        }
        self.uv_init = true;
    }

    /// Surface coordinates (metres) and membership at a source pixel, or
    /// `None` off the person. Requires [`DepthField::build_uv`] this frame.
    #[inline]
    pub fn uv(&self, x: f32, y: f32) -> Option<(f32, f32, f32)> {
        if !self.valid || x < 0.0 || y < 0.0 || x >= self.sw as f32 || y >= self.sh as f32 {
            return None;
        }
        let fx = (x / CELL as f32 - 0.5).max(0.0);
        let fy = (y / CELL as f32 - 0.5).max(0.0);
        let x0 = (fx as usize).min(self.gw - 1);
        let y0 = (fy as usize).min(self.gh - 1);
        let x1 = (x0 + 1).min(self.gw - 1);
        let y1 = (y0 + 1).min(self.gh - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let (mut u, mut v, mut s, mut wsum) = (0.0, 0.0, 0.0, 0.0);
        for (i, w) in [
            (y0 * self.gw + x0, (1.0 - tx) * (1.0 - ty)),
            (y0 * self.gw + x1, tx * (1.0 - ty)),
            (y1 * self.gw + x0, (1.0 - tx) * ty),
            (y1 * self.gw + x1, tx * ty),
        ] {
            if self.z[i] > 0.0 && w > 0.0 {
                u += self.su[i] * w;
                v += self.sv[i] * w;
                s += self.sub_s[i] * w;
                wsum += w;
            }
        }
        if wsum < 0.5 {
            return None;
        }
        Some((u / wsum, v / wsum, s / wsum))
    }

    /// Back-project a source pixel with known depth to camera space (metres).
    #[inline]
    pub fn unproject(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let i = self.intr.unwrap_or(Intrinsics {
            fx: 600.0,
            fy: 600.0,
            ppx: self.sw as f32 * 0.5,
            ppy: self.sh as f32 * 0.5,
        });
        [(x - i.ppx) * z / i.fx, (y - i.ppy) * z / i.fy, z]
    }

    /// Project a camera-space point back to source pixels.
    #[inline]
    pub fn project(&self, p: [f32; 3]) -> [f32; 2] {
        let i = self.intr.unwrap_or(Intrinsics {
            fx: 600.0,
            fy: 600.0,
            ppx: self.sw as f32 * 0.5,
            ppy: self.sh as f32 * 0.5,
        });
        let z = p[2].max(0.05);
        [p[0] * i.fx / z + i.ppx, p[1] * i.fy / z + i.ppy]
    }
}

/// Zero the connected pieces of `sub` (cells above 0.25) that are smaller
/// than 4% of the largest one (a hand is bigger than that), or tiny. A
/// second person stays.
fn drop_small_pieces(
    sub: &mut [f32],
    label: &mut Vec<u32>,
    stack: &mut Vec<usize>,
    gw: usize,
    gh: usize,
) {
    let n = gw * gh;
    label.clear();
    label.resize(n, 0);
    let mut sizes: Vec<usize> = vec![0];
    for start in 0..n {
        if label[start] != 0 || sub[start] <= 0.25 {
            continue;
        }
        let id = sizes.len() as u32;
        let mut size = 0;
        stack.clear();
        stack.push(start);
        label[start] = id;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % gw, i / gw);
            let mut visit = |j: usize| {
                if label[j] == 0 && sub[j] > 0.25 {
                    label[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < gw {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - gw);
            }
            if y + 1 < gh {
                visit(i + gw);
            }
        }
        sizes.push(size);
    }
    let biggest = sizes.iter().copied().max().unwrap_or(0);
    let keep = (biggest as f32 * 0.04).max(40.0) as usize;
    for i in 0..n {
        let l = label[i] as usize;
        if l != 0 && sizes[l] < keep {
            sub[i] = 0.0;
        }
    }
}

fn percentile(v: &mut [f32], p: f32) -> f32 {
    let k = ((v.len() - 1) as f32 * p) as usize;
    *v.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
}

/// 3x3 normalized convolution. With `fill_only`, only empty cells take the
/// blurred value (hole filling); otherwise every valid cell is smoothed.
fn normalized_blur(
    z: &mut [f32],
    w: &mut [f32],
    acc: &mut [f32],
    wacc: &mut [f32],
    gw: usize,
    gh: usize,
    fill_only: bool,
) {
    for y in 0..gh {
        for x in 0..gw {
            let (mut a, mut b) = (0.0, 0.0);
            for dy in -1isize..=1 {
                let yy = y as isize + dy;
                if yy < 0 || yy >= gh as isize {
                    continue;
                }
                for dx in -1isize..=1 {
                    let xx = x as isize + dx;
                    if xx < 0 || xx >= gw as isize {
                        continue;
                    }
                    let j = yy as usize * gw + xx as usize;
                    let k = if dx == 0 && dy == 0 { 2.0 } else { 1.0 } * w[j];
                    a += z[j] * k;
                    b += k;
                }
            }
            let i = y * gw + x;
            acc[i] = a;
            wacc[i] = b;
        }
    }
    for i in 0..gw * gh {
        if fill_only {
            // fill a hole only when it's mostly surrounded (not the outside)
            if w[i] == 0.0 && wacc[i] >= 4.0 {
                z[i] = acc[i] / wacc[i];
                w[i] = 1.0;
            }
        } else if w[i] > 0.0 && wacc[i] > 0.0 {
            z[i] = acc[i] / wacc[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ball 0.5 m away centred at (cx, cy) source pixels, on an optional
    /// wall at `wall` metres.
    fn scene(w: u32, h: u32, cx: f32, cy: f32, wall: f32) -> DepthMap {
        let intr = Intrinsics {
            fx: 600.0,
            fy: 600.0,
            ppx: w as f32 / 2.0,
            ppy: h as f32 / 2.0,
        };
        let scale = 0.000125;
        let mut data = vec![(wall / scale) as u16; (w * h) as usize];
        let r = h as f32 * 0.3;
        for y in 0..h {
            for x in 0..w {
                let dx = (x as f32 - cx) / r;
                let dy = (y as f32 - cy) / r;
                let d2 = dx * dx + dy * dy;
                if d2 < 1.0 {
                    // 0.5 m away, bulging 8 cm toward the camera in the middle
                    let z = 0.58 - 0.08 * (1.0 - d2).sqrt();
                    data[(y * w + x) as usize] = (z / scale) as u16;
                }
            }
        }
        DepthMap {
            width: w,
            height: h,
            data,
            scale,
            intr,
        }
    }

    fn sphere(w: u32, h: u32) -> DepthMap {
        scene(w, h, w as f32 / 2.0, h as f32 / 2.0, 0.0)
    }

    /// What the segmentation would say: the ball (plus `extra` pixels
    /// around it, like a halo).
    fn ball_mask(w: u32, h: u32, cx: f32, cy: f32, extra: f32) -> Mask {
        let (mw, mh) = (128usize, 96usize);
        let r = h as f32 * 0.3 + extra;
        let mut data = vec![0.0; mw * mh];
        for my in 0..mh {
            for mx in 0..mw {
                let x = (mx as f32 + 0.5) / mw as f32 * w as f32;
                let y = (my as f32 + 0.5) / mh as f32 * h as f32;
                if (x - cx).hypot(y - cy) < r {
                    data[my * mw + mx] = 1.0;
                }
            }
        }
        Mask { w: mw, h: mh, data }
    }

    #[test]
    fn field_finds_surface_and_normals() {
        let d = sphere(320, 240);
        let mut f = DepthField::new();
        f.update(&d, None, None, None, 1.0 / 30.0);
        assert!(f.valid);
        let c = f.surface(160.0, 120.0).unwrap();
        assert!((c.z - 0.50).abs() < 0.01, "centre depth {}", c.z);
        assert!(c.n[2] < -0.95, "centre faces the camera {:?}", c.n);
        // right side of the bulge leans away from the camera to the right
        let r = f.surface(160.0 + 60.0, 120.0).unwrap();
        assert!(r.n[0] > 0.2, "right normal {:?}", r.n);
        assert!(r.subject > 0.5);
        assert!(f.depth(5.0, 5.0).is_none());
        let p = f.unproject(200.0, 100.0, 0.5);
        let q = f.project(p);
        assert!((q[0] - 200.0).abs() < 1e-3 && (q[1] - 100.0).abs() < 1e-3);
    }

    #[test]
    fn dark_clothes_without_depth_still_count() {
        // a dark hoodie: the right half of the person has no depth at all,
        // some of it flickering; the mask still sees it
        let mask = ball_mask(320, 240, 160.0, 120.0, 0.0);
        let mut f = DepthField::new();
        let mut seed = 1u32;
        let mut worst = 1.0f32;
        for frame in 0..30 {
            let mut d = sphere(320, 240);
            for (i, v) in d.data.iter_mut().enumerate() {
                let x = (i % 320) as i32 - 160;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                if x > 10 || (x > -20 && seed >> 31 == 1) {
                    *v = 0;
                }
            }
            f.update(&d, None, Some(&mask), None, 1.0 / 30.0);
            if frame >= 2 {
                for x in [190.0, 220.0, 225.0] {
                    let s = f.surface(x, 120.0).expect("filled depth");
                    assert!((s.z - 0.52).abs() < 0.1, "filled depth {}", s.z);
                    worst = worst.min(s.subject);
                }
            }
        }
        assert!(worst > 0.8, "cutout dropped to {worst}");
    }

    #[test]
    fn mask_halo_over_the_background_is_trimmed() {
        // the mask spills 30 px past the person onto a wall 1.5 m back;
        // depth knows that's wall
        let d = scene(320, 240, 160.0, 120.0, 1.5);
        let mask = ball_mask(320, 240, 160.0, 120.0, 30.0);
        let mut f = DepthField::new();
        for _ in 0..5 {
            f.update(&d, None, Some(&mask), None, 1.0 / 30.0);
        }
        assert!(f.surface(160.0, 120.0).unwrap().subject > 0.9);
        let halo = f
            .surface(160.0 + 72.0 + 20.0, 120.0)
            .map_or(0.0, |s| s.subject);
        assert!(halo < 0.05, "halo kept at {halo}");
    }

    #[test]
    fn cutout_follows_movement_without_a_trail() {
        let mut f = DepthField::new();
        let mut cx = 100.0;
        for _ in 0..10 {
            let d = scene(320, 240, cx, 120.0, 1.5);
            f.update(
                &d,
                None,
                Some(&ball_mask(320, 240, cx, 120.0, 0.0)),
                None,
                1.0 / 30.0,
            );
        }
        // jump 120 px to the right: two frames later the old spot is clear
        cx = 220.0;
        for _ in 0..2 {
            let d = scene(320, 240, cx, 120.0, 1.5);
            f.update(
                &d,
                None,
                Some(&ball_mask(320, 240, cx, 120.0, 0.0)),
                None,
                1.0 / 30.0,
            );
        }
        let old = f.surface(60.0, 120.0).map_or(0.0, |s| s.subject);
        assert!(old < 0.05, "trail at {old}");
        assert!(f.surface(220.0, 120.0).unwrap().subject > 0.9);
    }

    #[test]
    fn nobody_is_the_person_once_the_face_is_gone() {
        let d = sphere(320, 240);
        let mut f = DepthField::new();
        // a short spell without a face changes nothing
        for _ in 0..60 {
            f.update(&d, None, None, None, 1.0 / 30.0);
        }
        assert!(f.surface(160.0, 120.0).unwrap().subject > 0.9);
        // a long one fades the person out completely
        for _ in 0..90 {
            f.update(&d, None, None, None, 1.0 / 30.0);
        }
        assert!(f.surface(160.0, 120.0).unwrap().subject < 0.01);
        assert!(f.valid);
    }
}
