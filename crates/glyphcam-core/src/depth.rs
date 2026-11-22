//! Depth camera support: a raw depth image aligned to the colour frame
//! ([`DepthMap`]), and a cleaned-up, coarse 3D surface built from it each
//! frame ([`DepthField`]). The field fills holes (glasses, IR dropouts),
//! smooths the sensor noise, keeps a little temporal memory, and decides which
//! pixels belong to the person in front of the camera.

use crate::track::Face;

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
    raw: Vec<f32>,
    w: Vec<f32>,
    /// Surface coordinates in metres (arc length along the skin from the
    /// face centre); NaN where unknown. Built by [`DepthField::build_uv`].
    su: Vec<f32>,
    sv: Vec<f32>,
    tmp_a: Vec<f32>,
    tmp_b: Vec<f32>,
    /// Depth of the face (or nearest surface), metres.
    pub z_ref: f32,
    pub valid: bool,
}

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
        self.z.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Rebuild the field from a new depth frame. `face` (in source pixels)
    /// anchors which depth counts as "you".
    pub fn update(&mut self, d: &DepthMap, face: Option<&Face>, dt: f32) {
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
                raw: vec![0.0; n],
                w: vec![0.0; n],
                su: vec![f32::NAN; n],
                sv: vec![f32::NAN; n],
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

        // 2. reference depth: the face if we know where it is, else the
        // nearest sizeable surface
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
        let z_now = if zs.len() >= 8 {
            percentile(&mut zs, 0.5)
        } else {
            zs = self.raw.iter().copied().filter(|&v| v > 0.0).collect();
            if zs.len() < 30 {
                self.valid = false;
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
            if v > 0.0 && (v < near || v > far) {
                self.raw[i] = 0.0;
                self.w[i] = 0.0;
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

        // 5. temporal blend; jumps (you moved) reset immediately
        let a = (dt * 14.0).clamp(0.2, 1.0);
        for i in 0..n {
            let v = self.raw[i];
            if v <= 0.0 {
                self.z[i] = 0.0;
                self.subject[i] = (self.subject[i] - a).max(0.0);
                continue;
            }
            let p = self.z[i];
            self.z[i] = if p <= 0.0 || (v - p).abs() > 0.04 {
                v
            } else {
                p + (v - p) * a
            };
            // fade the far edge of the band so the cutout isn't a hard wall
            let edge = ((far - v) / 0.08).clamp(0.0, 1.0);
            self.subject[i] = if p <= 0.0 {
                edge
            } else {
                self.subject[i] + (edge - self.subject[i]) * a.max(0.5)
            };
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
            subject: self.subject[gy * self.gw + gx],
        })
    }

    /// Build surface coordinates: `u` is the arc length along each grid row
    /// (through x and depth) from the centre column, `v` the arc length down
    /// each column from the centre row. Laying glyphs out in (u, v) paints
    /// them onto the skin: they foreshorten where the surface turns away and
    /// stretch over the nose. `(cx, cy)` is the anchor in source pixels.
    pub fn build_uv(&mut self, cx: f32, cy: f32) {
        let (gw, gh) = (self.gw, self.gh);
        if !self.valid || gw == 0 {
            return;
        }
        let c0 = ((cx / CELL as f32) as usize).min(gw - 1);
        let r0 = ((cy / CELL as f32) as usize).min(gh - 1);
        let pt = |f: &Self, gx: usize, gy: usize| -> Option<[f32; 3]> {
            let z = f.z[gy * gw + gx];
            (z > 0.0).then(|| {
                f.unproject(
                    (gx * CELL) as f32 + CELL as f32 * 0.5,
                    (gy * CELL) as f32 + CELL as f32 * 0.5,
                    z,
                )
            })
        };
        // reference point: where the anchor ray meets the face plane
        let anchor = self.unproject(cx, cy, self.z_ref);
        // Step length between neighbours: the true 3D distance in the
        // integration direction (x/z for u, y/z for v); across a depth gap
        // fall back to the flat distance so coordinates stay continuous.
        let arc = |a: Option<[f32; 3]>, b: Option<[f32; 3]>, axis: usize, flat: f32| -> f32 {
            match (a, b) {
                (Some(a), Some(b)) if (a[2] - b[2]).abs() < 0.04 => {
                    let d = b[axis] - a[axis];
                    let dz = b[2] - a[2];
                    d.signum() * (d * d + dz * dz).sqrt()
                }
                _ => flat,
            }
        };
        let flat_x = CELL as f32 * self.z_ref / self.intr.map(|i| i.fx).unwrap_or(600.0);
        let flat_y = CELL as f32 * self.z_ref / self.intr.map(|i| i.fy).unwrap_or(600.0);
        for gy in 0..gh {
            let row = gy * gw;
            let start = pt(self, c0, gy).map(|p| p[0]).unwrap_or(
                self.unproject((c0 * CELL) as f32 + CELL as f32 * 0.5, 0.0, self.z_ref)[0],
            ) - anchor[0];
            self.su[row + c0] = start;
            for gx in c0 + 1..gw {
                let d = arc(pt(self, gx - 1, gy), pt(self, gx, gy), 0, flat_x);
                self.su[row + gx] = self.su[row + gx - 1] + d;
            }
            for gx in (0..c0).rev() {
                let d = arc(pt(self, gx + 1, gy), pt(self, gx, gy), 0, -flat_x);
                self.su[row + gx] = self.su[row + gx + 1] + d;
            }
        }
        for gx in 0..gw {
            let start = pt(self, gx, r0).map(|p| p[1]).unwrap_or(
                self.unproject(0.0, (r0 * CELL) as f32 + CELL as f32 * 0.5, self.z_ref)[1],
            ) - anchor[1];
            self.sv[r0 * gw + gx] = start;
            for gy in r0 + 1..gh {
                let d = arc(pt(self, gx, gy - 1), pt(self, gx, gy), 1, flat_y);
                self.sv[gy * gw + gx] = self.sv[(gy - 1) * gw + gx] + d;
            }
            for gy in (0..r0).rev() {
                let d = arc(pt(self, gx, gy + 1), pt(self, gx, gy), 1, -flat_y);
                self.sv[gy * gw + gx] = self.sv[(gy + 1) * gw + gx] + d;
            }
        }
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
                s += self.subject[i] * w;
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

    fn sphere(w: u32, h: u32) -> DepthMap {
        let intr = Intrinsics {
            fx: 600.0,
            fy: 600.0,
            ppx: w as f32 / 2.0,
            ppy: h as f32 / 2.0,
        };
        let scale = 0.000125;
        let mut data = vec![0u16; (w * h) as usize];
        let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, h as f32 * 0.3);
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

    #[test]
    fn field_finds_surface_and_normals() {
        let d = sphere(320, 240);
        let mut f = DepthField::new();
        f.update(&d, None, 1.0 / 30.0);
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
}
