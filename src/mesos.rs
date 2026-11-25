//! Read the meso total off the inventory window.
//!
//! Mesos were being added up from the floating "You have gained mesos (+95)"
//! lines, which makes the figure hostage to a font that is still half
//! taught, counts nothing that was sold, and drifts by whatever the reader
//! missed while a window covered the corner.
//!
//! The total itself is on screen. The inventory draws it as one number on a
//! white plate, and a number you can read is worth more than a number you
//! accumulate: it cannot drift, it starts correct rather than at zero, and
//! the difference between how much it moved and how much was picked up off
//! the ground is what was sold, less what was spent.
//!
//! Finding it needs no new template. The plate is the brightest thing in
//! the client's lower half and nothing else of that shape holds a
//! comma-grouped number, so the field is located the same way the EXP field
//! is: by the shape of what is written in it, confirmed by reading it twice.
//!
//! The one inversion: every other reader here looks for light text on a
//! dark client. This is dark text on a white plate, so the ROI is read with
//! `invert` set and the threshold means the opposite of what it usually
//! does.

use serde::{Deserialize, Serialize};

use crate::ocr::{self, Frame, Profile, Roi};

/// Where and how to read the meso plate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mesos {
    /// Off until the plate has actually been seen on a real frame. A reader
    /// that has never been checked must not quietly put a number on the
    /// panel.
    pub enabled: bool,
    /// A pixel this bright on every channel is plate rather than ink.
    pub plate: u8,
    /// Ink on the plate is darker than this. Dark on light, not light on
    /// dark: see the note at the top.
    pub ink: u8,
    /// Narrowest plate worth looking at, in client pixels.
    pub min_w: u32,
    /// Tallest plate worth looking at. A tall white rectangle is a panel
    /// background, not a one-line field.
    pub max_h: u32,
    /// Shortest plate worth looking at, so a white border line is skipped.
    pub min_h: u32,
    /// How wide a dark interruption can be and still count as part of the
    /// same plate. The digits sit *on* the plate, so a row through them is
    /// bright, dark, bright, dark — without bridging those the only whole
    /// rows found are the few of padding above and below the text, and the
    /// field is never tall enough to notice.
    pub ink_gap: u32,
    /// Columns of clear plate that separate one digit from the next.
    pub min_gap: usize,
    /// Only look below this fraction of the client's height. The inventory
    /// sits low; the quest tracker and the chat box's white rows do not.
    pub from_y: f32,
}

impl Default for Mesos {
    fn default() -> Self {
        Mesos {
            enabled: false,
            plate: 215,
            ink: 140,
            min_w: 40,
            max_h: 20,
            min_h: 7,
            ink_gap: 8,
            min_gap: 1,
            from_y: 0.35,
        }
    }
}

/// A bright rectangle that might be holding a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plate {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Is this pixel part of a bright plate?
fn bright(f: &Frame, x: u32, y: u32, cut: u8) -> bool {
    let (r, g, b) = f.rgb(x, y);
    // All three channels, not luma: a saturated yellow is bright by luma
    // and is not a white plate.
    r >= cut && g >= cut && b.saturating_add(10) >= cut
}

/// Every bright rectangle in the lower part of the client.
///
/// Rows of bright pixels are found first and then stacked: a run on one row
/// joins the rectangle above it when they overlap by most of their width.
/// That keeps a plate together while splitting it from the panel chrome
/// above and below, which is a different width.
pub fn plates(f: &Frame, cfg: &Mesos) -> Vec<Plate> {
    let y0 = ((f.h as f32 * cfg.from_y) as u32).min(f.h);
    let mut open: Vec<(u32, u32, u32, u32)> = Vec::new(); // x, w, y, h
    let mut done: Vec<Plate> = Vec::new();

    for y in y0..f.h {
        // Bright spans, with dark interruptions up to `ink_gap` bridged so
        // the digits do not cut the plate into ribbons.
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let mut start: Option<u32> = None;
        let mut last_bright: Option<u32> = None;
        for x in 0..f.w {
            if bright(f, x, y, cfg.plate) {
                start.get_or_insert(x);
                last_bright = Some(x);
            } else if let (Some(s), Some(lb)) = (start, last_bright) {
                if x - lb > cfg.ink_gap {
                    if lb + 1 - s >= cfg.min_w {
                        runs.push((s, lb + 1 - s));
                    }
                    start = None;
                    last_bright = None;
                }
            }
        }
        if let (Some(s), Some(lb)) = (start, last_bright) {
            if lb + 1 - s >= cfg.min_w {
                runs.push((s, lb + 1 - s));
            }
        }

        let mut next: Vec<(u32, u32, u32, u32)> = Vec::new();
        let mut used = vec![false; runs.len()];
        for (ox, ow, oy, oh) in open.drain(..) {
            // The run on this row that lines up with the rectangle so far.
            let hit = runs.iter().enumerate().position(|(i, (rx, rw))| {
                !used[i] && {
                    let lo = ox.max(*rx);
                    let hi = (ox + ow).min(rx + rw);
                    hi > lo && (hi - lo) * 4 >= ow.max(*rw) * 3
                }
            });
            match hit {
                Some(i) => {
                    used[i] = true;
                    let (rx, rw) = runs[i];
                    next.push((ox.min(rx), ow.max(rw), oy, oh + 1));
                }
                None => {
                    if oh >= cfg.min_h && oh <= cfg.max_h {
                        done.push(Plate { x: ox, y: oy, w: ow, h: oh });
                    }
                }
            }
        }
        for (i, (rx, rw)) in runs.iter().enumerate() {
            if !used[i] {
                next.push((*rx, *rw, y, 1));
            }
        }
        open = next;
    }
    for (ox, ow, oy, oh) in open {
        if oh >= cfg.min_h && oh <= cfg.max_h {
            done.push(Plate { x: ox, y: oy, w: ow, h: oh });
        }
    }
    done
}

/// Does this text look like a meso total the client would print?
///
/// Comma-grouped, and a comma is required. Demanding one costs the first
/// 999 mesos of a brand new character and rejects nearly everything else
/// bright on the screen, which is a trade worth making — a wrong total is
/// worse than a missing one, and nobody opens a session tracker on a
/// character with three figures to their name.
pub fn parse(text: &str) -> Option<u64> {
    let t = text.trim();
    if !t.contains(',') || t.len() > 15 {
        return None;
    }
    let groups: Vec<&str> = t.split(',').collect();
    if groups.len() < 2 {
        return None;
    }
    let lead = groups[0];
    if lead.is_empty() || lead.len() > 3 || !lead.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    for g in &groups[1..] {
        if g.len() != 3 || !g.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    t.replace(',', "").parse().ok()
}

/// Every plate on this frame that holds something shaped like a meso total.
///
/// Plural because the shop window shows your mesos too, and an item list
/// can put a price on a pale row. The caller decides which to believe, by
/// watching which one behaves like a running total.
pub fn candidates(f: &Frame, cfg: &Mesos, profile: &Profile) -> Vec<(Plate, u64)> {
    let mut out = Vec::new();
    for p in plates(f, cfg) {
        let roi = Roi {
            name: "mesos".into(),
            x: p.x,
            y: p.y,
            w: p.w,
            h: p.h,
            threshold: cfg.ink,
            invert: true,
        };
        let cuts = ocr::segment(f, &roi, cfg.min_gap);
        if cuts.is_empty() || cuts.len() > 15 {
            continue;
        }
        let reading = ocr::classify_cuts(&cuts, &profile.atlas, None);
        if reading.unknown > 0 {
            continue;
        }
        if let Some(v) = parse(&reading.text) {
            out.push((p, v));
        }
    }
    out
}

/// Follows one plate across frames, and only believes it once it has
/// behaved like a running total twice in a row.
///
/// The same discipline the EXP reader uses. A single frame cannot tell the
/// meso counter from a price tag that happens to be comma-grouped; a plate
/// that is still in the same place with a plausible value a moment later
/// can only be the counter.
#[derive(Debug, Default)]
pub struct Watch {
    /// The plate currently believed, and the last value read from it.
    held: Option<(Plate, u64)>,
    /// A plate seen once, waiting for a second look.
    pending: Option<(Plate, u64)>,
}

impl Watch {
    /// Fold in one frame's candidates. Returns the total when one is
    /// confirmed, which is also when the caller should record it.
    pub fn observe(&mut self, found: &[(Plate, u64)]) -> Option<u64> {
        // Already following a plate: keep following it, wherever the window
        // has been dragged to is a new plate and starts over.
        if let Some((held, last)) = self.held {
            if let Some(&(p, v)) = found.iter().find(|(p, _)| same(*p, held)) {
                self.held = Some((p, v));
                return Some(v);
            }
            // Gone for now — the window was closed. Keep believing the last
            // number rather than forgetting the session's starting point.
            let _ = last;
            return None;
        }
        match self.pending {
            Some((p0, _)) => {
                if let Some(&(p, v)) = found.iter().find(|(p, _)| same(*p, p0)) {
                    self.pending = None;
                    self.held = Some((p, v));
                    return Some(v);
                }
                self.pending = found.first().copied();
                None
            }
            None => {
                self.pending = found.first().copied();
                None
            }
        }
    }

    /// The plate being followed, for the log.
    pub fn plate(&self) -> Option<Plate> {
        self.held.map(|(p, _)| p)
    }
}

/// Two sightings of the same field. The window does not move while it is
/// open, so this is deliberately tight; a couple of pixels covers a plate
/// that grew by a digit.
fn same(a: Plate, b: Plate) -> bool {
    a.y.abs_diff(b.y) <= 2 && a.x.abs_diff(b.x) <= 12 && a.h.abs_diff(b.h) <= 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::Glyph;

    const PROFILE_JSON: &str = include_str!("../assets/profile.json");

    fn profile() -> Profile {
        serde_json::from_str(PROFILE_JSON).expect("the bundled profile parses")
    }

    /// Paint a number onto a white plate the way the client does — dark ink,
    /// light field — so the reader can be tested without a live window.
    fn frame_with(text: &str, at: (u32, u32), size: (u32, u32), p: &Profile) -> (u32, u32, Vec<u8>) {
        let (w, h) = size;
        // A dark client, so the plate is the only bright thing on it.
        let mut px = vec![0u8; (w * h * 4) as usize];
        for i in 0..(w * h) as usize {
            px[i * 4 + 3] = 255;
        }
        let glyphs: Vec<&Glyph> = text
            .chars()
            .map(|c| {
                p.atlas
                    .get(&c.to_string())
                    .and_then(|v| v.first())
                    .unwrap_or_else(|| panic!("the atlas has no {c:?}"))
            })
            .collect();
        let pad = 3u32;
        let gap = 1u32;
        let text_w: u32 = glyphs.iter().map(|g| g.w as u32 + gap).sum::<u32>() - gap;
        let text_h: u32 = glyphs.iter().map(|g| g.h as u32).max().unwrap_or(7);
        let (pw, ph) = (text_w + pad * 2, text_h + pad * 2);

        let put = |px: &mut Vec<u8>, x: u32, y: u32, v: [u8; 3]| {
            if x < w && y < h {
                let i = ((y * w + x) * 4) as usize;
                px[i] = v[0];
                px[i + 1] = v[1];
                px[i + 2] = v[2];
            }
        };
        for y in 0..ph {
            for x in 0..pw {
                put(&mut px, at.0 + x, at.1 + y, [245, 245, 240]);
            }
        }
        let mut cx = at.0 + pad;
        for g in glyphs {
            for gy in 0..g.h {
                for gx in 0..g.w {
                    if g.on_at(gx, gy) {
                        put(&mut px, cx + gx as u32, at.1 + pad + gy as u32, [20, 20, 20]);
                    }
                }
            }
            cx += g.w as u32 + gap;
        }
        (w, h, px)
    }

    #[test]
    fn a_painted_total_reads_back() {
        let p = profile();
        let cfg = Mesos { enabled: true, ..Default::default() };
        let (w, h, px) = frame_with("1,291,787", (120, 400), (640, 480), &p);
        let f = Frame { w, h, px: &px };
        let found = candidates(&f, &cfg, &p);
        assert_eq!(found.len(), 1, "exactly one plate: {found:?}");
        assert_eq!(found[0].1, 1_291_787);
    }

    /// The whole point of reading it rather than accumulating it.
    #[test]
    fn the_session_total_is_a_difference_of_two_readings() {
        let p = profile();
        let cfg = Mesos { enabled: true, ..Default::default() };
        let mut watch = Watch::default();

        let open = frame_with("1,291,787", (120, 400), (640, 480), &p);
        let f = Frame { w: open.0, h: open.1, px: &open.2 };
        let c = candidates(&f, &cfg, &p);
        assert_eq!(watch.observe(&c), None, "one sighting is not enough");
        let start = watch.observe(&c).expect("the second confirms it");
        assert_eq!(start, 1_291_787);

        let later = frame_with("1,305,000", (120, 400), (640, 480), &p);
        let f2 = Frame { w: later.0, h: later.1, px: &later.2 };
        let now = watch.observe(&candidates(&f2, &cfg, &p)).expect("still following it");
        assert_eq!(now - start, 13_213);
    }

    /// Closing the inventory must not look like losing the money.
    #[test]
    fn a_closed_window_reports_nothing_rather_than_zero() {
        let p = profile();
        let cfg = Mesos { enabled: true, ..Default::default() };
        let mut watch = Watch::default();
        let open = frame_with("1,291,787", (120, 400), (640, 480), &p);
        let f = Frame { w: open.0, h: open.1, px: &open.2 };
        let c = candidates(&f, &cfg, &p);
        watch.observe(&c);
        assert!(watch.observe(&c).is_some());
        assert_eq!(watch.observe(&[]), None, "no plate, no reading — not a zero");
        assert!(watch.plate().is_some(), "and it is still following the same field");
    }

    #[test]
    fn only_comma_grouped_numbers_are_meso_totals() {
        assert_eq!(parse("1,291,787"), Some(1_291_787));
        assert_eq!(parse("12,000"), Some(12_000));
        assert_eq!(parse("0"), None, "no comma, no deal");
        assert_eq!(parse("1,29,787"), None, "groups are three digits");
        assert_eq!(parse("1,2917,87"), None);
        assert_eq!(parse(",787"), None);
        assert_eq!(parse("1,291,78?"), None);
        assert_eq!(parse("50.13%"), None, "the EXP percentage is not money");
    }

    /// A tall white rectangle is a panel, not a one-line field.
    #[test]
    fn a_big_white_box_is_not_a_plate() {
        let cfg = Mesos::default();
        let (w, h) = (400u32, 300u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 150..260 {
            for x in 40..340 {
                let i = ((y * w + x) * 4) as usize;
                px[i] = 250;
                px[i + 1] = 250;
                px[i + 2] = 248;
            }
        }
        let f = Frame { w, h, px: &px };
        assert!(plates(&f, &cfg).is_empty(), "110 rows tall is a panel");
    }
}
