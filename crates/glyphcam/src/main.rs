#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! glyphcam: real-time glyph-art face camera with a virtual webcam output.

mod camera;
mod config;
mod engine;
mod platform;
mod stream;
mod tray;
mod ui;
mod updater;
mod vcam;

use eframe::egui;
use std::io::Write;
use std::sync::Arc;

fn init_logging() {
    let dir = config::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("glyphcam.log");
    let _ = std::fs::rename(&path, dir.join("glyphcam.old.log"));
    let mut b = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,eframe=warn"),
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let arg_after = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };

    if has("--selftest") || has("--register-vcam") || has("--unregister-vcam") || has("--version") {
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
    if let Err(e) = vcam::install::ensure_registered() {
        log::warn!("virtual camera registration: {e:#}");
    }
    camera::init_permissions();

    let settings = config::load();
    let start_hidden = has("--minimized") || settings.start_minimized;
    let sh = engine::Shared::new(settings);
    let engine = engine::spawn(sh.clone());
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();

    let icon = image::load_from_memory(include_bytes!("../../../assets/icons/glyphcam-256.png"))
        .map(|i| i.to_rgba8())
        .ok()
        .map(|i| {
            let (w, h) = i.dimensions();
            Arc::new(egui::IconData {
                rgba: i.into_raw(),
                width: w,
                height: h,
            })
        });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("glyphcam")
        .with_app_id("glyphcam")
        .with_inner_size([1080.0, 720.0])
        .with_min_inner_size([780.0, 540.0])
        .with_visible(!start_hidden);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

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
        cmd_tx,
        cmd_rx,
    };
    if let Err(e) = eframe::run_native(
        "glyphcam",
        options,
        Box::new(move |cc| Ok(Box::new(ui::GlyphApp::new(cc, launch)))),
    ) {
        log::error!("ui failed: {e}");
    }
    log::info!("glyphcam exiting");
}
