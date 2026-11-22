//! Measure how much the Wrapped look flickers: mean frame-to-frame change of
//! the output over a stretch of a depth recording, for several Calm values
//! (Relief as a reference). Lower is steadier.
//!
//! cargo run -p glyphcam-core --release --example wrap_jitter -- <recording_dir> [start] [frames] [out_dir]
use glyphcam_core::settings::{Background, DepthMode, Settings};
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
    let runs: [(&str, DepthMode, f32); 6] = [
        ("relief", DepthMode::Relief, 0.0),
        ("relief-hide-bg", DepthMode::Relief, -1.0),
        ("wrapped-calm0", DepthMode::Wrapped, 0.0),
        ("wrapped-calm0.4", DepthMode::Wrapped, 0.4),
        ("wrapped-calm0.7", DepthMode::Wrapped, 0.7),
        ("wrapped-calm1", DepthMode::Wrapped, 1.0),
    ];
    for (name, mode, calm) in runs {
        let s = Settings {
            depth_mode: mode,
            wrap_calm: calm.max(0.0),
            // cutout flicker shows most against a hidden background
            background: if calm < 0.0 {
                Background::Hide
            } else {
                Background::Dim
            },
            ..Settings::default()
        };
        let mut p = Pipeline::new()?;
        p.synchronous = true;
        let warm = 30;
        let mut prev: Option<Vec<u8>> = None;
        let (mut sum, mut big, mut n) = (0.0f64, 0u64, 0u64);
        // the torso (lower middle of the output), where clothes are
        let (mut tsum, mut tn) = (0.0f64, 0u64);
        // cutout flicker: grid points whose person membership flips
        let mut prev_sub: Option<Vec<bool>> = None;
        let (mut flips, mut inside) = (0u64, 0u64);
        for i in start.saturating_sub(warm)..start + count {
            let f = src.frame_at(i % src.len())?;
            p.process(Some(&f), &s, 1.0 / 30.0);
            if i < start {
                continue;
            }
            let field = p.depth_field();
            let (sw, sh) = field.source_size();
            let mut sub = Vec::new();
            let mut y = 4.0;
            while y < sh {
                let mut x = 4.0;
                while x < sw {
                    sub.push(
                        field
                            .surface(x, y)
                            .map(|s| s.subject > 0.5)
                            .unwrap_or(false),
                    );
                    x += 8.0;
                }
                y += 8.0;
            }
            if let Some(ps) = &prev_sub {
                for (a, b) in sub.iter().zip(ps) {
                    flips += (a != b) as u64;
                    inside += (*a || *b) as u64;
                }
            }
            prev_sub = Some(sub);
            let rgb = p.frame().to_rgb();
            if let Some(pr) = &prev {
                let (w, h) = (p.frame().width as usize, p.frame().height as usize);
                for (k, (a, b)) in rgb.iter().zip(pr).enumerate() {
                    let d = (*a as i32 - *b as i32).unsigned_abs();
                    sum += d as f64;
                    big += (d > 64) as u64;
                    n += 1;
                    let (x, y) = ((k / 3) % w, (k / 3) / w);
                    if y > h * 6 / 10 && x > w / 4 && x < w * 3 / 4 {
                        tsum += d as f64;
                        tn += 1;
                    }
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
            "{name:18} mean_change={:.2}  torso_change={:.2}  big_changes={:.2}%  cutout_flips={:.2}%",
            sum / n.max(1) as f64,
            tsum / tn.max(1) as f64,
            100.0 * big as f64 / n.max(1) as f64,
            100.0 * flips as f64 / inside.max(1) as f64
        );
    }
    Ok(())
}
