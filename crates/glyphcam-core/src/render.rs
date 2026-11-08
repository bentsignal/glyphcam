//! Glyph renderer: samples the camera through the stabilized transform onto a
//! character grid, computes per-cell intensity (exposure, contrast, local
//! detail, 3D head shading, background treatment), then blits glyphs from the
//! atlas. No text layout happens per frame; the hot loop is a byte blend.

use crate::color::{Palette, Rgb};
use crate::glyphs::{Atlas, AtlasKey, LEVELS};
use crate::image::{Frame, RgbImage};
use crate::settings::{Background, ColorMode, Settings};
use crate::stabilize::FrameXf;
use crate::track::{Face, Mask};

/// Cell width / height ratio (monospace glyphs are roughly 0.6).
const CELL_ASPECT: f32 = 0.6;

#[inline]
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
fn hash2(a: u32, b: u32) -> u32 {
    hash(a.wrapping_mul(0x9E37_79B9) ^ hash(b))
}

#[inline]
fn hash01(a: u32) -> f32 {
    (hash(a) & 0xFFFF) as f32 / 65535.0
}

#[derive(Clone, Copy, Default)]
struct Drop {
    head: f32,
    speed: f32,
    len: f32,
}

#[derive(Clone, Copy, Default)]
struct Cell {
    rgb: Rgb,
    lum: f32,
    alpha: f32,
    shade: f32,
    in_head: bool,
}

/// 3D head proxy (rotated ellipsoid + nose/eye features) built from the
/// detector's keypoints. Gives depth and Lambert shading per source point.
struct HeadModel {
    c: [f32; 3],
    r: [f32; 3],
    /// Rotation matrix (row-major), local -> world.
    m: [[f32; 3]; 3],
    nose: [f32; 2],
    eyes: [[f32; 2]; 2],
    feat: f32,
}

impl HeadModel {
    fn new(f: &Face) -> Self {
        let roll = f.roll();
        let (yaw, pitch) = f.yaw_pitch();
        let (sr, cr) = roll.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        // R = Rz(roll) * Ry(yaw) * Rx(pitch)
        let rz = [[cr, -sr, 0.0], [sr, cr, 0.0], [0.0, 0.0, 1.0]];
        let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let rx = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
        let m = matmul(matmul(rz, ry), rx);
        let w = f.w * 1.3;
        let h = f.h * 1.6;
        HeadModel {
            c: [f.cx, f.cy - f.h * 0.1, 0.0],
            r: [w * 0.5, h * 0.5, w * 0.48],
            m,
            nose: f.kps[2],
            eyes: [f.kps[0], f.kps[1]],
            feat: f.w,
        }
    }

    /// Returns (depth 0..1, shade 0..1) where the head covers `(x, y)`.
    #[inline]
    fn eval(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let p = [x - self.c[0], y - self.c[1], 0.0];
        // local = R^T * p ; dir = R^T * (0,0,1)
        let m = &self.m;
        let pl = [
            (m[0][0] * p[0] + m[1][0] * p[1]) / self.r[0],
            (m[0][1] * p[0] + m[1][1] * p[1]) / self.r[1],
            (m[0][2] * p[0] + m[1][2] * p[1]) / self.r[2],
        ];
        let dl = [
            m[2][0] / self.r[0],
            m[2][1] / self.r[1],
            m[2][2] / self.r[2],
        ];
        let a = dl[0] * dl[0] + dl[1] * dl[1] + dl[2] * dl[2];
        let b = 2.0 * (pl[0] * dl[0] + pl[1] * dl[1] + pl[2] * dl[2]);
        let c = pl[0] * pl[0] + pl[1] * pl[1] + pl[2] * pl[2] - 1.0;
        let disc = b * b - 4.0 * a * c;
        if disc <= 0.0 {
            return None;
        }
        let t = (-b - disc.sqrt()) / (2.0 * a);
        let rmax = self.r[0].max(self.r[1]).max(self.r[2]);
        let mut depth = (-t / rmax).clamp(0.0, 1.0);
        // Surface normal (local gradient), rotated back to world.
        let hl = [pl[0] + t * dl[0], pl[1] + t * dl[1], pl[2] + t * dl[2]];
        let nl = [hl[0] / self.r[0], hl[1] / self.r[1], hl[2] / self.r[2]];
        let n = [
            m[0][0] * nl[0] + m[0][1] * nl[1] + m[0][2] * nl[2],
            m[1][0] * nl[0] + m[1][1] * nl[1] + m[1][2] * nl[2],
            m[2][0] * nl[0] + m[2][1] * nl[1] + m[2][2] * nl[2],
        ];
        let nlen = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        // Light from upper-left, toward the viewer (-z).
        let l = [-0.42, -0.5, -0.76];
        let mut shade = ((n[0] * l[0] + n[1] * l[1] + n[2] * l[2]) / nlen).clamp(0.0, 1.0);
        let g = |q: [f32; 2], s: f32| {
            let dx = x - q[0];
            let dy = y - q[1];
            (-(dx * dx + dy * dy) / (2.0 * s * s)).exp()
        };
        let nose = g(self.nose, self.feat * 0.11);
        depth = (depth + 0.22 * nose).min(1.0);
        shade = (shade + 0.15 * nose).min(1.0);
        let sockets = g(self.eyes[0], self.feat * 0.08) + g(self.eyes[1], self.feat * 0.08);
        depth = (depth - 0.1 * sockets).max(0.0);
        shade = (shade - 0.2 * sockets).max(0.0);
        Some((depth, shade))
    }
}

fn matmul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// What the renderer needs from tracking for one frame.
pub struct TrackView<'a> {
    pub face: Option<&'a Face>,
    pub mask: Option<&'a Mask>,
}

pub struct Renderer {
    atlas: Option<(AtlasKey, Atlas)>,
    drops: Vec<Drop>,
    exposure: (f32, f32),
    cells: Vec<Cell>,
    inten: Vec<f32>,
    tmp: Vec<f32>,
    glow: Vec<f32>,
    glyph_idx: Vec<u16>,
    fg: Vec<[u8; 3]>,
    bg: Vec<[u8; 3]>,
    rng: u32,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct GridInfo {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: usize,
    pub cell_h: usize,
}

pub fn grid_for(s: &Settings) -> GridInfo {
    let cw = ((s.out_width as f32 / s.columns.max(16) as f32).floor() as usize).max(4);
    let ch = ((cw as f32 / CELL_ASPECT).round() as usize).max(6);
    GridInfo {
        cols: s.out_width as usize / cw,
        rows: s.out_height as usize / ch,
        cell_w: cw,
        cell_h: ch,
    }
}

impl Renderer {
    pub fn new() -> Self {
        Renderer {
            atlas: None,
            drops: Vec::new(),
            exposure: (0.0, 1.0),
            cells: Vec::new(),
            inten: Vec::new(),
            tmp: Vec::new(),
            glow: Vec::new(),
            glyph_idx: Vec::new(),
            fg: Vec::new(),
            bg: Vec::new(),
            rng: 0x1234_5678,
        }
    }

    fn atlas_for(&mut self, key: AtlasKey) -> &Atlas {
        if self.atlas.as_ref().map(|(k, _)| *k != key).unwrap_or(true) {
            self.atlas = Some((key, Atlas::build(key)));
        }
        &self.atlas.as_ref().unwrap().1
    }

    fn rand(&mut self) -> f32 {
        self.rng = hash(self.rng.wrapping_add(0x9E37_79B9));
        (self.rng & 0xFFFF) as f32 / 65535.0
    }

    /// Render one frame. `time` is seconds since start, `dt` the frame delta.
    pub fn render(
        &mut self,
        src: Option<&RgbImage>,
        xf: &FrameXf,
        track: &TrackView,
        s: &Settings,
        time: f64,
        dt: f32,
        out: &mut Frame,
    ) {
        out.resize(s.out_width, s.out_height);
        let g = grid_for(s);
        let (cols, rows, cw, ch) = (g.cols, g.rows, g.cell_w, g.cell_h);
        let n = cols * rows;
        let ox = (s.out_width as usize - cols * cw) / 2;
        let oy = (s.out_height as usize - rows * ch) / 2;
        let key = AtlasKey {
            cell_w: cw,
            cell_h: ch,
            scripts: s.scripts,
            mirror_katakana: s.mirror_katakana,
        };
        self.atlas_for(key);

        self.cells.resize(n, Cell::default());
        self.inten.resize(n, 0.0);
        self.tmp.resize(n, 0.0);
        self.glow.resize(n, 0.0);
        self.glyph_idx.resize(n, 0);
        self.fg.resize(n, [0; 3]);
        self.bg.resize(n, [0; 3]);

        // ---- 1. sample camera, mask and head model per cell ----
        let head = track.face.map(HeadModel::new);
        let (sw, sh) = src
            .map(|i| (i.width as f32, i.height as f32))
            .unwrap_or((1.0, 1.0));
        let sub = [
            [-0.25f32, -0.25],
            [0.25, -0.25],
            [-0.25, 0.25],
            [0.25, 0.25],
        ];
        for gy in 0..rows {
            for gx in 0..cols {
                let cx = ox as f32 + (gx as f32 + 0.5) * cw as f32;
                let cy = oy as f32 + (gy as f32 + 0.5) * ch as f32;
                let mut acc = [0.0f32; 3];
                let mut hits = 0.0;
                if let Some(img) = src {
                    for o in sub {
                        let p = xf.map(cx + o[0] * cw as f32, cy + o[1] * ch as f32);
                        if let Some(v) = img.sample(p[0], p[1]) {
                            acc[0] += v[0];
                            acc[1] += v[1];
                            acc[2] += v[2];
                            hits += 1.0;
                        }
                    }
                }
                let cell = &mut self.cells[gy * cols + gx];
                if hits > 0.0 {
                    let k = 1.0 / 4.0; // out-of-frame samples count as black
                    cell.rgb = [acc[0] * k, acc[1] * k, acc[2] * k];
                } else {
                    cell.rgb = [0.0; 3];
                }
                cell.lum =
                    (0.299 * cell.rgb[0] + 0.587 * cell.rgb[1] + 0.114 * cell.rgb[2]) / 255.0;
                let p = xf.map(cx, cy);
                let in_frame =
                    src.is_some() && p[0] >= 0.0 && p[1] >= 0.0 && p[0] < sw && p[1] < sh;
                cell.alpha = match (track.mask, in_frame) {
                    (Some(m), true) => m.sample(p[0] / sw, p[1] / sh),
                    (Some(_), false) => 0.0,
                    (None, _) => {
                        if in_frame {
                            1.0
                        } else {
                            0.0
                        }
                    }
                };
                match head.as_ref().and_then(|h| h.eval(p[0], p[1])) {
                    Some((depth, shade)) => {
                        cell.in_head = true;
                        cell.shade = 0.55 * depth + 0.45 * shade;
                        cell.alpha = cell.alpha.max(depth.min(1.0) * 1.5).min(1.0);
                    }
                    None => {
                        cell.in_head = false;
                        cell.shade = if track.mask.is_some() {
                            0.45 * cell.alpha + 0.25
                        } else {
                            0.5
                        };
                    }
                }
            }
        }

        // ---- 2. auto exposure over the face (or person, or everything) ----
        if s.auto_exposure {
            let mut hist = [0u32; 64];
            let mut total = 0u32;
            let any_head = self.cells.iter().any(|c| c.in_head);
            for c in &self.cells {
                let use_it = if any_head { c.in_head } else { c.alpha > 0.5 };
                if use_it {
                    hist[((c.lum * 63.0) as usize).min(63)] += 1;
                    total += 1;
                }
            }
            if total > 16 {
                let pct = |p: f32| {
                    let target = (total as f32 * p) as u32;
                    let mut acc = 0;
                    for (i, &h) in hist.iter().enumerate() {
                        acc += h;
                        if acc >= target {
                            return i as f32 / 63.0;
                        }
                    }
                    1.0
                };
                let lo = pct(0.03);
                let hi = pct(0.97).max(lo + 0.15);
                let k = (dt * 2.5).clamp(0.0, 1.0);
                self.exposure.0 += (lo - self.exposure.0) * k;
                self.exposure.1 += (hi - self.exposure.1) * k;
            }
        } else {
            self.exposure = (0.0, 1.0);
        }
        let (lo, hi) = self.exposure;
        let inv = 1.0 / (hi - lo).max(0.05);
        for (i, c) in self.cells.iter().enumerate() {
            let mut v = ((c.lum - lo) * inv).clamp(0.0, 1.2);
            v = (v - 0.5) * s.contrast + 0.5 + s.brightness;
            v = v.clamp(0.0, 1.0).powf(s.gamma);
            // 3D head / depth shading
            v *= 1.0 + ((0.25 + 0.95 * c.shade) - 1.0) * s.depth;
            self.inten[i] = v;
        }

        // ---- 3. local detail (unsharp mask on the grid) ----
        if s.edge > 0.0 {
            box_blur(&self.inten, &mut self.tmp, cols, rows, 1);
            for i in 0..n {
                self.inten[i] =
                    (self.inten[i] + (self.inten[i] - self.tmp[i]) * s.edge * 1.6).clamp(0.0, 1.0);
            }
        }

        // ---- 4. background treatment ----
        let mut rain_head = vec![
            false;
            if s.background == Background::Rain {
                n
            } else {
                0
            }
        ];
        let mut is_rain = vec![false; rain_head.len()];
        match s.background {
            Background::Show => {}
            Background::Dim => {
                for (i, c) in self.cells.iter().enumerate() {
                    self.inten[i] *= s.bg_dim + (1.0 - s.bg_dim) * c.alpha;
                }
            }
            Background::Hide => {
                for (i, c) in self.cells.iter().enumerate() {
                    self.inten[i] *= smoothstep(0.15, 0.6, c.alpha);
                }
            }
            Background::Rain => {
                if self.drops.len() != cols {
                    self.drops = (0..cols)
                        .map(|_| Drop {
                            head: -(self.rand() * rows as f32 * 2.0),
                            speed: 0.0,
                            len: 0.0,
                        })
                        .collect();
                    for d in 0..cols {
                        self.respawn(d, rows, false);
                    }
                }
                for x in 0..cols {
                    let d = &mut self.drops[x];
                    d.head += d.speed * s.rain_speed * dt;
                    if d.head - d.len > rows as f32 {
                        self.respawn(x, rows, true);
                    }
                    let d = self.drops[x];
                    for y in 0..rows {
                        let i = y * cols + x;
                        let behind = d.head - y as f32;
                        let r = if behind >= 0.0 && behind < d.len {
                            let t = 1.0 - behind / d.len;
                            t * t * 0.8
                        } else {
                            0.0
                        };
                        let a = smoothstep(0.15, 0.6, self.cells[i].alpha);
                        let person = self.inten[i] * a;
                        let rain = r * (1.0 - a);
                        if rain > person {
                            self.inten[i] = rain;
                            is_rain[i] = true;
                            rain_head[i] = behind >= 0.0 && behind < 1.0;
                        } else {
                            self.inten[i] = person;
                        }
                    }
                }
            }
        }

        // ---- 5. glow ----
        if s.glow > 0.0 {
            box_blur(&self.inten, &mut self.tmp, cols, rows, 1);
            box_blur(&self.tmp, &mut self.glow, cols, rows, 1);
        }

        // ---- 6. pick glyphs and colors ----
        let palette = Palette::new(s.color_mode, s.custom_fg, s.custom_bg, s.saturation);
        let rain_palette = Palette::new(
            if s.color_mode == ColorMode::TrueColor {
                ColorMode::Matrix
            } else {
                s.color_mode
            },
            s.custom_fg,
            s.custom_bg,
            s.saturation,
        );
        let bgc = palette.bg;
        let atlas = &self.atlas.as_ref().unwrap().1;
        let t = time as f32;
        for gy in 0..rows {
            let yn = gy as f32 / rows.max(1) as f32;
            for gx in 0..cols {
                let i = gy * cols + gx;
                let v = self.inten[i];
                let rain = !is_rain.is_empty() && is_rain[i];
                let level = ((v * (LEVELS - 1) as f32).round() as usize).min(LEVELS - 1);
                let level = if v < 0.045 { 0 } else { level.max(1) };
                let cands = &atlas.levels[level];
                let cell_id = (gy * 4099 + gx) as u32;
                let churn = s.churn * if rain { 4.0 } else { 1.0 };
                let slot =
                    (t * churn * (0.4 + hash01(cell_id)) + hash01(cell_id ^ 0xABCD) * 97.0) as u32;
                self.glyph_idx[i] = cands[(hash2(cell_id, slot) as usize) % cands.len()];
                let pal = if rain { &rain_palette } else { &palette };
                let mut fg = pal.color(v, self.cells[i].rgb, yn);
                if rain && rain_head[i] {
                    fg = pal.color(1.0, [255.0; 3], yn);
                }
                let mut bgv = bgc;
                if s.glow > 0.0 {
                    let gv = self.glow[i];
                    let gc = pal.color(gv, self.cells[i].rgb, yn);
                    let k = s.glow * 0.4 * gv * gv;
                    bgv = [bgv[0] + gc[0] * k, bgv[1] + gc[1] * k, bgv[2] + gc[2] * k];
                }
                self.fg[i] = [
                    fg[0].min(255.0) as u8,
                    fg[1].min(255.0) as u8,
                    fg[2].min(255.0) as u8,
                ];
                self.bg[i] = [
                    bgv[0].min(255.0) as u8,
                    bgv[1].min(255.0) as u8,
                    bgv[2].min(255.0) as u8,
                ];
            }
        }

        // ---- 7. blit ----
        let w = s.out_width as usize;
        let bg8 = [bgc[2] as u8, bgc[1] as u8, bgc[0] as u8, 255u8];
        let data = &mut out.data;
        // margins
        for y in 0..s.out_height as usize {
            let inside_y = y >= oy && y < oy + rows * ch;
            let row = &mut data[y * w * 4..(y + 1) * w * 4];
            if !inside_y {
                for px in row.chunks_exact_mut(4) {
                    px.copy_from_slice(&bg8);
                }
                continue;
            }
            for px in row[..ox * 4].chunks_exact_mut(4) {
                px.copy_from_slice(&bg8);
            }
            for px in row[(ox + cols * cw) * 4..].chunks_exact_mut(4) {
                px.copy_from_slice(&bg8);
            }
        }
        for gy in 0..rows {
            for py in 0..ch {
                let y = oy + gy * ch + py;
                let row = &mut data[(y * w + ox) * 4..(y * w + ox + cols * cw) * 4];
                for gx in 0..cols {
                    let i = gy * cols + gx;
                    let bmp = &atlas.glyph(self.glyph_idx[i])[py * cw..(py + 1) * cw];
                    let f = self.fg[i];
                    let b = self.bg[i];
                    let (fb, fgc, fr) = (f[2] as i32, f[1] as i32, f[0] as i32);
                    let (bb, bgg, br) = (b[2] as i32, b[1] as i32, b[0] as i32);
                    let dst = &mut row[gx * cw * 4..(gx + 1) * cw * 4];
                    for (px, &a) in dst.chunks_exact_mut(4).zip(bmp) {
                        let a = a as i32;
                        px[0] = (bb + (((fb - bb) * a + 127) / 255)) as u8;
                        px[1] = (bgg + (((fgc - bgg) * a + 127) / 255)) as u8;
                        px[2] = (br + (((fr - br) * a + 127) / 255)) as u8;
                        px[3] = 255;
                    }
                }
            }
        }
    }

    fn respawn(&mut self, x: usize, rows: usize, from_top: bool) {
        let speed = 6.0 + self.rand() * 14.0;
        let len = 5.0 + self.rand() * rows as f32 * 0.7;
        let head = if from_top {
            -(self.rand() * rows as f32 * 0.8)
        } else {
            self.rand() * rows as f32 * 1.5 - rows as f32 * 0.5
        };
        self.drops[x] = Drop { head, speed, len };
    }
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Separable box blur on a `w x h` grid with clamped edges.
fn box_blur(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0.0f32; w * h];
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for k in 0..=2 * r {
                let xx = (x + k).saturating_sub(r).min(w - 1);
                acc += src[y * w + xx];
            }
            tmp[y * w + x] = acc * norm;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for k in 0..=2 * r {
                let yy = (y + k).saturating_sub(r).min(h - 1);
                acc += tmp[yy * w + x];
            }
            dst[y * w + x] = acc * norm;
        }
    }
}
