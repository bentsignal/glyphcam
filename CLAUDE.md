# glyphcam: notes for future work

Real-time glyph-art face camera with a virtual webcam. Rust workspace, Windows + macOS.

## Layout

| Crate | Role |
| --- | --- |
| `crates/glyphcam-core` | Pipeline (no platform code): `track.rs` (BlazeFace detection + selfie segmentation via `tract`, on a worker thread), `stabilize.rs` (spring-damper "view model" rig with a hard clamp so the face never leaves the frame), `render.rs` (cell sampling, exposure, depth shading from a fitted head ellipsoid, digital rain, glyph-atlas blit into BGRA), `glyphs.rs` (font rasterization into a density-sorted atlas), `color.rs`, `settings.rs`, `source.rs` (Demo + Image test sources). |
| `crates/glyphcam-shm` | Shared-memory frame protocol (`Local\glyphcam-vcam-v1`, 3 seqlocked BGRA slots, heartbeats both ways) + Windows per-user COM registration of the filter (`win.rs`). The layout must stay identical for 32/64-bit. Bump `VERSION` + mapping name on any change. |
| `crates/glyphcam-vcam-dshow` | `glyphcam_vcam.dll`: DirectShow source filter loaded *inside* Discord/Zoom/OBS/browsers. Reads the shared memory and converts to YUY2/NV12/I420/RGB24/RGB32 at 4 sizes. It shows a placeholder card when the app isn't feeding frames. The vtable is patched so `GetState` returns `VFW_S_CANT_CUE` (live source). |
| `crates/glyphcam` | The app: `engine.rs` (demand-driven loop: the camera opens only when the preview, a vcam reader or a stream client wants frames), `camera.rs` (nokhwa capture + own NV12/YUYV/MJPEG decode), `vcam.rs`, `stream.rs` (localhost MJPEG server, port 8737), `ui.rs` (egui control panel), `tray.rs`, `updater.rs` (GitHub Releases build manager), `platform.rs` (single instance on 127.0.0.1:47380, autostart, macOS hide/show). |

Models and fonts are embedded with `include_bytes!` from `assets/` (see THIRD-PARTY.md). Icons come from `packaging/icons/make_icons.py`.

## Build / run / test

```sh
cargo run -p glyphcam --release
cargo test --workspace --release
cargo run -p glyphcam-core --release --example render_test -- <image.jpg|demo> <outdir>   # PNG snapshots, timings
target/release/glyphcam --selftest out.png      # headless pipeline check (used in CI)
target/release/glyphcam --register-vcam         # Windows: (re)register the filter for this user
target/release/glyphcam --install-release nightly   # run the updater non-interactively (download + install + relaunch)
target/release/glyphcam --tab updates           # open on a given settings tab (screenshots)
```

Browser check (Chromium path, covers Chrome/Edge/Teams/Meet): drive the installed Edge with Playwright
(`p.chromium.launch(channel="msedge", args=["--use-fake-ui-for-media-stream"])`) on a page that calls
`enumerateDevices()` and draws a frame from the "glyphcam" device.

The `[profile.dev.package."*"] opt-level = 3` setting keeps inference usable in debug builds. Workspace crates themselves stay unoptimized in dev, so the DLL is slow in debug; test virtual-camera throughput with `--release`.

### Testing the Windows virtual camera without Discord

```sh
cargo build --release -p glyphcam-vcam-dshow
regsvr32 /s target\release\glyphcam_vcam.dll          # per-user registration (HKCU), no admin
ffmpeg -list_devices true -f dshow -i dummy           # "glyphcam" should be listed
ffmpeg -f dshow -list_options true -i video=glyphcam  # 5 pixel formats x 4 sizes
cargo run --release -p glyphcam-shm --example write_test 20 &   # test pattern writer
ffmpeg -f dshow -pixel_format bgr24 -video_size 640x360 -i video=glyphcam -frames:v 5 -update 1 out.png
```

The dev machine has no physical webcam. Its "N-LOG" camera is the user's own in-development app: don't use it for testing. Use the Demo / Image sources (and `write_test` for the filter) instead. "Automatic" camera selection skips names that look virtual.

## Packaging & releases

- `scripts/package-windows.sh`: builds the x64 app, x64 and x86 filter DLLs, and the NSIS installer `dist/glyphcam-<label>-windows-x64-setup.exe` (`packaging/windows/installer.nsi`). The installer is per-user (`%LOCALAPPDATA%\Programs\glyphcam`). DLLs go into `vcam\<version>-<sha>\` so updates never overwrite a DLL that's loaded in Discord etc. `setup.exe /S /RELAUNCH` is the silent update the app uses.
- `scripts/package-macos.sh`: builds the universal `glyphcam.app` (lipo), ad-hoc codesign, `.dmg`, and `.app.zip` (the updater uses the zip).
- CI (`.github/workflows/build.yml`) runs on push, PR and tag. Every push to `main` replaces the public pre-release **`nightly`**. Pushing a tag `vX.Y.Z` creates a public release; it must match `version` in the root `Cargo.toml`, which `scripts/ci-env.sh` enforces. Notes come from `.github/release-notes.md`.
- To cut a release: bump `[workspace.package] version` in `Cargo.toml`, commit, `git tag vX.Y.Z && git push origin main vX.Y.Z`.
- Build identity: `GLYPHCAM_CHANNEL` (stable/nightly/dev) and `GLYPHCAM_BUILD_SHA` are baked in by `crates/glyphcam/build.rs`. The updater uses them to mark the running build. Asset names must keep the suffixes `-windows-x64-setup.exe` and `-macos-universal.app.zip`, because `updater.rs` matches on them.

## macOS virtual camera (not done; needs the user's paid Apple account)

A real system camera on macOS 12.3+ means a CoreMediaIO Camera Extension (system extension) embedded in the app. That needs:
- Apple Developer Program membership.
- A Developer ID Application certificate.
- Provisioning profiles for the app (with `com.apple.developer.system-extension.install`) and the extension.
- Hardened runtime.
- Notarization.
- The app must live in /Applications, and the user approves the extension in System Settings.

The extension would be Swift (CMIOExtension provider/device/stream, plus a sink stream), fed from the app. Until then, the MJPEG server + OBS Virtual Camera is the documented route.

## Verified vs not (as of v0.1.0)

Verified on the Windows build machine:
- The DirectShow filter enumerates and streams at 30 fps at 720p/1080p in all formats via ffmpeg.
- Per-user registration works, and the 32-bit DLL loads (SysWOW64 regsvr32).
- Install, uninstall and silent install all work.
- App → virtual camera → consumer chain, demand-driven camera start, MJPEG server, and the core pipeline on a real photo.
- Microsoft Edge (Chromium) lists and streams the camera at 1280×720 / 30 fps.
- The CI-built nightly installer, and the in-app updater installing it (download, silent install, re-registration, relaunch).
- macOS CI: universal build, and `--selftest` passes on arm64 and on x86_64 under Rosetta.

Not verified here: a physical webcam, the Discord/Zoom/OBS UIs, anything interactive on macOS (window, tray, camera permission, AVFoundation capture), and the macOS updater swap.
