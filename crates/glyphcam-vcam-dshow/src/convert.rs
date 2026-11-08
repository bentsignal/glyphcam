//! BGRA (top-down) -> negotiated output format, with nearest-neighbour
//! scaling when the app renders at a different size than negotiated.

use crate::media::{Fmt, Pix};

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

/// Convert `src` (`sw x sh` BGRA, top-down) into `dst` laid out as `f`.
pub fn convert(src: &[u8], sw: usize, sh: usize, f: &Fmt, dst: &mut [u8]) {
    let (w, h) = (f.w as usize, f.h as usize);
    if dst.len() < f.image_size() || src.len() < sw * sh * 4 || sw == 0 || sh == 0 {
        return;
    }
    // Fit (letterbox) the source into the target while preserving aspect.
    let scale = (sw as f32 / w as f32).max(sh as f32 / h as f32);
    let ox = (w as f32 - sw as f32 / scale) * 0.5;
    let oy = (h as f32 - sh as f32 / scale) * 0.5;
    let xmap: Vec<Option<usize>> = (0..w)
        .map(|x| {
            let sx = ((x as f32 + 0.5 - ox) * scale).floor();
            (sx >= 0.0 && (sx as usize) < sw).then_some(sx as usize)
        })
        .collect();
    let ymap: Vec<Option<usize>> = (0..h)
        .map(|y| {
            let sy = ((y as f32 + 0.5 - oy) * scale).floor();
            (sy >= 0.0 && (sy as usize) < sh).then_some(sy as usize)
        })
        .collect();
    let px = |x: usize, y: usize| -> [u8; 3] {
        match (xmap[x], ymap[y]) {
            (Some(sx), Some(sy)) => {
                let i = (sy * sw + sx) * 4;
                [src[i], src[i + 1], src[i + 2]]
            }
            _ => [0, 0, 0],
        }
    };

    match f.pix {
        Pix::Rgb32 | Pix::Rgb24 => {
            let bpp = if f.pix == Pix::Rgb32 { 4 } else { 3 };
            let stride = f.rgb_stride();
            for y in 0..h {
                let dy = if f.top_down { y } else { h - 1 - y };
                let row = &mut dst[dy * stride..dy * stride + w * bpp];
                for x in 0..w {
                    let p = px(x, y);
                    let o = x * bpp;
                    row[o] = p[0];
                    row[o + 1] = p[1];
                    row[o + 2] = p[2];
                    if bpp == 4 {
                        row[o + 3] = 255;
                    }
                }
            }
        }
        Pix::Yuy2 => {
            for y in 0..h {
                let row = &mut dst[y * w * 2..(y + 1) * w * 2];
                for x in (0..w).step_by(2) {
                    let a = px(x, y);
                    let b = px((x + 1).min(w - 1), y);
                    let (y0, u0, v0) = yuv(a[0], a[1], a[2]);
                    let (y1, u1, v1) = yuv(b[0], b[1], b[2]);
                    let o = x * 2;
                    row[o] = y0;
                    row[o + 1] = ((u0 as u16 + u1 as u16) / 2) as u8;
                    row[o + 2] = y1;
                    row[o + 3] = ((v0 as u16 + v1 as u16) / 2) as u8;
                }
            }
        }
        Pix::Nv12 | Pix::I420 => {
            let (cw, ch) = (w / 2, h / 2);
            let (yplane, rest) = dst.split_at_mut(w * h);
            for y in 0..h {
                for x in 0..w {
                    let p = px(x, y);
                    yplane[y * w + x] = yuv(p[0], p[1], p[2]).0;
                }
            }
            for cy in 0..ch {
                for cx in 0..cw {
                    let mut us = 0u32;
                    let mut vs = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let p = px((cx * 2 + dx).min(w - 1), (cy * 2 + dy).min(h - 1));
                        let (_, u, v) = yuv(p[0], p[1], p[2]);
                        us += u as u32;
                        vs += v as u32;
                    }
                    let (u, v) = ((us / 4) as u8, (vs / 4) as u8);
                    if f.pix == Pix::Nv12 {
                        let o = (cy * cw + cx) * 2;
                        rest[o] = u;
                        rest[o + 1] = v;
                    } else {
                        rest[cy * cw + cx] = u;
                        rest[cw * ch + cy * cw + cx] = v;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_all_formats_without_panicking() {
        let src = vec![128u8; 1280 * 720 * 4];
        for f in crate::media::all_formats() {
            let mut dst = vec![0u8; f.image_size()];
            convert(&src, 1280, 720, &f, &mut dst);
            assert!(dst.iter().any(|&v| v != 0), "{f:?}");
        }
    }

    #[test]
    fn white_is_white_in_yuv() {
        let src = vec![255u8; 16 * 9 * 4];
        let f = Fmt::new(Pix::Yuy2, 640, 360);
        let mut dst = vec![0u8; f.image_size()];
        convert(&src, 16, 9, &f, &mut dst);
        assert_eq!(dst[0], 235);
        assert_eq!(dst[1], 128);
    }
}
