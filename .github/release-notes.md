## glyphcam @TAG@

Live glyph-art face camera with a virtual webcam. Build `@SHA@`.

### Windows 10/11 (x64)
1. Download **`glyphcam-@VERSION@-windows-x64-setup.exe`** and run it. It installs for your user only, so no admin prompt appears.
   - SmartScreen may say "Windows protected your PC" because the installer isn't code-signed yet. Click **More info**, then **Run anyway**.
2. glyphcam opens. Closing the window keeps it running in the tray.
3. In Discord, Zoom, OBS ("Video Capture Device"), Teams or your browser, pick the camera named **glyphcam**. Restart any app that was already open.

### macOS 11+ (Apple Silicon and Intel)
1. Download **`glyphcam-@VERSION@-macos-universal.dmg`**, open it and drag **glyphcam** to **Applications**.
2. The app isn't notarized yet, so the first launch is blocked. Open **System Settings, then Privacy & Security**, scroll down and click **Open Anyway**. Or run `xattr -dr com.apple.quarantine /Applications/glyphcam.app`.
3. Allow camera access when asked.
4. **Virtual camera on macOS:** a system camera requires an Apple-signed Camera Extension, which isn't in this build. For now, in OBS add a **Browser** source with `http://127.0.0.1:8737/` (1280×720), then click **Start Virtual Camera**. Then pick "OBS Virtual Camera" in any app.

### Updating
glyphcam's **Updates** tab lists every release plus the rolling `nightly` build. It installs any of them in one click, including downgrades.
