//! The output framing: the whole camera image scaled to fill the output
//! (cropping the longer side), optionally mirrored.

/// Maps output pixel coordinates to source pixel coordinates:
/// `src = center + A * (p - out_center)`.
#[derive(Clone, Copy, Debug)]
pub struct FrameXf {
    pub center: [f32; 2],
    pub out_center: [f32; 2],
    /// Row-major 2x2 matrix (includes zoom and mirroring).
    pub a: [f32; 4],
}

impl FrameXf {
    /// Fill `out` with the whole `src` image, centred; `mirror` flips it
    /// horizontally (selfie view).
    pub fn fill(src: (u32, u32), out: (u32, u32), mirror: bool) -> Self {
        let (sw, sh) = (src.0 as f32, src.1 as f32);
        let (ow, oh) = (out.0 as f32, out.1 as f32);
        let scale = (sw / ow).min(sh / oh);
        let mx = if mirror { -1.0 } else { 1.0 };
        FrameXf {
            center: [sw * 0.5, sh * 0.5],
            out_center: [ow * 0.5, oh * 0.5],
            a: [scale * mx, 0.0, 0.0, scale],
        }
    }

    #[inline]
    pub fn map(&self, x: f32, y: f32) -> [f32; 2] {
        let dx = x - self.out_center[0];
        let dy = y - self.out_center[1];
        [
            self.center[0] + self.a[0] * dx + self.a[1] * dy,
            self.center[1] + self.a[2] * dx + self.a[3] * dy,
        ]
    }

    /// Inverse mapping: source pixel to output pixel.
    pub fn unmap(&self, sx: f32, sy: f32) -> [f32; 2] {
        let [a, b, c, d] = self.a;
        let det = a * d - b * c;
        let dx = sx - self.center[0];
        let dy = sy - self.center[1];
        [
            self.out_center[0] + (d * dx - b * dy) / det,
            self.out_center[1] + (-c * dx + a * dy) / det,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_covers_output_and_round_trips() {
        let xf = FrameXf::fill((640, 480), (1280, 720), false);
        // 4:3 into 16:9: full width, top and bottom cropped
        let tl = xf.map(0.0, 0.0);
        let br = xf.map(1280.0, 720.0);
        assert!((tl[0] - 0.0).abs() < 1e-3 && (br[0] - 640.0).abs() < 1e-3);
        assert!(tl[1] > 0.0 && br[1] < 480.0);
        let m = FrameXf::fill((640, 480), (1280, 720), true);
        assert!((m.map(0.0, 360.0)[0] - 640.0).abs() < 1e-3);
        let p = m.unmap(100.0, 200.0);
        let q = m.map(p[0], p[1]);
        assert!((q[0] - 100.0).abs() < 1e-3 && (q[1] - 200.0).abs() < 1e-3);
    }
}
