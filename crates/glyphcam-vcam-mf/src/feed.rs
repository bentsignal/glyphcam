//! Where frames come from: glyphcam's machine-wide shared memory (or a
//! placeholder card), converted to the negotiated NV12/RGB32 format.

use glyphcam_shm::win::SharedMapping;
use glyphcam_shm::{placeholder, WriterState};
use std::time::{Duration, Instant};

pub struct Feed {
    map: Option<SharedMapping>,
    /// When the mapping was connected (glyphcam needs a moment to notice).
    since: Option<Instant>,
    last_try: Option<Instant>,
    last_no: u64,
    frame: Vec<u8>,
    fw: usize,
    fh: usize,
    have_frame: bool,
    ph: Option<(WriterState, usize, usize, Vec<u8>)>,
}

impl Feed {
    pub fn new() -> Self {
        Feed {
            map: None,
            since: None,
            last_try: None,
            last_no: 0,
            frame: Vec::new(),
            fw: 0,
            fh: 0,
            have_frame: false,
            ph: None,
        }
    }

    fn ensure_map(&mut self) {
        if self.map.is_some()
            || self
                .last_try
                .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.last_try = Some(Instant::now());
        // Inside the Frame Server (session 0) we can create it; elsewhere we
        // can only open it if it exists.
        self.map = SharedMapping::create_global()
            .or_else(|_| SharedMapping::open_global())
            .ok();
        if self.map.is_some() {
            self.since = Some(Instant::now());
        }
    }

    /// Latest BGRA frame (top-down) to show at `w x h`.
    pub fn current(&mut self, w: usize, h: usize) -> (&[u8], usize, usize) {
        self.ensure_map();
        let state = match &self.map {
            Some(m) => {
                let r = m.region();
                r.touch_reader(w as u32, h as u32);
                r.writer_state()
            }
            None => WriterState::Offline,
        };
        if state == WriterState::Live {
            if let Some(m) = &self.map {
                let frame = &mut self.frame;
                if let Some((n, (fw, fh))) = m.region().read_latest(self.last_no, |fw, fh, data| {
                    frame.clear();
                    frame.extend_from_slice(data);
                    (fw as usize, fh as usize)
                }) {
                    self.last_no = n;
                    self.fw = fw;
                    self.fh = fh;
                    self.have_frame = true;
                }
            }
            if self.have_frame {
                return (&self.frame, self.fw, self.fh);
            }
        } else {
            self.have_frame = false;
            self.last_no = 0;
        }
        // Right after connecting, glyphcam hasn't had a chance to answer yet.
        let fresh = self
            .since
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        let shown = match state {
            WriterState::Live => WriterState::Starting,
            WriterState::Offline if fresh => WriterState::Starting,
            s => s,
        };
        let stale =
            !matches!(&self.ph, Some((s, pw, ph, _)) if *s == shown && *pw == w && *ph == h);
        if stale {
            self.ph = Some((shown, w, h, placeholder::render(shown, w, h)));
        }
        let (_, pw, ph, data) = self.ph.as_ref().unwrap();
        (data, *pw, *ph)
    }
}

/// Nearest-neighbour letterbox mapping from target to source pixels.
fn maps(sw: usize, sh: usize, w: usize, h: usize) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let scale = (sw as f32 / w as f32).max(sh as f32 / h as f32);
    let ox = (w as f32 - sw as f32 / scale) * 0.5;
    let oy = (h as f32 - sh as f32 / scale) * 0.5;
    let xm = (0..w)
        .map(|x| {
            let s = ((x as f32 + 0.5 - ox) * scale).floor();
            (s >= 0.0 && (s as usize) < sw).then_some(s as usize)
        })
        .collect();
    let ym = (0..h)
        .map(|y| {
            let s = ((y as f32 + 0.5 - oy) * scale).floor();
            (s >= 0.0 && (s as usize) < sh).then_some(s as usize)
        })
        .collect();
    (xm, ym)
}

/// # Safety
/// `dst` must point to `h` rows of at least `w * 4` bytes, `pitch` apart.
pub unsafe fn write_rgb32(
    dst: *mut u8,
    pitch: isize,
    w: usize,
    h: usize,
    src: &[u8],
    sw: usize,
    sh: usize,
) {
    let (xm, ym) = maps(sw, sh, w, h);
    for y in 0..h {
        let row = std::slice::from_raw_parts_mut(dst.offset(pitch * y as isize), w * 4);
        match ym[y] {
            None => row.fill(0),
            Some(sy) => {
                let srow = &src[sy * sw * 4..(sy + 1) * sw * 4];
                for x in 0..w {
                    let o = x * 4;
                    match xm[x] {
                        Some(sx) => {
                            row[o..o + 3].copy_from_slice(&srow[sx * 4..sx * 4 + 3]);
                            row[o + 3] = 255;
                        }
                        None => row[o..o + 4].copy_from_slice(&[0, 0, 0, 255]),
                    }
                }
            }
        }
    }
}

#[inline]
fn yuv(b: u8, g: u8, r: u8) -> (u8, u8, u8) {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (
        y.clamp(0, 255) as u8,
        u.clamp(0, 255) as u8,
        v.clamp(0, 255) as u8,
    )
}

/// # Safety
/// `dst` must hold an NV12 image: `h` luma rows then `h/2` interleaved
/// chroma rows, all `pitch` bytes apart.
pub unsafe fn write_nv12(
    dst: *mut u8,
    pitch: isize,
    w: usize,
    h: usize,
    src: &[u8],
    sw: usize,
    sh: usize,
) {
    let (xm, ym) = maps(sw, sh, w, h);
    let px = |x: usize, y: usize| -> [u8; 3] {
        match (xm[x], ym[y]) {
            (Some(sx), Some(sy)) => {
                let i = (sy * sw + sx) * 4;
                [src[i], src[i + 1], src[i + 2]]
            }
            _ => [0, 0, 0],
        }
    };
    for y in 0..h {
        let row = std::slice::from_raw_parts_mut(dst.offset(pitch * y as isize), w);
        for (x, out) in row.iter_mut().enumerate() {
            let p = px(x, y);
            *out = yuv(p[0], p[1], p[2]).0;
        }
    }
    let uv = dst.offset(pitch * h as isize);
    for cy in 0..h / 2 {
        let row = std::slice::from_raw_parts_mut(uv.offset(pitch * cy as isize), w);
        for cx in 0..w / 2 {
            let (mut us, mut vs) = (0u32, 0u32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px((cx * 2 + dx).min(w - 1), (cy * 2 + dy).min(h - 1));
                let (_, u, v) = yuv(p[0], p[1], p[2]);
                us += u as u32;
                vs += v as u32;
            }
            row[cx * 2] = (us / 4) as u8;
            row[cx * 2 + 1] = (vs / 4) as u8;
        }
    }
}
