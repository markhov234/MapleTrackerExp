//! Locate the player's character sprite in a captured frame, automatically.
//!
//! The trick is that the client renders the character's name twice in the same
//! bitmap font at the same size: once on the bottom status bar, and once on the
//! label directly beneath the sprite. So no letter atlas and no text
//! recognition is needed — the name as drawn on the status bar IS the search
//! template. Find where else that exact bitmap appears, and the sprite is the
//! box sitting directly above it.
//!
//! Pipeline:
//!   1. binarize a generous slice of the status bar, trim to the ink
//!   2. slide that template over the frame, skipping the bar it came from
//!   3. crop the box above each strong match
//!   4. flood-fill the background away from the borders inward
//!   5. keep the candidate that actually looks like a sprite
//!
//! Step 5 matters because the name can be drawn elsewhere: in chat, in a buddy
//! list, in another player's message, on a shop sign. Those matches score just
//! as well on text, so the text score alone cannot pick the right one. What
//! separates them is what sits above the tag: a sprite keys down to a
//! mid-sized, many-coloured island, whereas UI chrome keys to nearly nothing,
//! to nearly everything, or to one flat hue.
//!
//! Everything here reads pixels. Nothing touches the game.
//!
//! Usage: find-character [frame.png] [out.png]
//!        find-character --at X Y        score the template at one position
//!        find-character --selftest      regression check against captures/

use image::{Rgba, RgbaImage};
use std::collections::HashSet;

/// Luminance cut separating the light name glyphs from their dark plate.
const INK: u8 = 150;
/// Background flood tolerance, per channel.
const KEY_TOL: i32 = 34;
/// Text-match score a candidate must reach before we bother keying it.
const ACCEPT: f32 = 0.90;
/// Scores below this are not even worth reporting as runners-up.
const REPORT: f32 = 0.80;

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Reference frame all the geometry below was measured on: the game window at
/// 2048x1152 logical pixels, captured at 125% display scaling, so 2560x1440
/// physical pixels in the bitmap.
const REF_W: u32 = 2560;
const REF_H: u32 = 1440;

/// Layout derived from the frame's own dimensions.
///
/// Encoded assumptions, in order of how likely they are to bite:
///
///  * **The UI scales with frame height.** The status bar and the name label
///    are fixed-size bitmaps in the client's own pixels, so their size in the
///    capture is set by the display scale factor — which the frame alone does
///    not reveal. We stand in for it with `frame_height / 1440`, which is
///    exact whenever the logical window height is the reference 1152 (any
///    display scale), and wrong if someone runs a different logical
///    resolution. When it is wrong the derived name box lands on the wrong
///    rows, no template matches, and the tool exits 2 — the caller keeps the
///    generic avatar. That is the intended failure: wrong, loudly, not quietly
///    plausible.
///  * **The status bar is bottom-centred**, so the name box is placed by its
///    offset left of the frame centre and up from the bottom edge, not by
///    absolute coordinates. Wider frames therefore work unchanged.
///  * **The name is left-aligned** in its slot, with the level/job text
///    immediately to its left and the line above. Hence the left edge of the
///    box is tight (it must not swallow that neighbouring ink, which would be
///    trimmed into the template) while the right edge is generous — verified
///    clear for 220 reference pixels, so a maximum-length name still fits.
///  * **The frame is the game window alone**, not a desktop capture with the
///    window somewhere inside it.
struct Geom {
    /// Status-bar slice holding the name: (x, y, w, h).
    name: (u32, u32, u32, u32),
    /// Rows at or below this are status-bar chrome; never match there.
    ui_floor: u32,
    sprite_w: u32,
    sprite_h: u32,
    sprite_gap: u32,
}

impl Geom {
    fn derive(fw: u32, fh: u32) -> Geom {
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

    /// The crop that should hold the sprite for a tag found at `(mx, my)` with
    /// a template `tw` wide. `None` when the box will not fit on the frame,
    /// which by itself disqualifies the candidate.
    fn sprite_box(&self, fw: u32, mx: u32, my: u32, tw: u32) -> Option<(u32, u32, u32, u32)> {
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

// ---------------------------------------------------------------------------
// Bitmap matching
// ---------------------------------------------------------------------------

struct Mask {
    w: usize,
    h: usize,
    bits: Vec<bool>,
}

impl Mask {
    #[inline]
    fn at(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.w + x]
    }
}

fn luma(p: &Rgba<u8>) -> u8 {
    ((p.0[0] as f32) * 0.299 + (p.0[1] as f32) * 0.587 + (p.0[2] as f32) * 0.114) as u8
}

fn binarize(img: &RgbaImage, x0: u32, y0: u32, w: u32, h: u32) -> Mask {
    let mut bits = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x0 + x, y0 + y);
            bits.push(luma(px) >= INK);
        }
    }
    Mask { w: w as usize, h: h as usize, bits }
}

/// Trim a mask to its ink. Returns the trimmed mask and its offset.
fn trim(m: &Mask) -> Option<(Mask, usize, usize)> {
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
fn probes(t: &Mask) -> Vec<(usize, usize, bool)> {
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

// ---------------------------------------------------------------------------
// Keying and plausibility
// ---------------------------------------------------------------------------

/// Thresholds for "this crop could be a character sprite". Deliberately loose:
/// every bound sits a long way from what the real sprite measures, because the
/// cost of rejecting the true match (a generic avatar for one more frame) is
/// small but the cost of a false accept (a nonsense avatar, held with full
/// confidence) is not.
const MIN_KEPT_FRAC: f32 = 0.08;
const MAX_KEPT_FRAC: f32 = 0.97;
/// Distinct colours among the kept pixels, quantised to 4 bits per channel.
const MIN_COLOURS: usize = 24;
/// Largest share of the strips beside a tag that may be bright ink.
const MAX_SIDE_INK: f32 = 0.10;
/// Largest share any single 30°-wide hue bin may hold of the saturated pixels.
/// Set high on purpose: a character in a single-colour outfit measures 0.7-0.8
/// on this, so anything tighter would start rejecting real sprites. It is here
/// to catch a large flat coloured panel, which the colour-variety check misses
/// when the panel is gradient-shaded.
const MAX_HUE_DOMINANCE: f32 = 0.95;

/// How much bright ink sits immediately left and right of a matched tag.
///
/// A character's own name label stands alone on its plate. The same name drawn
/// in chat is part of a line — `[Lv.40]<name> : ...` — so there is other text
/// hard up against it. This turns out to be the one cheap signal that
/// separates those two cases: the keyed-crop statistics below cannot, because
/// a chat box in this client is translucent, so the crop above a chat match is
/// busy, many-coloured game world and looks every bit as sprite-like as the
/// real thing.
///
/// `gap` skips the glyph antialiasing hard against the tag; `reach` is how far
/// out to look.
fn side_ink(frame: &Mask, mx: usize, my: usize, tw: usize, th: usize, gap: usize, reach: usize) -> f32 {
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

struct Keyed {
    /// Background-keyed crop, trimmed to what survived.
    img: RgbaImage,
    kept: usize,
    kept_frac: f32,
    colours: usize,
    hue_dom: f32,
}

/// Which 30° hue bin a pixel falls in, or `None` if it is too grey to have one.
fn hue_bin(p: &Rgba<u8>) -> Option<usize> {
    let (r, g, b) = (p.0[0] as i32, p.0[1] as i32, p.0[2] as i32);
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
fn key_sprite(img: &RgbaImage, x0: u32, y0: u32, w: u32, h: u32) -> Option<Keyed> {
    let mut cut = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            cut.put_pixel(x, y, *img.get_pixel(x0 + x, y0 + y));
        }
    }

    // Flood from the border inward rather than testing colours globally: the
    // character's white clothing is the same colour as plenty of backgrounds,
    // and a global test punches holes straight through it.
    let bg = *cut.get_pixel(0, 0);
    let n = (w * h) as usize;
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
        let p = cut.get_pixel(x, y);
        let near = (0..3).all(|c| (p.0[c] as i32 - bg.0[c] as i32).abs() <= KEY_TOL);
        if !near {
            continue;
        }
        clear[i] = true;
        if x > 0 { stack.push(i - 1); }
        if x + 1 < w { stack.push(i + 1); }
        if y > 0 { stack.push(i - w as usize); }
        if y + 1 < h { stack.push(i + w as usize); }
    }

    let mut kept = 0usize;
    let (mut ax0, mut ay0, mut ax1, mut ay1) = (w, h, 0u32, 0u32);
    let mut palette: HashSet<u16> = HashSet::new();
    let mut hues = [0usize; 12];
    let mut saturated = 0usize;
    for y in 0..h {
        for x in 0..w {
            if clear[(y * w + x) as usize] {
                cut.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                continue;
            }
            kept += 1;
            ax0 = ax0.min(x);
            ay0 = ay0.min(y);
            ax1 = ax1.max(x);
            ay1 = ay1.max(y);
            let p = cut.get_pixel(x, y);
            palette.insert(
                ((p.0[0] as u16 >> 4) << 8) | ((p.0[1] as u16 >> 4) << 4) | (p.0[2] as u16 >> 4),
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
    let mut out = RgbaImage::new(tw, th);
    for y in 0..th {
        for x in 0..tw {
            out.put_pixel(x, y, *cut.get_pixel(ax0 + x, ay0 + y));
        }
    }

    Some(Keyed {
        img: out,
        kept,
        kept_frac: kept as f32 / (w * h) as f32,
        colours: palette.len(),
        hue_dom,
    })
}

/// `Ok` if this crop is plausibly a sprite, `Err(reason)` otherwise.
fn judge(k: &Keyed, side: f32) -> Result<(), &'static str> {
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

/// How sprite-like a surviving candidate is, in 0..1. Used to order candidates
/// that all pass `judge`, in place of the text score — two renderings of the
/// same name score alike on text, so the text score cannot break that tie.
fn sprite_score(k: &Keyed, side: f32) -> f32 {
    let frac = 1.0 - ((k.kept_frac - 0.5).abs() / 0.5);
    let colour = (k.colours as f32 / 200.0).min(1.0);
    let hue = 1.0 - k.hue_dom;
    let alone = 1.0 - (side / MAX_SIDE_INK).min(1.0);
    0.35 * colour + 0.25 * frac + 0.15 * hue + 0.25 * alone
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

enum Fail {
    /// The frame does not fit the layout we know how to read.
    Layout(String),
    /// No name tag found: character off-screen, hidden, or geometry wrong.
    NoTag { best: Option<(u32, u32, f32)> },
    /// Tags found, but nothing above any of them looks like a sprite.
    NotASprite(Vec<(u32, u32, &'static str)>),
}

impl Fail {
    fn code(&self) -> i32 {
        match self {
            Fail::Layout(_) => 1,
            Fail::NoTag { .. } => 2,
            Fail::NotASprite(_) => 3,
        }
    }
}

struct Candidate {
    x: u32,
    y: u32,
    text: f32,
    keyed: Keyed,
    side: f32,
    sprite: f32,
}

struct Found {
    /// Distinct text matches above `ACCEPT`, before plausibility.
    distinct: usize,
    best: Candidate,
    /// Plausible candidates that lost, and implausible ones with the reason.
    rejected: Vec<(u32, u32, &'static str)>,
    runners_up: Vec<(u32, u32, f32, f32)>,
}

fn locate(img: &RgbaImage, verbose: bool) -> Result<Found, Fail> {
    let (fw, fh) = img.dimensions();
    let g = Geom::derive(fw, fh);
    let (nx, ny, nw, nh) = g.name;
    if fw < nx + nw || fh < ny + nh || nw < 8 || nh < 8 || g.ui_floor < g.sprite_h {
        return Err(Fail::Layout(format!(
            "frame {fw}x{fh} is too small for the expected status-bar layout \
             (name box would be {nw}x{nh} at {nx},{ny})"
        )));
    }

    // 1. the template
    let raw = binarize(img, nx, ny, nw, nh);
    let Some((tmpl, tx, ty)) = trim(&raw) else {
        return Err(Fail::Layout(
            "no ink in the name box — is the status bar where we think?".into(),
        ));
    };
    if verbose {
        println!(
            "geometry: name box {nw}x{nh} at ({nx}, {ny}), ui floor {}, sprite {}x{} gap {}",
            g.ui_floor, g.sprite_w, g.sprite_h, g.sprite_gap
        );
        println!(
            "template: {}x{} px, taken from ({}, {})",
            tmpl.w,
            tmpl.h,
            nx + tx as u32,
            ny + ty as u32
        );
    }

    // 2. search everything above the status-bar chrome
    let frame = binarize(img, 0, 0, fw, g.ui_floor.min(fh));
    let pr = probes(&tmpl);
    let total = (tmpl.w * tmpl.h) as f32;
    let mut hits: Vec<(usize, usize, f32)> = Vec::new();

    // The probe filter must tolerate a few misses. The same string drawn at a
    // different sub-pixel offset rasterises slightly differently once the
    // frame has been DPI-upscaled, so the correct match still fails one or two
    // probe points. Demanding a clean sweep rejects the answer.
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
        if !peaks
            .iter()
            .any(|p| p.0.abs_diff(h.0) < tmpl.w && p.1.abs_diff(h.1) < tmpl.h)
        {
            peaks.push(*h);
        }
    }
    if verbose {
        println!("matches : {} raw, {} distinct", hits.len(), peaks.len());
        for (i, p) in peaks.iter().take(5).enumerate() {
            println!("   #{}  ({}, {})  {:.1}%", i + 1, p.0, p.1, p.2 * 100.0);
        }
    }

    let strong: Vec<(usize, usize, f32)> =
        peaks.iter().copied().filter(|p| p.2 >= ACCEPT).collect();
    if strong.is_empty() {
        return Err(Fail::NoTag {
            best: peaks.first().map(|p| (p.0 as u32, p.1 as u32, p.2)),
        });
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
        let Some(keyed) = key_sprite(img, x0, y0, w, h) else {
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
        if verbose {
            println!(
                "   cand ({mx}, {my}): text {:.1}%  kept {} ({:.0}%)  colours {}  \
                 hue-dom {:.2}  side-ink {:.2}",
                text * 100.0,
                keyed.kept,
                keyed.kept_frac * 100.0,
                keyed.colours,
                keyed.hue_dom,
                side
            );
        }
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

// ---------------------------------------------------------------------------
// Self-test
// ---------------------------------------------------------------------------

/// Regression check against the committed test frame. Fails loudly, with a
/// non-zero exit, if a retuned constant breaks detection.
///
/// `captures/frame.png` is 2560x1440, a level 40 Bandit.
fn selftest() -> i32 {
    const FRAME: &str = "captures/frame.png";
    let img = match image::open(FRAME) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("selftest FAIL: could not open {FRAME}: {e}");
            eprintln!("  (run it from the crate root, where captures/ lives)");
            return 1;
        }
    };
    assert_frame_dims(&img);

    let found = match locate(&img, true) {
        Ok(f) => f,
        Err(e) => {
            let what = match &e {
                Fail::Layout(m) => format!("layout rejected: {m}"),
                Fail::NoTag { best } => format!("no name tag found (best {best:?})"),
                Fail::NotASprite(r) => format!("no plausible sprite: {r:?}"),
            };
            eprintln!("selftest FAIL: {what}");
            return 1;
        }
    };

    let mut bad: Vec<String> = Vec::new();
    let mut check = |ok: bool, msg: String| {
        if !ok {
            bad.push(msg);
        }
    };

    let (ex, ey) = (799u32, 542u32);
    let (w, h) = found.best.keyed.img.dimensions();
    let kept = found.best.keyed.kept;
    let score = found.best.text;

    check(
        found.distinct == 1,
        format!("expected exactly 1 distinct match, got {}", found.distinct),
    );
    check(
        found.best.x.abs_diff(ex) <= 2 && found.best.y.abs_diff(ey) <= 2,
        format!(
            "expected tag within 2 px of ({ex}, {ey}), got ({}, {})",
            found.best.x, found.best.y
        ),
    );
    check(score > 0.90, format!("expected text score > 0.90, got {score:.4}"));
    check(
        w.abs_diff(124) <= 6 && h.abs_diff(143) <= 6,
        format!("expected sprite near 124x143, got {w}x{h}"),
    );
    check(kept > 8000, format!("expected more than 8000 kept px, got {kept}"));

    // The plausibility checks must not be sitting on top of the real answer.
    let k = &found.best.keyed;
    check(
        k.kept_frac > MIN_KEPT_FRAC * 2.0 && k.kept_frac < MAX_KEPT_FRAC - 0.1,
        format!(
            "kept fraction {:.3} is uncomfortably close to a plausibility bound \
             ({MIN_KEPT_FRAC}..{MAX_KEPT_FRAC})",
            k.kept_frac
        ),
    );
    check(
        k.colours >= MIN_COLOURS * 2,
        format!("colour count {} leaves no margin over MIN_COLOURS {MIN_COLOURS}", k.colours),
    );
    check(
        k.hue_dom < MAX_HUE_DOMINANCE - 0.1,
        format!("hue dominance {:.2} leaves no margin under {MAX_HUE_DOMINANCE}", k.hue_dom),
    );
    check(
        found.best.side < MAX_SIDE_INK / 2.0,
        format!(
            "side ink {:.3} leaves no margin under {MAX_SIDE_INK} — the real tag \
             should be standing alone on its plate",
            found.best.side
        ),
    );

    // Derived geometry must reproduce the hand-measured reference exactly.
    let g = Geom::derive(REF_W, REF_H);
    check(
        g.name == (684, 1407, 200, 24),
        format!("reference name box drifted: {:?}", g.name),
    );
    check(
        (g.ui_floor, g.sprite_w, g.sprite_h, g.sprite_gap) == (1360, 124, 150, 8),
        format!(
            "reference sprite geometry drifted: {:?}",
            (g.ui_floor, g.sprite_w, g.sprite_h, g.sprite_gap)
        ),
    );
    // And it must scale, not just hit the reference.
    let half = Geom::derive(REF_W / 2, REF_H / 2);
    check(
        half.sprite_w == 62 && half.name.0 == 640 - 298 && half.name.1 == 720 - 17,
        format!("half-scale geometry looks wrong: {:?} {}", half.name, half.sprite_w),
    );

    if bad.is_empty() {
        println!("\nselftest OK — tag ({}, {}) {:.2}%, sprite {w}x{h}, {kept} px", found.best.x, found.best.y, score * 100.0);
        0
    } else {
        eprintln!("\nselftest FAILED: {} problem(s)", bad.len());
        for b in &bad {
            eprintln!("  - {b}");
        }
        1
    }
}

fn assert_frame_dims(img: &RgbaImage) {
    let (w, h) = img.dimensions();
    if (w, h) != (REF_W, REF_H) {
        eprintln!("selftest note: test frame is {w}x{h}, expected {REF_W}x{REF_H}");
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// Score the template at one position, bypassing the probe early-reject, to
/// separate "template is wrong" from "probes are too strict".
fn diagnose_at(img: &RgbaImage, ax: usize, ay: usize) {
    let (fw, fh) = img.dimensions();
    let g = Geom::derive(fw, fh);
    let (nx, ny, nw, nh) = g.name;
    let raw = binarize(img, nx, ny, nw, nh);
    let Some((tmpl, _, _)) = trim(&raw) else {
        eprintln!("no ink in the name box");
        return;
    };
    let frame = binarize(img, 0, 0, fw, g.ui_floor.min(fh));
    if ax + tmpl.w > frame.w || ay + tmpl.h > frame.h {
        eprintln!("({ax}, {ay}) + template {}x{} falls outside the search area", tmpl.w, tmpl.h);
        return;
    }
    let mut agree = 0usize;
    let mut on_hit = 0usize;
    let mut on_total = 0usize;
    for ty in 0..tmpl.h {
        for tx in 0..tmpl.w {
            let f = frame.at(ax + tx, ay + ty);
            let t = tmpl.at(tx, ty);
            if f == t {
                agree += 1;
            }
            if t {
                on_total += 1;
                if f {
                    on_hit += 1;
                }
            }
        }
    }
    let total = (tmpl.w * tmpl.h) as f32;
    println!(
        "score at ({ax}, {ay}): overall {:.1}%  ink-recall {:.1}% ({on_hit}/{on_total})",
        agree as f32 / total * 100.0,
        on_hit as f32 / on_total as f32 * 100.0
    );
    let pr = probes(&tmpl);
    let bad = pr
        .iter()
        .filter(|&&(px, py, want)| frame.at(ax + px, ay + py) != want)
        .count();
    println!("probes failing here: {bad} of {}", pr.len());

    let side = side_ink(
        &frame,
        ax,
        ay,
        tmpl.w,
        tmpl.h,
        g.sprite_gap as usize,
        (g.sprite_gap * 4) as usize,
    );
    println!("side ink beside the tag: {side:.3}");
    let profile: Vec<String> = [2u32, 4, 8, 16, 24, 32, 48]
        .iter()
        .map(|&r| {
            format!(
                "{r}:{:.2}",
                side_ink(&frame, ax, ay, tmpl.w, tmpl.h, g.sprite_gap as usize, r as usize)
            )
        })
        .collect();
    println!("  by reach: {}", profile.join("  "));

    if std::env::args().any(|a| a == "--map") {
        println!("  ink map of the tag band ('#' = bright, '.' = dark):");
        let x_from = ax.saturating_sub(60);
        let x_to = (ax + tmpl.w + 60).min(frame.w);
        for y in ay..(ay + tmpl.h).min(frame.h) {
            let row: String = (x_from..x_to)
                .map(|x| if frame.at(x, y) { '#' } else { '.' })
                .collect();
            println!("  {row}");
        }
    }

    if let Some((x0, y0, w, h)) = g.sprite_box(fw, ax as u32, ay as u32, tmpl.w as u32) {
        match key_sprite(img, x0, y0, w, h) {
            None => println!("sprite box ({x0}, {y0}) {w}x{h}: keys out entirely"),
            Some(k) => println!(
                "sprite box ({x0}, {y0}) {w}x{h}: kept {} ({:.0}%)  colours {}  \
                 hue-dom {:.2}  sprite-score {:.3}  verdict {}",
                k.kept,
                k.kept_frac * 100.0,
                k.colours,
                k.hue_dom,
                sprite_score(&k, side),
                match judge(&k, side) {
                    Ok(()) => "plausible".to_string(),
                    Err(w) => format!("rejected — {w}"),
                }
            ),
        }
    } else {
        println!("no room for a sprite box above ({ax}, {ay})");
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.iter().any(|a| a == "--selftest") {
        std::process::exit(selftest());
    }

    let positional: Vec<&String> = argv.iter().filter(|a| !a.starts_with("--")).collect();
    let at = argv.iter().position(|a| a == "--at");
    // `--at X Y`: the two numbers are not input paths.
    let skip_tail = if at.is_some() { 2 } else { 0 };
    let paths: Vec<&String> = positional[..positional.len().saturating_sub(skip_tail)].to_vec();

    let frame_path: String = paths.first().map(|s| s.to_string()).unwrap_or_else(|| "captures/frame.png".into());
    let out_path: String = paths.get(1).map(|s| s.to_string()).unwrap_or_else(|| "ui/character.png".into());

    let img = match image::open(&frame_path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {frame_path}: {e}");
            std::process::exit(1);
        }
    };
    let (fw, fh) = img.dimensions();
    println!("frame   : {frame_path}  {fw}x{fh}");

    if let Some(i) = at {
        let (Some(ax), Some(ay)) = (
            argv.get(i + 1).and_then(|v| v.parse::<usize>().ok()),
            argv.get(i + 2).and_then(|v| v.parse::<usize>().ok()),
        ) else {
            eprintln!("--at needs two integers: --at X Y");
            std::process::exit(1);
        };
        diagnose_at(&img, ax, ay);
        return;
    }

    let found = match locate(&img, true) {
        Ok(f) => f,
        Err(e) => {
            match &e {
                Fail::Layout(m) => eprintln!("\n{m}"),
                Fail::NoTag { best } => {
                    eprintln!("\nNo name tag found. The character may be off-screen or hidden.");
                    if let Some((x, y, s)) = best {
                        eprintln!("Best near-miss: ({x}, {y}) at {:.1}%.", s * 100.0);
                    }
                    eprintln!("Fall back to the generic avatar and retry on a later frame.");
                }
                Fail::NotASprite(r) => {
                    eprintln!("\nFound the name, but nothing above it looks like a character:");
                    for (x, y, why) in r {
                        eprintln!("  ({x}, {y}): {why}");
                    }
                    eprintln!("The name was probably drawn in chat or a list, not under a sprite.");
                }
            }
            std::process::exit(e.code());
        }
    };

    for (x, y, why) in &found.rejected {
        println!("rejected: ({x}, {y}) — {why}");
    }
    for (x, y, text, sprite) in &found.runners_up {
        println!(
            "also ok : ({x}, {y}) text {:.1}% sprite-score {:.3}",
            text * 100.0,
            sprite
        );
    }

    let c = &found.best;
    let (tw, th) = c.keyed.img.dimensions();
    println!(
        "\ntag     : ({}, {}) at {:.1}%  sprite-score {:.3}",
        c.x,
        c.y,
        c.text * 100.0,
        c.sprite
    );

    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = c.keyed.img.save(&out_path) {
        eprintln!("could not write {out_path}: {e}");
        std::process::exit(1);
    }
    println!("kept    : {} px, trimmed to {tw}x{th}\nwrote   : {out_path}", c.keyed.kept);
    // One machine-readable line, for the self-test and for callers.
    println!(
        "result  : tag={},{} score={:.4} distinct={} sprite={tw}x{th} kept={}",
        c.x, c.y, c.text, found.distinct, c.keyed.kept
    );
}
