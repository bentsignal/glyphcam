//! Ties tracking, depth and rendering together.

use crate::depth::DepthField;
use crate::framing::FrameXf;
use crate::image::{Frame, RgbImage};
use crate::render::{Renderer, TrackView};
use crate::settings::Settings;
use crate::source::SourceFrame;
use crate::track::{Face, Tracker};
use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct PipelineStats {
    pub face_found: bool,
    /// Depth data arrived and produced a usable surface.
    pub depth_ok: bool,
    /// Distance to the face from the depth camera, metres (0 = unknown).
    pub depth_m: f32,
    pub detect_ms: f32,
    pub segment_ms: f32,
    pub render_ms: f32,
    /// Parts of `render_ms`: waiting for tracking, the depth field, drawing.
    pub wait_ms: f32,
    pub depth_ms: f32,
    pub draw_ms: f32,
}

pub struct Pipeline {
    tracker: Tracker,
    renderer: Renderer,
    depth: DepthField,
    frame: Frame,
    time: f64,
    /// Wait for tracking results each frame (deterministic, for tests).
    pub synchronous: bool,
    pub stats: PipelineStats,
}

impl Pipeline {
    pub fn new() -> Result<Self> {
        Ok(Self {
            tracker: Tracker::spawn()?,
            renderer: Renderer::new(),
            depth: DepthField::new(),
            frame: Frame::default(),
            time: 0.0,
            synchronous: false,
            stats: PipelineStats::default(),
        })
    }

    /// Forget temporal state (new source or a jump in the recording).
    pub fn reset_motion(&mut self) {
        self.depth.clear();
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// The depth camera surface (for tools and tests).
    pub fn depth_field(&self) -> &DepthField {
        &self.depth
    }

    /// Process one source frame (or `None` for "no signal") and return the
    /// rendered output.
    pub fn process(&mut self, src: Option<&SourceFrame>, s: &Settings, dt: f32) -> &Frame {
        self.time += dt as f64;
        let t0 = Instant::now();
        let img: Option<&Arc<RgbImage>> = src.map(|f| &f.image);
        let hinted = src.map(|f| f.face_hint.is_some()).unwrap_or(false);

        // The mask is wanted with a depth camera too: dark clothes barely
        // reflect its infrared, so the colour image fills in what depth misses.
        let st = match src.filter(|_| !hinted) {
            Some(f) => {
                self.tracker
                    .set_want_mask(f.depth.is_some() || s.background.needs_mask() || s.depth > 0.0);
                let seq = self.tracker.submit(f.image.clone());
                // Wait for this frame's result so the cutout and the image
                // line up (no trail behind you). If inference ever runs
                // slower than the frame rate, fall back to the latest result.
                let budget = if self.synchronous { 500 } else { 30 };
                self.tracker.wait_for(seq, Duration::from_millis(budget))
            }
            None => self.tracker.state(),
        };
        let t1 = Instant::now();
        let src_size = img
            .map(|i| (i.width, i.height))
            .unwrap_or((s.out_width, s.out_height));
        let (face, mask): (Option<Face>, _) = match src {
            Some(f) if hinted => (f.face_hint, f.mask_hint.clone()),
            Some(_) if st.src_size == src_size => (st.face, st.mask.clone()),
            _ => (None, None),
        };
        self.stats.face_found = face.is_some();
        self.stats.detect_ms = st.detect_ms;
        self.stats.segment_ms = st.segment_ms;

        let xf = FrameXf::fill(src_size, (s.out_width, s.out_height), s.mirror);
        match src.and_then(|f| f.depth.as_ref()) {
            Some(d) => {
                self.depth.update(
                    d,
                    face.as_ref(),
                    mask.as_deref(),
                    img.map(|i| i.as_ref()),
                    dt,
                );
                if s.depth_mode == crate::settings::DepthMode::Wrapped {
                    let (cx, cy) = face
                        .as_ref()
                        .map(|f| (f.cx, f.cy))
                        .unwrap_or((d.width as f32 * 0.5, d.height as f32 * 0.4));
                    self.depth.build_uv(cx, cy, s.wrap_calm, dt);
                }
            }
            None => self.depth.clear(),
        }
        let t2 = Instant::now();
        self.stats.depth_ok = self.depth.valid;
        self.stats.depth_m = if self.depth.valid {
            self.depth.z_ref
        } else {
            0.0
        };
        let view = TrackView {
            face: face.as_ref(),
            mask: mask.as_deref(),
            depth: Some(&self.depth).filter(|d| d.valid),
        };
        self.renderer.render(
            img.map(|i| i.as_ref()),
            &xf,
            &view,
            s,
            self.time,
            dt,
            &mut self.frame,
        );
        let ms = |a: Instant, b: Instant| (b - a).as_secs_f32() * 1000.0;
        let t3 = Instant::now();
        self.stats.wait_ms = ms(t0, t1);
        self.stats.depth_ms = ms(t1, t2);
        self.stats.draw_ms = ms(t2, t3);
        self.stats.render_ms = ms(t0, t3);
        &self.frame
    }
}
