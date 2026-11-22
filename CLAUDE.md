# glyphcam: notes for future work

Real-time glyph-art face camera with a virtual webcam. Rust workspace, Windows + macOS.

## Layout

| Crate | Role |
| --- | --- |
| `crates/glyphcam-core` | Pipeline (no platform code): `track.rs` (BlazeFace detection + selfie segmentation via `tract`, both on every frame, side by side on a worker; the pipeline waits up to 30 ms for the current frame's result so the cutout never lags the image), `framing.rs` (fixed fill + mirror transform; the old face-stabilizer "view model" was removed at the user's request), `render.rs` (cell sampling, face exposure plus a lift-only body exposure so dark clothes show, depth shading from a fitted head ellipsoid, digital rain, glyph-atlas blit into BGRA), `glyphs.rs` (font rasterization into a density-sorted atlas), `color.rs`, `settings.rs`, `source.rs` (Demo + Image test sources). |
| `crates/glyphcam-shm` | Shared-memory frame protocol (`Local\glyphcam-vcam-v1`, 3 seqlocked BGRA slots, heartbeats both ways) + Windows per-user COM registration of the filter (`win.rs`). The layout must stay identical for 32/64-bit. Bump `VERSION` + mapping name on any change. |
| `crates/glyphcam-vcam-dshow` | `glyphcam_vcam.dll`: DirectShow source filter loaded *inside* Discord/Zoom/OBS/browsers. Reads the shared memory and converts to YUY2/NV12/I420/RGB24/RGB32 at 4 sizes. It shows a placeholder card when the app isn't feeding frames. The vtable is patched so `GetState` returns `VFW_S_CANT_CUE` (live source). |
| `crates/glyphcam-vcam-mf` | `glyphcam_mfcam.dll`: Windows 11 Media Foundation virtual camera source (`IMFActivate` → `IMFMediaSourceEx` + `IMFMediaStream2`, ported from smourier/VCamSample). The Frame Server service loads it (session 0, LOCAL SERVICE), so it must be registered in **HKLM** and live in `%ProgramFiles%\glyphcam\camera\p<proto>-<build>\`. The app does that via an elevated `--install-system-camera` and creates the camera itself with `MFCreateVirtualCamera` (system lifetime, current user; no admin). Frames come through `Global\glyphcam-mf-v1`, which the source creates and the app polls for (`OpenFileMapping`). **Discord only sees this camera** (it enumerates via Media Foundation). |
| `crates/glyphcam` | The app: `engine.rs` (demand-driven loop: the camera opens only when the preview, a vcam reader or a stream client wants frames, and lingers 20 s after; it wakes the UI on status changes so the tray tooltip stays current), `camera.rs` (nokhwa capture + own NV12/YUYV/MJPEG decode), `vcam.rs`, `stream.rs` (MJPEG server on port 8737; localhost only unless `stream_lan` is set, which binds 0.0.0.0 and lists LAN/Tailscale addresses), `share.rs` (copy/paste settings code `glyphcam1:<base64 json>` with an allowlist of look/framing keys), `ui/` (control panel in GPUI + gpui-component via `gpui-kit`: `view.rs` layout, window lifecycle and preview; `tabs.rs` settings tabs; `widgets.rs` cards/rows/sliders/segmented/dropdowns bound to `Settings`; `theme.rs` colours, re-applied whenever gpui-component resets its theme), `tray.rs`, `updater.rs` (GitHub Releases build manager), `platform.rs` (single instance on 127.0.0.1:47380, autostart, macOS hide/show). |

Models and fonts are embedded with `include_bytes!` from `assets/` (see THIRD-PARTY.md). Icons come from `packaging/icons/make_icons.py`.

### Control panel (GPUI) notes

- The app runs in `QuitMode::Explicit`: hiding to the tray *closes* the window and showing it opens a new one around the same long-lived `GlyphUi` entity. Quit goes through `GlyphUi::quit` (saves, stops the engine, `cx.quit()`).
- Other threads (engine preview frames, tray, updater, single-instance "show") wake the UI through `ui::Waker` (a coalescing channel drained on the GPUI foreground executor).
- Preview frames become `RenderImage`s (BGRA); retired images are dropped with `window.drop_image` on the next render.
- Theme edits must go through `Theme::update` (components read derived tokens). GPUI embeds its own Windows manifest, so `packaging/windows/glyphcam.rc` only carries the icon.
- Only the icons we use are embedded (`UiAssets`: gpui-component's default set plus `assets/ui-icons/`).
- Drive the window from scripts for checks: `--tab look|camera|output|app` opens a given tab.

## Build / run / test

```sh
cargo run -p glyphcam --release
cargo test --workspace --release
cargo run -p glyphcam-core --release --example render_test -- <image.jpg|demo> <outdir>   # PNG snapshots, timings
target/release/glyphcam --selftest out.png      # headless pipeline check (used in CI)
target/release/glyphcam --register-vcam         # Windows: (re)register the filter for this user
target/release/glyphcam --install-release nightly   # run the updater non-interactively (download + install + relaunch)
target/release/glyphcam --tab app               # open on a given settings tab (screenshots)
```

Windows 11 camera checks (after `glyphcam --setup-system-camera`, which shows one UAC prompt):
```sh
cargo run --release -p glyphcam-vcam-mf --example mf_list -- glyphcam   # Media Foundation (Discord-style) device list + grab a frame
cargo run --release -p glyphcam-vcam-mf --example source_test           # drive the source in-process (no admin)
cargo run --release -p glyphcam-shm --example global_probe              # is the Global mapping there? (needs a consumer streaming)
ffmpeg -f dshow -i video="glyphcam (Windows Virtual Camera)" -frames:v 1 -update 1 out.png
glyphcam --remove-system-camera                                         # remove (UAC prompt for the HKLM part)
```
Note: heredocs in this environment's bash halve backslashes, so write Rust/NSIS paths with the editor tools rather than shell heredocs.

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

## Depth camera (Intel RealSense, Windows)

- `crates/glyphcam-core/src/depth.rs`: `DepthMap` (raw depth aligned to the colour frame) and `DepthField`, which hole-fills, smooths and temporally blends the depth on a 4-px grid, picks out the person by depth around the face *fused with the colour segmentation mask* (after 3 s with no face detected nobody counts as the person, so the whole frame gets the background treatment instead of the nearest chair), and builds surface (arc-length) coordinates for the Wrapped look. Depth replaces the selfie-segmentation mask and the ellipsoid head guess when present.
- Cutout rules (`DepthField::update`): depth in the person's band counts if the mask agrees; depth well behind you overrules the mask (kills its halos); no depth (dark fabric, hair, headphones absorb the SR305's IR; screens too) falls back to a confident mask, with depth grown in from the body around it. The mask is time-averaged only where neither the image nor the depth moves (a monitor the network flip-flops on), so moving people never trail; small stray pieces are dropped. Don't add hold/fade memory to the cutout: that was the ~0.75 s trail users saw. Jitter on dark fabric: spots that rarely have a reading (`rel` < 0.6) or disagree with their neighbours by > 3 cm are dropped and filled in; depth and the Wrapped surface are averaged over time only where the image is still (`mov`). The renderer filters camera grain per cell (small brightness changes are averaged, big ones pass) and blends face/body exposure by a smooth head ellipse (`head_w`), not a hard in-head test. Measure with `wrap_jitter` (`torso_change`) on a recording; `glyphcam-scratch/segm/darken_rec.py` makes a dark-hoodie copy of one (grain, dropouts, outliers). `depth_test` also dumps `cutout-<look>-<frame>.png` (green = membership, grey = depth).
- Looks (`DepthMode`): **Relief** (default; real-surface lighting), **Ridgelines** (scan lines lifted by depth, drawn as thin lines), **Wrapped** (experimental; per-pixel glyphs laid out on the skin in surface coordinates, rendered on all cores; the Calm setting `wrap_calm` smooths the surface coordinates, per-glyph tone and shimmer over time, and `--example wrap_jitter -- <recording_dir>` measures the flicker). Unknown saved names deserialize to Relief, so removed looks never reset the whole settings file.
- `crates/glyphcam/src/realsense.rs` loads `realsense2.dll` at runtime (`libloading`), streams 640x480 colour + Z16 depth at 30 fps, and aligns depth to colour in the SDK. The engine uses it when the selected camera's name contains "RealSense" and the DLL loads; otherwise it falls back to nokhwa.
- **SDK version matters.** The SR305 is dropped in librealsense ≥ 2.55, and 2.51–2.54 crash (access violation in `wait_for_frames`) on this Windows build. **2.48.0 works.** `scripts/build-realsense.cmd` builds it from source (static CRT, `/FI` force-includes for current MSVC) into `third_party/realsense/` (gitignored). CI caches that folder. `package-windows.sh` ships the DLL (required for nightly/stable builds).
- Test without the camera: `cargo run -p glyphcam-core --release --example depth_test -- <recording_dir> <out_dir> [frames]`. A recording is a folder with `meta.json`, `color/NNNNNN.jpg` and `depth/NNNNNN.png` (16-bit); the app can also play one (Camera tab → Depth recording). Live check: `glyphcam --depth-check out.png`.

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
