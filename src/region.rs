//! Which part of the world the player is in, and what colour it is.
//!
//! The minimap's header carries a badge for the region — not for the map.
//! Measured across two captures from different maps in the same region, and
//! from clients rendering at different resolutions, it is the same badge:
//! Primeval Forest I at 1366x768 and Forgotten Entrance III at 1280x720 both
//! show the mossy orb of Shallow Passage. That makes it exactly the key the
//! overlay wants, because a theme should change when you cross into Kerning
//! City and not when you walk through a door.
//!
//! The badge is also the answer rather than a lookup into one. It is the
//! artists' own colour for the region, so the panel can be tinted from it
//! directly — no table of hand-picked hues to write, and regions nobody has
//! catalogued still work. The two captures above give 117.0 and 121.4
//! degrees, four degrees apart across different resolutions, which is the
//! stability the idea needs.
//!
//! Finding it starts from the words "MINI MAP" in the title bar. One fixed
//! template, dark on its light bar, and it answers two questions at once:
//! where the panel is, and whether it is open at all. A closed minimap means
//! no badge, which means the theme is left alone rather than guessed at.
//!
//! Nothing here reads the game. It is arithmetic on a pixel buffer.

use serde::{Deserialize, Serialize};

use crate::ocr::{Frame, Glyph};

/// Where the minimap is found, and where its badge sits once it is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Minimap {
    /// Whether the tracker should look for it.
    #[serde(default)]
    pub enabled: bool,
    /// "MINI MAP", as it appears on the client's own grid.
    ///
    /// Several forms for the same reason the status bar once needed them:
    /// the title sits near the top of the frame, and the vertical scaling
    /// lands it on a slightly different row at different client heights.
    /// One form taught per resolution matches its own at 1.0; measured
    /// across resolutions the same form still scores 0.84, which is why the
    /// floor below is not 0.9.
    pub anchor: Vec<Glyph>,
    /// Grey level below which a pixel counts as ink. The title is dark text
    /// on a light bar, the opposite of everything else here.
    pub threshold: u8,
    /// How far from the top-left corner to look, in client pixels.
    pub search_w: u32,
    pub search_h: u32,
    /// Agreement the title needs. Deliberately below the status bar's floor.
    pub min_match: f32,
    /// The badge, relative to the title's top-left corner.
    pub dx: i32,
    pub dy: i32,
    pub w: u32,
    pub h: u32,
    /// How colourful a pixel must be to count toward the region's hue.
    ///
    /// The panel's own chrome is a pale blue and would otherwise swamp the
    /// badge: measured, the chrome sits around 50 and the badge around 100.
    pub chroma_floor: u8,
}

impl Default for Minimap {
    fn default() -> Self {
        Minimap {
            enabled: false,
            anchor: Vec::new(),
            threshold: 120,
            search_w: 140,
            search_h: 90,
            min_match: 0.80,
            dx: 0,
            dy: 12,
            w: 46,
            h: 36,
            chroma_floor: 70,
        }
    }
}

/// What the badge says about where the player is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Hue in degrees, 0 to 360.
    pub hue: f32,
    /// How colourful the badge is on average, 0 to 255. A washed-out
    /// reading is a reading to distrust.
    pub chroma: f32,
    /// How many pixels the hue was taken from.
    pub pixels: usize,
}

/// Hue and chroma of one colour, hue in degrees.
fn hue_of(r: u8, g: u8, b: u8) -> (f32, f32) {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    if c <= 0.0 {
        return (0.0, 0.0);
    }
    let h = if max == r {
        60.0 * (((g - b) / c) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / c + 2.0)
    } else {
        60.0 * ((r - g) / c + 4.0)
    };
    ((h + 360.0) % 360.0, c)
}

/// Where the words "MINI MAP" are on this frame, if they are on it.
///
/// Searching rather than assuming, for the usual reason: the panel is not
/// at the same place on every client, and a position measured once is a
/// position that is wrong on somebody else's machine.
pub fn find_anchor(frame: &Frame, cfg: &Minimap) -> Option<(u32, u32)> {
    let first = cfg.anchor.first()?;
    let (tw, th) = (first.w as u32, first.h as u32);
    if tw == 0 || th == 0 {
        return None;
    }
    let ink = |x: u32, y: u32| frame.grey(x, y) < cfg.threshold;

    let mut best: Option<(f32, u32, u32)> = None;
    let max_y = cfg.search_h.min(frame.h.saturating_sub(th));
    let max_x = cfg.search_w.min(frame.w.saturating_sub(tw));
    for y in 0..=max_y {
        for x in 0..=max_x {
            for form in &cfg.anchor {
                if form.w as u32 != tw || form.h as u32 != th {
                    continue;
                }
                let mut same = 0usize;
                for j in 0..th {
                    for i in 0..tw {
                        if ink(x + i, y + j) == form.on_at(i as usize, j as usize) {
                            same += 1;
                        }
                    }
                }
                let score = same as f32 / (tw * th) as f32;
                if best.map(|(b, _, _)| score > b).unwrap_or(true) {
                    best = Some((score, x, y));
                }
            }
        }
    }
    match best {
        Some((score, x, y)) if score >= cfg.min_match => Some((x, y)),
        _ => None,
    }
}

/// The region's colour, taken from the badge beside the minimap's title.
///
/// `None` means the minimap is not open, or the badge is not colourful
/// enough to be trusted. Either way the caller should leave the theme as it
/// is: a panel that changes colour because a window happened to be closed
/// would be worse than one that never changed at all.
pub fn region(frame: &Frame, cfg: &Minimap) -> Option<Region> {
    let (ax, ay) = find_anchor(frame, cfg)?;
    let x0 = (ax as i32 + cfg.dx).max(0) as u32;
    let y0 = (ay as i32 + cfg.dy).max(0) as u32;

    // A circular mean, because hue wraps: averaging 359 and 1 the ordinary
    // way gives 180, which is the opposite colour. Each pixel pulls on the
    // result as hard as it is colourful, so the badge's washed-out edges
    // count for little and its core for a lot.
    let (mut vx, mut vy, mut total, mut n) = (0f32, 0f32, 0f32, 0usize);
    for y in y0..(y0 + cfg.h).min(frame.h) {
        for x in x0..(x0 + cfg.w).min(frame.w) {
            let (r, g, b) = frame.rgb(x, y);
            let (h, c) = hue_of(r, g, b);
            if c <= cfg.chroma_floor as f32 {
                continue;
            }
            let w = c / 255.0;
            let rad = h.to_radians();
            vx += w * rad.cos();
            vy += w * rad.sin();
            total += c;
            n += 1;
        }
    }
    if n < 24 {
        return None;
    }
    let hue = (vy.atan2(vx).to_degrees() + 360.0) % 360.0;
    Some(Region { hue, chroma: total / n as f32, pixels: n })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_of(px: Vec<u8>, w: u32, h: u32) -> Vec<u8> {
        assert_eq!(px.len(), (w * h * 4) as usize);
        px
    }

    /// Paint a flat panel with a patch of one colour where the badge goes.
    fn scene(w: u32, h: u32, badge: (u8, u8, u8), at: (u32, u32, u32, u32)) -> Vec<u8> {
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let inside = x >= at.0 && x < at.0 + at.2 && y >= at.1 && y < at.1 + at.3;
                let (r, g, b) = if inside { badge } else { (200, 200, 205) };
                px[i] = r;
                px[i + 1] = g;
                px[i + 2] = b;
                px[i + 3] = 255;
            }
        }
        frame_of(px, w, h)
    }

    fn title() -> Glyph {
        // Four columns of ink, which is enough to be found but not enough to
        // be found twice.
        let (w, h) = (6usize, 3usize);
        let bits: String = (0..w * h)
            .map(|i| if i % 6 < 2 { '1' } else { '0' })
            .collect();
        Glyph { w, h, bits }
    }

    fn cfg() -> Minimap {
        Minimap { anchor: vec![title()], dx: 0, dy: 4, w: 8, h: 8, ..Minimap::default() }
    }

    #[test]
    fn a_green_badge_reads_as_green() {
        let mut px = scene(40, 30, (40, 200, 60), (3, 7, 8, 8));
        // Stamp the title at (3, 3): ink is dark.
        let t = title();
        for j in 0..t.h {
            for i in 0..t.w {
                if t.on_at(i, j) {
                    let k = (((3 + j as u32) * 40 + 3 + i as u32) * 4) as usize;
                    px[k] = 10;
                    px[k + 1] = 10;
                    px[k + 2] = 10;
                }
            }
        }
        let f = Frame { w: 40, h: 30, px: &px };
        let r = region(&f, &cfg()).expect("badge found");
        assert!((r.hue - 132.0).abs() < 12.0, "hue was {}", r.hue);
    }

    #[test]
    fn a_closed_minimap_gives_nothing() {
        // No title anywhere: the panel is shut, so there is nothing to say.
        let px = scene(40, 30, (40, 200, 60), (3, 7, 8, 8));
        let f = Frame { w: 40, h: 30, px: &px };
        assert!(region(&f, &cfg()).is_none());
    }

    #[test]
    fn hue_wraps_the_short_way_round() {
        // Reds either side of zero must average to red, not to cyan.
        assert_eq!(hue_of(255, 0, 0).0, 0.0);
        let (h, _) = hue_of(255, 8, 0);
        assert!(h < 5.0, "hue was {h}");
        let (h, _) = hue_of(255, 0, 8);
        assert!(h > 355.0, "hue was {h}");
    }

    #[test]
    fn a_grey_badge_is_refused_rather_than_called_red() {
        let mut px = scene(40, 30, (150, 150, 150), (3, 7, 8, 8));
        let t = title();
        for j in 0..t.h {
            for i in 0..t.w {
                if t.on_at(i, j) {
                    let k = (((3 + j as u32) * 40 + 3 + i as u32) * 4) as usize;
                    px[k] = 10;
                    px[k + 1] = 10;
                    px[k + 2] = 10;
                }
            }
        }
        let f = Frame { w: 40, h: 30, px: &px };
        assert!(region(&f, &cfg()).is_none(), "a colourless badge says nothing");
    }
}
