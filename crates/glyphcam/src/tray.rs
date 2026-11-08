//! Tray (Windows) / menu bar (macOS) icon.

use std::sync::mpsc::Sender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    ToggleVcam,
    TogglePreview,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    vcam_item: CheckMenuItem,
}

impl Tray {
    pub fn set_vcam_checked(&self, on: bool) {
        if self.vcam_item.is_checked() != on {
            self.vcam_item.set_checked(on);
        }
    }
}

fn load_icon() -> Option<Icon> {
    #[cfg(target_os = "macos")]
    let bytes: &[u8] = include_bytes!("../../../assets/icons/tray-template-64.png");
    #[cfg(not(target_os = "macos"))]
    let bytes: &[u8] = include_bytes!("../../../assets/icons/tray-64.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// Build the tray icon. Events are forwarded to `tx`, then `wake` is called
/// so the (possibly hidden) UI processes them.
pub fn create(
    vcam_on: bool,
    tx: Sender<TrayCmd>,
    wake: impl Fn() + Send + Sync + Clone + 'static,
) -> anyhow::Result<Tray> {
    let menu = Menu::new();
    let open = MenuItem::new("Open glyphcam", true, None);
    let vcam_label = if cfg!(windows) {
        "Virtual camera"
    } else {
        "Output (stream)"
    };
    let vcam_item = CheckMenuItem::new(vcam_label, true, vcam_on, None);
    let preview = MenuItem::new("Toggle preview", true, None);
    let quit = MenuItem::new("Quit glyphcam", true, None);
    menu.append_items(&[
        &open,
        &PredefinedMenuItem::separator(),
        &vcam_item,
        &preview,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    let ids = (
        open.id().clone(),
        vcam_item.id().clone(),
        preview.id().clone(),
        quit.id().clone(),
    );
    {
        let tx = tx.clone();
        let wake = wake.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = if e.id == ids.0 {
                TrayCmd::Show
            } else if e.id == ids.1 {
                TrayCmd::ToggleVcam
            } else if e.id == ids.2 {
                TrayCmd::TogglePreview
            } else if e.id == ids.3 {
                TrayCmd::Quit
            } else {
                return;
            };
            let _ = tx.send(cmd);
            wake();
        }));
    }
    {
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let show = match e {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    cfg!(windows)
                }
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => true,
                _ => false,
            };
            if show {
                let _ = tx.send(TrayCmd::Show);
                wake();
            }
        }));
    }

    let mut b = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("glyphcam");
    if let Some(icon) = load_icon() {
        b = b.with_icon(icon);
    }
    #[cfg(target_os = "macos")]
    {
        b = b.with_icon_as_template(true).with_menu_on_left_click(true);
    }
    #[cfg(windows)]
    {
        b = b.with_menu_on_left_click(false);
    }
    let icon = b.build()?;
    Ok(Tray {
        _icon: icon,
        vcam_item,
    })
}
