# Third-party assets

glyphcam bundles the following assets. Their licenses are included in
`assets/` and apply to those files only.

| Asset | Source | License |
| --- | --- | --- |
| `assets/models/face_detection_short_range.onnx`, `face_detection_anchors.csv` | Google MediaPipe BlazeFace (short range), ONNX conversion from [unity/inference-engine-blaze-face](https://huggingface.co/unity/inference-engine-blaze-face) | Apache-2.0 |
| `assets/models/selfie_segmentation_landscape.onnx` | Google MediaPipe Selfie Segmentation (landscape), ONNX conversion from [onnx-community/mediapipe_selfie_segmentation_landscape](https://huggingface.co/onnx-community/mediapipe_selfie_segmentation_landscape) (batch dimension fixed to 1) | Apache-2.0 |
| `assets/fonts/DejaVuSansMono.ttf`, `glyphcam-braille.ttf` (subset of DejaVu Sans) | [DejaVu fonts](https://dejavu-fonts.github.io/) | Bitstream Vera / DejaVu license (`assets/fonts/LICENSE-DejaVu.txt`) |
| `assets/fonts/glyphcam-kana.ttf` (half-width katakana subset of Noto Sans JP, weight 500) | [Noto Sans JP](https://fonts.google.com/noto/specimen/Noto+Sans+JP) | SIL Open Font License 1.1 (`assets/fonts/LICENSE-NotoSansJP-OFL.txt`) |

Rust crate dependencies are listed in `Cargo.lock`; notable ones include
`tract` (MIT/Apache-2.0), `egui`/`eframe` (MIT/Apache-2.0), `nokhwa`
(Apache-2.0), `tray-icon` (MIT/Apache-2.0) and `windows` (MIT/Apache-2.0).
