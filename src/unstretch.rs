//! Undoing the window's scaling, so the font is a bitmap font again.
//!
//! The client renders its UI at a fixed internal resolution and then the
//! window stretches that image to whatever size it has been dragged to. On
//! this machine the render is 1366x768 and the window is 2560x1440, a factor
//! of 1.874 — and because that is not a whole number, a glyph's pixels depend
//! on where the glyph happens to sit. The same '2' is 19 rows tall with gaps
//! on one line and 17 solid rows on the next. Template matching, which is
//! exact by nature, is then asked to be approximate, and the atlas grows a
//! form per character per sub-pixel phase without ever quite covering them
//! all. That is what makes a digit come back unreadable after the bar moves
//! nine pixels.
//!
//! The fix is to take the scaling back out before reading anything. Resample
//! the captured pixels down to the client's own grid and the font returns to
//! what it always was: one bitmap per character, 5x7, strokes one pixel wide,
//! identical wherever on the screen it is drawn. Across six captures with the
//! EXP bar in four different places, 85 glyphs collapse to 16 distinct
//! bitmaps this way — against 36 to 49 at every other candidate scale, which
//! is also how the right scale is recognised.
//!
//! Two details carry the whole idea:
//!
//!   * **The grid is anchored at the frame's origin, never at the region
//!     being read.** Anchoring it to the region would reintroduce exactly the
//!     phase dependence this removes, since the region moves.
//!   * **Resampling happens on grey values, before thresholding.** Shrinking
//!     a bitmap that has already been thresholded and trimmed cannot work:
//!     the sub-pixel information was thrown away at the threshold. Measured
//!     on the taught atlas, that approach collapses 90 forms to 59 and the
//!     characters start colliding with each other; done in this order, the
//!     same font needs 16.
//!
//! Nothing here touches the game. It is arithmetic on a pixel buffer.

use serde::{Deserialize, Serialize};

use crate::ocr::Frame;

/// How much larger the captured frame is than the client's own render.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub sx: f64,
    pub sy: f64,
}

impl Scale {
    /// The frame is already the client's own grid; reading it needs no work.
    pub const NONE: Scale = Scale { sx: 1.0, sy: 1.0 };

    pub fn is_none(&self) -> bool {
        (self.sx - 1.0).abs() < 1e-9 && (self.sy - 1.0).abs() < 1e-9
    }

    /// The size a frame shrinks to under this scale.
    pub fn native(&self, w: u32, h: u32) -> (u32, u32) {
        (((w as f64) / self.sx) as u32, ((h as f64) / self.sy) as u32)
    }

    /// Where a point on the captured frame lands on the client's grid.
    pub fn to_native(&self, x: u32, y: u32) -> (u32, u32) {
        (((x as f64) / self.sx) as u32, ((y as f64) / self.sy) as u32)
    }
}

/// The internal resolutions this client is known to render at.
///
/// Worth stating plainly: this is a shortlist, not a measurement. The scale
/// is not taken from it — [`candidates`] only proposes, and the caller keeps
/// whichever proposal actually reads a field that passes its arithmetic. An
/// absent resolution therefore costs a reading, never a wrong one.
const RENDER_SIZES: &[(u32, u32)] = &[
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 768),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
];

/// Scales worth trying for a frame of this size, likeliest first.
///
/// A candidate has to be a magnification, and it has to be close to uniform:
/// the window keeps the client's aspect ratio, so a pair of axis factors that
/// disagree describes some other pair of resolutions, not this one.
pub fn candidates(frame_w: u32, frame_h: u32) -> Vec<Scale> {
    let mut out: Vec<Scale> = RENDER_SIZES
        .iter()
        .filter(|(nw, nh)| *nw <= frame_w && *nh <= frame_h)
        .map(|(nw, nh)| Scale { sx: frame_w as f64 / *nw as f64, sy: frame_h as f64 / *nh as f64 })
        .filter(|s| (s.sx - s.sy).abs() <= 0.05)
        .collect();
    // Largest first: a frame that really is at 1:1 still matches every
    // smaller render size, and trying the big reductions first means the
    // scale that collapses the font is found before the ones that merely
    // happen to read.
    out.sort_by(|a, b| b.sx.partial_cmp(&a.sx).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (a.sx - b.sx).abs() < 1e-9 && (a.sy - b.sy).abs() < 1e-9);
    out
}

/// A rectangle of a frame, resampled back onto the client's pixel grid.
///
/// Its own coordinates run from zero; [`Plate::x`] and [`Plate::y`] say where
/// it sits on that grid, so a region found in one plate can be re-cut from a
/// later frame.
pub struct Plate {
    /// Position on the client's grid, in client pixels.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    px: Vec<u8>,
}

impl Plate {
    /// A view the reader can use directly; its coordinates are plate-local.
    pub fn frame(&self) -> Frame<'_> {
        Frame { w: self.w, h: self.h, px: &self.px }
    }

    /// Move a plate-local point onto the client's grid.
    pub fn to_native(&self, x: u32, y: u32) -> (u32, u32) {
        (self.x + x, self.y + y)
    }
}

/// Weights mapping one output pixel back onto the source pixels it averages.
fn weights(i: u32, s: f64, limit: u32) -> (u32, Vec<f32>) {
    let (lo, hi) = (i as f64 * s, (i + 1) as f64 * s);
    let first = lo.floor() as u32;
    let last = (hi.ceil() as u32).min(limit);
    let mut w = Vec::with_capacity((last.saturating_sub(first)) as usize);
    for j in first..last {
        let overlap = (j as f64 + 1.0).min(hi) - (j as f64).max(lo);
        w.push(overlap.max(0.0) as f32);
    }
    (first, w)
}

/// Resample a rectangle of the client's grid out of a stretched frame.
///
/// `(x, y, w, h)` are in client pixels. The result is grey, written into all
/// three colour channels so the existing reader sees it as an ordinary frame.
/// How bright a pixel is, which is what the status bar needs.
pub fn grey(r: u8, g: u8, b: u8) -> u8 {
    (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) as u8
}

/// How much warm light a pixel carries, which is what the floating lines
/// need.
///
/// Those lines are drawn over the map itself, and brightness alone does not
/// find them: lit grass is as bright as text. Colour does. The client draws
/// them in two colours — plain messages in a pale grey, party lines in gold
/// — and measured off real frames those are (166, 166, 165) and
/// (175, 179, 22). What the two share is red and green together; what the
/// scenery behind them has instead is green alone, (25, 118, 0) for the
/// grass. The smaller of red and green is therefore high for either colour
/// of text and low for foliage, which separates them where a grey level
/// cannot.
pub fn warm(r: u8, g: u8, _b: u8) -> u8 {
    r.min(g)
}

pub fn plate(frame: &Frame, s: Scale, x: u32, y: u32, w: u32, h: u32) -> Plate {
    plate_with(frame, s, x, y, w, h, grey)
}

/// Resample a rectangle, measuring each pixel with `f` rather than by
/// brightness.
///
/// The measuring happens before the averaging, deliberately: a stroke one
/// client-pixel wide has to be averaged as the thing it is, and deciding
/// afterwards what colour the blended pixel was would be deciding about a
/// colour that was never on screen.
pub fn plate_with(
    frame: &Frame,
    s: Scale,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    f: fn(u8, u8, u8) -> u8,
) -> Plate {
    let (nw, nh) = s.native(frame.w, frame.h);
    let w = w.min(nw.saturating_sub(x.min(nw)));
    let h = h.min(nh.saturating_sub(y.min(nh)));
    if w == 0 || h == 0 {
        return Plate { x, y, w: 0, h: 0, px: Vec::new() };
    }

    // Column weights once, reused for every row: the horizontal pass is the
    // same arithmetic repeated, and the frames are 2560 wide.
    let cols: Vec<(u32, Vec<f32>)> = (x..x + w).map(|i| weights(i, s.sx, frame.w)).collect();
    let rows: Vec<(u32, Vec<f32>)> = (y..y + h).map(|j| weights(j, s.sy, frame.h)).collect();

    let src_y0 = rows.first().map(|(f, _)| *f).unwrap_or(0);
    let src_y1 = rows
        .last()
        .map(|(f, ws)| f + ws.len() as u32)
        .unwrap_or(0)
        .min(frame.h);

    // Horizontal pass: every source row in range, narrowed to output columns.
    let span = (src_y1.saturating_sub(src_y0)) as usize;
    let mut band = vec![0f32; span * w as usize];
    for (r, sy) in (src_y0..src_y1).enumerate() {
        for (c, (first, ws)) in cols.iter().enumerate() {
            let mut acc = 0f32;
            let mut total = 0f32;
            for (k, weight) in ws.iter().enumerate() {
                let sx = first + k as u32;
                if sx >= frame.w {
                    break;
                }
                let (r, g, b) = frame.rgb(sx, sy);
                acc += f(r, g, b) as f32 * weight;
                total += weight;
            }
            band[r * w as usize + c] = if total > 0.0 { acc / total } else { 0.0 };
        }
    }

    // Vertical pass over the narrowed band.
    let mut px = vec![0u8; (w * h) as usize * 4];
    for (j, (first, ws)) in rows.iter().enumerate() {
        for c in 0..w as usize {
            let mut acc = 0f32;
            let mut total = 0f32;
            for (k, weight) in ws.iter().enumerate() {
                let sy = first + k as u32;
                if sy < src_y0 || sy >= src_y1 {
                    continue;
                }
                acc += band[(sy - src_y0) as usize * w as usize + c] * weight;
                total += weight;
            }
            let v = if total > 0.0 { (acc / total).round().clamp(0.0, 255.0) as u8 } else { 0 };
            let i = (j * w as usize + c) * 4;
            px[i] = v;
            px[i + 1] = v;
            px[i + 2] = v;
            px[i + 3] = 255;
        }
    }

    Plate { x, y, w, h, px }
}

/// Resample a rectangle keeping its colour.
///
/// [`plate_with`] measures each pixel down to one number before averaging,
/// which is what a reader wants and what a colour does not survive. The
/// minimap's badge is read for its hue, so its three channels have to be
/// carried through the resampling separately.
pub fn plate_rgb(frame: &Frame, s: Scale, x: u32, y: u32, w: u32, h: u32) -> Plate {
    let (nw, nh) = s.native(frame.w, frame.h);
    let w = w.min(nw.saturating_sub(x.min(nw)));
    let h = h.min(nh.saturating_sub(y.min(nh)));
    if w == 0 || h == 0 {
        return Plate { x, y, w: 0, h: 0, px: Vec::new() };
    }
    let cols: Vec<(u32, Vec<f32>)> = (x..x + w).map(|i| weights(i, s.sx, frame.w)).collect();
    let rows: Vec<(u32, Vec<f32>)> = (y..y + h).map(|j| weights(j, s.sy, frame.h)).collect();
    let src_y0 = rows.first().map(|(f, _)| *f).unwrap_or(0);
    let src_y1 = rows
        .last()
        .map(|(f, ws)| f + ws.len() as u32)
        .unwrap_or(0)
        .min(frame.h);

    let span = (src_y1.saturating_sub(src_y0)) as usize;
    let stride = w as usize * 3;
    let mut band = vec![0f32; span * stride];
    for (r, sy) in (src_y0..src_y1).enumerate() {
        for (c, (first, ws)) in cols.iter().enumerate() {
            let mut acc = [0f32; 3];
            let mut total = 0f32;
            for (k, weight) in ws.iter().enumerate() {
                let sx = first + k as u32;
                if sx >= frame.w {
                    break;
                }
                let (pr, pg, pb) = frame.rgb(sx, sy);
                acc[0] += pr as f32 * weight;
                acc[1] += pg as f32 * weight;
                acc[2] += pb as f32 * weight;
                total += weight;
            }
            for ch in 0..3 {
                band[r * stride + c * 3 + ch] = if total > 0.0 { acc[ch] / total } else { 0.0 };
            }
        }
    }

    let mut px = vec![0u8; (w * h) as usize * 4];
    for (j, (first, ws)) in rows.iter().enumerate() {
        for c in 0..w as usize {
            let mut acc = [0f32; 3];
            let mut total = 0f32;
            for (k, weight) in ws.iter().enumerate() {
                let sy = first + k as u32;
                if sy < src_y0 || sy >= src_y1 {
                    continue;
                }
                let base = (sy - src_y0) as usize * stride + c * 3;
                for ch in 0..3 {
                    acc[ch] += band[base + ch] * weight;
                }
                total += weight;
            }
            let i = (j * w as usize + c) * 4;
            for ch in 0..3 {
                px[i + ch] =
                    if total > 0.0 { (acc[ch] / total).round().clamp(0.0, 255.0) as u8 } else { 0 };
            }
            px[i + 3] = 255;
        }
    }
    Plate { x, y, w, h, px }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a frame by magnifying a small pattern by a whole number, the one
    /// case where the original is recoverable exactly.
    fn magnified(pattern: &[&str], k: u32) -> (u32, u32, Vec<u8>) {
        let (w, h) = (pattern[0].len() as u32, pattern.len() as u32);
        let (fw, fh) = (w * k, h * k);
        let mut px = vec![0u8; (fw * fh) as usize * 4];
        for y in 0..fh {
            for x in 0..fw {
                let on = pattern[(y / k) as usize].as_bytes()[(x / k) as usize] == b'#';
                let v = if on { 255 } else { 0 };
                let i = ((y * fw + x) * 4) as usize;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
                px[i + 3] = 255;
            }
        }
        (fw, fh, px)
    }

    fn render(p: &Plate) -> Vec<String> {
        (0..p.h)
            .map(|y| {
                (0..p.w)
                    .map(|x| if p.px[((y * p.w + x) * 4) as usize] >= 128 { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_whole_number_magnification_is_undone_exactly() {
        let pattern = ["..##..", ".#..#.", "######", "#....#"];
        let (w, h, px) = magnified(&pattern, 3);
        let frame = Frame { w, h, px: &px };
        let p = plate(&frame, Scale { sx: 3.0, sy: 3.0 }, 0, 0, 6, 4);
        assert_eq!(render(&p), pattern);
    }

    #[test]
    fn a_region_reads_the_same_wherever_it_sits() {
        // The point of anchoring the grid at the frame's origin: cutting a
        // plate around a glyph must not depend on having cut it tightly.
        let pattern = ["........", "..####..", "..#..#..", "..####..", "........"];
        let (w, h, px) = magnified(&pattern, 2);
        let frame = Frame { w, h, px: &px };
        let tight = plate(&frame, Scale { sx: 2.0, sy: 2.0 }, 2, 1, 4, 3);
        let loose = plate(&frame, Scale { sx: 2.0, sy: 2.0 }, 0, 0, 8, 5);
        let inner: Vec<String> =
            render(&loose)[1..4].iter().map(|r| r[2..6].to_string()).collect();
        assert_eq!(render(&tight), inner);
    }

    #[test]
    fn a_fractional_scale_keeps_stroke_width_and_position() {
        // 1.874 is the scale this client actually runs at. A one-pixel stroke
        // magnified and brought back must still be one pixel wide and in the
        // same column, which is the property the atlas depends on.
        let (w, h) = (1366u32, 16u32);
        let (fw, fh) = (2560u32, 30u32);
        let s = Scale { sx: fw as f64 / w as f64, sy: fh as f64 / h as f64 };
        for col in [5u32, 97, 613, 1000] {
            let mut px = vec![0u8; (fw * fh) as usize * 4];
            for y in 0..fh {
                for x in 0..fw {
                    // The source: one lit column at `col` on the client grid.
                    let on = ((x as f64 / s.sx) as u32) == col;
                    let v = if on { 255 } else { 0 };
                    let i = ((y * fw + x) * 4) as usize;
                    px[i] = v;
                    px[i + 1] = v;
                    px[i + 2] = v;
                    px[i + 3] = 255;
                }
            }
            let frame = Frame { w: fw, h: fh, px: &px };
            let p = plate(&frame, s, 0, 0, w, 4);
            let lit: Vec<u32> =
                (0..p.w).filter(|x| p.px[((0 * p.w + x) * 4) as usize] >= 128).collect();
            assert_eq!(lit, vec![col], "stroke moved or spread at column {col}");
        }
    }

    #[test]
    fn candidates_are_magnifications_that_keep_the_aspect_ratio() {
        let c = candidates(2560, 1440);
        assert!(c.iter().any(|s| (s.sx - 2560.0 / 1366.0).abs() < 1e-6));
        assert!(c.iter().all(|s| s.sx >= 1.0 && (s.sx - s.sy).abs() <= 0.05));
        // 1024x768 is 4:3 and must not be proposed for a 16:9 frame.
        assert!(c.iter().all(|s| (s.sx - 2560.0 / 1024.0).abs() > 1e-6));
        assert_eq!(c.first().map(|s| s.sx > 1.9), Some(true));
    }

    #[test]
    fn a_plate_cannot_run_off_the_clients_grid() {
        let (w, h, px) = magnified(&["##", "##"], 4);
        let frame = Frame { w, h, px: &px };
        let p = plate(&frame, Scale { sx: 4.0, sy: 4.0 }, 1, 1, 99, 99);
        assert_eq!((p.w, p.h), (1, 1));
    }
}
