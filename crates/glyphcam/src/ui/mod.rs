//! The control panel (GPUI + gpui-component). The window is *not* the camera
//! output: it toggles the virtual camera and preview, edits settings and
//! manages builds. Closing it hides glyphcam to the tray / menu bar; the
//! engine keeps running. "Hidden" means the window is actually closed and is
//! rebuilt when shown again (the app is in explicit-quit mode).

mod tabs;
mod theme;
mod view;
mod widgets;

use crate::config;
use crate::engine::Shared;
use crate::tray::TrayCmd;
use futures::StreamExt as _;
use gpui::*;
use gpui_kit::component::Root;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
pub use view::GlyphUi;

pub struct Launch {
    pub sh: Arc<Shared>,
    pub engine: std::thread::JoinHandle<()>,
    pub start_hidden: bool,
    /// Initial settings tab (dev/screenshot helper: --tab output).
    pub initial_tab: Option<String>,
    pub cmd_tx: Sender<TrayCmd>,
    pub cmd_rx: Receiver<TrayCmd>,
}

gpui_kit::actions!(glyphcam, [Quit, CloseWindow]);

/// Wakes the UI thread from other threads (engine frames, tray, updater).
#[derive(Clone)]
pub struct Waker {
    tx: futures::channel::mpsc::UnboundedSender<()>,
    pending: Arc<AtomicBool>,
}

impl Waker {
    pub fn wake(&self) {
        // coalesce: one pending wake is enough
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.tx.unbounded_send(());
        }
    }
}

pub fn run(l: Launch) {
    let app = gpui_kit::application()
        .with_assets(UiAssets)
        .with_quit_mode(QuitMode::Explicit);
    // macOS: clicking the Dock icon brings the window back
    let reopen_ui: Rc<RefCell<Option<Entity<GlyphUi>>>> = Rc::default();
    app.on_reopen({
        let slot = reopen_ui.clone();
        move |cx| {
            if let Some(ui) = slot.borrow().clone() {
                GlyphUi::show(&ui, cx);
            }
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(cx);
        cx.bind_keys([
            KeyBinding::new(
                if cfg!(target_os = "macos") {
                    "cmd-q"
                } else {
                    "ctrl-q"
                },
                Quit,
                None,
            ),
            KeyBinding::new(
                if cfg!(target_os = "macos") {
                    "cmd-w"
                } else {
                    "ctrl-w"
                },
                CloseWindow,
                None,
            ),
        ]);
        cx.set_menus([Menu {
            name: "glyphcam".into(),
            items: vec![MenuItem::action("Quit glyphcam", Quit)],
            disabled: false,
        }]);

        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let waker = Waker {
            tx,
            pending: Arc::new(AtomicBool::new(false)),
        };
        {
            let w = waker.clone();
            *l.sh.repaint.lock() = Some(Box::new(move || w.wake()));
        }
        let start_hidden = l.start_hidden;
        let ui = cx.new(|cx| GlyphUi::new(l, waker.clone(), cx));

        cx.on_action({
            let ui = ui.clone();
            move |_: &Quit, cx| GlyphUi::quit(&ui, cx)
        });
        cx.on_action({
            let ui = ui.clone();
            move |_: &CloseWindow, cx| GlyphUi::request_close(&ui, cx)
        });
        *reopen_ui.borrow_mut() = Some(ui.clone());

        // Cross-thread wakes: tray commands, new preview frames, updater.
        cx.spawn({
            let ui = ui.clone();
            let pending = waker.pending.clone();
            async move |cx| {
                while rx.next().await.is_some() {
                    pending.store(false, Ordering::Release);
                    cx.update(|cx| GlyphUi::on_wake(&ui, cx));
                }
            }
        })
        .detach();
        // Housekeeping: status text, tray tooltip, debounced save.
        cx.spawn({
            let ui = ui.clone();
            async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                cx.update(|cx| GlyphUi::tick(&ui, cx));
            }
        })
        .detach();

        let has_tray = ui.read(cx).has_tray();
        if !start_hidden || !has_tray {
            GlyphUi::show(&ui, cx);
        }
    });
    log::info!("ui loop ended");
}

/// gpui-component's default icon set plus the few extra Lucide icons we use
/// (the full catalogue is 1,800+ files).
struct UiAssets;

impl AssetSource for UiAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let extra: Option<&'static [u8]> = match path {
            "icons/webcam.svg" => Some(include_bytes!("../../../../assets/ui-icons/webcam.svg")),
            "icons/download.svg" => {
                Some(include_bytes!("../../../../assets/ui-icons/download.svg"))
            }
            "icons/clipboard-paste.svg" => Some(include_bytes!(
                "../../../../assets/ui-icons/clipboard-paste.svg"
            )),
            _ => None,
        };
        match extra {
            Some(b) => Ok(Some(std::borrow::Cow::Borrowed(b))),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

/// Open the control panel window around the (long-lived) UI entity.
fn open_window(ui: &Entity<GlyphUi>, cx: &mut App) -> Option<AnyWindowHandle> {
    let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(1000.), px(660.))),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("glyphcam {}", config::version_string()).into()),
            ..Default::default()
        }),
        app_id: Some("glyphcam".into()),
        ..Default::default()
    };
    let ui = ui.clone();
    match cx.open_window(opts, move |window, cx| {
        let close_ui = ui.clone();
        window.on_window_should_close(cx, move |_, cx| GlyphUi::on_should_close(&close_ui, cx));
        cx.new(|cx| Root::new(ui.clone(), window, cx))
    }) {
        Ok(h) => Some(h.into()),
        Err(e) => {
            log::error!("couldn't open window: {e:#}");
            None
        }
    }
}
