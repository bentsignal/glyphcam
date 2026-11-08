//! Glyph sets and the pre-rasterized glyph atlas.
//!
//! Every glyph is rasterized once into a fixed `cell_w x cell_h` coverage
//! bitmap. Glyphs are sorted by ink density so that a brightness value maps
//! to a "level", and each level holds several glyphs of similar density
//! (often from different scripts) that the renderer picks between to get the
//! shimmering, mixed-script look.

use crate::settings::Scripts;
use fontdue::{Font, FontSettings};
use std::sync::OnceLock;

static FONT_MONO: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
static FONT_KANA: &[u8] = include_bytes!("../../../assets/fonts/glyphcam-kana.ttf");
static FONT_BRAILLE: &[u8] = include_bytes!("../../../assets/fonts/glyphcam-braille.ttf");

/// Raw font bytes, also used by the UI so glyph previews render correctly.
pub fn font_bytes() -> [(&'static str, &'static [u8]); 3] {
    [
        ("DejaVuSansMono", FONT_MONO),
        ("glyphcam-kana", FONT_KANA),
        ("glyphcam-braille", FONT_BRAILLE),
    ]
}

struct Fonts {
    mono: Font,
    kana: Font,
    braille: Font,
}

fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(|| {
        let load = |b: &[u8]| Font::from_bytes(b, FontSettings::default()).expect("embedded font");
        Fonts {
            mono: load(FONT_MONO),
            kana: load(FONT_KANA),
            braille: load(FONT_BRAILLE),
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Latin,
    Digits,
    Punctuation,
    Katakana,
    Greek,
    Cyrillic,
    BoxDrawing,
    Blocks,
    Math,
    Braille,
}

fn chars_for(f: Family) -> Vec<char> {
    let range = |a: u32, b: u32| (a..=b).filter_map(char::from_u32).collect::<Vec<_>>();
    match f {
        Family::Latin => ('A'..='Z').chain('a'..='z').collect(),
        Family::Digits => ('0'..='9').collect(),
        Family::Punctuation => "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".chars().collect(),
        // Half-width katakana (the classic "digital rain" set).
        Family::Katakana => range(0xFF66, 0xFF9D),
        Family::Greek => {
            let mut v = range(0x391, 0x3A9);
            v.extend(range(0x3B1, 0x3C9));
            v.retain(|&c| c != '\u{3A2}');
            v
        }
        Family::Cyrillic => range(0x410, 0x44F),
        Family::BoxDrawing => "─│┌┐└┘├┤┬┴┼═║╔╗╚╝╠╣╦╩╬╱╲╳┄┆┈┊╭╮╯╰".chars().collect(),
        Family::Blocks => "░▒▓█▀▄▌▐▖▗▘▙▚▛▜▝▞▟".chars().collect(),
        Family::Math => "∑∏∫∮√∞≈≠≡≤≥∂∇∆∈∉∋∩∪⊂⊃⊆⊇⊕⊗⊥∀∃∄¬∧∨±×÷∝∠∴∵≪≫⊢⊨⋆∘∙"
            .chars()
            .collect(),
        Family::Braille => range(0x2801, 0x28FF),
    }
}

pub fn families(s: &Scripts) -> Vec<Family> {
    let mut v = Vec::new();
    let mut push = |on: bool, f| {
        if on {
            v.push(f)
        }
    };
    push(s.latin, Family::Latin);
    push(s.digits, Family::Digits);
    push(s.punctuation, Family::Punctuation);
    push(s.katakana, Family::Katakana);
    push(s.greek, Family::Greek);
    push(s.cyrillic, Family::Cyrillic);
    push(s.box_drawing, Family::BoxDrawing);
    push(s.blocks, Family::Blocks);
    push(s.math, Family::Math);
    push(s.braille, Family::Braille);
    if v.is_empty() {
        v.push(Family::Digits);
    }
    v
}

/// A short sample string for UI previews of a script set.
pub fn sample_text(s: &Scripts, n: usize) -> String {
    let mut out = String::new();
    let fams = families(s);
    let f = fonts();
    let lists: Vec<Vec<char>> = fams
        .iter()
        .map(|&fam| {
            let font = match fam {
                Family::Katakana => &f.kana,
                Family::Braille => &f.braille,
                _ => &f.mono,
            };
            chars_for(fam)
                .into_iter()
                .filter(|&c| font.lookup_glyph_index(c) != 0)
                .collect::<Vec<_>>()
        })
        .filter(|l: &Vec<char>| !l.is_empty())
        .collect();
    if lists.is_empty() {
        return String::new();
    }
    let mut i = 0usize;
    while out.chars().count() < n {
        let l = &lists[i % lists.len()];
        out.push(l[(i * 7 + i / lists.len() * 3) % l.len()]);
        i += 1;
    }
    out
}

pub const LEVELS: usize = 48;

pub struct Atlas {
    pub cell_w: usize,
    pub cell_h: usize,
    /// Coverage bitmaps, `cell_w * cell_h` bytes each. Index 0 is blank.
    pub bitmaps: Vec<u8>,
    pub chars: Vec<char>,
    /// For each brightness level, candidate glyph indices of similar density.
    pub levels: Vec<Vec<u16>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AtlasKey {
    pub cell_w: usize,
    pub cell_h: usize,
    pub scripts: Scripts,
    pub mirror_katakana: bool,
}

impl Atlas {
    pub fn glyph(&self, idx: u16) -> &[u8] {
        let n = self.cell_w * self.cell_h;
        &self.bitmaps[idx as usize * n..(idx as usize + 1) * n]
    }

    pub fn build(key: AtlasKey) -> Atlas {
        let AtlasKey {
            cell_w: cw,
            cell_h: ch,
            scripts,
            mirror_katakana,
        } = key;
        let fonts = fonts();
        let n = cw * ch;
        let mut bitmaps = vec![0u8; n]; // index 0: blank
        let mut chars = vec![' '];
        let mut density = vec![0.0f32];

        for fam in families(&scripts) {
            for c in chars_for(fam) {
                let font = match fam {
                    Family::Katakana => &fonts.kana,
                    Family::Braille => &fonts.braille,
                    _ => &fonts.mono,
                };
                if font.lookup_glyph_index(c) == 0 {
                    continue;
                }
                let mirror = mirror_katakana && fam == Family::Katakana;
                let mut cell = vec![0u8; n];
                rasterize_into(font, c, cw, ch, mirror, fam, &mut cell);
                let sum: u32 = cell.iter().map(|&v| v as u32).sum();
                if sum == 0 {
                    continue;
                }
                density.push(sum as f32 / (n as f32 * 255.0));
                bitmaps.extend_from_slice(&cell);
                chars.push(c);
            }
        }

        // Normalize densities and bucket into levels.
        // Normalize to the 92nd percentile so the densest few glyphs don't
        // monopolize the brightest levels (more variety up top).
        let mut sorted: Vec<f32> = density.iter().skip(1).cloned().collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let dmax = sorted
            .get((sorted.len() as f32 * 0.92) as usize)
            .copied()
            .unwrap_or(1.0)
            .max(1e-6);
        let norm: Vec<f32> = density.iter().map(|d| (d / dmax).min(1.0)).collect();
        let mut levels = Vec::with_capacity(LEVELS);
        for l in 0..LEVELS {
            let t = l as f32 / (LEVELS - 1) as f32;
            if l == 0 {
                levels.push(vec![0u16]);
                continue;
            }
            // Nearest density, then everything within a small window of it.
            let nearest = norm
                .iter()
                .enumerate()
                .skip(1)
                .map(|(i, &d)| (i, (d - t).abs()))
                .fold((1usize, f32::MAX), |a, b| if b.1 < a.1 { b } else { a });
            let window = nearest.1 + 0.035;
            let mut cands: Vec<u16> = norm
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(_, &d)| (d - t).abs() <= window)
                .map(|(i, _)| i as u16)
                .collect();
            if cands.is_empty() {
                cands.push(nearest.0 as u16);
            }
            levels.push(cands);
        }
        Atlas {
            cell_w: cw,
            cell_h: ch,
            bitmaps,
            chars,
            levels,
        }
    }
}

fn rasterize_into(
    font: &Font,
    c: char,
    cw: usize,
    ch: usize,
    mirror: bool,
    fam: Family,
    out: &mut [u8],
) {
    // Size the font so its line height fills ~95% of the cell.
    let lm = font
        .horizontal_line_metrics(100.0)
        .expect("horizontal font");
    let line = (lm.ascent - lm.descent) / 100.0;
    let mut px = ch as f32 * 0.95 / line;
    if matches!(fam, Family::Katakana) {
        px *= 1.08;
    }
    let mut attempt = 0;
    loop {
        let (m, bmp) = font.rasterize(c, px);
        if (m.width > cw || m.height > ch) && attempt < 6 {
            let f = (cw as f32 / m.width.max(1) as f32)
                .min(ch as f32 / m.height.max(1) as f32)
                .min(0.95);
            px *= f;
            attempt += 1;
            continue;
        }
        let lm = font.horizontal_line_metrics(px).unwrap();
        let pad_top = (ch as f32 - (lm.ascent - lm.descent)) * 0.5;
        let baseline = pad_top + lm.ascent;
        let x0 = (cw as i32 - m.width as i32) / 2;
        let mut y0 = (baseline - m.height as f32 - m.ymin as f32).round() as i32;
        // Box drawing / blocks should touch the cell edges; center others that overflow.
        if y0 < 0 || y0 + m.height as i32 > ch as i32 {
            y0 = (ch as i32 - m.height as i32) / 2;
        }
        for y in 0..m.height {
            let ty = y0 + y as i32;
            if ty < 0 || ty >= ch as i32 {
                continue;
            }
            for x in 0..m.width {
                let sx = if mirror { m.width - 1 - x } else { x };
                let tx = x0 + x as i32;
                if tx < 0 || tx >= cw as i32 {
                    continue;
                }
                out[ty as usize * cw + tx as usize] = bmp[y * m.width + sx];
            }
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::GlyphPreset;

    #[test]
    fn atlas_builds_for_all_presets() {
        for p in GlyphPreset::ALL {
            let a = Atlas::build(AtlasKey {
                cell_w: 10,
                cell_h: 17,
                scripts: p.scripts(),
                mirror_katakana: true,
            });
            assert!(a.chars.len() > 10, "{p:?} has {} glyphs", a.chars.len());
            assert_eq!(a.levels.len(), LEVELS);
            assert!(a.levels.iter().all(|l| !l.is_empty()));
        }
    }

    #[test]
    fn katakana_present() {
        let a = Atlas::build(AtlasKey {
            cell_w: 12,
            cell_h: 20,
            scripts: GlyphPreset::Matrix.scripts(),
            mirror_katakana: false,
        });
        assert!(a
            .chars
            .iter()
            .any(|&c| ('\u{FF66}'..='\u{FF9D}').contains(&c)));
    }
}
