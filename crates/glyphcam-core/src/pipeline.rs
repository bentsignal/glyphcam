//! Ties tracking, stabilization and rendering together.

use crate::image::{Frame, RgbImage};
use crate::render::{Renderer, TrackView};
use crate::settings::Settings;
use crate::source::SourceFrame;
use crate::stabilize::{FaceObs, Stabilizer};
use crate::track::{Face, Tracker};
use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct PipelineStats {
    pub face_found: bool,
    pub detect_ms: f32,
    pub segment_ms: f32,
    pub render_ms: f32,
}

pub struct Pipeline {
    tracker: Tracker,
    stab: Stabilizer,
    renderer: Renderer,
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
            stab: Stabilizer::new(),
            renderer: Renderer::new(),
            frame: Frame::default(),
            time: 0.0,
            synchronous: false,
            stats: PipelineStats::default(),
        })
    }

    pub fn reset_motion(&mut self) {
        self.stab.reset();
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Process one source frame (or `None` for "no signal") and return the
    /// rendered output.
    pub fn process(&mut self, src: Option<&SourceFrame>, s: &Settings, dt: f32) -> &Frame {
        self.time += dt as f64;
        let t0 = Instant::now();
        let img: Option<&Arc<RgbImage>> = src.map(|f| &f.image);
        let hinted = src.map(|f| f.face_hint.is_some()).unwrap_or(false);

        if let Some(f) = src.filter(|_| !hinted) {
            self.tracker
                .set_want_mask(s.background.needs_mask() || s.depth > 0.0);
            self.tracker.submit(f.image.clone());
            if self.synchronous {
                self.tracker.wait_idle(Duration::from_millis(500));
                // give the worker a moment to publish
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let st = self.tracker.state();
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

        let xf = self.stab.update(
            dt,
            face.as_ref().map(FaceObs::from),
            src_size,
            (s.out_width, s.out_height),
            s,
        );
        let view = TrackView {
            face: face.as_ref(),
            mask: mask.as_deref(),
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
        self.stats.render_ms = t0.elapsed().as_secs_f32() * 1000.0;
        &self.frame
    }
}
