//! glyphcam's dark, green-on-black theme on top of gpui-component's.

use gpui::{px, rgb, rgba, App, Hsla};
use gpui_kit::component::{Theme, ThemeMode};

pub fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

pub fn hexa(c: u32) -> Hsla {
    rgba(c).into()
}

/// Brand accent (the glyph green).
pub fn accent() -> Hsla {
    hex(0x35e07a)
}
pub fn accent_soft() -> Hsla {
    hexa(0x35e07a26)
}
pub fn warn() -> Hsla {
    hex(0xffbe5c)
}
pub fn danger() -> Hsla {
    hex(0xff7a66)
}
/// Window background.
pub fn bg() -> Hsla {
    hex(0x0a0d0c)
}
/// Right-hand settings panel.
pub fn panel() -> Hsla {
    hex(0x0f1412)
}
/// Cards.
pub fn card() -> Hsla {
    hex(0x151c19)
}
pub fn card_hover() -> Hsla {
    hex(0x1a2420)
}
pub fn border() -> Hsla {
    hex(0x24302b)
}
pub fn text() -> Hsla {
    hex(0xe8efeb)
}
pub fn muted() -> Hsla {
    hex(0x8da299)
}

/// Install the theme and keep it: gpui-component re-applies its stock
/// colours when a window follows the system appearance, so put ours back
/// whenever the theme changes underneath us.
pub fn apply(cx: &mut App) {
    set(cx);
    cx.observe_global::<Theme>(|cx| {
        let t = Theme::global(cx);
        if !t.mode.is_dark() || t.primary != accent() || t.background != bg() {
            set(cx);
        }
    })
    .detach();
}

fn set(cx: &mut App) {
    if !Theme::global(cx).mode.is_dark() {
        Theme::change(ThemeMode::Dark, None, cx);
    }
    Theme::update(cx, |t| {
        t.font_size = px(15.);
        t.radius = px(8.);
        t.background = bg();
        t.foreground = text();
        t.border = border();
        t.muted = card_hover();
        t.muted_foreground = muted();
        t.primary = accent();
        t.primary_hover = hex(0x4cec8d);
        t.primary_active = hex(0x2bc86b);
        t.primary_foreground = hex(0x04140b);
        t.button_primary = accent();
        t.button_primary_hover = hex(0x4cec8d);
        t.button_primary_active = hex(0x2bc86b);
        t.button_primary_foreground = hex(0x04140b);
        t.ring = accent();
        t.switch = hex(0x2a3631);
        t.slider_bar = accent();
        t.slider_thumb = hex(0xf2fff7);
        t.progress_bar = accent();
        t.selection = hexa(0x35e07a40);
        t.popover = hex(0x141b18);
        t.input = border();
        t.secondary = hex(0x1b2420);
        t.secondary_hover = hex(0x222d29);
        t.secondary_active = hex(0x283531);
        t.secondary_foreground = text();
        t.tab_bar = hex(0x0f1412);
        t.tab_bar_segmented = hex(0x141b18);
        t.tab_active = hex(0x223029);
        t.tab_active_foreground = text();
        t.tab_foreground = muted();
        t.accent = card_hover();
        t.accent_foreground = text();
        t.list_hover = card_hover();
        t.list_active = hexa(0x35e07a26);
        t.scrollbar_thumb = hex(0x2c3a34);
        t.scrollbar_thumb_hover = hex(0x3a4b44);
    });
}
