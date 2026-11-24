//! Intel RealSense depth cameras through librealsense's C API, loaded at
//! runtime so glyphcam still runs where the library isn't installed.
//!
//! Colour and depth stream at 640x480 / 30 fps, with depth aligned to the
//! colour image by the SDK. The SR300/SR305 need librealsense <= 2.54 (later
//! versions dropped them); 2.48 is the version verified on Windows here.

use anyhow::{anyhow, bail, Context, Result};
use glyphcam_core::depth::{DepthMap, Intrinsics};
use glyphcam_core::RgbImage;
use libloading::Library;
use parking_lot::{Condvar, Mutex};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const API_VERSION: c_int = 2 * 10000 + 48 * 100;
const STREAM_DEPTH: c_int = 1;
const STREAM_COLOR: c_int = 2;
const FORMAT_Z16: c_int = 1;
const FORMAT_RGB8: c_int = 5;
const W: c_int = 640;
const H: c_int = 480;
const FPS: c_int = 30;
/// `RS2_OPTION_AUTO_EXPOSURE_PRIORITY`: when on, the colour sensor halves
/// its frame rate in dim light to expose longer.
const OPTION_AUTO_EXPOSURE_PRIORITY: c_int = 30;

type Err = *mut c_void;
type P = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct RsIntrinsics {
    width: c_int,
    height: c_int,
    ppx: f32,
    ppy: f32,
    fx: f32,
    fy: f32,
    model: c_int,
    coeffs: [f32; 5],
}

/// The handful of librealsense entry points we use.
struct Api {
    _lib: Library,
    create_context: unsafe extern "C" fn(c_int, *mut Err) -> P,
    delete_context: unsafe extern "C" fn(P),
    query_devices: unsafe extern "C" fn(P, *mut Err) -> P,
    get_device_count: unsafe extern "C" fn(P, *mut Err) -> c_int,
    delete_device_list: unsafe extern "C" fn(P),
    create_pipeline: unsafe extern "C" fn(P, *mut Err) -> P,
    delete_pipeline: unsafe extern "C" fn(P),
    create_config: unsafe extern "C" fn(*mut Err) -> P,
    delete_config: unsafe extern "C" fn(P),
    config_enable_stream:
        unsafe extern "C" fn(P, c_int, c_int, c_int, c_int, c_int, c_int, *mut Err),
    pipeline_start_with_config: unsafe extern "C" fn(P, P, *mut Err) -> P,
    delete_pipeline_profile: unsafe extern "C" fn(P),
    pipeline_stop: unsafe extern "C" fn(P, *mut Err),
    pipeline_wait_for_frames: unsafe extern "C" fn(P, c_uint, *mut Err) -> P,
    pipeline_poll_for_frames: unsafe extern "C" fn(P, *mut P, *mut Err) -> c_int,
    create_align: unsafe extern "C" fn(c_int, *mut Err) -> P,
    delete_processing_block: unsafe extern "C" fn(P),
    create_frame_queue: unsafe extern "C" fn(c_int, *mut Err) -> P,
    delete_frame_queue: unsafe extern "C" fn(P),
    start_processing_queue: unsafe extern "C" fn(P, P, *mut Err),
    process_frame: unsafe extern "C" fn(P, P, *mut Err),
    wait_for_frame: unsafe extern "C" fn(P, c_uint, *mut Err) -> P,
    embedded_frames_count: unsafe extern "C" fn(P, *mut Err) -> c_int,
    extract_frame: unsafe extern "C" fn(P, c_int, *mut Err) -> P,
    release_frame: unsafe extern "C" fn(P),
    get_frame_data: unsafe extern "C" fn(P, *mut Err) -> *const c_void,
    get_frame_width: unsafe extern "C" fn(P, *mut Err) -> c_int,
    get_frame_height: unsafe extern "C" fn(P, *mut Err) -> c_int,
    get_frame_stride_in_bytes: unsafe extern "C" fn(P, *mut Err) -> c_int,
    get_frame_stream_profile: unsafe extern "C" fn(P, *mut Err) -> P,
    get_stream_profile_data: unsafe extern "C" fn(
        P,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut Err,
    ),
    get_video_stream_intrinsics: unsafe extern "C" fn(P, *mut RsIntrinsics, *mut Err),
    depth_frame_get_units: unsafe extern "C" fn(P, *mut Err) -> f32,
    pipeline_profile_get_device: unsafe extern "C" fn(P, *mut Err) -> P,
    delete_device: unsafe extern "C" fn(P),
    query_sensors: unsafe extern "C" fn(P, *mut Err) -> P,
    get_sensors_count: unsafe extern "C" fn(P, *mut Err) -> c_int,
    create_sensor: unsafe extern "C" fn(P, c_int, *mut Err) -> P,
    delete_sensor: unsafe extern "C" fn(P),
    delete_sensor_list: unsafe extern "C" fn(P),
    supports_option: unsafe extern "C" fn(P, c_int, *mut Err) -> c_int,
    set_option: unsafe extern "C" fn(P, c_int, f32, *mut Err),
    get_error_message: unsafe extern "C" fn(Err) -> *const c_char,
    free_error: unsafe extern "C" fn(Err),
}

// The function pointers are plain C entry points; the library stays loaded.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

fn lib_name() -> &'static str {
    if cfg!(windows) {
        "realsense2.dll"
    } else if cfg!(target_os = "macos") {
        "librealsense2.dylib"
    } else {
        "librealsense2.so"
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("GLYPHCAM_REALSENSE_LIB") {
        v.push(PathBuf::from(p));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
    {
        v.push(dir.join(lib_name()));
        v.push(dir.join("realsense").join(lib_name()));
    }
    v.push(PathBuf::from(lib_name()));
    v
}

impl Api {
    fn load() -> Result<Api> {
        let mut last = None;
        for path in candidates() {
            // SAFETY: loading a C library with no unusual init requirements.
            match unsafe { Library::new(&path) } {
                Ok(lib) => {
                    return Self::bind(lib).with_context(|| format!("binding {}", path.display()))
                }
                Err(e) => last = Some(e),
            }
        }
        Err(anyhow!(
            "{} not found ({})",
            lib_name(),
            last.map(|e| e.to_string()).unwrap_or_default()
        ))
    }

    fn bind(lib: Library) -> Result<Api> {
        macro_rules! sym {
            ($name:literal) => {
                // SAFETY: the declared signature matches librealsense's C header.
                unsafe { *lib.get(concat!($name, "\0").as_bytes())? }
            };
        }
        Ok(Api {
            create_context: sym!("rs2_create_context"),
            delete_context: sym!("rs2_delete_context"),
            query_devices: sym!("rs2_query_devices"),
            get_device_count: sym!("rs2_get_device_count"),
            delete_device_list: sym!("rs2_delete_device_list"),
            create_pipeline: sym!("rs2_create_pipeline"),
            delete_pipeline: sym!("rs2_delete_pipeline"),
            create_config: sym!("rs2_create_config"),
            delete_config: sym!("rs2_delete_config"),
            config_enable_stream: sym!("rs2_config_enable_stream"),
            pipeline_start_with_config: sym!("rs2_pipeline_start_with_config"),
            delete_pipeline_profile: sym!("rs2_delete_pipeline_profile"),
            pipeline_stop: sym!("rs2_pipeline_stop"),
            pipeline_wait_for_frames: sym!("rs2_pipeline_wait_for_frames"),
            pipeline_poll_for_frames: sym!("rs2_pipeline_poll_for_frames"),
            create_align: sym!("rs2_create_align"),
            delete_processing_block: sym!("rs2_delete_processing_block"),
            create_frame_queue: sym!("rs2_create_frame_queue"),
            delete_frame_queue: sym!("rs2_delete_frame_queue"),
            start_processing_queue: sym!("rs2_start_processing_queue"),
            process_frame: sym!("rs2_process_frame"),
            wait_for_frame: sym!("rs2_wait_for_frame"),
            embedded_frames_count: sym!("rs2_embedded_frames_count"),
            extract_frame: sym!("rs2_extract_frame"),
            release_frame: sym!("rs2_release_frame"),
            get_frame_data: sym!("rs2_get_frame_data"),
            get_frame_width: sym!("rs2_get_frame_width"),
            get_frame_height: sym!("rs2_get_frame_height"),
            get_frame_stride_in_bytes: sym!("rs2_get_frame_stride_in_bytes"),
            get_frame_stream_profile: sym!("rs2_get_frame_stream_profile"),
            get_stream_profile_data: sym!("rs2_get_stream_profile_data"),
            get_video_stream_intrinsics: sym!("rs2_get_video_stream_intrinsics"),
            depth_frame_get_units: sym!("rs2_depth_frame_get_units"),
            pipeline_profile_get_device: sym!("rs2_pipeline_profile_get_device"),
            delete_device: sym!("rs2_delete_device"),
            query_sensors: sym!("rs2_query_sensors"),
            get_sensors_count: sym!("rs2_get_sensors_count"),
            create_sensor: sym!("rs2_create_sensor"),
            delete_sensor: sym!("rs2_delete_sensor"),
            delete_sensor_list: sym!("rs2_delete_sensor_list"),
            supports_option: sym!("rs2_supports_option"),
            set_option: sym!("rs2_set_option"),
            get_error_message: sym!("rs2_get_error_message"),
            free_error: sym!("rs2_free_error"),
            _lib: lib,
        })
    }

    /// Turn an `rs2_error` out-parameter into a Result.
    fn check(&self, e: Err, what: &str) -> Result<()> {
        if e.is_null() {
            return Ok(());
        }
        // SAFETY: e is a live rs2_error from the last call; freed once here.
        let msg = unsafe {
            let m = (self.get_error_message)(e);
            let s = if m.is_null() {
                String::from("unknown error")
            } else {
                CStr::from_ptr(m).to_string_lossy().into_owned()
            };
            (self.free_error)(e);
            s
        };
        bail!("{what}: {msg}")
    }
}

static API: OnceLock<std::result::Result<Arc<Api>, String>> = OnceLock::new();

fn api() -> Result<Arc<Api>> {
    API.get_or_init(|| Api::load().map(Arc::new).map_err(|e| format!("{e:#}")))
        .clone()
        .map_err(|e| anyhow!(e))
}

/// Whether the RealSense library can be loaded on this machine.
pub fn available() -> bool {
    match api() {
        Ok(_) => true,
        Err(e) => {
            log::debug!("RealSense support unavailable: {e}");
            false
        }
    }
}

/// Camera names (as Windows/nokhwa list them) that belong to a RealSense.
pub fn is_realsense(name: &str) -> bool {
    name.to_ascii_lowercase().contains("realsense")
}

/// Number of RealSense devices connected (0 when the library is missing).
pub fn device_count() -> usize {
    let Ok(api) = api() else { return 0 };
    let mut e: Err = std::ptr::null_mut();
    // SAFETY: straightforward create/query/delete sequence; errors checked.
    unsafe {
        let ctx = (api.create_context)(API_VERSION, &mut e);
        if api.check(e, "create context").is_err() {
            return 0;
        }
        let list = (api.query_devices)(ctx, &mut e);
        let n = if api.check(e, "query devices").is_ok() {
            let n = (api.get_device_count)(list, &mut e);
            (api.delete_device_list)(list);
            if api.check(e, "device count").is_ok() {
                n.max(0) as usize
            } else {
                0
            }
        } else {
            0
        };
        (api.delete_context)(ctx);
        n
    }
}

pub struct RsFrame {
    pub image: Arc<RgbImage>,
    pub depth: Arc<DepthMap>,
}

struct Slot {
    frame: Option<(Arc<RgbImage>, Arc<DepthMap>)>,
    seq: u64,
    error: Option<String>,
}

/// Background capture thread for one RealSense, same shape as
/// [`crate::camera::CaptureHandle`].
pub struct RealSenseCapture {
    shared: Arc<(Mutex<Slot>, Condvar)>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub format_desc: Arc<Mutex<String>>,
}

impl RealSenseCapture {
    pub fn start() -> Self {
        let shared = Arc::new((
            Mutex::new(Slot {
                frame: None,
                seq: 0,
                error: None,
            }),
            Condvar::new(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let format_desc = Arc::new(Mutex::new(String::from("starting depth camera…")));
        let (sh, st, fd) = (shared.clone(), stop.clone(), format_desc.clone());
        let thread = std::thread::Builder::new()
            .name("glyphcam-realsense".into())
            .spawn(move || {
                if let Err(e) = run(&sh, &st, &fd) {
                    log::warn!("RealSense capture failed: {e:#}");
                    let (m, cv) = &*sh;
                    m.lock().error = Some(format!("Depth camera: {e:#}"));
                    cv.notify_all();
                }
            })
            .expect("spawn realsense thread");
        Self {
            shared,
            stop,
            thread: Some(thread),
            format_desc,
        }
    }

    pub fn next_frame(&self, last_seq: u64, timeout: Duration) -> Option<(u64, RsFrame)> {
        let (m, cv) = &*self.shared;
        let mut s = m.lock();
        if s.seq == last_seq && s.error.is_none() {
            cv.wait_for(&mut s, timeout);
        }
        if s.seq != last_seq {
            if let Some((img, d)) = &s.frame {
                return Some((
                    s.seq,
                    RsFrame {
                        image: img.clone(),
                        depth: d.clone(),
                    },
                ));
            }
        }
        None
    }

    pub fn error(&self) -> Option<String> {
        self.shared.0.lock().error.clone()
    }
}

impl Drop for RealSenseCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Owns the SDK objects for one streaming session; tears them down in order.
struct Session<'a> {
    api: &'a Api,
    ctx: P,
    pipe: P,
    cfg: P,
    profile: P,
    align: P,
    queue: P,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        let a = self.api;
        let mut e: Err = std::ptr::null_mut();
        // SAFETY: each handle was created by the matching rs2_create_* call
        // (or is null) and is deleted exactly once.
        unsafe {
            if !self.profile.is_null() {
                (a.pipeline_stop)(self.pipe, &mut e);
                let _ = a.check(e, "stop");
                (a.delete_pipeline_profile)(self.profile);
            }
            if !self.align.is_null() {
                (a.delete_processing_block)(self.align);
            }
            if !self.queue.is_null() {
                (a.delete_frame_queue)(self.queue);
            }
            if !self.cfg.is_null() {
                (a.delete_config)(self.cfg);
            }
            if !self.pipe.is_null() {
                (a.delete_pipeline)(self.pipe);
            }
            if !self.ctx.is_null() {
                (a.delete_context)(self.ctx);
            }
        }
    }
}

fn run(shared: &(Mutex<Slot>, Condvar), stop: &AtomicBool, desc: &Mutex<String>) -> Result<()> {
    let api = api()?;
    let a = &*api;
    let mut e: Err = std::ptr::null_mut();
    let mut s = Session {
        api: a,
        ctx: std::ptr::null_mut(),
        pipe: std::ptr::null_mut(),
        cfg: std::ptr::null_mut(),
        profile: std::ptr::null_mut(),
        align: std::ptr::null_mut(),
        queue: std::ptr::null_mut(),
    };
    // SAFETY: C API calls in the documented order; every error is checked
    // before the result is used, and Session frees everything on exit.
    unsafe {
        s.ctx = (a.create_context)(API_VERSION, &mut e);
        a.check(e, "create context")?;
        s.pipe = (a.create_pipeline)(s.ctx, &mut e);
        a.check(e, "create pipeline")?;
        s.cfg = (a.create_config)(&mut e);
        a.check(e, "create config")?;
        (a.config_enable_stream)(s.cfg, STREAM_DEPTH, -1, W, H, FORMAT_Z16, FPS, &mut e);
        a.check(e, "enable depth")?;
        (a.config_enable_stream)(s.cfg, STREAM_COLOR, -1, W, H, FORMAT_RGB8, FPS, &mut e);
        a.check(e, "enable colour")?;
        s.profile = (a.pipeline_start_with_config)(s.pipe, s.cfg, &mut e);
        a.check(
            e,
            "start (is the camera plugged into USB 3, and not in use?)",
        )?;
        s.align = (a.create_align)(STREAM_COLOR, &mut e);
        a.check(e, "create align")?;
        s.queue = (a.create_frame_queue)(1, &mut e);
        a.check(e, "create queue")?;
        (a.start_processing_queue)(s.align, s.queue, &mut e);
        a.check(e, "start align")?;
    }
    // SAFETY: as above.
    unsafe { keep_full_frame_rate(a, s.profile) };
    *desc.lock() = format!("{W}x{H} colour + depth @ {FPS} fps (RealSense)");
    log::info!("RealSense streaming {W}x{H} colour + aligned depth");

    let mut timeouts = 0;
    let mut intr: Option<Intrinsics> = None;
    let (mut n, mut skipped, mut work) = (0u32, 0u32, Duration::ZERO);
    let (mut incomplete, mut since) = (0u32, std::time::Instant::now());
    while !stop.load(Ordering::Relaxed) {
        // SAFETY: as above; every frame handle obtained here is released.
        let got = unsafe {
            let mut fs = (a.pipeline_wait_for_frames)(s.pipe, 1000, &mut e);
            if !e.is_null() {
                let r = a.check(e, "wait for frames");
                e = std::ptr::null_mut();
                timeouts += 1;
                if timeouts >= 5 {
                    r?;
                }
                continue;
            }
            timeouts = 0;
            // If we fell behind, the SDK has newer framesets queued: skip
            // to the newest so you never see yourself late.
            loop {
                let mut newer: P = std::ptr::null_mut();
                let got = (a.pipeline_poll_for_frames)(s.pipe, &mut newer, &mut e);
                if !e.is_null() {
                    let _ = a.check(e, "poll for frames");
                    e = std::ptr::null_mut();
                    break;
                }
                if got == 0 || newer.is_null() {
                    break;
                }
                (a.release_frame)(fs);
                fs = newer;
                skipped += 1;
            }
            let t0 = std::time::Instant::now();
            // process_frame takes ownership of fs
            (a.process_frame)(s.align, fs, &mut e);
            a.check(e, "align")?;
            let aligned = (a.wait_for_frame)(s.queue, 1000, &mut e);
            if !e.is_null() {
                let _ = a.check(e, "aligned frame");
                e = std::ptr::null_mut();
                continue;
            }
            let r = read_composite(a, aligned, &mut intr);
            (a.release_frame)(aligned);
            work += t0.elapsed();
            n += 1;
            if matches!(r, Ok(None)) {
                incomplete += 1;
            }
            if n == 300 {
                log::info!(
                    "RealSense: {:.1} framesets/s, align+copy {:.1} ms/frame, skipped {skipped} stale and {incomplete} incomplete framesets in {n}",
                    n as f32 / since.elapsed().as_secs_f32(),
                    work.as_secs_f32() * 1000.0 / n as f32
                );
                (n, skipped, incomplete, work) = (0, 0, 0, Duration::ZERO);
                since = std::time::Instant::now();
            }
            r?
        };
        if let Some((img, depth)) = got {
            let (m, cv) = shared;
            let mut slot = m.lock();
            slot.frame = Some((Arc::new(img), Arc::new(depth)));
            slot.seq += 1;
            cv.notify_all();
        }
    }
    Ok(())
}

/// Turn off auto-exposure priority on every sensor that has it: in a dim
/// room the colour camera otherwise drops from 30 to 15 fps to expose
/// longer. The picture gets darker instead, which our exposure lifts.
///
/// # Safety
/// `profile` must be a live pipeline profile.
unsafe fn keep_full_frame_rate(a: &Api, profile: P) {
    let mut e: Err = std::ptr::null_mut();
    let dev = (a.pipeline_profile_get_device)(profile, &mut e);
    if a.check(e, "device").is_err() || dev.is_null() {
        return;
    }
    e = std::ptr::null_mut();
    let list = (a.query_sensors)(dev, &mut e);
    if a.check(e, "sensors").is_ok() && !list.is_null() {
        e = std::ptr::null_mut();
        let count = (a.get_sensors_count)(list, &mut e);
        let count = if a.check(e, "sensor count").is_ok() {
            count
        } else {
            0
        };
        for i in 0..count {
            e = std::ptr::null_mut();
            let sensor = (a.create_sensor)(list, i, &mut e);
            if a.check(e, "sensor").is_err() || sensor.is_null() {
                continue;
            }
            e = std::ptr::null_mut();
            let has = (a.supports_option)(sensor, OPTION_AUTO_EXPOSURE_PRIORITY, &mut e);
            if a.check(e, "supports option").is_ok() && has != 0 {
                e = std::ptr::null_mut();
                (a.set_option)(sensor, OPTION_AUTO_EXPOSURE_PRIORITY, 0.0, &mut e);
                match a.check(e, "auto-exposure priority") {
                    Ok(()) => log::info!("RealSense: auto-exposure priority off (sensor {i})"),
                    Err(err) => log::warn!("RealSense: {err:#}"),
                }
            }
            (a.delete_sensor)(sensor);
        }
        (a.delete_sensor_list)(list);
    }
    (a.delete_device)(dev);
}

/// Copy the colour and depth out of an aligned frameset.
///
/// # Safety
/// `fs` must be a live composite frame from librealsense.
unsafe fn read_composite(
    a: &Api,
    fs: P,
    intr: &mut Option<Intrinsics>,
) -> Result<Option<(RgbImage, DepthMap)>> {
    let mut e: Err = std::ptr::null_mut();
    let n = (a.embedded_frames_count)(fs, &mut e);
    a.check(e, "frame count")?;
    let mut color: Option<RgbImage> = None;
    let mut depth: Option<(u32, u32, Vec<u16>, f32)> = None;
    for i in 0..n {
        let f = (a.extract_frame)(fs, i, &mut e);
        a.check(e, "extract frame")?;
        let r = (|| -> Result<()> {
            let prof = (a.get_frame_stream_profile)(f, &mut e);
            a.check(e, "frame profile")?;
            let (mut stream, mut format, mut idx, mut uid, mut rate) = (0, 0, 0, 0, 0);
            (a.get_stream_profile_data)(
                prof,
                &mut stream,
                &mut format,
                &mut idx,
                &mut uid,
                &mut rate,
                &mut e,
            );
            a.check(e, "profile data")?;
            let w = (a.get_frame_width)(f, &mut e).max(0) as usize;
            let h = (a.get_frame_height)(f, &mut e).max(0) as usize;
            let stride = (a.get_frame_stride_in_bytes)(f, &mut e).max(0) as usize;
            a.check(e, "frame size")?;
            let data = (a.get_frame_data)(f, &mut e) as *const u8;
            a.check(e, "frame data")?;
            if data.is_null() || w == 0 || h == 0 {
                return Ok(());
            }
            if stream == STREAM_COLOR && format == FORMAT_RGB8 {
                if intr.is_none() {
                    let mut ri = RsIntrinsics::default();
                    (a.get_video_stream_intrinsics)(prof, &mut ri, &mut e);
                    a.check(e, "intrinsics")?;
                    *intr = Some(Intrinsics {
                        fx: ri.fx,
                        fy: ri.fy,
                        ppx: ri.ppx,
                        ppy: ri.ppy,
                    });
                }
                let mut out = vec![0u8; w * h * 3];
                for y in 0..h {
                    let src = std::slice::from_raw_parts(data.add(y * stride), w * 3);
                    out[y * w * 3..(y + 1) * w * 3].copy_from_slice(src);
                }
                color = RgbImage::from_raw(w as u32, h as u32, out);
            } else if stream == STREAM_DEPTH && format == FORMAT_Z16 {
                let units = (a.depth_frame_get_units)(f, &mut e);
                a.check(e, "depth units")?;
                let mut out = vec![0u16; w * h];
                for y in 0..h {
                    let row = data.add(y * stride) as *const u16;
                    // rows are 2-byte aligned; read unaligned to be safe
                    for x in 0..w {
                        out[y * w + x] = row.add(x).read_unaligned();
                    }
                }
                depth = Some((w as u32, h as u32, out, units));
            }
            Ok(())
        })();
        (a.release_frame)(f);
        r?;
    }
    let (Some(img), Some((dw, dh, data, scale)), Some(i)) = (color, depth, *intr) else {
        return Ok(None);
    };
    if (dw, dh) != (img.width, img.height) {
        return Ok(None);
    }
    Ok(Some((
        img,
        DepthMap {
            width: dw,
            height: dh,
            data,
            scale,
            intr: i,
        },
    )))
}
