#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! glyphcam: real-time glyph-art face camera with a virtual webcam output.

mod camera;
mod config;
mod engine;
#[cfg(windows)]
mod mfcam;
mod platform;
mod realsense;
mod share;
mod stream;
mod tray;
mod ui;
mod updater;
mod vcam;

use std::io::Write;

fn init_logging() {
    init_logging_to("glyphcam.log");
}

fn init_logging_to(file: &str) {
    let dir = config::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(file);
    let _ = std::fs::rename(&path, path.with_extension("old.log"));
    let mut b = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,gpui=warn,naga=warn"),
    );
    if let Ok(f) = std::fs::File::create(&path) {
        b.target(env_logger::Target::Pipe(Box::new(f)));
    }
    b.format(|buf, r| {
        writeln!(
            buf,
            "[{} {} {}] {}",
            buf.timestamp_millis(),
            r.level(),
            r.target(),
            r.args()
        )
    });
    let _ = b.try_init();
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
        eprintln!("panic: {info}");
    }));
}

#[cfg(windows)]
fn attach_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
#[cfg(not(windows))]
fn attach_console() {}

/// Headless check used by CI and for debugging: loads the models, renders
/// demo frames through the full pipeline, optionally writes a PNG.
fn selftest(out: Option<&str>) -> anyhow::Result<()> {
    use glyphcam_core::source::DemoSource;
    let t0 = std::time::Instant::now();
    let mut p = glyphcam_core::Pipeline::new()?;
    let demo = DemoSource::new(640, 480);
    let s = glyphcam_core::Settings::default();
    for i in 0..60 {
        let f = demo.frame(i as f64 / 30.0);
        p.process(Some(&f), &s, 1.0 / 30.0);
    }
    let frame = p.frame();
    anyhow::ensure!(
        frame.width == s.out_width && frame.height == s.out_height,
        "bad frame size"
    );
    let lit = frame.data.chunks_exact(4).filter(|px| px[1] > 60).count();
    anyhow::ensure!(lit > 1000, "rendered frame is empty ({lit} lit pixels)");
    if let Some(path) = out {
        image::save_buffer(
            path,
            &frame.to_rgb(),
            frame.width,
            frame.height,
            image::ColorType::Rgb8,
        )?;
    }
    println!(
        "glyphcam selftest ok: {} {} | {} lit px | {:.0} ms total | last frame {:.2} ms",
        config::version_string(),
        std::env::consts::ARCH,
        lit,
        t0.elapsed().as_secs_f32() * 1000.0,
        p.stats.render_ms
    );
    Ok(())
}

/// Headless RealSense check: streams ~3 s of colour + depth through the full
/// pipeline (ridgelines look) and optionally writes the last frame as PNG.
fn depth_check(out: Option<&str>) -> anyhow::Result<()> {
    use glyphcam_core::settings::DepthMode;
    use glyphcam_core::source::SourceFrame;
    anyhow::ensure!(
        realsense::available(),
        "RealSense library not found next to glyphcam (realsense2.dll)"
    );
    let n = realsense::device_count();
    println!("RealSense devices: {n}");
    anyhow::ensure!(n > 0, "no RealSense camera connected");
    let cap = realsense::RealSenseCapture::start();
    let mut p = glyphcam_core::Pipeline::new()?;
    let s = glyphcam_core::Settings {
        depth_mode: DepthMode::Ridges,
        ..Default::default()
    };
    let (mut seq, mut frames, mut with_depth) = (0u64, 0u32, 0u32);
    let t0 = std::time::Instant::now();
    while frames < 90 && t0.elapsed() < std::time::Duration::from_secs(20) {
        if let Some(e) = cap.error() {
            anyhow::bail!(e);
        }
        let Some((sq, f)) = cap.next_frame(seq, std::time::Duration::from_millis(500)) else {
            continue;
        };
        seq = sq;
        let valid = f.depth.data.iter().filter(|&&v| v != 0).count() as f32
            / f.depth.data.len().max(1) as f32;
        p.process(
            Some(&SourceFrame {
                image: f.image,
                face_hint: None,
                mask_hint: None,
                depth: Some(f.depth),
            }),
            &s,
            1.0 / 30.0,
        );
        frames += 1;
        with_depth += p.stats.depth_ok as u32;
        if frames % 30 == 0 {
            println!(
                "frame {frames}: depth px {:.0}%  face {}  face distance {:.2} m  render {:.1} ms",
                valid * 100.0,
                p.stats.face_found,
                p.stats.depth_m,
                p.stats.render_ms
            );
        }
    }
    let fps = frames as f32 / t0.elapsed().as_secs_f32();
    println!(
        "{} | {frames} frames, {with_depth} with usable depth, {fps:.1} fps",
        cap.format_desc.lock()
    );
    if let Some(path) = out {
        let frame = p.frame();
        image::save_buffer(
            path,
            &frame.to_rgb(),
            frame.width,
            frame.height,
            image::ColorType::Rgb8,
        )?;
    }
    anyhow::ensure!(frames > 0, "no frames from the depth camera");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let arg_after = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };

    if has("--selftest")
        || has("--depth-check")
        || has("--register-vcam")
        || has("--unregister-vcam")
        || has("--version")
        || has("--install-release")
        || has("--install-system-camera")
        || has("--remove-system-camera")
    {
        attach_console();
    }
    if has("--version") {
        println!("glyphcam {}", config::version_string());
        return;
    }
    if has("--selftest") {
        match selftest(arg_after("--selftest").filter(|a| !a.starts_with("--"))) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("selftest failed: {e:#}");
                std::process::exit(1);
            }
        }
    }
    if has("--depth-check") {
        match depth_check(arg_after("--depth-check").filter(|a| !a.starts_with("--"))) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("depth check failed: {e:#}");
                std::process::exit(1);
            }
        }
    }
    if let Some(tag) = arg_after("--install-release") {
        match updater::install_blocking(tag) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("install failed: {e:#}");
                std::process::exit(1);
            }
        }
    }
    #[cfg(windows)]
    {
        if has("--register-vcam") {
            match vcam::install::ensure_registered() {
                Ok(()) => println!("virtual camera registered"),
                Err(e) => {
                    eprintln!("register failed: {e:#}");
                    std::process::exit(1);
                }
            }
            return;
        }
        if has("--install-system-camera") {
            // Elevated helper: copy + register the Windows 11 camera source.
            init_logging_to("system-camera.log");
            match mfcam::install_admin() {
                Ok(p) => {
                    log::info!("system camera installed: {}", p.display());
                    println!("system camera installed: {}", p.display());
                }
                Err(e) => {
                    log::error!("system camera install failed: {e:#}");
                    eprintln!("system camera install failed: {e:#}");
                    std::process::exit(1);
                }
            }
            return;
        }
        if has("--uninstall-system-camera-admin") {
            mfcam::uninstall_admin();
            return;
        }
        if has("--setup-system-camera") {
            // Non-elevated: runs the admin step (UAC) if needed, then creates the camera.
            attach_console();
            match mfcam::setup() {
                Ok(()) => println!("system camera ready"),
                Err(e) => {
                    eprintln!("system camera setup failed: {e:#}");
                    std::process::exit(1);
                }
            }
            return;
        }
        if has("--remove-system-camera") {
            mfcam::teardown();
            println!("system camera removed");
            return;
        }
        if has("--unregister-vcam") {
            vcam::install::unregister();
            println!("virtual camera unregistered");
            return;
        }
    }

    init_logging();
    log::info!(
        "glyphcam {} starting ({} {})",
        config::version_string(),
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let Some(instance) = platform::single_instance() else {
        log::info!("already running; asked the other instance to show itself");
        return;
    };

    #[cfg(windows)]
    {
        if let Err(e) = vcam::install::ensure_registered() {
            log::warn!("virtual camera registration: {e:#}");
        }
        // Make sure the Windows 11 camera exists for this user (cheap, no admin).
        std::thread::spawn(|| {
            if mfcam::status().installed() {
                match mfcam::ensure_camera() {
                    Ok(()) => log::info!("Windows 11 camera ready"),
                    Err(e) => log::warn!("Windows 11 camera: {e:#}"),
                }
            }
        });
    }
    camera::init_permissions();

    let settings = config::load();
    let start_hidden = has("--minimized") || settings.start_minimized;
    let sh = engine::Shared::new(settings);
    let engine = engine::spawn(sh.clone());
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();

    let show_tx = cmd_tx.clone();
    let sh_for_show = sh.clone();
    platform::listen_for_show(instance, move || {
        let _ = show_tx.send(tray::TrayCmd::Show);
        if let Some(r) = sh_for_show.repaint.lock().as_ref() {
            r();
        }
    });

    let launch = ui::Launch {
        sh,
        engine,
        start_hidden,
        initial_tab: arg_after("--tab").map(str::to_owned),
        cmd_tx,
        cmd_rx,
    };
    ui::run(launch);
    log::info!("glyphcam exiting");
}
