//! Minimal image containers used throughout the pipeline.

/// Packed 8-bit RGB image (row-major, no padding).
#[derive(Clone, Debug, Default)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width * height * 3) as usize],
        }
    }

    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        if data.len() == (width * height * 3) as usize {
            Some(Self {
                width,
                height,
                data,
            })
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Bilinear sample at continuous pixel coordinates (pixel centers at +0.5).
    /// Returns `None` outside the image.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        let fx = x - 0.5;
        let fy = y - 0.5;
        if fx < -0.5 || fy < -0.5 || fx > self.width as f32 - 0.5 || fy > self.height as f32 - 0.5 {
            return None;
        }
        let x0 = fx.floor().max(0.0) as u32;
        let y0 = fy.floor().max(0.0) as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = (fx - x0 as f32).clamp(0.0, 1.0);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        let w = self.width as usize;
        let d = &self.data;
        let i00 = (y0 as usize * w + x0 as usize) * 3;
        let i10 = (y0 as usize * w + x1 as usize) * 3;
        let i01 = (y1 as usize * w + x0 as usize) * 3;
        let i11 = (y1 as usize * w + x1 as usize) * 3;
        let mut out = [0.0f32; 3];
        for c in 0..3 {
            let a = d[i00 + c] as f32 + (d[i10 + c] as f32 - d[i00 + c] as f32) * tx;
            let b = d[i01 + c] as f32 + (d[i11 + c] as f32 - d[i01 + c] as f32) * tx;
            out[c] = a + (b - a) * ty;
        }
        Some(out)
    }

    /// Area-average downscale into a `dw x dh` region of `dst` (RGB f32, 0..1),
    /// with the source letterboxed to preserve aspect ratio. Returns
    /// `(scale, pad_x, pad_y)` such that `dst = src * scale + pad`.
    pub fn letterbox_into(
        &self,
        dst: &mut [f32],
        dw: usize,
        dh: usize,
        map: impl Fn(f32) -> f32,
    ) -> (f32, f32, f32) {
        let scale = (dw as f32 / self.width as f32).min(dh as f32 / self.height as f32);
        let sw = self.width as f32 * scale;
        let sh = self.height as f32 * scale;
        let pad_x = (dw as f32 - sw) * 0.5;
        let pad_y = (dh as f32 - sh) * 0.5;
        let pad_val = map(0.0);
        let inv = 1.0 / scale;
        for y in 0..dh {
            for x in 0..dw {
                let o = (y * dw + x) * 3;
                let sx0 = (x as f32 - pad_x) * inv;
                let sy0 = (y as f32 - pad_y) * inv;
                let sx1 = sx0 + inv;
                let sy1 = sy0 + inv;
                if sx1 <= 0.0 || sy1 <= 0.0 || sx0 >= self.width as f32 || sy0 >= self.height as f32
                {
                    dst[o] = pad_val;
                    dst[o + 1] = pad_val;
                    dst[o + 2] = pad_val;
                    continue;
                }
                let v = self.box_average(sx0, sy0, sx1, sy1);
                dst[o] = map(v[0]);
                dst[o + 1] = map(v[1]);
                dst[o + 2] = map(v[2]);
            }
        }
        (scale, pad_x, pad_y)
    }

    /// Stretch-resize into `dw x dh` (planar CHW, 0..1).
    pub fn resize_chw(&self, dst: &mut [f32], dw: usize, dh: usize) {
        let sx = self.width as f32 / dw as f32;
        let sy = self.height as f32 / dh as f32;
        let plane = dw * dh;
        for y in 0..dh {
            for x in 0..dw {
                let v = self.box_average(
                    x as f32 * sx,
                    y as f32 * sy,
                    (x + 1) as f32 * sx,
                    (y + 1) as f32 * sy,
                );
                let i = y * dw + x;
                dst[i] = v[0] / 255.0;
                dst[plane + i] = v[1] / 255.0;
                dst[2 * plane + i] = v[2] / 255.0;
            }
        }
    }

    /// Average of the integer pixels overlapped by a source-space box (cheap,
    /// samples at most a 4x4 grid inside the box).
    fn box_average(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> [f32; 3] {
        let xa = x0.max(0.0) as u32;
        let ya = y0.max(0.0) as u32;
        let xb = (x1.ceil() as u32).clamp(xa + 1, self.width);
        let yb = (y1.ceil() as u32).clamp(ya + 1, self.height);
        let stepx = ((xb - xa) / 4).max(1);
        let stepy = ((yb - ya) / 4).max(1);
        let mut acc = [0u32; 3];
        let mut n = 0u32;
        let mut y = ya;
        while y < yb {
            let mut x = xa;
            while x < xb {
                let p = self.get(x.min(self.width - 1), y.min(self.height - 1));
                acc[0] += p[0] as u32;
                acc[1] += p[1] as u32;
                acc[2] += p[2] as u32;
                n += 1;
                x += stepx;
            }
            y += stepy;
        }
        let n = n.max(1) as f32;
        [acc[0] as f32 / n, acc[1] as f32 / n, acc[2] as f32 / n]
    }
}

/// Packed 8-bit BGRA output frame, top-down rows, `stride == width * 4`.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width * height * 4) as usize],
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width != width || self.height != height {
            *self = Frame::new(width, height);
        }
    }

    /// Convert to packed RGB (for JPEG encoding / PNG output).
    pub fn to_rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity((self.width * self.height * 3) as usize);
        for px in self.data.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        out
    }

    /// Nearest-neighbour downscale to RGBA, used for UI previews.
    pub fn downscale_rgba(&self, max_w: u32) -> (u32, u32, Vec<u8>) {
        let f = (self.width as f32 / max_w as f32).max(1.0);
        let w = (self.width as f32 / f) as u32;
        let h = (self.height as f32 / f) as u32;
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let sy = ((y as f32 + 0.5) * f) as u32;
            let srow = (sy.min(self.height - 1) * self.width) as usize * 4;
            for x in 0..w {
                let sx = ((x as f32 + 0.5) * f) as u32;
                let s = srow + sx.min(self.width - 1) as usize * 4;
                let d = ((y * w + x) * 4) as usize;
                out[d] = self.data[s + 2];
                out[d + 1] = self.data[s + 1];
                out[d + 2] = self.data[s];
                out[d + 3] = 255;
            }
        }
        (w, h, out)
    }
}
