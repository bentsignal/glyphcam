//! Face detection (MediaPipe BlazeFace short-range) and person segmentation
//! (MediaPipe selfie segmentation, landscape), both run with `tract` on a
//! background thread so the render loop never waits for inference.

use crate::image::RgbImage;
use anyhow::{Context, Result};
use parking_lot::{Condvar, Mutex};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tract_onnx::prelude::*;

static FACE_MODEL: &[u8] = include_bytes!("../../../assets/models/face_detection_short_range.onnx");
static FACE_ANCHORS: &str = include_str!("../../../assets/models/face_detection_anchors.csv");
static SEG_MODEL: &[u8] =
    include_bytes!("../../../assets/models/selfie_segmentation_landscape.onnx");

type Model = std::sync::Arc<TypedRunnableModel>;

/// A detected face, in source-image pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// right eye, left eye, nose tip, mouth, right ear, left ear (subject's
    /// right/left, i.e. image left/right for an unmirrored camera).
    pub kps: [[f32; 2]; 6],
}

impl Face {
    /// Head roll in radians (0 = eyes level), from the eye line.
    pub fn roll(&self) -> f32 {
        let [r, l] = [self.kps[0], self.kps[1]];
        (l[1] - r[1]).atan2(l[0] - r[0])
    }

    /// Rough yaw/pitch in radians estimated from keypoint geometry.
    pub fn yaw_pitch(&self) -> (f32, f32) {
        let roll = self.roll();
        let (s, c) = (-roll).sin_cos();
        let rot = |p: [f32; 2]| {
            let x = p[0] - self.cx;
            let y = p[1] - self.cy;
            [x * c - y * s, x * s + y * c]
        };
        let re = rot(self.kps[0]);
        let le = rot(self.kps[1]);
        let nose = rot(self.kps[2]);
        let mouth = rot(self.kps[3]);
        let rear = rot(self.kps[4]);
        let lear = rot(self.kps[5]);
        let ear_mid = (rear[0] + lear[0]) * 0.5;
        let ear_span = (lear[0] - rear[0]).abs().max(1.0);
        let yaw = (2.0 * (nose[0] - ear_mid) / ear_span)
            .clamp(-0.95, 0.95)
            .asin();
        let eye_y = (re[1] + le[1]) * 0.5;
        let span = (mouth[1] - eye_y).max(1.0);
        let ratio = (nose[1] - eye_y) / span; // ~0.55 when facing the camera
        let pitch = ((ratio - 0.55) * 2.2).clamp(-0.8, 0.8);
        (yaw, pitch)
    }
}

/// Person mask, `w x h` alphas (0..1) stretched over the full source image.
#[derive(Clone, Debug)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Mask {
    /// Sample at normalized source coordinates (0..1).
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return 0.0;
        }
        let x = u * self.w as f32 - 0.5;
        let y = v * self.h as f32 - 0.5;
        let x0 = x.floor().max(0.0) as usize;
        let y0 = y.floor().max(0.0) as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let tx = (x - x0 as f32).clamp(0.0, 1.0);
        let ty = (y - y0 as f32).clamp(0.0, 1.0);
        let d = &self.data;
        let a = d[y0 * self.w + x0] + (d[y0 * self.w + x1] - d[y0 * self.w + x0]) * tx;
        let b = d[y1 * self.w + x0] + (d[y1 * self.w + x1] - d[y1 * self.w + x0]) * tx;
        a + (b - a) * ty
    }
}

pub struct FaceDetector {
    model: Model,
    anchors: Vec<[f32; 2]>,
    input: Vec<f32>,
}

const DET: usize = 128;

impl FaceDetector {
    pub fn new() -> Result<Self> {
        let model = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(FACE_MODEL))?
            .with_input_fact(0, f32::fact([1, DET, DET, 3]).into())?
            .into_optimized()?
            .into_runnable()
            .context("loading face detector")?;
        let anchors = FACE_ANCHORS
            .lines()
            .filter_map(|l| {
                let mut it = l.split(',').map(|v| v.trim().parse::<f32>());
                Some([it.next()?.ok()?, it.next()?.ok()?])
            })
            .collect::<Vec<_>>();
        anyhow::ensure!(
            anchors.len() == 896,
            "unexpected anchor count {}",
            anchors.len()
        );
        Ok(Self {
            model,
            anchors,
            input: vec![0.0; DET * DET * 3],
        })
    }

    pub fn detect(&mut self, img: &RgbImage, min_score: f32) -> Result<Vec<Face>> {
        let (scale, pad_x, pad_y) =
            img.letterbox_into(&mut self.input, DET, DET, |v| v / 127.5 - 1.0);
        let t = Tensor::from_shape(&[1, DET, DET, 3], &self.input)?;
        let out = self.model.run(tvec!(t.into()))?;
        let reg = out[0].try_as_plain_ram()?;
        let reg = reg.as_slice::<f32>().context("regressors")?;
        let cls = out[1].try_as_plain_ram()?;
        let cls = cls.as_slice::<f32>().context("scores")?;
        let to_img = |x: f32, y: f32| [(x - pad_x) / scale, (y - pad_y) / scale];
        let mut faces = Vec::new();
        for (i, a) in self.anchors.iter().enumerate() {
            let score = 1.0 / (1.0 + (-cls[i].clamp(-80.0, 80.0)).exp());
            if score < min_score {
                continue;
            }
            let r = &reg[i * 16..i * 16 + 16];
            let ax = a[0] * DET as f32;
            let ay = a[1] * DET as f32;
            let c = to_img(ax + r[0], ay + r[1]);
            let mut kps = [[0.0; 2]; 6];
            for (k, kp) in kps.iter_mut().enumerate() {
                *kp = to_img(ax + r[4 + k * 2], ay + r[5 + k * 2]);
            }
            faces.push(Face {
                cx: c[0],
                cy: c[1],
                w: r[2] / scale,
                h: r[3] / scale,
                score,
                kps,
            });
        }
        Ok(weighted_nms(faces, 0.3))
    }
}

fn iou(a: &Face, b: &Face) -> f32 {
    let ix = ((a.cx + a.w / 2.0).min(b.cx + b.w / 2.0) - (a.cx - a.w / 2.0).max(b.cx - b.w / 2.0))
        .max(0.0);
    let iy = ((a.cy + a.h / 2.0).min(b.cy + b.h / 2.0) - (a.cy - a.h / 2.0).max(b.cy - b.h / 2.0))
        .max(0.0);
    let inter = ix * iy;
    inter / (a.w * a.h + b.w * b.h - inter).max(1e-6)
}

/// MediaPipe-style weighted NMS: overlapping detections are blended by score,
/// which noticeably reduces jitter compared to plain NMS.
fn weighted_nms(mut faces: Vec<Face>, thresh: f32) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out = Vec::new();
    while let Some(best) = faces.first().copied() {
        let (group, rest): (Vec<Face>, Vec<Face>) =
            faces.into_iter().partition(|f| iou(&best, f) > thresh);
        faces = rest;
        let total: f32 = group.iter().map(|f| f.score).sum();
        let mut m = Face {
            score: best.score,
            ..best
        };
        m.cx = group.iter().map(|f| f.cx * f.score).sum::<f32>() / total;
        m.cy = group.iter().map(|f| f.cy * f.score).sum::<f32>() / total;
        m.w = group.iter().map(|f| f.w * f.score).sum::<f32>() / total;
        m.h = group.iter().map(|f| f.h * f.score).sum::<f32>() / total;
        for k in 0..6 {
            for d in 0..2 {
                m.kps[k][d] = group.iter().map(|f| f.kps[k][d] * f.score).sum::<f32>() / total;
            }
        }
        out.push(m);
    }
    out
}

pub struct Segmenter {
    model: Model,
    input: Vec<f32>,
}

const SEG_W: usize = 256;
const SEG_H: usize = 144;

impl Segmenter {
    pub fn new() -> Result<Self> {
        let model = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(SEG_MODEL))?
            .with_input_fact(0, f32::fact([1, 3, SEG_H, SEG_W]).into())?
            .into_optimized()?
            .into_runnable()
            .context("loading segmenter")?;
        Ok(Self {
            model,
            input: vec![0.0; 3 * SEG_W * SEG_H],
        })
    }

    pub fn segment(&mut self, img: &RgbImage) -> Result<Mask> {
        img.resize_chw(&mut self.input, SEG_W, SEG_H);
        let t = Tensor::from_shape(&[1, 3, SEG_H, SEG_W], &self.input)?;
        let out = self.model.run(tvec!(t.into()))?;
        let a = out[0].try_as_plain_ram()?;
        let data = a.as_slice::<f32>().context("mask")?.to_vec();
        Ok(Mask {
            w: SEG_W,
            h: SEG_H,
            data,
        })
    }
}

/// Latest tracking result shared with the renderer.
#[derive(Clone, Default)]
pub struct TrackState {
    pub face: Option<Face>,
    /// When `face` was last observed.
    pub face_time: Option<Instant>,
    pub mask: Option<Arc<Mask>>,
    /// Source dimensions the result refers to.
    pub src_size: (u32, u32),
    pub detect_ms: f32,
    pub segment_ms: f32,
}

struct Shared {
    pending: Mutex<Option<Arc<RgbImage>>>,
    cv: Condvar,
    state: Mutex<TrackState>,
    want_mask: AtomicBool,
    stop: AtomicBool,
}

/// Background tracking worker. `submit` never blocks; older frames that
/// haven't been processed yet are dropped.
pub struct Tracker {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Tracker {
    pub fn spawn() -> Result<Self> {
        // Load models up-front so errors surface to the caller.
        let mut det = FaceDetector::new()?;
        let mut seg = Segmenter::new()?;
        let shared = Arc::new(Shared {
            pending: Mutex::new(None),
            cv: Condvar::new(),
            state: Mutex::new(TrackState::default()),
            want_mask: AtomicBool::new(true),
            stop: AtomicBool::new(false),
        });
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("glyphcam-track".into())
            .spawn(move || {
                let mut prev: Option<Face> = None;
                let mut mask_smooth: Option<Mask> = None;
                let mut n: u64 = 0;
                loop {
                    let img = {
                        let mut p = sh.pending.lock();
                        while p.is_none() && !sh.stop.load(Ordering::Relaxed) {
                            sh.cv.wait_for(&mut p, Duration::from_millis(500));
                        }
                        if sh.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        p.take().unwrap()
                    };
                    n += 1;
                    let t0 = Instant::now();
                    let faces = det.detect(&img, 0.55).unwrap_or_default();
                    let detect_ms = t0.elapsed().as_secs_f32() * 1000.0;
                    let face = pick_face(&faces, prev.as_ref());
                    if face.is_some() {
                        prev = face;
                    }
                    // Segmentation every other frame keeps CPU low; the mask
                    // is temporally smoothed so it doesn't flicker.
                    let mut segment_ms = None;
                    if sh.want_mask.load(Ordering::Relaxed) && n % 2 == 0 {
                        let t1 = Instant::now();
                        if let Ok(m) = seg.segment(&img) {
                            segment_ms = Some(t1.elapsed().as_secs_f32() * 1000.0);
                            mask_smooth = Some(match mask_smooth.take() {
                                Some(mut s) if s.w == m.w && s.h == m.h => {
                                    for (a, b) in s.data.iter_mut().zip(&m.data) {
                                        *a = *a * 0.35 + b * 0.65;
                                    }
                                    s
                                }
                                _ => m,
                            });
                        }
                    }
                    let mut st = sh.state.lock();
                    st.src_size = (img.width, img.height);
                    st.detect_ms = detect_ms;
                    if let Some(f) = face {
                        st.face = Some(f);
                        st.face_time = Some(Instant::now());
                    } else if st
                        .face_time
                        .is_some_and(|t| t.elapsed() > Duration::from_millis(400))
                    {
                        st.face = None;
                    }
                    if let Some(ms) = segment_ms {
                        st.segment_ms = ms;
                        st.mask = mask_smooth.clone().map(Arc::new);
                    }
                    if !sh.want_mask.load(Ordering::Relaxed) {
                        st.mask = None;
                    }
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub fn submit(&self, img: Arc<RgbImage>) {
        *self.shared.pending.lock() = Some(img);
        self.shared.cv.notify_one();
    }

    pub fn set_want_mask(&self, on: bool) {
        self.shared.want_mask.store(on, Ordering::Relaxed);
    }

    pub fn state(&self) -> TrackState {
        self.shared.state.lock().clone()
    }

    /// Wait until no frame is pending (used by deterministic tests).
    pub fn wait_idle(&self, timeout: Duration) {
        let start = Instant::now();
        while self.shared.pending.lock().is_some() && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Prefer the face closest to the previous one (stable identity), otherwise
/// the biggest confident face.
fn pick_face(faces: &[Face], prev: Option<&Face>) -> Option<Face> {
    if faces.is_empty() {
        return None;
    }
    if let Some(p) = prev {
        let near = faces
            .iter()
            .map(|f| (f, ((f.cx - p.cx).powi(2) + (f.cy - p.cy).powi(2)).sqrt()))
            .filter(|(_, d)| *d < p.w * 1.5)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((f, _)) = near {
            return Some(*f);
        }
    }
    faces
        .iter()
        .max_by(|a, b| (a.w * a.h * a.score).total_cmp(&(b.w * b.h * b.score)))
        .copied()
}
