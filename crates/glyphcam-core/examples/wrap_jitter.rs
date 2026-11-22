//! Measure how much the Wrapped look flickers: mean frame-to-frame change of
//! the output over a stretch of a depth recording, for several Calm values
//! (Relief as a reference). Lower is steadier.
//!
//! cargo run -p glyphcam-core --release --example wrap_jitter -- <recording_dir> [start] [frames] [out_dir]
use glyphcam_core::settings::{DepthMode, Settings};
use glyphcam_core::source::DepthRecordingSource;
use glyphcam_core::Pipeline;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let rec = args
        .get(1)
        .expect("usage: wrap_jitter <recording_dir> [start] [frames] [out_dir]");
    let start: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(300);
    let count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(120);
    let out_dir = args.get(4);
    if let Some(d) = out_dir {
        std::fs::create_dir_all(d)?;
    }
    let mut src = DepthRecordingSource::open(rec)?;
    let runs: [(&str, DepthMode, f32); 5] = [
        ("relief", DepthMode::Relief, 0.0),
        ("wrapped-calm0", DepthMode::Wrapped, 0.0),
        ("wrapped-calm0.4", DepthMode::Wrapped, 0.4),
        ("wrapped-calm0.7", DepthMode::Wrapped, 0.7),
        ("wrapped-calm1", DepthMode::Wrapped, 1.0),
    ];
    for (name, mode, calm) in runs {
        let s = Settings {
            depth_mode: mode,
            wrap_calm: calm,
            ..Settings::default()
        };
        let mut p = Pipeline::new()?;
        p.synchronous = true;
        let warm = 30;
        let mut prev: Option<Vec<u8>> = None;
        let (mut sum, mut big, mut n) = (0.0f64, 0u64, 0u64);
        for i in start.saturating_sub(warm)..start + count {
            let f = src.frame_at(i % src.len())?;
            p.process(Some(&f), &s, 1.0 / 30.0);
            if i < start {
                continue;
            }
            let rgb = p.frame().to_rgb();
            if let Some(pr) = &prev {
                for (a, b) in rgb.iter().zip(pr) {
                    let d = (*a as i32 - *b as i32).unsigned_abs();
                    sum += d as f64;
                    big += (d > 64) as u64;
                    n += 1;
                }
            }
            if let Some(d) = out_dir {
                if i < start + 6 {
                    let out = p.frame();
                    image::save_buffer(
                        format!("{d}/{name}-{i}.png"),
                        &rgb,
                        out.width,
                        out.height,
                        image::ColorType::Rgb8,
                    )?;
                }
            }
            prev = Some(rgb);
        }
        println!(
            "{name:18} mean_change={:.2}  big_changes={:.2}%",
            sum / n.max(1) as f64,
            100.0 * big as f64 / n.max(1) as f64
        );
    }
    Ok(())
}
