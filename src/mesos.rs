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

use std::collections::BTreeMap;

use crate::ocr::{self, Frame, Glyph, Roi};

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
    /// The inventory's own digits.
    ///
    /// Not the status bar's. The first live frame settled that: the status
    /// bar writes `9` five wide and seven tall with its loop closed at the
    /// bottom, and the inventory writes it five by eight with a tail that
    /// trails off to the left. Same client, two typefaces, and matching one
    /// against the other reads nothing — which is what it did.
    ///
    /// Kept here rather than merged into `Profile::atlas` so that teaching
    /// the purse can never cost the EXP field a glyph.
    #[serde(default)]
    pub atlas: BTreeMap<String, Vec<Glyph>>,
}

/// What the purse has to know before it may put a number on the panel.
pub const NEEDED: &str = "0123456789,";

impl Mesos {
    /// The characters still missing. A reader that has not been taught the
    /// digit `4` must not be switched on: a total it cannot spell is a
    /// total it will decline to read, every time a 4 turns up in it.
    pub fn untaught(&self) -> Vec<char> {
        NEEDED.chars().filter(|c| !self.atlas.contains_key(&c.to_string())).collect()
    }
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
            atlas: BTreeMap::new(),
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
pub fn candidates(f: &Frame, cfg: &Mesos) -> Vec<(Plate, u64)> {
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
        if cuts.is_empty() || cuts.len() > MAX_CUTS {
            continue;
        }
        if let Some(v) = number_among(&cuts, cfg) {
            out.push((p, v));
        }
    }
    out
}

/// The most cuts one plate is worth segmenting. The field holds a border,
/// a coin, the number and the word MESOS; well past that and it is not a
/// one-line field.
const MAX_CUTS: usize = 48;

/// The meso total written somewhere on this plate.
///
/// The number does not have the plate to itself — the client draws a border
/// down each side, a coin to the left of it and the word MESOS to the right
/// — so demanding that every glyph on the plate be recognised asks the
/// status-bar atlas to read a coin. It never will, and the first live frame
/// with the inventory open duly read nothing off a field whose digits were
/// perfectly legible.
///
/// So each glyph is classified on its own, unrecognised ones break the
/// string, and the longest stretch of recognised glyphs that is shaped like
/// a comma-grouped total is the answer. Nothing is guessed: an unknown
/// glyph is still unknown, it just no longer condemns its neighbours.
fn number_among(cuts: &[ocr::Cut], cfg: &Mesos) -> Option<u64> {
    let chars: Vec<Option<char>> = cuts
        .iter()
        .map(|c| {
            let r = ocr::classify_cuts(std::slice::from_ref(c), &cfg.atlas, None);
            if r.unknown > 0 { None } else { r.text.chars().next().filter(char::is_ascii) }
        })
        .collect();

    let mut best: Option<u64> = None;
    let mut best_len = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].is_none() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_some() {
            j += 1;
        }
        let run: String = chars[i..j].iter().map(|c| c.unwrap()).collect();
        // Longest first: a total with something legible hard against it
        // should still read, and the longest match is the whole number.
        for len in (1..=run.len()).rev() {
            if len <= best_len {
                break;
            }
            let mut hit = None;
            for s in 0..=run.len() - len {
                if let Some(v) = parse(&run[s..s + len]) {
                    hit = Some(v);
                    break;
                }
            }
            if let Some(v) = hit {
                best = Some(v);
                best_len = len;
                break;
            }
        }
        i = j;
    }
    best
}

/// Cut the digits of a known total out of a frame, so they can be taught.
///
/// The total has to be supplied by somebody who can read it, because this
/// is the bootstrap: there is no atlas yet, so nothing on the plate can be
/// recognised, and a reader that guessed at the identity of a shape it had
/// never seen would be exactly the kind of reader this codebase refuses to
/// ship.
///
/// The window is found by arithmetic rather than by position: a run of cuts
/// as long as the total, where every digit stands the same height and every
/// comma stands shorter. Exactly one run must qualify. Two would mean the
/// frame holds a second comma-grouped number of the same length, and
/// picking one of them at random is how an atlas ends up with a `5` that is
/// really a `6`.
pub fn teach(f: &Frame, cfg: &Mesos, text: &str) -> Result<BTreeMap<String, Vec<Glyph>>, String> {
    if parse(text).is_none() {
        return Err(format!("{text:?} is not shaped like a meso total"));
    }
    let want: Vec<char> = text.chars().collect();
    let mut hits: Vec<(Plate, Vec<Glyph>)> = Vec::new();

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
        if cuts.len() < want.len() {
            continue;
        }
        for start in 0..=cuts.len() - want.len() {
            let win = &cuts[start..start + want.len()];
            let digit_h: Vec<usize> = win
                .iter()
                .zip(&want)
                .filter(|(_, c)| c.is_ascii_digit())
                .map(|(k, _)| k.glyph.h)
                .collect();
            let Some(&h) = digit_h.first() else { continue };
            if h < 5 || digit_h.iter().any(|&d| d != h) {
                continue;
            }
            let commas_shorter = win
                .iter()
                .zip(&want)
                .filter(|(_, c)| **c == ',')
                .all(|(k, _)| k.glyph.h < h);
            if !commas_shorter {
                continue;
            }
            hits.push((p, win.iter().map(|k| k.glyph.clone()).collect()));
        }
    }

    if hits.is_empty() {
        return Err(format!(
            "no run of {} glyphs on any plate is shaped like {text:?}",
            want.len()
        ));
    }
    if hits.len() > 1 {
        let where_: Vec<String> = hits.iter().map(|(p, _)| format!("({},{})", p.x, p.y)).collect();
        return Err(format!(
            "{} runs could be {text:?}: {} — ambiguous",
            hits.len(),
            where_.join(" ")
        ));
    }

    let (_, glyphs) = hits.pop().expect("exactly one hit");
    let mut out = cfg.atlas.clone();
    for (c, g) in want.iter().zip(glyphs) {
        let key = c.to_string();
        let forms = out.entry(key.clone()).or_default();
        if let Some(seen) = forms.first() {
            if seen.bits != g.bits || seen.w != g.w || seen.h != g.h {
                return Err(format!(
                    "{c:?} has already been taught a different shape — the run was misaligned,                      or the total given does not match the one on screen"
                ));
            }
        } else {
            forms.push(g);
        }
    }

    // The house rule: two characters that share a bitmap are two characters
    // the reader cannot tell apart, and it must not pretend otherwise.
    let flat: Vec<(&String, &Glyph)> =
        out.iter().filter_map(|(k, v)| v.first().map(|g| (k, g))).collect();
    for (i, (ka, ga)) in flat.iter().enumerate() {
        for (kb, gb) in &flat[i + 1..] {
            if ga.bits == gb.bits && ga.w == gb.w && ga.h == gb.h {
                return Err(format!("{ka} and {kb} came out identical — refusing to write that"));
            }
        }
    }
    Ok(out)
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
    use crate::ocr::{Glyph, Profile};

    const PROFILE_JSON: &str = include_str!("../assets/profile.json");

    fn profile() -> Profile {
        serde_json::from_str(PROFILE_JSON).expect("the bundled profile parses")
    }

    /// The reader as shipped, taught glyphs and all.
    fn shipped() -> Mesos {
        let mut m = profile().mesos.expect("the profile carries a meso reader");
        m.enabled = true;
        m
    }

    /// The rule, as a test: a reader missing a digit is a reader that will
    /// decline every total containing it, so it must not be switched on.
    #[test]
    fn the_shipped_reader_knows_every_digit_and_the_comma() {
        let m = profile().mesos.expect("the profile carries a meso reader");
        assert!(m.untaught().is_empty(), "not taught: {:?}", m.untaught());
        assert!(m.enabled, "and so there is no reason for it to be off");
    }

    /// Paint a number onto a white plate the way the client does — dark ink,
    /// light field — so the reader can be tested without a live window.
    fn frame_with(text: &str, at: (u32, u32), size: (u32, u32), p: &Mesos) -> (u32, u32, Vec<u8>) {
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
        let cfg = shipped();
        let p = &cfg;
        let (w, h, px) = frame_with("1,291,787", (120, 400), (640, 480), p);
        let f = Frame { w, h, px: &px };
        let found = candidates(&f, &cfg);
        assert_eq!(found.len(), 1, "exactly one plate: {found:?}");
        assert_eq!(found[0].1, 1_291_787);
    }

    /// The whole point of reading it rather than accumulating it.
    #[test]
    fn the_session_total_is_a_difference_of_two_readings() {
        let cfg = shipped();
        let p = &cfg;
        let mut watch = Watch::default();

        let open = frame_with("1,291,787", (120, 400), (640, 480), p);
        let f = Frame { w: open.0, h: open.1, px: &open.2 };
        let c = candidates(&f, &cfg);
        assert_eq!(watch.observe(&c), None, "one sighting is not enough");
        let start = watch.observe(&c).expect("the second confirms it");
        assert_eq!(start, 1_291_787);

        let later = frame_with("1,305,000", (120, 400), (640, 480), p);
        let f2 = Frame { w: later.0, h: later.1, px: &later.2 };
        let now = watch.observe(&candidates(&f2, &cfg)).expect("still following it");
        assert_eq!(now - start, 13_213);
    }

    /// Closing the inventory must not look like losing the money.
    #[test]
    fn a_closed_window_reports_nothing_rather_than_zero() {
        let cfg = shipped();
        let p = &cfg;
        let mut watch = Watch::default();
        let open = frame_with("1,291,787", (120, 400), (640, 480), p);
        let f = Frame { w: open.0, h: open.1, px: &open.2 };
        let c = candidates(&f, &cfg);
        watch.observe(&c);
        assert!(watch.observe(&c).is_some());
        assert_eq!(watch.observe(&[]), None, "no plate, no reading — not a zero");
        assert!(watch.plate().is_some(), "and it is still following the same field");
    }

    /// The bug the first live frame found.
    ///
    /// The number does not have the plate to itself: there is a border down
    /// each side, a coin to its left and the word MESOS to its right. The
    /// reader used to require that every glyph on the plate be recognised,
    /// so a perfectly legible 983,602 read as nothing at all.
    #[test]
    fn a_total_still_reads_with_a_coin_and_a_label_beside_it() {
        let cfg = shipped();
        let (w, h, mut px) = frame_with("984,447", (140, 400), (640, 480), &cfg);

        let mut paint = |x0: u32, y0: u32, w0: u32, h0: u32, v: [u8; 3]| {
            for y in y0..y0 + h0 {
                for x in x0..x0 + w0 {
                    let i = ((y * w + x) * 4) as usize;
                    px[i] = v[0];
                    px[i + 1] = v[1];
                    px[i + 2] = v[2];
                }
            }
        };
        // Widen the plate either side of the number, the way the field is
        // actually drawn, and put a coin on the left and a label on the
        // right — both in shapes the atlas has never been taught.
        const PLATE: [u8; 3] = [245, 245, 240];
        const INK: [u8; 3] = [20, 20, 20];
        paint(118, 400, 22, 14, PLATE);
        paint(186, 400, 26, 14, PLATE);
        paint(121, 403, 8, 8, INK); // the coin
        paint(196, 404, 11, 4, INK); // MESOS

        let f = Frame { w, h, px: &px };
        let found = candidates(&f, &cfg);
        assert_eq!(found.len(), 1, "the number is still in there: {found:?}");
        assert_eq!(found[0].1, 984_447);
    }

    /// Why the purse has an atlas of its own.
    ///
    /// Same client, two typefaces. The status bar closes the loop of its
    /// `9` at the bottom in seven rows; the inventory draws it in eight
    /// with a tail. Matching one against the other reads nothing, which is
    /// exactly what the first live frame did.
    #[test]
    fn the_inventory_does_not_write_in_the_status_bar_font() {
        let p = profile();
        let m = p.mesos.clone().expect("a meso reader");
        for c in NEEDED.chars().filter(|c| *c != ',') {
            let key = c.to_string();
            let bar = &p.atlas[&key][0];
            let inv = &m.atlas[&key][0];
            assert!(
                bar.h != inv.h || bar.bits != inv.bits,
                "{c:?} came out identical in both fonts — then one of the atlases is wrong"
            );
        }
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
