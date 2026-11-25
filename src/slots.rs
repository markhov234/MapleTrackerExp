//! The quick slot bar: what the player has to hand, and how much is left.
//!
//! This is the cheapest honest source of potion use in the whole client.
//! The bar draws each assigned item as its own icon with the remaining count
//! printed over it, so "how many potions did I drink" is a subtraction
//! rather than a sentence to parse — no wording to teach, no fading text,
//! nothing that stops working when the chat is closed.
//!
//! Finding it needs no coordinates. The panel is a fixed 145 by 75 client
//! pixels with a bright rule along its top and bottom edge, and measured on
//! two different clients it sits at (893, 576) and (937, 624) — the same
//! shape in different places, which is exactly the case a search handles
//! and a calibration does not.
//!
//! The counts are drawn white with a black outline, over icons that are
//! often bright themselves: a lemon is as bright as white text. Brightness
//! therefore cannot read them and the blue channel can, because white keeps
//! all of it and the yellows, oranges and reds of potion icons keep almost
//! none.
//!
//! Nothing here reads the game. It is arithmetic on a pixel buffer.

use serde::{Deserialize, Serialize};

use crate::ocr::{Frame, Roi};

/// Where the bar is, and how it is divided up.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Slots {
    #[serde(default)]
    pub enabled: bool,
    /// The panel's size in client pixels, and how far either may be out.
    pub panel_w: u32,
    pub panel_h: u32,
    pub tolerance: u32,
    /// How bright the panel's edge rules are.
    pub edge: u8,
    /// The grid inside it: where the first slot starts, how far apart they
    /// are, and how big each one is.
    pub left: u32,
    pub top: u32,
    pub pitch_x: u32,
    pub pitch_y: u32,
    pub slot: u32,
    pub cols: u32,
    pub rows: u32,
    /// How much of a slot's lower-left corner the count occupies.
    pub count_w: u32,
    pub count_h: u32,
    /// Blue level above which a pixel is part of the count rather than the
    /// icon behind it.
    pub count_blue: u8,
}

impl Default for Slots {
    fn default() -> Self {
        Slots {
            enabled: false,
            panel_w: 145,
            panel_h: 75,
            tolerance: 8,
            edge: 150,
            left: 9,
            top: 6,
            pitch_x: 34,
            pitch_y: 34,
            slot: 32,
            cols: 4,
            rows: 2,
            count_w: 22,
            count_h: 11,
            count_blue: 170,
        }
    }
}

/// Where the bar was found, in client pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The longest bright run on a row, as (start, length).
fn longest_run(frame: &Frame, y: u32, edge: u8) -> (u32, u32) {
    let (mut best, mut best_at) = (0u32, 0u32);
    let (mut run, mut at) = (0u32, 0u32);
    for x in 0..frame.w {
        if frame.grey(x, y) >= edge {
            if run == 0 {
                at = x;
            }
            run += 1;
            if run > best {
                best = run;
                best_at = at;
            }
        } else {
            run = 0;
        }
    }
    (best_at, best)
}

/// Find the quick slot bar by its two edge rules.
///
/// One bright rule of about the right length, and another the panel's
/// height below it starting at the same column. Plenty of other chrome
/// draws a long bright line — the status bar, the chat frame, the window
/// borders — but a matched pair at exactly this spacing and width is the
/// bar, and the search runs bottom-upward because that is where it lives.
pub fn find_panel(frame: &Frame, cfg: &Slots) -> Option<Panel> {
    let tol = cfg.tolerance;
    let want = cfg.panel_w;
    let from = frame.h / 2;
    for y in (from..frame.h.saturating_sub(cfg.panel_h)).rev() {
        let (x0, len) = longest_run(frame, y, cfg.edge);
        if len + tol < want || len > want + tol {
            continue;
        }
        for dh in [cfg.panel_h, cfg.panel_h - 1, cfg.panel_h + 1] {
            let (x1, len1) = longest_run(frame, y + dh, cfg.edge);
            if len1 + tol < want || len1 > want + tol || x0.abs_diff(x1) > 4 {
                continue;
            }
            return Some(Panel { x: x0.min(x1), y, w: want, h: dh });
        }
    }
    None
}

/// The rectangle of one slot, counting left to right then top to bottom.
pub fn slot_rect(panel: &Panel, cfg: &Slots, i: u32) -> Option<Roi> {
    if i >= cfg.cols * cfg.rows {
        return None;
    }
    let (cx, cy) = (i % cfg.cols, i / cfg.cols);
    Some(Roi {
        name: format!("slot{i}"),
        x: panel.x + cfg.left + cx * cfg.pitch_x,
        y: panel.y + cfg.top + cy * cfg.pitch_y,
        w: cfg.slot,
        h: cfg.slot,
        threshold: cfg.count_blue,
        invert: false,
    })
}

/// The lower-left corner of a slot, where the remaining count is printed.
pub fn count_rect(panel: &Panel, cfg: &Slots, i: u32) -> Option<Roi> {
    let s = slot_rect(panel, cfg, i)?;
    Some(Roi {
        name: format!("count{i}"),
        x: s.x,
        y: s.y + s.h.saturating_sub(cfg.count_h),
        w: cfg.count_w.min(s.w),
        h: cfg.count_h.min(s.h),
        ..s
    })
}

/// A mask of the count's digits, read off the blue channel.
///
/// White text keeps all its blue; the lemons, oranges and reds behind it
/// keep almost none. Brightness cannot tell them apart and this can.
pub fn count_mask(frame: &Frame, roi: &Roi, cfg: &Slots) -> Vec<bool> {
    let mut out = Vec::with_capacity((roi.w * roi.h) as usize);
    for y in roi.y..roi.y + roi.h {
        for x in roi.x..roi.x + roi.w {
            let lit = if x >= frame.w || y >= frame.h {
                false
            } else {
                let (_, _, b) = frame.rgb(x, y);
                b >= cfg.count_blue
            };
            out.push(lit);
        }
    }
    out
}

/// One slot's artwork, as raw RGBA, for the panel to draw.
///
/// The picture comes from the player's own screen rather than a library of
/// item sprites, which is what makes it correct without anyone maintaining
/// it: whatever they put in that slot is what appears, including items this
/// program has never heard of.
pub fn icon_rgba(frame: &Frame, panel: &Panel, cfg: &Slots, i: u32) -> Option<(u32, u32, Vec<u8>)> {
    let r = slot_rect(panel, cfg, i)?;
    if r.x + r.w > frame.w || r.y + r.h > frame.h {
        return None;
    }
    let mut out = Vec::with_capacity((r.w * r.h * 4) as usize);
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let (cr, cg, cb) = frame.rgb(x, y);
            out.extend_from_slice(&[cr, cg, cb, 255]);
        }
    }
    Some((r.w, r.h, out))
}

/// Whether two readings of the same slot show different items.
///
/// Not whether their pixels differ. Measured on two live captures two
/// seconds apart, seven of the eight slots were byte for byte identical
/// and the eighth differed over five per cent of its area — that eighth
/// being the potion whose count was ticking down. So the count is exactly
/// the part that changes while the item stays the same, and it is the part
/// this ignores: only the artwork above it is compared.
///
/// Getting this wrong is not subtle. The artwork is thirty kilobytes, and
/// treating a ticking count as a new item pushes all of it through to the
/// overlay every couple of seconds to redraw the same eight pictures.
pub fn differs(a: &[u8], b: &[u8], w: u32, h: u32, count_h: u32) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return true;
    }
    let keep = h.saturating_sub(count_h).max(1);
    let row = (w * 4) as usize;
    let upper = (keep as usize * row).min(a.len());
    let moved = a[..upper]
        .iter()
        .zip(&b[..upper])
        .step_by(3)
        .filter(|(x, y)| x.abs_diff(**y) > 24)
        .count();
    // A swapped icon redraws most of its square; nothing else comes close.
    moved * 5 > upper / 3
}

/// How much of the artwork moved, as a fraction. For diagnosis.
pub fn drift(a: &[u8], b: &[u8], w: u32, h: u32, count_h: u32) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    let keep = h.saturating_sub(count_h).max(1);
    let upper = ((keep * w * 4) as usize).min(a.len());
    let moved = a[..upper]
        .iter()
        .zip(&b[..upper])
        .step_by(3)
        .filter(|(x, y)| x.abs_diff(**y) > 24)
        .count();
    moved as f32 / (upper / 3).max(1) as f32
}

/// Base64, because the overlay receives JSON and raw bytes are not JSON.
///
/// Thirty lines of encoder rather than a dependency: this is the only place
/// in the program that needs it, and the alphabet has not changed since
/// 1987.
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with one bright rule, a gap, and another rule below it.
    fn bar(w: u32, h: u32, at: (u32, u32), len: u32, gap: u32) -> Vec<u8> {
        let mut px = vec![0u8; (w * h * 4) as usize];
        for &y in &[at.1, at.1 + gap] {
            for x in at.0..(at.0 + len).min(w) {
                let i = ((y * w + x) * 4) as usize;
                px[i] = 230;
                px[i + 1] = 230;
                px[i + 2] = 230;
                px[i + 3] = 255;
            }
        }
        px
    }

    #[test]
    fn a_matched_pair_of_rules_is_the_bar() {
        let cfg = Slots::default();
        let px = bar(400, 300, (60, 180), 145, 75);
        let f = Frame { w: 400, h: 300, px: &px };
        assert_eq!(
            find_panel(&f, &cfg),
            Some(Panel { x: 60, y: 180, w: 145, h: 75 })
        );
    }

    #[test]
    fn one_rule_on_its_own_is_not() {
        // The status bar and the chat frame both draw long bright lines.
        let cfg = Slots::default();
        let mut px = vec![0u8; (400 * 300 * 4) as usize];
        for x in 60..205u32 {
            let i = ((180 * 400 + x) * 4) as usize;
            px[i] = 230;
            px[i + 1] = 230;
            px[i + 2] = 230;
            px[i + 3] = 255;
        }
        let f = Frame { w: 400, h: 300, px: &px };
        assert_eq!(find_panel(&f, &cfg), None);
    }

    #[test]
    fn a_rule_of_the_wrong_length_is_not_either() {
        let cfg = Slots::default();
        let px = bar(600, 300, (60, 180), 300, 75);
        let f = Frame { w: 600, h: 300, px: &px };
        assert_eq!(find_panel(&f, &cfg), None);
    }

    #[test]
    fn the_slots_tile_the_panel_in_reading_order() {
        let cfg = Slots::default();
        let p = Panel { x: 100, y: 200, w: 145, h: 75 };
        let a = slot_rect(&p, &cfg, 0).unwrap();
        let b = slot_rect(&p, &cfg, 1).unwrap();
        let e = slot_rect(&p, &cfg, 4).unwrap();
        assert_eq!((a.x, a.y), (109, 206));
        assert_eq!((b.x, b.y), (143, 206), "second slot is one pitch right");
        assert_eq!((e.x, e.y), (109, 240), "fifth slot starts the second row");
        assert_eq!((a.w, a.h), (32, 32));
        assert!(slot_rect(&p, &cfg, 8).is_none(), "there are only eight");
        // Every slot has to fit inside the panel it came from.
        for i in 0..8 {
            let s = slot_rect(&p, &cfg, i).unwrap();
            assert!(s.x + s.w <= p.x + p.w && s.y + s.h <= p.y + p.h, "slot {i} overflows");
        }
    }

    #[test]
    fn the_count_sits_in_the_lower_left_of_its_slot() {
        let cfg = Slots::default();
        let p = Panel { x: 100, y: 200, w: 145, h: 75 };
        let s = slot_rect(&p, &cfg, 0).unwrap();
        let c = count_rect(&p, &cfg, 0).unwrap();
        assert_eq!(c.x, s.x);
        assert_eq!(c.y + c.h, s.y + s.h);
        assert!(c.w < s.w);
    }

    #[test]
    fn a_swapped_icon_counts_as_a_change() {
        let a = vec![30u8; 4096];
        let b = vec![200u8; 4096];
        assert!(differs(&a, &b, 32, 32, 12));
        assert!(differs(&a, &[], 32, 32, 12), "a missing slot is a change");
    }

    #[test]
    fn a_ticking_count_does_not() {
        // The measured case: identical artwork, and the count beneath it
        // completely redrawn.
        let a = vec![120u8; 32 * 32 * 4];
        let mut b = a.clone();
        for y in 20..32usize {
            for i in 0..32 * 4usize {
                b[y * 32 * 4 + i] = 255;
            }
        }
        assert!(!differs(&a, &b, 32, 32, 12), "a ticking count is not a new item");
    }

    #[test]
    fn a_shade_of_drift_across_the_whole_icon_does_not_either() {
        let a = vec![120u8; 32 * 32 * 4];
        let b: Vec<u8> = a.iter().map(|v| v + 7).collect();
        assert!(!differs(&a, &b, 32, 32, 12));
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[255u8, 0, 128]), "/wCA");
    }

    #[test]
    fn white_digits_survive_a_bright_yellow_icon() {
        // The case brightness cannot handle: a lemon is as bright as the
        // text printed over it, and differs only in blue.
        let cfg = Slots::default();
        let (w, h) = (8u32, 4u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let white = x < 3;
                px[i] = 255;
                px[i + 1] = if white { 255 } else { 215 };
                px[i + 2] = if white { 255 } else { 20 };
                px[i + 3] = 255;
            }
        }
        let f = Frame { w, h, px: &px };
        let roi = Roi { name: "c".into(), x: 0, y: 0, w, h, threshold: 0, invert: false };
        let m = count_mask(&f, &roi, &cfg);
        assert!(m[0] && m[1] && m[2], "white text should be ink");
        assert!(!m[3] && !m[7], "the lemon behind it should not");
    }
}
