# glyphcam

A real-time glyph-art face camera. glyphcam takes your webcam feed, tracks your face, and renders it as Matrix-style glyph art. The glyphs mix katakana, Greek, Cyrillic, box drawing, math symbols, braille and Latin. It then exposes the result as a **virtual webcam**, so it shows up as a camera in Discord, OBS, Zoom, Teams and browsers.

- **Face-stabilized like a game view model.** Your face stays roughly fixed in the frame while you move around. It sways and lags a little, but it never leaves the frame. Expressions stay fully in sync because every glyph comes from the live camera image.
- **Shading and depth mapped to glyphs.** Brightness picks the glyph density. A 3D head model fitted to your face shades the result. Background separation can dim or hide the background, or replace it with falling digital rain.
- **Looks:** Matrix green, true color, monochrome, amber, ice, crimson, synthwave, thermal, or your own colors. Six glyph presets, or a custom mix of scripts.
- **Light enough to leave running.** It's written in Rust with pure-Rust inference (`tract`). The real camera only turns on while something is watching, such as Discord using the glyphcam camera, the preview or a stream viewer. It stays warm for 20 seconds afterwards. Idle in the tray it uses about 1–2% CPU and the webcam light is off. You can choose **Keep the webcam on** in Output if you prefer instant starts.
- **Control panel, not output window.** Toggle the virtual camera and the preview, and adjust settings. Closing the window hides glyphcam to the tray (Windows) or menu bar (macOS).
- **Built-in build manager.** Browse GitHub releases and the rolling `nightly` build, then install, update or downgrade in one click.

## Install

Download the latest build from **[Releases](https://github.com/bentsignal/glyphcam/releases)**.

### Windows 10 / 11

1. Run `glyphcam-<version>-windows-x64-setup.exe`. It installs per user, so you won't see an admin prompt.
   - SmartScreen may warn you because the installer isn't code-signed. Click **More info**, then **Run anyway**.
2. In Discord, Zoom, OBS (*Video Capture Device*), Teams, Chrome, Edge or Firefox, choose the camera **glyphcam**. Restart apps that were already open so they see the new camera.

The virtual camera is a DirectShow source filter, registered for your user, in both 64-bit and 32-bit versions. When glyphcam isn't running, the camera shows a "glyphcam is not running" card instead of failing.

### macOS 11+ (universal: Apple Silicon and Intel)

1. Open `glyphcam-<version>-macos-universal.dmg` and drag **glyphcam** into **Applications**.
2. The app is ad-hoc signed and not notarized, so macOS blocks the first launch. Go to **System Settings → Privacy & Security** and click **Open Anyway**. Alternatively, run:
   ```sh
   xattr -dr com.apple.quarantine /Applications/glyphcam.app
   ```
3. Allow camera access.

**Virtual camera on macOS:** modern macOS only accepts virtual cameras as a CoreMediaIO **Camera Extension**. That requires a paid Apple Developer account for the Developer ID certificate, the system-extension provisioning profile and notarization. Until then, use OBS's built-in, Apple-signed virtual camera:

1. In OBS, add a **Browser** source with URL `http://127.0.0.1:8737/`, sized 1280×720.
2. Click **Start Virtual Camera** in OBS.
3. Choose **OBS Virtual Camera** in Discord, Zoom, FaceTime or your browser.

The same local stream works on Windows too. Any app can read `http://127.0.0.1:8737/stream.mjpg` (MJPEG).

### Watching from another computer

To run glyphcam on one machine (the one with the webcam) and use it on another:

1. On the glyphcam machine, open **Output → Web stream** and turn on **Allow other devices on my network**. glyphcam shows addresses such as `http://192.168.1.20:8737/`, plus your Tailscale address if you use Tailscale.
2. On the other computer, add a **Browser** source in OBS with that address (1280×720), then click **Start Virtual Camera** to use it in Discord or Zoom. You can also just open the address in a browser.
3. If the other computer can't connect, allow glyphcam through the firewall:
   - Windows asks the first time you turn on network access. You can also click **Allow through Windows Firewall** in glyphcam.
   - On macOS, accept the "incoming connections" prompt.

Anyone on your network can watch while network access is on. It is off by default.

### Copying your setup to another machine

Click **Copy settings** (in **App**, or at the bottom of **Look**) to copy a code. Send it to yourself, then click **Paste settings** in glyphcam on the other machine. The code carries your look, framing and output format. Camera choice and app options stay per computer.

## Using it

| Control | What it does |
| --- | --- |
| **Virtual camera ON/OFF** | When OFF, apps see a "paused" card. |
| **Preview ON/OFF** | Shows the output in the window. Turning it off saves CPU. |
| **Look** | Color mode, glyph scripts, density, tone, depth shading, glow, shimmer and background treatment. |
| **Framing** | Stabilization on/off, face size and position, sway, tilt, idle drift, keep upright, mirror. |
| **Camera** | Camera device and quality, or **Demo** / **Image file** inputs for testing without a webcam. |
| **Output** | Resolution (640×360 to 1920×1080), fps, virtual camera status and repair, local stream server. |
| **App** | Copy/paste settings between machines, start at login, start hidden, close-to-tray, logs. |
| **Updates** | List, install, update or switch builds from GitHub. |

## Building from source

Requires a recent stable Rust toolchain.

```sh
cargo run -p glyphcam --release            # the app
cargo test --workspace --release           # tests
cargo run -p glyphcam-core --release --example render_test -- demo out/   # headless render check
glyphcam --selftest out.png                # headless check of a built binary
```

Packaging: `scripts/package-windows.sh` (needs NSIS) and `scripts/package-macos.sh` (needs Xcode command-line tools). See [CLAUDE.md](CLAUDE.md) for architecture, release process and verification notes.

## License

MIT. Bundled models and fonts are under their own licenses; see [THIRD-PARTY.md](THIRD-PARTY.md).
