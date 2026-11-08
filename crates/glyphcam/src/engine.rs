//! The engine thread: owns the camera, the glyph pipeline and all outputs.
//! It runs independently of the UI so the virtual camera keeps working
//! while the window is hidden. The physical camera is only open while
//! someone needs frames (an app using the virtual camera, the preview, a
//! stream client), so the webcam light stays off when idle.

use crate::camera::{self, CameraDevice, CaptureHandle};
use crate::stream::StreamServer;
use crate::vcam::{VcamOutput, WriterState};
use glyphcam_core::settings::SourceKind;
use glyphcam_core::source::{DemoSource, ImageSource, SourceFrame};
use glyphcam_core::{Pipeline, Settings};
use parking_lot::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Camera / pipeline running.
    pub active: bool,
    pub source: String,
    pub source_detail: String,
    pub fps: f32,
    pub face: bool,
    pub detect_ms: f32,
    pub segment_ms: f32,
    pub render_ms: f32,
    pub vcam_in_use: bool,
    pub stream_clients: usize,
    pub stream_url: Option<String>,
    pub stream_error: Option<String>,
    pub error: Option<String>,
}

pub struct Preview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub seq: u64,
}

pub struct Shared {
    pub settings: RwLock<Settings>,
    /// Bumped whenever settings change (by the UI or the tray).
    pub settings_rev: AtomicU64,
    pub status: Mutex<Status>,
    pub preview: Mutex<Option<Preview>>,
    pub preview_wanted: AtomicBool,
    pub cameras: Mutex<Vec<CameraDevice>>,
    pub refresh_cameras: AtomicBool,
    pub quit: AtomicBool,
    pub repaint: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Shared {
    pub fn new(settings: Settings) -> Arc<Self> {
        Arc::new(Shared {
            settings: RwLock::new(settings),
            settings_rev: AtomicU64::new(1),
            status: Mutex::new(Status::default()),
            preview: Mutex::new(None),
            preview_wanted: AtomicBool::new(false),
            cameras: Mutex::new(Vec::new()),
            refresh_cameras: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            repaint: Mutex::new(None),
        })
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = self.settings.write();
        f(&mut s);
        s.sanitize();
        self.settings_rev.fetch_add(1, Ordering::SeqCst);
    }

    fn repaint(&self) {
        if let Some(r) = self.repaint.lock().as_ref() {
            r();
        }
    }
}

pub fn spawn(sh: Arc<Shared>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("glyphcam-engine".into())
        .spawn(move || run(sh))
        .expect("spawn engine")
}

enum Source {
    None,
    Camera {
        dev: CameraDevice,
        quality: u32,
        cap: CaptureHandle,
        last_seq: u64,
        opened: Instant,
    },
    Demo(DemoSource),
    Image(String, ImageSource),
}

fn run(sh: Arc<Shared>) {
    let vcam = VcamOutput::new();
    let mut pipeline = match Pipeline::new() {
        Ok(p) => p,
        Err(e) => {
            log::error!("pipeline init failed: {e:#}");
            sh.status.lock().error = Some(format!("Failed to load face models: {e}"));
            return;
        }
    };
    let mut stream: Option<StreamServer> = None;
    let mut stream_failed_port: Option<u16> = None;
    let mut source = Source::None;
    let mut camera_retry_at = Instant::now();
    let mut last_frame = Instant::now();
    let mut next_due = Instant::now();
    let mut fps_window = (Instant::now(), 0u32);
    let mut preview_seq = 0u64;
    let mut log_tick = 0u64;
    let start = Instant::now();
    let mut last_list = Instant::now() - Duration::from_secs(3600);

    while !sh.quit.load(Ordering::Relaxed) {
        let s = sh.settings.read().clone();

        // --- stream server lifecycle ---
        if s.stream_server {
            if stream.as_ref().map(|x| x.port) != Some(s.stream_port)
                && stream_failed_port != Some(s.stream_port)
            {
                stream = None;
                match StreamServer::start(s.stream_port) {
                    Ok(srv) => {
                        log::info!("stream server on {}", srv.url());
                        stream_failed_port = None;
                        stream = Some(srv);
                    }
                    Err(e) => {
                        log::warn!("stream server failed on port {}: {e}", s.stream_port);
                        stream_failed_port = Some(s.stream_port);
                        sh.status.lock().stream_error =
                            Some(format!("Port {} unavailable: {e}", s.stream_port));
                    }
                }
            }
        } else {
            stream = None;
            stream_failed_port = None;
        }

        // --- camera list ---
        if sh.refresh_cameras.swap(false, Ordering::SeqCst)
            || last_list.elapsed() > Duration::from_secs(30)
        {
            // Don't re-enumerate every 30 s while a camera is streaming.
            if !matches!(source, Source::Camera { .. })
                || last_list.elapsed() > Duration::from_secs(3600)
            {
                *sh.cameras.lock() = camera::list();
            }
            last_list = Instant::now();
        }

        let consumer = s.vcam_enabled && vcam.consumer_active();
        let watchers = stream.as_ref().map(|x| x.clients()).unwrap_or(0);
        let previewing = sh.preview_wanted.load(Ordering::Relaxed);
        let demand = previewing || consumer || watchers > 0 || s.keep_camera_warm;

        {
            let mut st = sh.status.lock();
            st.vcam_in_use = consumer;
            st.stream_clients = watchers;
            st.stream_url = stream.as_ref().map(|x| x.url());
            if stream.is_some() {
                st.stream_error = None;
            }
        }

        if !demand {
            if !matches!(source, Source::None) {
                log::info!("idle: releasing source");
                source = Source::None;
                pipeline.reset_motion();
            }
            vcam.set_state(if s.vcam_enabled {
                WriterState::Starting
            } else {
                WriterState::Paused
            });
            {
                let mut st = sh.status.lock();
                st.active = false;
                st.fps = 0.0;
                st.face = false;
            }
            std::thread::sleep(Duration::from_millis(120));
            continue;
        }

        // --- make sure the right source is open ---
        let wanted_ok = match (&s.source, &source) {
            (SourceKind::Camera(name), Source::Camera { dev, quality, .. }) => {
                *quality == s.camera_quality
                    && name.as_ref().map(|n| n == &dev.name).unwrap_or(true)
            }
            (SourceKind::Demo, Source::Demo(_)) => true,
            (SourceKind::Image(p), Source::Image(q, _)) => p == q,
            _ => false,
        };
        if !wanted_ok {
            source = Source::None;
            pipeline.reset_motion();
            match &s.source {
                SourceKind::Camera(name) => {
                    if Instant::now() >= camera_retry_at {
                        let cams = sh.cameras.lock().clone();
                        let dev = match name {
                            Some(n) => cams.iter().find(|c| &c.name == n).cloned(),
                            None => None,
                        }
                        .or_else(|| cams.first().cloned());
                        match dev {
                            Some(dev) => {
                                let cap = CaptureHandle::start(dev.clone(), s.camera_quality);
                                source = Source::Camera {
                                    dev,
                                    quality: s.camera_quality,
                                    cap,
                                    last_seq: 0,
                                    opened: Instant::now(),
                                };
                            }
                            None => {
                                let mut st = sh.status.lock();
                                st.error = Some("No camera found. Plug one in, or pick Demo in Camera settings.".into());
                                st.source = "No camera".into();
                                camera_retry_at = Instant::now() + Duration::from_secs(3);
                                sh.refresh_cameras.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
                SourceKind::Demo => source = Source::Demo(DemoSource::new(640, 480)),
                SourceKind::Image(p) => match ImageSource::open(p, 640, 480) {
                    Ok(src) => source = Source::Image(p.clone(), src),
                    Err(e) => sh.status.lock().error = Some(format!("{e:#}")),
                },
            }
        }

        // --- grab a frame ---
        let t = start.elapsed().as_secs_f64();
        let frame: Option<SourceFrame> = match &mut source {
            Source::None => {
                vcam.set_state(WriterState::NoCamera);
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Source::Camera {
                dev,
                cap,
                last_seq,
                opened,
                ..
            } => {
                if let Some(err) = cap.error() {
                    {
                        let mut st = sh.status.lock();
                        st.error = Some(err);
                        st.active = false;
                    }
                    vcam.set_state(WriterState::NoCamera);
                    source = Source::None;
                    camera_retry_at = Instant::now() + Duration::from_secs(3);
                    sh.refresh_cameras.store(true, Ordering::SeqCst);
                    continue;
                }
                match cap.next_frame(*last_seq, Duration::from_millis(150)) {
                    Some((seq, img)) => {
                        *last_seq = seq;
                        let mut st = sh.status.lock();
                        st.source = dev.name.clone();
                        st.source_detail = cap.format_desc.lock().clone();
                        st.error = None;
                        Some(SourceFrame {
                            image: img,
                            face_hint: None,
                            mask_hint: None,
                        })
                    }
                    None => {
                        if *last_seq == 0 {
                            vcam.set_state(WriterState::Starting);
                            if opened.elapsed() > Duration::from_secs(12) {
                                sh.status.lock().error =
                                    Some(format!("{} isn't sending frames", dev.name));
                            }
                        }
                        continue;
                    }
                }
            }
            Source::Demo(d) => {
                let mut st = sh.status.lock();
                st.source = "Demo".into();
                st.source_detail = "procedural test pattern".into();
                st.error = None;
                drop(st);
                Some(d.frame(t))
            }
            Source::Image(p, src) => {
                let mut st = sh.status.lock();
                st.source = "Image".into();
                st.source_detail = p.clone();
                st.error = None;
                drop(st);
                Some(src.frame(t, 1.0))
            }
        };

        // --- pacing ---
        let interval = Duration::from_secs_f32(1.0 / s.fps.max(1) as f32);
        let synthetic = !matches!(source, Source::Camera { .. });
        let now = Instant::now();
        if now < next_due {
            if synthetic {
                std::thread::sleep(next_due - now);
            } else if next_due - now > interval / 4 {
                continue; // camera faster than output fps: drop this frame
            }
        }
        let now = Instant::now();
        next_due = if now > next_due + interval {
            now + interval
        } else {
            next_due + interval
        };
        let dt = last_frame.elapsed().as_secs_f32().min(0.25);
        last_frame = now;

        // --- render ---
        pipeline.process(frame.as_ref(), &s, dt);
        let out = pipeline.frame();
        if s.vcam_enabled {
            vcam.write(out);
        } else {
            vcam.set_state(WriterState::Paused);
        }
        if let Some(srv) = &stream {
            srv.publish(out);
        }
        if previewing && s.preview {
            let (w, h, rgba) = out.downscale_rgba(640);
            preview_seq += 1;
            *sh.preview.lock() = Some(Preview {
                width: w,
                height: h,
                rgba,
                seq: preview_seq,
            });
            sh.repaint();
        }

        // --- status ---
        fps_window.1 += 1;
        let el = fps_window.0.elapsed().as_secs_f32();
        let mut st = sh.status.lock();
        if el >= 1.0 {
            st.fps = fps_window.1 as f32 / el;
            fps_window = (Instant::now(), 0);
            log_tick += 1;
            if log_tick % 10 == 1 {
                log::info!(
                    "engine: {:.1} fps, detect {:.1} ms, mask {:.1} ms, frame {:.1} ms, face {}, preview {}, vcam reader {}",
                    st.fps, pipeline.stats.detect_ms, pipeline.stats.segment_ms, pipeline.stats.render_ms,
                    pipeline.stats.face_found, previewing, consumer
                );
            }
        }
        st.active = true;
        st.face = pipeline.stats.face_found;
        st.detect_ms = pipeline.stats.detect_ms;
        st.segment_ms = pipeline.stats.segment_ms;
        st.render_ms = pipeline.stats.render_ms;
    }
    drop(source);
    vcam.set_state(WriterState::Offline);
}
