//! Find the player's own character sprite in a captured frame.
//!
//! The trick is that the client renders the character's name twice in the
//! same bitmap font at the same size: once on the bottom status bar, and
//! once on the label directly beneath the sprite. So no letter atlas and no
//! text recognition is needed — the name as drawn on the status bar *is* the
//! search template. Find where else that exact bitmap appears, and the
//! sprite is the box sitting directly above it.
//!
//! Pipeline:
//!   1. binarize a generous slice of the status bar, trim to the ink
//!   2. slide that template over the frame, skipping the bar it came from
//!   3. crop the box above each strong match
//!   4. flood-fill the background away from the borders inward
//!   5. keep the candidate that actually looks like a sprite
//!
//! Step 5 matters because the name can be drawn elsewhere: in chat, in a
//! buddy list, in another player's message, on a shop sign. Those matches
//! score just as well on text, so the text score alone cannot pick the right
//! one. What separates them is what sits above the tag: a sprite keys down
//! to a mid-sized, many-coloured island, whereas UI chrome keys to nearly
//! nothing, to nearly everything, or to one flat hue.
//!
//! This reads a pixel buffer and does arithmetic. It opens no file, decodes
//! no image format, and knows nothing about the game beyond where its status
//! bar sits.

use std::collections::HashSet;

/// Luminance cut separating the light name glyphs from their dark plate.
const INK: u8 = 150;
/// Background flood tolerance, per channel.
const KEY_TOL: i32 = 34;
/// Text-match score a candidate must reach before we bother keying it.
///
/// This is a shortlist, not the answer. Two renderings of the same name
/// score alike on text, so what actually picks the character out of chat
/// lines and shop signs is `judge` and `side_ink` below — and those are
/// strict. Measured live at 0.90, the player's own tag scored 0.85 on a
/// stretched frame and 0.88 unstretched, and was thrown away before the
/// checks that would have recognised it ran at all.
///
/// The two renderings are not pixel-identical even on the client's own
/// grid: the status-bar name and the overhead label sit on different
/// backdrops, so their glyph edges antialias differently and the luma cut
/// lands in different places along them. 0.85 admits the real tag with
/// room to spare while staying far above anything the frame produces by
/// coincidence; the cost is that a few more candidates get keyed and
/// judged, which is work, not risk.
pub const ACCEPT: f32 = 0.85;
/// Scores below this are not even worth reporting as runners-up.
pub const REPORT: f32 = 0.80;

/// Reference frame all the geometry below was measured on: the game window at
/// 2048x1152 logical pixels, captured at 125% display scaling, so 2560x1440
/// physical pixels in the bitmap.
pub const REF_W: u32 = 2560;
pub const REF_H: u32 = 1440;

/// A borrowed RGBA8 frame.
///
/// The same buffer the compositor hands the tracker, and the same one the
/// workbench gets out of a decoded PNG — so the pipeline below is shared
/// rather than written twice and tuned twice.
pub struct View<'a> {
    pub w: u32,
    pub h: u32,
    pub px: &'a [u8],
}

impl View<'_> {
    #[inline]
    pub fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        match self.px.get(i..i + 4) {
            Some(p) => [p[0], p[1], p[2], p[3]],
            None => [0, 0, 0, 0],
        }
    }
}

/// A cut-out sprite with its background keyed to transparent.
#[derive(Clone, Debug)]
pub struct Sprite {
    pub w: u32,
    pub h: u32,
    /// Row-major RGBA8, alpha 0 where the background was keyed away.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct Geom {
    /// Status-bar slice holding the name: (x, y, w, h).
    pub name: (u32, u32, u32, u32),
    /// Rows at or below this are status-bar chrome; never match there.
    pub ui_floor: u32,
    pub sprite_w: u32,
    pub sprite_h: u32,
    pub sprite_gap: u32,
}

impl Geom {
    pub fn derive(fw: u32, fh: u32) -> Geom {
        let s = fh as f32 / REF_H as f32;
        let px = |v: f32| ((v * s).round() as u32).max(1);

        // Measured on the reference frame, then re-expressed as offsets:
        // 1280 - 684 = 596 left of centre, 1440 - 1407 = 33 up from the bottom.
        let name_x = (fw / 2).saturating_sub(px(596.0));
        let name_y = fh.saturating_sub(px(33.0));
        let name_w = px(200.0).min(fw.saturating_sub(name_x));

        Geom {
            name: (name_x, name_y, name_w, px(24.0)),
            // 1440 - 1360 = 80 reference rows of chrome at the bottom.
            ui_floor: fh.saturating_sub(px(80.0)),
            sprite_w: px(124.0),
            sprite_h: px(150.0),
            sprite_gap: px(8.0),
        }
    }

    /// The crop that should hold the sprite for a tag found at `(mx, my)`
    /// with a template `tw` wide. `None` when the box will not fit on the
    /// frame, which by itself disqualifies the candidate.
    pub fn sprite_box(&self, fw: u32, mx: u32, my: u32, tw: u32) -> Option<(u32, u32, u32, u32)> {
        if fw < self.sprite_w {
            return None;
        }
        let cx = mx + tw / 2;
        let x0 = cx.saturating_sub(self.sprite_w / 2).min(fw - self.sprite_w);
        let bottom = my.saturating_sub(self.sprite_gap);
        let y0 = bottom.saturating_sub(self.sprite_h);
        let h = bottom - y0;
        // A tag near the top of the frame leaves no room for a body above it.
        if h < self.sprite_h / 2 {
            return None;
        }
        Some((x0, y0, self.sprite_w, h))
    }
}

pub struct Mask {
    pub w: usize,
    pub h: usize,
    bits: Vec<bool>,
}

impl Mask {
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.w + x]
    }
}

fn luma(p: [u8; 4]) -> u8 {
    ((p[0] as f32) * 0.299 + (p[1] as f32) * 0.587 + (p[2] as f32) * 0.114) as u8
}

pub fn binarize(v: &View, x0: u32, y0: u32, w: u32, h: u32) -> Mask {
    let mut bits = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            bits.push(luma(v.at(x0 + x, y0 + y)) >= INK);
        }
    }
    Mask { w: w as usize, h: h as usize, bits }
}

/// Trim a mask to its ink. Returns the trimmed mask and its offset.
pub fn trim(m: &Mask) -> Option<(Mask, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..m.h {
        for x in 0..m.w {
            if m.at(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut bits = Vec::with_capacity(w * h);
    for y in y0..=y1 {
        for x in x0..=x1 {
            bits.push(m.at(x, y));
        }
    }
    Some((Mask { w, h, bits }, x0, y0))
}

/// A handful of scattered ink / non-ink points used to reject a candidate
/// position in a few comparisons instead of a few thousand.
pub fn probes(t: &Mask) -> Vec<(usize, usize, bool)> {
    let mut on = Vec::new();
    let mut off = Vec::new();
    for y in 0..t.h {
        for x in 0..t.w {
            if t.at(x, y) { &mut on } else { &mut off }.push((x, y));
        }
    }
    let mut out = Vec::new();
    for set in [&on, &off] {
        if set.is_empty() {
            continue;
        }
        let step = (set.len() / 10).max(1);
        for i in (0..set.len()).step_by(step).take(10) {
            out.push((set[i].0, set[i].1, t.at(set[i].0, set[i].1)));
        }
    }
    out
}

/// Thresholds for "this crop could be a character sprite". Deliberately
/// loose: every bound sits a long way from what the real sprite measures,
/// because the cost of rejecting the true match (a generic avatar for one
/// more frame) is small but the cost of a false accept (a nonsense avatar,
/// held with full confidence) is not.
pub const MIN_KEPT_FRAC: f32 = 0.08;
pub const MAX_KEPT_FRAC: f32 = 0.97;
/// Distinct colours among the kept pixels, quantised to 4 bits per channel.
pub const MIN_COLOURS: usize = 24;
/// Largest share of the strips beside a tag that may be bright ink.
pub const MAX_SIDE_INK: f32 = 0.10;
/// Largest share any single 30°-wide hue bin may hold of the saturated
/// pixels. Set high on purpose: a character in a single-colour outfit
/// measures 0.7-0.8 on this, so anything tighter would start rejecting real
/// sprites. It is here to catch a large flat coloured panel, which the
/// colour-variety check misses when the panel is gradient-shaded.
pub const MAX_HUE_DOMINANCE: f32 = 0.95;

/// How much bright ink sits immediately left and right of a matched tag.
///
/// A character's own name label stands alone on its plate. The same name
/// drawn in chat is part of a line — `[Lv.40]<name> : ...` — so there is
/// other text hard up against it. This turns out to be the one cheap signal
/// that separates those two cases: the keyed-crop statistics below cannot,
/// because a chat box in this client is translucent, so the crop above a
/// chat match is busy, many-coloured game world and looks every bit as
/// sprite-like as the real thing.
///
/// `gap` skips the glyph antialiasing hard against the tag; `reach` is how
/// far out to look.
pub fn side_ink(
    frame: &Mask,
    mx: usize,
    my: usize,
    tw: usize,
    th: usize,
    gap: usize,
    reach: usize,
) -> f32 {
    let mut ink = 0usize;
    let mut seen = 0usize;
    for y in my..(my + th).min(frame.h) {
        for d in 0..reach {
            if let Some(x) = mx.checked_sub(gap + 1 + d) {
                seen += 1;
                if frame.at(x, y) {
                    ink += 1;
                }
            }
            let x = mx + tw + gap + d;
            if x < frame.w {
                seen += 1;
                if frame.at(x, y) {
                    ink += 1;
                }
            }
        }
    }
    if seen == 0 { 0.0 } else { ink as f32 / seen as f32 }
}

pub struct Keyed {
    /// Background-keyed crop, trimmed to what survived.
    pub sprite: Sprite,
    pub kept: usize,
    pub kept_frac: f32,
    pub colours: usize,
    pub hue_dom: f32,
}

/// Which 30° hue bin a pixel falls in, or `None` if it is too grey to have
/// one.
fn hue_bin(p: [u8; 4]) -> Option<usize> {
    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let c = mx - mn;
    if c < 24 {
        return None;
    }
    let cf = c as f32;
    let h = if mx == r {
        ((g - b) as f32 / cf).rem_euclid(6.0)
    } else if mx == g {
        (b - r) as f32 / cf + 2.0
    } else {
        (r - g) as f32 / cf + 4.0
    };
    Some(((h * 2.0) as usize).min(11))
}

/// Crop `(x0, y0, w, h)` and key its background by flooding inward from the
/// border, then measure the island that is left. `None` if nothing survives.
pub fn key_sprite(v: &View, x0: u32, y0: u32, w: u32, h: u32) -> Option<Keyed> {
    let n = (w * h) as usize;
    let mut cut: Vec<[u8; 4]> = Vec::with_capacity(n);
    for y in 0..h {
        for x in 0..w {
            cut.push(v.at(x0 + x, y0 + y));
        }
    }

    // Flood from the border inward rather than testing colours globally: the
    // character's white clothing is the same colour as plenty of
    // backgrounds, and a global test punches holes straight through it.
    let bg = cut[0];
    let mut clear = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        stack.push(x as usize);
        stack.push(((h - 1) * w + x) as usize);
    }
    for y in 0..h {
        stack.push((y * w) as usize);
        stack.push((y * w + w - 1) as usize);
    }
    while let Some(i) = stack.pop() {
        if clear[i] {
            continue;
        }
        let (x, y) = (i as u32 % w, i as u32 / w);
        let p = cut[i];
        let near = (0..3).all(|c| (p[c] as i32 - bg[c] as i32).abs() <= KEY_TOL);
        if !near {
            continue;
        }
        clear[i] = true;
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w as usize);
        }
        if y + 1 < h {
            stack.push(i + w as usize);
        }
    }

    let mut kept = 0usize;
    let (mut ax0, mut ay0, mut ax1, mut ay1) = (w, h, 0u32, 0u32);
    let mut palette: HashSet<u16> = HashSet::new();
    let mut hues = [0usize; 12];
    let mut saturated = 0usize;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if clear[i] {
                cut[i] = [0, 0, 0, 0];
                continue;
            }
            kept += 1;
            ax0 = ax0.min(x);
            ay0 = ay0.min(y);
            ax1 = ax1.max(x);
            ay1 = ay1.max(y);
            let p = cut[i];
            palette.insert(
                ((p[0] as u16 >> 4) << 8) | ((p[1] as u16 >> 4) << 4) | (p[2] as u16 >> 4),
            );
            if let Some(b) = hue_bin(p) {
                hues[b] += 1;
                saturated += 1;
            }
        }
    }
    if kept == 0 || ax1 < ax0 {
        return None;
    }

    // A crop with hardly any saturated pixels is not "all one hue", it is
    // achromatic; the colour-variety check is what judges those.
    let hue_dom = if saturated * 10 < kept {
        0.0
    } else {
        *hues.iter().max().unwrap() as f32 / saturated as f32
    };

    let (tw, th) = (ax1 - ax0 + 1, ay1 - ay0 + 1);
    let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        for x in 0..tw {
            rgba.extend_from_slice(&cut[((ay0 + y) * w + ax0 + x) as usize]);
        }
    }

    Some(Keyed {
        sprite: Sprite { w: tw, h: th, rgba },
        kept,
        kept_frac: kept as f32 / (w * h) as f32,
        colours: palette.len(),
        hue_dom,
    })
}

/// `Ok` if this crop is plausibly a sprite, `Err(reason)` otherwise.
pub fn judge(k: &Keyed, side: f32) -> Result<(), &'static str> {
    if side > MAX_SIDE_INK {
        return Err("more text hard up against the tag — a line of chat, not a label");
    }
    if k.kept_frac < MIN_KEPT_FRAC {
        return Err("keyed out almost entirely");
    }
    if k.kept_frac > MAX_KEPT_FRAC {
        return Err("nothing keyed out — no background around it");
    }
    if k.colours < MIN_COLOURS {
        return Err("too few distinct colours for a sprite");
    }
    if k.hue_dom > MAX_HUE_DOMINANCE {
        return Err("almost entirely one hue");
    }
    Ok(())
}

/// How sprite-like a surviving candidate is, in 0..1. Used to order
/// candidates that all pass `judge`, in place of the text score — two
/// renderings of the same name score alike on text, so the text score cannot
/// break that tie.
pub fn sprite_score(k: &Keyed, side: f32) -> f32 {
    let frac = 1.0 - ((k.kept_frac - 0.5).abs() / 0.5);
    let colour = (k.colours as f32 / 200.0).min(1.0);
    let hue = 1.0 - k.hue_dom;
    let alone = 1.0 - (side / MAX_SIDE_INK).min(1.0);
    0.35 * colour + 0.25 * frac + 0.15 * hue + 0.25 * alone
}

pub enum Fail {
    /// The frame does not fit the layout we know how to read.
    Layout(String),
    /// No name tag found: character off-screen, hidden, or geometry wrong.
    NoTag { best: Option<(u32, u32, f32)> },
    /// Tags found, but nothing above any of them looks like a sprite.
    NotASprite(Vec<(u32, u32, &'static str)>),
}

impl Fail {
    pub fn code(&self) -> i32 {
        match self {
            Fail::Layout(_) => 1,
            Fail::NoTag { .. } => 2,
            Fail::NotASprite(_) => 3,
        }
    }

    /// One line, for a log that nobody is watching live.
    pub fn why(&self) -> String {
        match self {
            Fail::Layout(m) => m.clone(),
            Fail::NoTag { best: Some((x, y, s)) } => {
                format!("no name tag; closest was ({x}, {y}) at {:.0}%", s * 100.0)
            }
            Fail::NoTag { best: None } => "no name tag anywhere on the frame".into(),
            Fail::NotASprite(r) => match r.first() {
                Some((x, y, why)) => format!("tag at ({x}, {y}) rejected: {why}"),
                None => "nothing above any tag looked like a sprite".into(),
            },
        }
    }
}

pub struct Candidate {
    pub x: u32,
    pub y: u32,
    pub text: f32,
    pub keyed: Keyed,
    pub side: f32,
    pub sprite: f32,
}

pub struct Found {
    /// Distinct text matches above `ACCEPT`, before plausibility.
    pub distinct: usize,
    pub best: Candidate,
    /// Plausible candidates that lost, and implausible ones with the reason.
    pub rejected: Vec<(u32, u32, &'static str)>,
    pub runners_up: Vec<(u32, u32, f32, f32)>,
}

/// The whole pipeline. `note` is handed each line of progress, so the
/// workbench can print it and the overlay can throw it away.
pub fn locate(v: &View, mut note: impl FnMut(String)) -> Result<Found, Fail> {
    let (fw, fh) = (v.w, v.h);
    let g = Geom::derive(fw, fh);
    let (nx, ny, nw, nh) = g.name;
    if fw < nx + nw || fh < ny + nh || nw < 8 || nh < 8 || g.ui_floor < g.sprite_h {
        return Err(Fail::Layout(format!(
            "frame {fw}x{fh} is too small for the expected status-bar layout \
             (name box would be {nw}x{nh} at {nx},{ny})"
        )));
    }

    // 1. the template
    let raw = binarize(v, nx, ny, nw, nh);
    let Some((tmpl, tx, ty)) = trim(&raw) else {
        return Err(Fail::Layout(
            "no ink in the name box — is the status bar where we think?".into(),
        ));
    };
    note(format!(
        "geometry: name box {nw}x{nh} at ({nx}, {ny}), ui floor {}, sprite {}x{} gap {}",
        g.ui_floor, g.sprite_w, g.sprite_h, g.sprite_gap
    ));
    note(format!(
        "template: {}x{} px, taken from ({}, {})",
        tmpl.w,
        tmpl.h,
        nx + tx as u32,
        ny + ty as u32
    ));

    // 2. search everything above the status-bar chrome
    let frame = binarize(v, 0, 0, fw, g.ui_floor.min(fh));
    let pr = probes(&tmpl);
    let total = (tmpl.w * tmpl.h) as f32;
    let mut hits: Vec<(usize, usize, f32)> = Vec::new();

    // The probe filter must tolerate a few misses. The same string drawn at
    // a different sub-pixel offset rasterises slightly differently once the
    // frame has been DPI-upscaled, so the correct match still fails one or
    // two probe points. Demanding a clean sweep rejects the answer.
    let max_fail = (pr.len() / 4).max(2);

    for y in 0..frame.h.saturating_sub(tmpl.h) {
        'next: for x in 0..frame.w.saturating_sub(tmpl.w) {
            let mut fails = 0usize;
            for &(px, py, want) in &pr {
                if frame.at(x + px, y + py) != want {
                    fails += 1;
                    if fails > max_fail {
                        continue 'next;
                    }
                }
            }
            let mut agree = 0usize;
            for ty2 in 0..tmpl.h {
                for tx2 in 0..tmpl.w {
                    if frame.at(x + tx2, y + ty2) == tmpl.at(tx2, ty2) {
                        agree += 1;
                    }
                }
            }
            // Collect near-misses too: when nothing clears the bar, the
            // runners-up say whether the template was wrong or the character
            // simply is not on screen.
            let score = agree as f32 / total;
            if score > REPORT {
                hits.push((x, y, score));
            }
        }
    }

    hits.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    // Collapse neighbouring hits on the same tag down to the best one.
    let mut peaks: Vec<(usize, usize, f32)> = Vec::new();
    for h in &hits {
        if !peaks.iter().any(|p| p.0.abs_diff(h.0) < tmpl.w && p.1.abs_diff(h.1) < tmpl.h) {
            peaks.push(*h);
        }
    }
    note(format!("matches : {} raw, {} distinct", hits.len(), peaks.len()));
    for (i, p) in peaks.iter().take(5).enumerate() {
        note(format!("   #{}  ({}, {})  {:.1}%", i + 1, p.0, p.1, p.2 * 100.0));
    }

    let strong: Vec<(usize, usize, f32)> = peaks.iter().copied().filter(|p| p.2 >= ACCEPT).collect();
    if strong.is_empty() {
        return Err(Fail::NoTag { best: peaks.first().map(|p| (p.0 as u32, p.1 as u32, p.2)) });
    }

    // 3 & 4. key the box above every strong tag, and see which ones could
    // actually be a character.
    let mut good: Vec<Candidate> = Vec::new();
    let mut rejected: Vec<(u32, u32, &'static str)> = Vec::new();
    for &(mx, my, text) in &strong {
        let (mx, my) = (mx as u32, my as u32);
        let Some((x0, y0, w, h)) = g.sprite_box(fw, mx, my, tmpl.w as u32) else {
            rejected.push((mx, my, "no room for a sprite above the tag"));
            continue;
        };
        let Some(keyed) = key_sprite(v, x0, y0, w, h) else {
            rejected.push((mx, my, "sprite box keyed out entirely"));
            continue;
        };
        let side = side_ink(
            &frame,
            mx as usize,
            my as usize,
            tmpl.w,
            tmpl.h,
            g.sprite_gap as usize,
            (g.sprite_gap * 4) as usize,
        );
        note(format!(
            "   cand ({mx}, {my}): text {:.1}%  kept {} ({:.0}%)  colours {}  \
             hue-dom {:.2}  side-ink {:.2}",
            text * 100.0,
            keyed.kept,
            keyed.kept_frac * 100.0,
            keyed.colours,
            keyed.hue_dom,
            side
        ));
        match judge(&keyed, side) {
            Err(why) => rejected.push((mx, my, why)),
            Ok(()) => {
                let sprite = sprite_score(&keyed, side);
                good.push(Candidate { x: mx, y: my, text, keyed, side, sprite });
            }
        }
    }

    if good.is_empty() {
        return Err(Fail::NotASprite(rejected));
    }

    // 5. most sprite-like wins, not highest text score.
    good.sort_by(|a, b| b.sprite.partial_cmp(&a.sprite).unwrap());
    let runners_up = good[1..].iter().map(|c| (c.x, c.y, c.text, c.sprite)).collect();
    let best = good.remove(0);
    Ok(Found { distinct: strong.len(), best, rejected, runners_up })
}

/// Just the sprite, for the overlay, which has no use for the diagnostics.
pub fn find(v: &View) -> Result<Sprite, Fail> {
    locate(v, |_| {}).map(|f| f.best.keyed.sprite)
}

/// Standard base64, no padding omitted, no line breaks.
///
/// Hand-written rather than pulled in: the only thing this program ever
/// encodes is a few kilobytes of pixels on its way to its own webview, which
/// is not worth a dependency and the supply chain that comes with one.
pub fn base64(bytes: &[u8]) -> String {
    const SET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(SET[(n >> 18) as usize & 63] as char);
        out.push(SET[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { SET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { SET[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat frame has no name tag, and must say so rather than returning
    /// whatever the flood fill left behind.
    #[test]
    fn a_blank_frame_finds_nothing() {
        let px = vec![30u8; (640 * 480 * 4) as usize];
        let v = View { w: 640, h: 480, px: &px };
        assert!(find(&v).is_err());
    }

    /// Smaller than the status-bar layout is a layout failure, not a crash.
    #[test]
    fn a_tiny_frame_is_refused_by_geometry() {
        let px = vec![0u8; (16 * 16 * 4) as usize];
        let v = View { w: 16, h: 16, px: &px };
        match find(&v) {
            Err(Fail::Layout(_)) => {}
            Err(e) => panic!("wrong failure: {}", e.why()),
            Ok(_) => panic!("found a sprite in a 16x16 frame"),
        }
    }

    #[test]
    fn base64_round_trips_through_a_known_vector() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(&[0, 0, 0, 255]), "AAAA/w==");
    }

    /// The geometry is derived from the frame, so a client at another size
    /// still gets a plausible name box rather than the reference one.
    #[test]
    fn geometry_scales_with_the_frame() {
        let a = Geom::derive(2560, 1440);
        let b = Geom::derive(1280, 720);
        assert_eq!(a.sprite_w, b.sprite_w * 2, "sprite box scales with height");
        assert!(a.name.1 > b.name.1, "the name box sits near the bottom of each");
        assert!(a.ui_floor > a.name.1 - a.name.3 * 4, "the floor is above the bar");
    }
}
