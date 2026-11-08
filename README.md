# glyphcam

A real-time ASCII/glyph face camera. glyphcam takes your webcam feed, tracks your face, and renders it as Matrix-style glyph art (Latin, katakana, Greek, Cyrillic, box-drawing, symbols…). It then exposes the result as a **virtual webcam**, so it shows up as a camera in Discord, OBS, Zoom, and anything else.

Status: under construction.

## Goals

- **Cross-platform:** macOS and Windows.
- **Fast and light:** written in Rust, uses few resources, and can run in the background all day.
- **Face-stabilized:** your face stays roughly centered with a gentle, game-viewmodel-style sway and never leaves the frame. Expressions stay fully in sync.
- **Shading and depth mapped to glyph density**, with color modes: true color, Matrix green, monochrome, and custom palettes.
- **Virtual camera output** for Discord, OBS, Zoom, and similar apps.
- **Control panel UI:** start/stop the virtual cam, quick preview on/off, settings. Minimizes to the tray or dock.
- **Built-in updater:** browse GitHub Releases builds, download them, and switch between versions.
