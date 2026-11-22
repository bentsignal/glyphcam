//! Render a colour + depth recording through every depth look and write PNG
//! snapshots plus timings.
//!
//! cargo run -p glyphcam-core --release --example depth_test -- <recording_dir> <out_dir> [frame,frame,...]
use glyphcam_core::settings::{Background, ColorMode, DepthMode, GlyphPreset, Settings};
use glyphcam_core::source::DepthRecordingSource;
use glyphcam_core::Pipeline;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let rec = args
        .get(1)
        .expect("usage: depth_test <recording_dir> <out_dir> [frames]");
    let out_dir = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "depth-test-out".into());
    let shots: Vec<usize> = args
        .get(3)
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| vec![300]);
    std::fs::create_dir_all(&out_dir)?;
    let mut src = DepthRecordingSource::open(rec)?;
    let mut p = Pipeline::new()?;
    p.synchronous = true;

    let looks: [(&str, DepthMode, ColorMode, GlyphPreset, Background); 3] = [
        (
            "relief",
            DepthMode::Relief,
            ColorMode::Ice,
            GlyphPreset::Matrix,
            Background::Dim,
        ),
        (
            "ridges",
            DepthMode::Ridges,
            ColorMode::Amber,
            GlyphPreset::Ascii,
            Background::Hide,
        ),
        (
            "wrapped",
            DepthMode::Wrapped,
            ColorMode::Ice,
            GlyphPreset::Matrix,
            Background::Dim,
        ),
    ];
    for (name, mode, color, preset, bg) in looks {
        let s = Settings {
            depth_mode: mode,
            color_mode: color,
            scripts: preset.scripts(),
            background: bg,
            ..Settings::default()
        };
        p.reset_motion();
        let (mut total, mut frames, mut faces) = (0.0f32, 0u32, 0u32);
        let last = *shots.iter().max().unwrap();
        // warm up the stabilizer for a second before the first shot
        let first = shots.iter().min().unwrap().saturating_sub(30);
        for i in first..=last {
            let f = src.frame_at(i % src.len())?;
            let t0 = Instant::now();
            p.process(Some(&f), &s, 1.0 / 30.0);
            total += t0.elapsed().as_secs_f32() * 1000.0;
            frames += 1;
            faces += p.stats.face_found as u32;
            if shots.contains(&i) {
                let out = p.frame();
                image::save_buffer(
                    format!("{out_dir}/{name}-{i}.png"),
                    &out.to_rgb(),
                    out.width,
                    out.height,
                    image::ColorType::Rgb8,
                )?;
            }
        }
        println!(
            "{name:16} frames={frames} faces={faces} avg_ms={:.2} render_ms={:.2} depth_ok={} face_at={:.2}m",
            total / frames as f32,
            p.stats.render_ms,
            p.stats.depth_ok,
            p.stats.depth_m
        );
    }
    Ok(())
}
