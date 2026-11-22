# Third-party assets

glyphcam bundles the following assets. Their licenses are included in
`assets/` and apply to those files only.

| Asset | Source | License |
| --- | --- | --- |
| `assets/models/face_detection_short_range.onnx`, `face_detection_anchors.csv` | Google MediaPipe BlazeFace (short range), ONNX conversion from [unity/inference-engine-blaze-face](https://huggingface.co/unity/inference-engine-blaze-face) | Apache-2.0 |
| `assets/models/selfie_segmentation_landscape.onnx` | Google MediaPipe Selfie Segmentation (landscape), ONNX conversion from [onnx-community/mediapipe_selfie_segmentation_landscape](https://huggingface.co/onnx-community/mediapipe_selfie_segmentation_landscape) (batch dimension fixed to 1) | Apache-2.0 |
| `assets/fonts/DejaVuSansMono.ttf`, `glyphcam-braille.ttf` (subset of DejaVu Sans) | [DejaVu fonts](https://dejavu-fonts.github.io/) | Bitstream Vera / DejaVu license (`assets/fonts/LICENSE-DejaVu.txt`) |
| `assets/fonts/glyphcam-kana.ttf` (half-width katakana subset of Noto Sans JP, weight 500) | [Noto Sans JP](https://fonts.google.com/noto/specimen/Noto+Sans+JP) | SIL Open Font License 1.1 (`assets/fonts/LICENSE-NotoSansJP-OFL.txt`) |

The Windows build also ships `realsense2.dll`, Intel RealSense SDK
([librealsense](https://github.com/IntelRealSense/librealsense) v2.48.0,
Apache-2.0), for depth cameras. It is built from source by
`scripts/build-realsense.cmd`; its license and notice files are installed
next to it (`LICENSE-librealsense.txt`, `NOTICE-librealsense.txt`).

Rust crate dependencies are listed in `Cargo.lock`; notable ones include
`tract` (MIT/Apache-2.0), `egui`/`eframe` (MIT/Apache-2.0), `nokhwa`
(Apache-2.0), `tray-icon` (MIT/Apache-2.0) and `windows` (MIT/Apache-2.0).

## Lucide icons

UI icons (`assets/ui-icons/` and the icons bundled by gpui-component) are from [Lucide](https://lucide.dev), via gpui-kit-assets.

```
ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

---

The following Lucide icons are derived from the Feather project:

airplay, alert-circle, alert-octagon, alert-triangle, aperture, arrow-down-circle, arrow-down-left, arrow-down-right, arrow-down, arrow-left-circle, arrow-left, arrow-right-circle, arrow-right, arrow-up-circle, arrow-up-left, arrow-up-right, arrow-up, at-sign, calendar, cast, check, chevron-down, chevron-left, chevron-right, chevron-up, chevrons-down, chevrons-left, chevrons-right, chevrons-up, circle, clipboard, clock, code, columns, command, compass, corner-down-left, corner-down-right, corner-left-down, corner-left-up, corner-right-down, corner-right-up, corner-up-left, corner-up-right, crosshair, database, divide-circle, divide-square, dollar-sign, download, external-link, feather, frown, hash, headphones, help-circle, info, italic, key, layout, life-buoy, link-2, link, loader, lock, log-in, log-out, maximize, meh, minimize, minimize-2, minus-circle, minus-square, minus, monitor, moon, more-horizontal, more-vertical, move, music, navigation-2, navigation, octagon, pause-circle, percent, plus-circle, plus-square, plus, power, radio, rss, search, server, share, shopping-bag, sidebar, smartphone, smile, square, table-2, tablet, target, terminal, trash-2, trash, triangle, tv, type, upload, x-circle, x-octagon, x-square, x, zoom-in, zoom-out

The MIT License (MIT) (for the icons listed above)

Copyright (c) 2013-present Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## GPUI / gpui-component

The control panel uses [GPUI](https://github.com/zed-industries/zed) (Apache-2.0) and [gpui-component / GPUI Kit](https://github.com/longbridge/gpui-kit) (Apache-2.0).
