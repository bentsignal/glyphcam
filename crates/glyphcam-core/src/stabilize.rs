//! View-model style face stabilization.
//!
//! A virtual "rig" (crop center, zoom, rotation) chases the face with
//! damped springs. Because the rig lags and overshoots, the face appears to
//! sway around its rest position the way a first-person weapon sways when
//! you move the mouse. The offset between face and rig is hard-clamped so the
//! face can never leave the frame.

use crate::settings::Settings;
use crate::track::Face;

/// Maps output pixel coordinates to source pixel coordinates:
/// `src = center + A * (p - out_center)`.
#[derive(Clone, Copy, Debug)]
pub struct FrameXf {
    pub center: [f32; 2],
    pub out_center: [f32; 2],
    /// Row-major 2x2 matrix (includes zoom, rotation and mirroring).
    pub a: [f32; 4],
}

impl FrameXf {
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

    fn build(
        center: [f32; 2],
        out_w: f32,
        out_h: f32,
        scale: f32,
        angle: f32,
        mirror: bool,
    ) -> Self {
        let (s, c) = angle.sin_cos();
        let mx = if mirror { -1.0 } else { 1.0 };
        // A = scale * R(angle) * diag(mx, 1)
        let a = [scale * c * mx, -scale * s, scale * s * mx, scale * c];
        FrameXf {
            center,
            out_center: [out_w * 0.5, out_h * 0.5],
            a,
        }
    }
}

/// One-Euro filter (Casiez et al.) for jitter-free yet responsive tracking.
#[derive(Clone, Debug)]
struct OneEuro {
    min_cutoff: f32,
    beta: f32,
    x: Option<f32>,
    dx: f32,
}

impl OneEuro {
    fn new(min_cutoff: f32, beta: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            x: None,
            dx: 0.0,
        }
    }
    fn alpha(cutoff: f32, dt: f32) -> f32 {
        let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        1.0 / (1.0 + tau / dt)
    }
    fn filter(&mut self, v: f32, dt: f32) -> f32 {
        let dt = dt.max(1e-3);
        match self.x {
            None => {
                self.x = Some(v);
                v
            }
            Some(prev) => {
                let d = (v - prev) / dt;
                self.dx += (d - self.dx) * Self::alpha(1.0, dt);
                let cutoff = self.min_cutoff + self.beta * self.dx.abs();
                let x = prev + (v - prev) * Self::alpha(cutoff, dt);
                self.x = Some(x);
                x
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FaceObs {
    pub cx: f32,
    pub cy: f32,
    /// Face height in source pixels.
    pub size: f32,
    pub roll: f32,
}

impl From<&Face> for FaceObs {
    fn from(f: &Face) -> Self {
        // BlazeFace boxes are tight around eyes/mouth; the visual head is
        // larger and centered a little higher.
        FaceObs {
            cx: f.cx,
            cy: f.cy - f.h * 0.08,
            size: f.h * 1.45,
            roll: f.roll(),
        }
    }
}

pub struct Stabilizer {
    c: [f32; 2],
    v: [f32; 2],
    scale: f32,
    scale_v: f32,
    angle: f32,
    angle_v: f32,
    time: f32,
    initialized: bool,
    fx: OneEuro,
    fy: OneEuro,
    fs: OneEuro,
    fr: OneEuro,
    last_obs: Option<FaceObs>,
    since_face: f32,
    src_size: (u32, u32),
    /// Current offset of the face from its rest position, in output pixels
    /// (for diagnostics / tests).
    pub last_offset: [f32; 2],
}

impl Default for Stabilizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Integrate `x'' = k (target - x) - d x'` with semi-implicit Euler.
#[inline]
fn spring(x: &mut f32, v: &mut f32, target: f32, omega: f32, zeta: f32, dt: f32) {
    let k = omega * omega;
    let d = 2.0 * zeta * omega;
    *v += (k * (target - *x) - d * *v) * dt;
    *x += *v * dt;
}

impl Stabilizer {
    pub fn new() -> Self {
        Self {
            c: [0.0; 2],
            v: [0.0; 2],
            scale: 1.0,
            scale_v: 0.0,
            angle: 0.0,
            angle_v: 0.0,
            time: 0.0,
            initialized: false,
            fx: OneEuro::new(1.2, 0.015),
            fy: OneEuro::new(1.2, 0.015),
            fs: OneEuro::new(0.6, 0.005),
            fr: OneEuro::new(0.8, 0.3),
            last_obs: None,
            since_face: f32::MAX,
            src_size: (0, 0),
            last_offset: [0.0; 2],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Advance by `dt` seconds and return the transform for this frame.
    pub fn update(
        &mut self,
        dt: f32,
        face: Option<FaceObs>,
        src: (u32, u32),
        out: (u32, u32),
        s: &Settings,
    ) -> FrameXf {
        let dt = dt.clamp(0.0, 0.25);
        self.time += dt;
        let (sw, sh) = (src.0 as f32, src.1 as f32);
        let (ow, oh) = (out.0 as f32, out.1 as f32);
        if self.src_size != src {
            self.src_size = src;
            self.initialized = false;
        }
        // Scale that fills the output with the whole camera image.
        let fill_scale = (sw / ow).min(sh / oh);

        let face = if s.stabilize { face } else { None };
        let obs = face.map(|f| FaceObs {
            cx: self.fx.filter(f.cx, dt.max(1e-3)),
            cy: self.fy.filter(f.cy, dt.max(1e-3)),
            size: self.fs.filter(f.size, dt.max(1e-3)),
            roll: self.fr.filter(f.roll, dt.max(1e-3)),
        });
        if obs.is_some() {
            self.since_face = 0.0;
            self.last_obs = obs;
        } else {
            self.since_face += dt;
        }
        // Hold the last position briefly when tracking drops out.
        let tracking = s.stabilize && self.since_face < 1.2;
        let target_obs = if tracking { self.last_obs } else { None };

        let mirror = s.mirror;
        let (target_scale, target_angle, rest) = match target_obs {
            Some(o) => {
                let ts = (o.size / (s.face_size * oh)).clamp(fill_scale * 0.12, fill_scale * 2.2);
                let ta = if mirror { -o.roll } else { o.roll } * s.roll_lock;
                (ts, ta, [ow * 0.5, oh * s.vertical])
            }
            None => (fill_scale, 0.0, [ow * 0.5, oh * 0.5]),
        };

        // Idle bob: a slow figure-eight drift like a view model at rest.
        let bob = s.idle_bob * oh * 0.012;
        let rest = [
            rest[0] + bob * (self.time * 0.9).sin(),
            rest[1] + bob * 0.6 * (self.time * 1.8).sin(),
        ];

        let sway = s.sway.clamp(0.0, 1.0);
        let omega = (30.0f32.ln() + (3.2f32.ln() - 30.0f32.ln()) * sway).exp();
        let zeta = 1.0 + (0.32 - 1.0) * sway;

        if !self.initialized {
            self.scale = target_scale;
            self.angle = target_angle;
            let xf = FrameXf::build([0.0, 0.0], ow, oh, self.scale, self.angle, mirror);
            let fc = target_obs
                .map(|o| [o.cx, o.cy])
                .unwrap_or([sw * 0.5, sh * 0.5]);
            // center such that face lands at `rest`
            let off = [rest[0] - ow * 0.5, rest[1] - oh * 0.5];
            self.c = [
                fc[0] - (xf.a[0] * off[0] + xf.a[1] * off[1]),
                fc[1] - (xf.a[2] * off[0] + xf.a[3] * off[1]),
            ];
            self.v = [0.0; 2];
            self.initialized = true;
        }

        // Sub-step for stability with stiff springs.
        let steps = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        let zoom_omega = if tracking { 5.0 } else { 2.0 };
        for _ in 0..steps {
            spring(
                &mut self.scale,
                &mut self.scale_v,
                target_scale,
                zoom_omega,
                1.0,
                h,
            );
            // Tilt: lean into horizontal motion (in output-space units).
            let vel_out = self.v[0] / self.scale.max(1e-3) / ow;
            let lean = if mirror { vel_out } else { -vel_out };
            let tilt_target = target_angle + (lean * 0.9 * s.tilt).clamp(-0.18, 0.18);
            spring(
                &mut self.angle,
                &mut self.angle_v,
                tilt_target,
                9.0,
                0.55,
                h,
            );

            let xf = FrameXf::build([0.0, 0.0], ow, oh, self.scale, self.angle, mirror);
            let off = [rest[0] - ow * 0.5, rest[1] - oh * 0.5];
            let fc = target_obs
                .map(|o| [o.cx, o.cy])
                .unwrap_or([sw * 0.5, sh * 0.5]);
            let tc = [
                fc[0] - (xf.a[0] * off[0] + xf.a[1] * off[1]),
                fc[1] - (xf.a[2] * off[0] + xf.a[3] * off[1]),
            ];
            let (om, ze) = if tracking { (omega, zeta) } else { (2.5, 1.0) };
            for k in 0..2 {
                spring(&mut self.c[k], &mut self.v[k], tc[k], om, ze, h);
            }
        }

        let xf = FrameXf::build(self.c, ow, oh, self.scale, self.angle, mirror);

        // Hard clamp: keep the face center within a safe box around rest.
        if let Some(o) = target_obs {
            let p = xf.unmap(o.cx, o.cy);
            let face_half = (o.size / self.scale) * 0.5;
            let max_x = (ow * 0.5 - face_half * 0.8).max(0.0).min(ow * 0.22);
            let max_y_up = (rest[1] - face_half * 0.9).max(0.0).min(oh * 0.2);
            let max_y_down = (oh - rest[1] - face_half * 0.9).max(0.0).min(oh * 0.2);
            let dx = p[0] - rest[0];
            let dy = p[1] - rest[1];
            let cx = dx.clamp(-max_x, max_x);
            let cy = dy.clamp(-max_y_up, max_y_down);
            if cx != dx || cy != dy {
                // Shift the rig so the face sits on the boundary, and kill the
                // velocity component that pushed it out.
                let shift_out = [dx - cx, dy - cy];
                let shift_src = [
                    xf.a[0] * shift_out[0] + xf.a[1] * shift_out[1],
                    xf.a[2] * shift_out[0] + xf.a[3] * shift_out[1],
                ];
                self.c[0] += shift_src[0];
                self.c[1] += shift_src[1];
                let len = (shift_src[0].powi(2) + shift_src[1].powi(2))
                    .sqrt()
                    .max(1e-6);
                let n = [shift_src[0] / len, shift_src[1] / len];
                let vn = self.v[0] * n[0] + self.v[1] * n[1];
                if vn > 0.0 {
                    self.v[0] -= vn * n[0];
                    self.v[1] -= vn * n[1];
                }
            }
            let xf2 = FrameXf::build(self.c, ow, oh, self.scale, self.angle, mirror);
            let p2 = xf2.unmap(o.cx, o.cy);
            self.last_offset = [p2[0] - rest[0], p2[1] - rest[1]];
            return xf2;
        }
        self.last_offset = [0.0; 2];
        xf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            idle_bob: 0.0,
            ..Settings::default()
        }
    }

    #[test]
    fn face_settles_at_rest_position() {
        let s = settings();
        let mut st = Stabilizer::new();
        let obs = FaceObs {
            cx: 200.0,
            cy: 300.0,
            size: 150.0,
            roll: 0.0,
        };
        let mut xf = st.update(1.0 / 30.0, Some(obs), (640, 480), (1280, 720), &s);
        for _ in 0..120 {
            xf = st.update(1.0 / 30.0, Some(obs), (640, 480), (1280, 720), &s);
        }
        let p = xf.unmap(obs.cx, obs.cy);
        assert!((p[0] - 640.0).abs() < 2.0, "{p:?}");
        assert!((p[1] - 720.0 * s.vertical).abs() < 2.0, "{p:?}");
    }

    #[test]
    fn face_never_leaves_frame_under_violent_motion() {
        let mut s = settings();
        s.sway = 1.0;
        let mut st = Stabilizer::new();
        for i in 0..600 {
            // Teleport the face between corners every 10 frames.
            let corner = (i / 10) % 4;
            let (x, y) = [(60.0, 60.0), (580.0, 60.0), (580.0, 420.0), (60.0, 420.0)][corner];
            let obs = FaceObs {
                cx: x,
                cy: y,
                size: 120.0,
                roll: 0.0,
            };
            let xf = st.update(1.0 / 30.0, Some(obs), (640, 480), (1280, 720), &s);
            let p = xf.unmap(x, y);
            assert!(
                p[0] > 0.0 && p[0] < 1280.0 && p[1] > 0.0 && p[1] < 720.0,
                "frame {i}: {p:?}"
            );
        }
    }

    #[test]
    fn sway_lags_behind_motion() {
        let s = settings();
        let mut st = Stabilizer::new();
        let mut x = 320.0;
        let mut max_off: f32 = 0.0;
        for _ in 0..30 {
            st.update(
                1.0 / 30.0,
                Some(FaceObs {
                    cx: x,
                    cy: 240.0,
                    size: 120.0,
                    roll: 0.0,
                }),
                (640, 480),
                (1280, 720),
                &s,
            );
        }
        for _ in 0..15 {
            x += 8.0;
            st.update(
                1.0 / 30.0,
                Some(FaceObs {
                    cx: x,
                    cy: 240.0,
                    size: 120.0,
                    roll: 0.0,
                }),
                (640, 480),
                (1280, 720),
                &s,
            );
            max_off = max_off.max(st.last_offset[0].abs());
        }
        assert!(max_off > 3.0, "expected visible sway, got {max_off}");
    }
}
