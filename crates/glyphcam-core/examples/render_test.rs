//! Headless pipeline check: renders frames from the demo source or an image
//! (with simulated motion) and writes PNG snapshots plus timing stats.
//!
//! cargo run -p glyphcam-core --release --example render_test -- [image.jpg] [out_dir]
use glyphcam_core::settings::{Background, ColorMode, GlyphPreset, Settings};
use glyphcam_core::source::{DemoSource, ImageSource};
use glyphcam_core::Pipeline;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let img_path = args.get(1).filter(|a| a.as_str() != "demo").cloned();
    let out_dir = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "render-test-out".into());
    std::fs::create_dir_all(&out_dir)?;
    let mut p = Pipeline::new()?;
    p.synchronous = true;
    let demo = DemoSource::new(640, 480);
    let imgsrc = img_path
        .as_deref()
        .map(|p| ImageSource::open(p, 640, 480))
        .transpose()?;
    let looks: [(&str, ColorMode, GlyphPreset, Background); 5] = [
        (
            "matrix",
            ColorMode::Matrix,
            GlyphPreset::Matrix,
            Background::Rain,
        ),
        (
            "truecolor",
            ColorMode::TrueColor,
            GlyphPreset::Polyglot,
            Background::Show,
        ),
        (
            "mono",
            ColorMode::Mono,
            GlyphPreset::Ascii,
            Background::Hide,
        ),
        (
            "synth",
            ColorMode::Synthwave,
            GlyphPreset::GreekMath,
            Background::Dim,
        ),
        (
            "amber",
            ColorMode::Amber,
            GlyphPreset::Blocks,
            Background::Show,
        ),
    ];
    let mut total_ms = 0.0f32;
    let mut frames = 0;
    let mut faces = 0;
    for (name, mode, preset, bg) in looks {
        let s = Settings {
            color_mode: mode,
            scripts: preset.scripts(),
            background: bg,
            ..Settings::default()
        };
        p.reset_motion();
        for i in 0..45 {
            let t = i as f64 / 30.0;
            let f = match &imgsrc {
                Some(src) => src.frame(t, 1.0),
                None => demo.frame(t),
            };
            let t0 = Instant::now();
            p.process(Some(&f), &s, 1.0 / 30.0);
            total_ms += t0.elapsed().as_secs_f32() * 1000.0;
            frames += 1;
            if p.stats.face_found {
                faces += 1;
            }
            let out = p.frame();
            if i == 44 {
                let rgb = out.to_rgb();
                image::save_buffer(
                    format!("{out_dir}/{name}.png"),
                    &rgb,
                    out.width,
                    out.height,
                    image::ColorType::Rgb8,
                )?;
                image::save_buffer(
                    format!("{out_dir}/{name}-src.png"),
                    &f.image.data,
                    f.image.width,
                    f.image.height,
                    image::ColorType::Rgb8,
                )?;
            }
        }
    }
    println!(
        "frames={frames} faces_found={faces} avg_ms={:.2} detect_ms={:.2} segment_ms={:.2}",
        total_ms / frames as f32,
        p.stats.detect_ms,
        p.stats.segment_ms
    );
    Ok(())
}
