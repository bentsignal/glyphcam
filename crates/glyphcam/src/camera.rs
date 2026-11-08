//! Physical camera capture on a dedicated thread (nokhwa: Media Foundation
//! on Windows, AVFoundation on macOS). The engine pulls the latest frame.

use anyhow::{anyhow, Result};
use glyphcam_core::RgbImage;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
};
use nokhwa::Camera;
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct CameraDevice {
    pub name: String,
    pub index: CameraIndex,
}

/// Names that belong to virtual cameras we must never capture from
/// (feedback loop) or that are pointless as an input.
fn is_own_output(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == glyphcam_shm::CAMERA_NAME || n.starts_with("glyphcam")
}

pub fn list() -> Vec<CameraDevice> {
    match nokhwa::query(ApiBackend::Auto) {
        Ok(list) => list
            .into_iter()
            .filter(|c| !is_own_output(&c.human_name()))
            .map(|c| CameraDevice {
                name: c.human_name(),
                index: c.index().clone(),
            })
            .collect(),
        Err(e) => {
            log::warn!("camera query failed: {e}");
            Vec::new()
        }
    }
}

/// Request camera permission early on macOS (no-op elsewhere).
pub fn init_permissions() {
    #[cfg(target_os = "macos")]
    nokhwa::nokhwa_initialize(|granted| log::info!("camera permission granted: {granted}"));
}

struct Slot {
    frame: Option<Arc<RgbImage>>,
    seq: u64,
    error: Option<String>,
}

pub struct CaptureHandle {
    shared: Arc<(Mutex<Slot>, Condvar)>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub format_desc: Arc<Mutex<String>>,
}

/// Choose a capture format: ~640x480-1280x720 at >=24 fps, preferring
/// formats that are cheap to decode.
fn pick_format(formats: &[CameraFormat], quality: u32) -> Option<CameraFormat> {
    let target_h = quality.clamp(360, 1080) as i64;
    formats
        .iter()
        .copied()
        .filter(|f| {
            matches!(
                f.format(),
                FrameFormat::MJPEG | FrameFormat::YUYV | FrameFormat::NV12 | FrameFormat::RAWRGB
            )
        })
        .min_by_key(|f| {
            let fps_pen = if f.frame_rate() >= 24 {
                0
            } else {
                10_000 + (24 - f.frame_rate() as i64) * 100
            };
            let h = f.resolution().height() as i64;
            let res_pen = (h - target_h).abs();
            let aspect = f.resolution().width() as f32 / h.max(1) as f32;
            let aspect_pen =
                if (aspect - 16.0 / 9.0).abs() < 0.05 || (aspect - 4.0 / 3.0).abs() < 0.05 {
                    0
                } else {
                    200
                };
            let fmt_pen = match f.format() {
                FrameFormat::NV12 | FrameFormat::YUYV => 0,
                FrameFormat::RAWRGB => 5,
                _ => 20,
            };
            let fps_bonus = if f.frame_rate() > 30 { 5 } else { 0 };
            fps_pen + res_pen + aspect_pen + fmt_pen + fps_bonus
        })
}

fn open(dev: &CameraDevice, quality: u32) -> Result<(Camera, String)> {
    let mut cam = Camera::new(
        dev.index.clone(),
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate),
    )
    .map_err(|e| anyhow!("{e}"))?;
    if let Ok(formats) = cam.compatible_camera_formats() {
        if let Some(f) = pick_format(&formats, quality) {
            if let Err(e) = cam.set_camera_requset(RequestedFormat::new::<RgbFormat>(
                RequestedFormatType::Exact(f),
            )) {
                log::warn!("could not set format {f}: {e}");
            }
        }
    }
    cam.open_stream().map_err(|e| anyhow!("{e}"))?;
    let f = cam.camera_format();
    let desc = format!(
        "{}x{} {} @ {} fps",
        f.resolution().width(),
        f.resolution().height(),
        f.format(),
        f.frame_rate()
    );
    Ok((cam, desc))
}

impl CaptureHandle {
    /// Start capturing from `dev`. Opening happens on the capture thread;
    /// failures surface through [`CaptureHandle::error`].
    pub fn start(dev: CameraDevice, quality: u32) -> Self {
        let shared = Arc::new((
            Mutex::new(Slot {
                frame: None,
                seq: 0,
                error: None,
            }),
            Condvar::new(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let format_desc = Arc::new(Mutex::new(String::from("opening…")));
        let (sh, st, fd, d) = (
            shared.clone(),
            stop.clone(),
            format_desc.clone(),
            dev.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("glyphcam-capture".into())
            .spawn(move || {
                let (mut cam, desc) = match open(&d, quality) {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("opening camera {}: {e}", d.name);
                        sh.0.lock().error = Some(format!("Couldn't open {}: {e}", d.name));
                        sh.1.notify_all();
                        return;
                    }
                };
                log::info!("camera {} opened: {desc}", d.name);
                *fd.lock() = desc;
                let mut failures = 0;
                let mut buf: Vec<u8> = Vec::new();
                let mut decode_ms = 0.0f32;
                let mut n_frames = 0u64;
                let mut last_decode = std::time::Instant::now() - Duration::from_secs(1);
                while !st.load(Ordering::Relaxed) {
                    match cam.frame() {
                        Ok(frame) => {
                            failures = 0;
                            // Some (virtual) cameras deliver far more than 30 fps;
                            // never decode faster than ~40 fps.
                            if last_decode.elapsed() < Duration::from_millis(24) {
                                continue;
                            }
                            last_decode = std::time::Instant::now();
                            let res = frame.resolution();
                            let (w, h) = (res.width(), res.height());
                            buf.resize((w * h * 3) as usize, 0);
                            let td = std::time::Instant::now();
                            let ok = decode_into(&frame, &mut buf);
                            decode_ms = decode_ms * 0.9 + td.elapsed().as_secs_f32() * 100.0;
                            if n_frames % 300 == 0 {
                                log::info!(
                                    "capture {}x{} {:?}: decode {:.2} ms",
                                    w,
                                    h,
                                    frame.source_frame_format(),
                                    decode_ms
                                );
                            }
                            n_frames += 1;
                            if ok {
                                let img = RgbImage::from_raw(w, h, std::mem::take(&mut buf))
                                    .map(Arc::new);
                                let mut s = sh.0.lock();
                                s.frame = img;
                                s.seq += 1;
                                s.error = None;
                                drop(s);
                                sh.1.notify_all();
                            }
                        }
                        Err(e) => {
                            failures += 1;
                            if failures > 30 {
                                sh.0.lock().error =
                                    Some(format!("Camera stopped delivering frames: {e}"));
                                sh.1.notify_all();
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(30));
                        }
                    }
                }
                let _ = cam.stop_stream();
            })
            .expect("spawn capture thread");
        CaptureHandle {
            shared,
            stop,
            thread: Some(thread),
            format_desc,
        }
    }

    /// Wait up to `timeout` for a frame newer than `last_seq`.
    pub fn next_frame(&self, last_seq: u64, timeout: Duration) -> Option<(u64, Arc<RgbImage>)> {
        let (m, cv) = &*self.shared;
        let mut s = m.lock();
        if s.seq == last_seq && s.error.is_none() {
            cv.wait_for(&mut s, timeout);
        }
        if s.seq != last_seq {
            s.frame.clone().map(|f| (s.seq, f))
        } else {
            None
        }
    }

    pub fn error(&self) -> Option<String> {
        self.shared.0.lock().error.clone()
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            // Don't hang the engine on a stuck driver; detach after a while.
            let start = std::time::Instant::now();
            while !t.is_finished() && start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

#[inline]
fn yuv_to_rgb(y: i32, u: i32, v: i32) -> [u8; 3] {
    let c = 298 * (y - 16);
    let d = u - 128;
    let e = v - 128;
    [
        ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8,
        ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8,
        ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8,
    ]
}

/// Fast paths for the common uncompressed formats; nokhwa for the rest.
fn decode_into(frame: &nokhwa::Buffer, out: &mut [u8]) -> bool {
    let res = frame.resolution();
    let (w, h) = (res.width() as usize, res.height() as usize);
    let d = frame.buffer();
    match frame.source_frame_format() {
        FrameFormat::NV12 if d.len() >= w * h * 3 / 2 && out.len() >= w * h * 3 => {
            let (yp, uv) = d.split_at(w * h);
            for y in 0..h {
                let yrow = &yp[y * w..(y + 1) * w];
                let uvrow = &uv[(y / 2) * w..(y / 2) * w + w];
                let orow = &mut out[y * w * 3..(y + 1) * w * 3];
                for x in 0..w {
                    let u = uvrow[x & !1] as i32;
                    let v = uvrow[(x & !1) + 1] as i32;
                    orow[x * 3..x * 3 + 3].copy_from_slice(&yuv_to_rgb(yrow[x] as i32, u, v));
                }
            }
            true
        }
        FrameFormat::YUYV if d.len() >= w * h * 2 && out.len() >= w * h * 3 => {
            for (src, dst) in d[..w * h * 2].chunks_exact(4).zip(out.chunks_exact_mut(6)) {
                let (y0, u, y1, v) = (src[0] as i32, src[1] as i32, src[2] as i32, src[3] as i32);
                dst[..3].copy_from_slice(&yuv_to_rgb(y0, u, v));
                dst[3..].copy_from_slice(&yuv_to_rgb(y1, u, v));
            }
            true
        }
        FrameFormat::MJPEG => decode_mjpeg(d, w, h, out),
        _ => frame.decode_image_to_buffer::<RgbFormat>(out).is_ok(),
    }
}

/// Pure-Rust MJPEG decode (keeps the build free of C JPEG libraries).
fn decode_mjpeg(data: &[u8], w: usize, h: usize, out: &mut [u8]) -> bool {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_strict_mode(false);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), opts);
    if dec.decode_headers().is_err() {
        return false;
    }
    match dec.output_buffer_size() {
        Some(n) if n == w * h * 3 && out.len() >= n => dec.decode_into(&mut out[..n]).is_ok(),
        _ => false,
    }
}
