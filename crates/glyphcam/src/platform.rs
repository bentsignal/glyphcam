//! Small platform helpers: autostart, single instance, app hide/show.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// Loopback port used only to detect/notify an already-running instance.
const INSTANCE_PORT: u16 = 47_380;

/// Try to become the single running instance. Returns the listener, or
/// `None` after asking the existing instance to show its window.
pub fn single_instance() -> Option<TcpListener> {
    match TcpListener::bind(("127.0.0.1", INSTANCE_PORT)) {
        Ok(l) => Some(l),
        Err(_) => {
            if let Ok(mut s) = TcpStream::connect_timeout(
                &([127, 0, 0, 1], INSTANCE_PORT).into(),
                Duration::from_millis(500),
            ) {
                let _ = s.write_all(b"glyphcam:show\n");
                return None;
            }
            // Port taken by something else; run anyway.
            log::warn!("instance port busy; continuing without single-instance check");
            TcpListener::bind(("127.0.0.1", 0)).ok()
        }
    }
}

/// Call `on_show` whenever a second launch asks us to show the window.
pub fn listen_for_show(l: TcpListener, on_show: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("glyphcam-instance".into())
        .spawn(move || {
            for s in l.incoming().flatten() {
                let mut line = String::new();
                let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
                if BufReader::new(s).read_line(&mut line).is_ok() && line.trim() == "glyphcam:show"
                {
                    on_show();
                }
            }
        })
        .ok();
}

fn auto_launch() -> anyhow::Result<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    let exe = {
        // Point the login item at the .app bundle when running from one.
        let s = exe.to_string_lossy().to_string();
        match s.find(".app/Contents/MacOS/") {
            Some(i) => std::path::PathBuf::from(&s[..i + 4]),
            None => exe,
        }
    };
    let mut b = auto_launch::AutoLaunchBuilder::new();
    b.set_app_name("glyphcam")
        .set_app_path(&exe.to_string_lossy())
        .set_args(&["--minimized"]);
    #[cfg(target_os = "macos")]
    b.set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent);
    Ok(b.build()?)
}

pub fn set_autostart(on: bool) -> anyhow::Result<()> {
    let a = auto_launch()?;
    if on {
        a.enable()?;
    } else if a.is_enabled().unwrap_or(false) {
        a.disable()?;
    }
    Ok(())
}

pub fn autostart_enabled() -> bool {
    auto_launch()
        .and_then(|a| Ok(a.is_enabled()?))
        .unwrap_or(false)
}

/// Hide the whole app (macOS: like Cmd-H; the dock icon stays).
#[cfg(target_os = "macos")]
pub fn macos_hide() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        #[allow(unused_unsafe)]
        unsafe {
            app.hide(None)
        };
    }
}

#[cfg(target_os = "macos")]
pub fn macos_show() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        #[allow(unused_unsafe, deprecated)]
        unsafe {
            app.unhide(None);
            app.activateIgnoringOtherApps(true);
        }
    }
}

pub fn open_path(p: &std::path::Path) {
    let _ = std::fs::create_dir_all(p);
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
}
