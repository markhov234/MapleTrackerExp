//! Reading numbers off the game's status bar.
//!
//! MapleStory Classic draws its UI numerals as a fixed bitmap font: the same
//! glyph is the same pixels every time, with no antialiasing of its own. So
//! this does template matching against a taught atlas rather than running a
//! general OCR engine — it is exact where Tesseract would be approximate, and
//! it costs microseconds instead of milliseconds.
//!
//! Two wrinkles the real client forces on us:
//!
//!   * **DPI upscaling.** A DPI-unaware client renders at its logical size and
//!     the compositor scales the result, so at 125% a glyph rasterises up to
//!     four different ways depending on its sub-pixel phase. The atlas
//!     therefore stores several forms per character and matching takes the
//!     best. Teaching more frames only makes it more accurate.
//!   * **Inverted fields.** Most text is light on a dark plate, but the level
//!     badge is dark digits on a bright orange one. Each region carries its
//!     own `invert` flag.
//!
//! Nothing here touches the game: it reads a pixel buffer and nothing else.

use serde::{Deserialize, Serialize};

/// An RGBA8 view over a frame, however it was obtained.
pub struct Frame<'a> {
    pub w: u32,
    pub h: u32,
    pub px: &'a [u8],
}

impl<'a> Frame<'a> {
    /// The colour at a pixel.
    #[inline]
    pub fn rgb(&self, x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * self.w + x) * 4) as usize;
        if i + 2 >= self.px.len() {
            return (0, 0, 0);
        }
        (self.px[i], self.px[i + 1], self.px[i + 2])
    }

    /// The grey level at a pixel. Public because [`crate::unstretch`] reads
    /// grey values directly: resampling has to happen before thresholding.
    #[inline]
    pub fn grey(&self, x: u32, y: u32) -> u8 {
        let i = ((y * self.w + x) * 4) as usize;
        if i + 2 >= self.px.len() {
            return 0;
        }
        let (r, g, b) = (self.px[i] as f32, self.px[i + 1] as f32, self.px[i + 2] as f32);
        (r * 0.299 + g * 0.587 + b * 0.114) as u8
    }
}

/// A binary glyph bitmap, trimmed to its own ink.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Glyph {
    pub w: usize,
    pub h: usize,
    /// Row-major '0'/'1' string. Compact in JSON and trivial to eyeball.
    pub bits: String,
}

impl Glyph {
    /// Whether this pixel is ink. Public so other readers can match against
    /// a taught shape without going through the atlas.
    #[inline]
    pub fn on_at(&self, x: usize, y: usize) -> bool {
        x < self.w && y < self.h && self.bits.as_bytes()[y * self.w + x] == b'1'
    }

    #[inline]
    fn on(&self, x: usize, y: usize) -> bool {
        self.bits.as_bytes()[y * self.w + x] == b'1'
    }

    /// Pixel agreement with another glyph, padding both to a common box.
    /// Padding rather than scaling: these are bitmap fonts, never resized.
    fn agreement(&self, other: &Glyph) -> f32 {
        let w = self.w.max(other.w);
        let h = self.h.max(other.h);
        let mut same = 0usize;
        for y in 0..h {
            for x in 0..w {
                let a = x < self.w && y < self.h && self.on(x, y);
                let b = x < other.w && y < other.h && other.on(x, y);
                if a == b {
                    same += 1;
                }
            }
        }
        same as f32 / (w * h) as f32
    }
}

/// One rectangle of the status bar holding one field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Roi {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub threshold: u8,
    /// True where the field is dark text on a bright plate (the level badge).
    #[serde(default)]
    pub invert: bool,
}

/// The chat log's geometry and its own font.
///
/// A separate atlas because the chat is a different typeface at a different
/// size from the status bar — sharing one map would have 'o' competing with
/// '0' across two fonts, which is how a reader starts inventing numbers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chat {
    /// Whether the tracker should read chat at all.
    ///
    /// The font can be taught long before the reading is trustworthy, and a
    /// half-taught chat feeds wrong kill counts into the session and then
    /// into the history, where they outlive the mistake. Off until the
    /// reading has been checked against real event lines.
    #[serde(default)]
    pub enabled: bool,
    /// The whole scrollback band, not one line: lines are found inside it by
    /// row projection, so the band keeps working when the box is resized or
    /// only half full.
    pub roi: Roi,
    /// Blank columns needed to split two glyphs.
    pub min_gap: usize,
    /// Blank columns that mean a space rather than a letter gap. Words have
    /// to survive into the text, because the classifier works on keywords.
    pub space_gap: usize,
    /// Blank rows needed to separate two lines.
    pub line_gap: usize,
    /// Separate marks a row needs before it counts as a line of text.
    ///
    /// The chat window is translucent, so the game shows through it: a
    /// bright object behind the box — the mouse cursor, a lantern — puts ink
    /// in every row it spans and welds the lines above and below it into one
    /// band. Pixel counts do not separate them, because a dim line of text
    /// and a bright cursor light up a similar number. What differs is how
    /// the ink is arranged: a line of text is dozens of separate strokes
    /// across the width, and the cursor is one blob.
    #[serde(default)]
    pub min_row_ink: usize,
    /// Choose each line's threshold from that line's own pixels.
    ///
    /// The chat window is translucent and the client fades older lines, so
    /// how bright the text is depends on which line it is and what scenery
    /// is behind it — a single threshold that reads the newest line cleanly
    /// turns the one above it into crumbs. `roi.threshold` stays as the
    /// floor, so a band holding no text does not have a threshold invented
    /// for its background noise.
    #[serde(default)]
    pub adaptive: bool,
    /// Agreement a chat glyph needs, where 0 means the status bar's floor.
    ///
    /// Lower than the status bar's, and it has to be. That floor guards ten
    /// digits whose misreading changes a number by an order of magnitude.
    /// This font has seventy-odd characters, so the same character at two
    /// sub-pixel phases differs by more than a wrong digit does — and a
    /// letter misread inside a sentence costs nothing, because the line is
    /// classified by keywords and every number it yields is still checked
    /// for unreadable glyphs beside it.
    #[serde(default)]
    pub min_match: f32,
    pub atlas: std::collections::BTreeMap<String, Vec<Glyph>>,
}


/// Everything needed to read this client's status bar, on any screen.
///
/// What is deliberately *not* here is where anything is. The bar moves when
/// the window is resized, when the UI is rescaled, and it was never in the
/// same place on somebody else's machine, so the reader finds it by its
/// shape and checks it by its arithmetic instead. What is left is the render
/// size, a grey level and a font — none of which depend on the monitor.
/// The lines the client floats over the game world, low and to the right.
///
/// This is where kills and mesos actually are. They are not in the chat
/// log, whatever one might expect — "You received EXP (+107)" and "You have
/// gained mesos (+95)" are drawn straight onto the scene, above the status
/// bar, and they fade out over a couple of seconds.
///
/// Two consequences shape everything here. There is no plate behind the
/// text, so the background is whatever the map looks like and no single
/// threshold can serve; and the fade means two lines of the same colour
/// arrive at different brightness — measured on one frame, 200 and 132.
/// Each row therefore gets a threshold chosen from its own pixels.
///
/// The region is given as fractions of the client's frame rather than
/// pixels. Nothing is calibrated to a monitor anywhere else in this reader
/// and this is no exception: the band is deliberately generous, and the
/// lines that matter are picked out by what they say, not by where they
/// sit. A line that is not about EXP or mesos simply matches no rule.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notices {
    /// Whether the tracker should read them at all.
    ///
    /// Off until the font has been checked against real lines. A
    /// half-taught reader feeds wrong kill counts into the session and then
    /// into the history, where they outlive the mistake.
    #[serde(default)]
    pub enabled: bool,
    /// The search area, as fractions of the client's frame: left, top,
    /// right, bottom.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// The lowest a row's threshold may go, so empty scenery cannot have a
    /// threshold invented for its noise.
    pub threshold: u8,
    pub min_gap: usize,
    pub space_gap: usize,
    pub line_gap: usize,
    pub min_row_ink: usize,
    pub min_match: f32,
    pub atlas: std::collections::BTreeMap<String, Vec<Glyph>>,
}

impl Notices {
    /// The search area in the pixels of a given client frame.
    pub fn band(&self, w: u32, h: u32) -> Roi {
        let px = |f: f32, n: u32| ((f.clamp(0.0, 1.0) * n as f32) as u32).min(n);
        let (x0, y0) = (px(self.x0, w), px(self.y0, h));
        let (x1, y1) = (px(self.x1, w).max(x0), px(self.y1, h).max(y0));
        Roi {
            name: "notices".into(),
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
            threshold: self.threshold,
            invert: false,
        }
    }

    pub fn rows(&self) -> Rows<'_> {
        Rows {
            atlas: &self.atlas,
            space_gap: self.space_gap,
            line_gap: self.line_gap,
            min_row_ink: self.min_row_ink,
            // Always. There is no plate behind this text.
            adaptive: true,
            min_match: self.min_match,
        }
    }
}

/// Read the floating lines of a client frame.
pub fn read_notices(frame: &Frame, cfg: &Notices) -> Vec<String> {
    read_rows(frame, &cfg.band(frame.w, frame.h), &cfg.rows())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub version: u32,
    /// The client's own render size, in its own pixels.
    ///
    /// Recording the size rather than the scale is what makes this portable:
    /// the scale is whatever frame arrives divided by this, so one profile
    /// reads a window of any size on any monitor. It is a first guess rather
    /// than a rule — a client told to render at another resolution is doing
    /// something else, and the reader tries the alternatives behind it.
    pub render_w: u32,
    pub render_h: u32,
    /// Grey level separating text from plate, on the client's own grid.
    ///
    /// Lower than it would be on the captured image, and necessarily so.
    /// Resampling averages pixels together, so a serif one client-pixel wide
    /// arrives about half as bright as the stroke it hangs off; a cut-off
    /// chosen for the captured pixels shears those serifs away, and '[' and
    /// ']' become the same bare column.
    pub threshold: u8,
    /// Blank columns needed to split two glyphs.
    pub min_gap: usize,
    /// character -> the forms it has been seen in.
    ///
    /// On the client's grid this is one form per character: a bitmap font
    /// read on the grid it was drawn for is the same pixels every time. The
    /// several-forms-per-character shape is left in place because the chat
    /// font is not yet read this way.
    pub atlas: std::collections::BTreeMap<String, Vec<Glyph>>,
    /// Absent until the chat log has been calibrated; the tracker simply
    /// does not read chat in that case.
    #[serde(default)]
    pub chat: Option<Chat>,
    /// Absent until the floating lines have been taught. While it is absent
    /// the kill and meso counts stay blank, which is the honest answer:
    /// "nothing killed" and "not being counted" must not look alike.
    #[serde(default)]
    pub notices: Option<Notices>,
    /// Absent until the minimap's title has been taught. While it is absent
    /// the overlay keeps its own colours, which is the right default: a
    /// theme that changes for the wrong reason is worse than one that does
    /// not change.
    #[serde(default)]
    pub minimap: Option<crate::region::Minimap>,
    /// Absent until the meso plate has been seen on a real frame. The
    /// session's meso figures come from reading the inventory's own total
    /// rather than adding up the floating lines.
    #[serde(default)]
    pub mesos: Option<crate::mesos::Mesos>,
}

impl Profile {
    pub fn glyph_count(&self) -> usize {
        self.atlas.values().map(|v| v.len()).sum()
    }

    /// How much a frame of this size has been stretched by.
    pub fn scale(&self, frame_w: u32, frame_h: u32) -> crate::unstretch::Scale {
        crate::unstretch::Scale {
            sx: frame_w as f64 / self.render_w as f64,
            sy: frame_h as f64 / self.render_h as f64,
        }
    }

    /// A region covering a resampled plate, at this profile's grey level.
    pub fn over(&self, name: &str, w: u32, h: u32) -> Roi {
        Roi {
            name: name.into(),
            x: 0,
            y: 0,
            w,
            h,
            threshold: self.threshold,
            invert: false,
        }
    }
}

/// What a region read as.
#[derive(Debug, Clone)]
pub struct Reading {
    pub text: String,
    /// Glyphs no taught form matched. Any at all makes the reading unusable.
    pub unknown: usize,
    /// Agreement of the worst glyph in the region.
    pub confidence: f32,
    /// Gap between best and second-best for the worst glyph. The number to
    /// watch: a high score with a thin margin means two characters look alike
    /// and the field will flip under load.
    pub margin: f32,
    pub glyphs: usize,
}

/// A segmented, trimmed glyph plus where it sat in the region.
pub struct Cut {
    pub dx: usize,
    pub glyph: Glyph,
}

/// Binarize a region into a column-major-friendly mask.
fn mask(frame: &Frame, roi: &Roi) -> Vec<bool> {
    let mut out = Vec::with_capacity((roi.w * roi.h) as usize);
    for y in 0..roi.h {
        for x in 0..roi.w {
            let (px, py) = (roi.x + x, roi.y + y);
            let lit = if px >= frame.w || py >= frame.h {
                false
            } else {
                let l = frame.grey(px, py);
                if roi.invert { l < roi.threshold } else { l >= roi.threshold }
            };
            out.push(lit);
        }
    }
    out
}

/// Split a region into glyphs by column projection, then trim each vertically.
pub fn segment(frame: &Frame, roi: &Roi, min_gap: usize) -> Vec<Cut> {
    let m = mask(frame, roi);
    let (w, h) = (roi.w as usize, roi.h as usize);
    let col_has_ink = |x: usize| (0..h).any(|y| m[y * w + x]);

    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut blanks = 0usize;
    for x in 0..=w {
        let on = x < w && col_has_ink(x);
        if on {
            if start.is_none() {
                start = Some(x);
            }
            blanks = 0;
        } else if let Some(s) = start {
            blanks += 1;
            if blanks >= min_gap || x == w {
                runs.push((s, x - blanks));
                start = None;
                blanks = 0;
            }
        }
    }

    let mut cuts = Vec::new();
    for (x0, x1) in runs {
        let mut y0 = usize::MAX;
        let mut y1 = 0usize;
        for y in 0..h {
            for x in x0..=x1 {
                if m[y * w + x] {
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if y0 == usize::MAX {
            continue;
        }
        let (gw, gh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut bits = String::with_capacity(gw * gh);
        for y in y0..=y1 {
            for x in x0..=x1 {
                bits.push(if m[y * w + x] { '1' } else { '0' });
            }
        }
        cuts.push(Cut { dx: x0, glyph: Glyph { w: gw, h: gh, bits } });
    }
    cuts
}

/// A glyph scoring below this against every known form is reported as unknown
/// rather than as the nearest match.
///
/// Without a floor, a digit the atlas has never seen silently becomes
/// whichever taught digit it resembles most — a wrong number carrying a
/// confident-looking score. An unreadable field is recoverable; a plausible
/// wrong one is not.
pub const MIN_MATCH: f32 = 0.90;

/// A glyph must also beat its runner-up by this much.
///
/// A score floor alone is not enough. 8 and 9 differ by a handful of pixels
/// in this font, so an unseen sub-pixel form of a 9 can match a taught 8 at
/// over 90% — a wrong digit wearing a confident score, which is exactly the
/// failure the floor was meant to prevent. Clean readings leave margins of
/// 8% and up, so anything this tight is a coin toss and must be refused.
pub const MIN_MARGIN: f32 = 0.03;

/// Best character for a cut glyph, with its score and margin over the runner-up.
fn classify(g: &Glyph, atlas: &std::collections::BTreeMap<String, Vec<Glyph>>) -> (char, f32, f32) {
    let mut best = ('?', 0.0f32);
    let mut second = 0.0f32;
    for (ch, forms) in atlas {
        let c = ch.chars().next().unwrap_or('?');
        for form in forms {
            let s = g.agreement(form);
            if s > best.1 {
                // Only a different character counts as the runner-up; another
                // form of the same character is not a competing reading.
                if best.0 != c {
                    second = second.max(best.1);
                }
                best = (c, s);
            } else if c != best.0 {
                second = second.max(s);
            }
        }
    }
    let margin = best.1 - second;
    // A form that matches pixel for pixel was taught from exactly these
    // pixels, so nothing can beat it and the runner-up is irrelevant. This
    // matters for prose: the chat alphabet has seventy-odd characters rather
    // than ten, and small letters routinely sit within three percent of each
    // other, which would refuse glyphs the atlas knows perfectly well.
    if best.1 >= 1.0 {
        return (best.0, best.1, margin);
    }
    if best.1 < MIN_MATCH || margin < MIN_MARGIN {
        return ('?', best.1, margin);
    }
    (best.0, best.1, margin)
}

/// How many characters a single glyph may be considered as.
///
/// Three is generous for a bitmap font: a thin margin means two characters
/// are close, and a third is already well behind.
const TOP_K: usize = 3;

/// Ceiling on strings tried when reconciling.
///
/// The widened pass offers an unknown glyph every character in the atlas,
/// which is nineteen: three unknowns is under seven thousand strings, each
/// costing two parses and a division. Four is a hundred and thirty thousand
/// — and a field with four unreadable glyphs is not an ambiguous reading
/// anyway, it is a region pointed at the wrong pixels.
const MAX_COMBOS: usize = 8192;

/// The characters this glyph could be, best first, with their scores.
/// Public so the calibration tools can show why a glyph was refused.
pub fn candidates(
    g: &Glyph,
    atlas: &std::collections::BTreeMap<String, Vec<Glyph>>,
) -> Vec<(char, f32)> {
    let mut best: Vec<(char, f32)> = Vec::new();
    for (ch, forms) in atlas {
        let c = ch.chars().next().unwrap_or('?');
        let s = forms.iter().map(|f| g.agreement(f)).fold(0.0f32, f32::max);
        if s >= MIN_MATCH {
            best.push((c, s));
        }
    }
    best.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    best.truncate(TOP_K);
    best
}

/// Every character the atlas knows, scored, with no floor — best first.
///
/// Used only once the floor has already failed to produce a reading that
/// agrees with the percentage. At that point the question is no longer "which
/// taught form is this" but "which character must it be for the bar to be
/// telling the truth", and a score below the floor is still a ranking.
///
/// Public so the calibration tools can show the whole ranking.
///
/// Nothing is dropped from the ranking. Punctuation has to compete, because
/// the thousands comma and the decimal point are blobs two or three pixels
/// across and are the first glyphs a new sub-pixel phase makes
/// unrecognisable — and agreement over a padded box punishes a size
/// difference hardest when the glyph is tiny, so a two-pixel decimal point
/// scores below sixty-seven percent against a three-pixel taught one and
/// does not place in any shortlist. It still has to be the decimal point,
/// because no other character leaves the field saying something the
/// percentage agrees with. That is the whole point of reconciling: the
/// arithmetic identifies what the pixels cannot.
pub fn all_scores(
    g: &Glyph,
    atlas: &std::collections::BTreeMap<String, Vec<Glyph>>,
) -> Vec<(char, f32)> {
    let mut v: Vec<(char, f32)> = atlas
        .iter()
        .map(|(ch, forms)| {
            let c = ch.chars().next().unwrap_or('?');
            (c, forms.iter().map(|f| g.agreement(f)).fold(0.0f32, f32::max))
        })
        .collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// An EXP reading, with whatever the field could be made to say.
#[derive(Debug, Clone)]
pub struct ExpReading {
    pub reading: Reading,
    pub absolute: Option<u64>,
    pub percent: Option<f64>,
    /// The level implied, when it could be worked out.
    pub level: Option<usize>,
    /// True when the percentage settled a glyph the pixels could not.
    pub reconciled: bool,
}

/// Does this number agree with the percentage printed beside it?
///
/// The bar states the same fact twice, and that redundancy is the only check
/// on a reading that cannot be fooled by confident-looking pixels. Where the
/// level is already known the test is exact: that number, out of that
/// level's requirement, has to round to that percentage. Where it is not,
/// the pair has to identify a level on its own, and `infer_level` refuses
/// when more than one would do.
///
/// Returns the level the reading implies, or `None` if no level makes it
/// true — which means the reading is wrong, whatever it looked like.
fn agrees(a: u64, p: f64, known_level: Option<usize>, table: &[u64]) -> Option<usize> {
    match known_level {
        Some(l) if l < table.len() && table[l] > 0 => {
            let shown = ((a as f64 / table[l] as f64) * 10_000.0).round() / 100.0;
            // One level up is the ordinary case for a number that does not
            // fit this level: the bar has rolled over and the session has
            // not been told yet.
            if (shown - p).abs() <= 0.005 {
                return Some(l);
            }
            let next = l + 1;
            if next < table.len() && table[next] > 0 {
                let shown = ((a as f64 / table[next] as f64) * 10_000.0).round() / 100.0;
                if (shown - p).abs() <= 0.005 {
                    return Some(next);
                }
            }
            None
        }
        _ => crate::expcurve::infer_level(a, p, table),
    }
}

/// Read the EXP field, letting the percentage settle glyphs the pixels cannot.
///
/// The status bar states the same fact twice — an absolute number and the
/// percentage of the level it represents — and either one predicts the other.
/// That redundancy is worth more than another thousand taught glyph forms,
/// because it does not run out: every reading carries its own check.
///
/// Pixels first. A glyph whose best match clears the score floor and beats
/// its runner-up is simply that character. Only the ambiguous ones are
/// enumerated, and only an assignment whose absolute value rounds to the
/// displayed percentage is accepted. Where several do, the pixels break the
/// tie — they were close, and every surviving option already agrees with the
/// percentage, so the error is bounded by the percentage's own resolution
/// (a hundredth of a level: tens of EXP out of tens of thousands) rather
/// than being the order-of-magnitude error a bare guess risks.
///
/// Where nothing is consistent, this reports the unreadable field as before.
/// Reconciling must never be able to invent a number the bar does not show.
pub fn read_exp(
    frame: &Frame,
    roi: &Roi,
    profile: &Profile,
    known_level: Option<usize>,
    table: &[u64],
) -> ExpReading {
    let plain = read(frame, roi, profile);
    let unresolved = ExpReading {
        absolute: exp_absolute(&plain.text),
        percent: percent(&plain.text),
        level: None,
        reading: plain.clone(),
        reconciled: false,
    };
    // Every glyph recognised. That is not the same as the field being read
    // correctly, and the difference used to be academic: before the font was
    // taught on the client's own grid a doubtful glyph was common, so most
    // readings went down the reconciling path below and were checked against
    // the percentage there. Now every glyph matches exactly and nothing ever
    // reaches that path, which quietly removed the only check there was.
    //
    // What that cost, observed live: one frame during a redraw of the strip
    // read as a confident, well-formed, entirely imaginary number, and the
    // session recorded a gain of two billion EXP.
    //
    // So the same test applies here. The bar states its number twice, and a
    // reading that fails its own arithmetic is refused however clean its
    // pixels looked.
    if plain.unknown == 0 || plain.glyphs == 0 {
        return match (unresolved.absolute, unresolved.percent) {
            (Some(a), Some(p)) => match agrees(a, p, known_level, table) {
                Some(level) => ExpReading { level: Some(level), ..unresolved },
                // Well-formed and impossible. Reported as unreadable, which
                // is recoverable; reported as a number it would be a wrong
                // total, and totals are kept.
                None => ExpReading { absolute: None, level: None, ..unresolved },
            },
            _ => ExpReading { level: None, ..unresolved },
        };
    }

    let cuts = segment(frame, roi, profile.min_gap);

    // Two passes over the same search. The first trusts the score floor and
    // only chooses between characters the atlas already recognises. If that
    // produces nothing the bar agrees with, the floor itself was the problem:
    // the true digit is a form never taught, scoring below it, so the second
    // pass drops the floor and lets every digit compete. The arithmetic is
    // doing the identifying either way — the scores only rank what is left.
    for widen in [false, true] {
        let mut options: Vec<Vec<(char, f32)>> = Vec::with_capacity(cuts.len());
        let mut combos: usize = 1;
        let mut ok = true;
        for c in &cuts {
            let (ch, score, _) = classify(&c.glyph, &profile.atlas);
            if ch != '?' {
                options.push(vec![(ch, score)]);
                continue;
            }
            let alts = if widen {
                all_scores(&c.glyph, &profile.atlas)
            } else {
                candidates(&c.glyph, &profile.atlas)
            };
            if alts.is_empty() {
                ok = false;
                break;
            }
            combos = combos.saturating_mul(alts.len());
            options.push(alts);
        }
        if !ok || combos > MAX_COMBOS {
            continue;
        }

        // Walk the cartesian product by counting in mixed radix.
        // (total score — what ranks the assignments, worst glyph — what the
        // confidence reports). Keeping both matters: ranking by the total and
        // reporting the weakest link are different questions, and comparing
        // one against the other picks the wrong number.
        let mut best: Option<(f32, f32, u64, f64, usize)> = None;
        for mut idx in 0..combos {
            let mut text = String::with_capacity(options.len());
            let mut worst = 1.0f32;
            let mut score_sum = 0.0f32;
            for opts in &options {
                let (ch, s) = opts[idx % opts.len()];
                idx /= opts.len();
                text.push(ch);
                worst = worst.min(s);
                score_sum += s;
            }
            let (Some(a), Some(p)) = (exp_absolute(&text), percent(&text)) else { continue };
            let level = match known_level {
                // The level is already known, so the check is the stronger
                // one: does this number round to the percentage on the bar?
                Some(l) if l < table.len() && table[l] > 0 => {
                    let shown = ((a as f64 / table[l] as f64) * 10_000.0).round() / 100.0;
                    if (shown - p).abs() > 0.005 {
                        continue;
                    }
                    Some(l)
                }
                // No level yet: the pair has to identify one on its own, and
                // `infer_level` already refuses when it is ambiguous.
                _ => match crate::expcurve::infer_level(a, p, table) {
                    Some(l) => Some(l),
                    None => continue,
                },
            };
            if best.map(|(total, ..)| score_sum > total).unwrap_or(true) {
                best = Some((score_sum, worst, a, p, level.unwrap_or(0)));
            }
        }

        if let Some((_, worst, absolute, pct, level)) = best {
            return ExpReading {
                reading: Reading {
                    text: format!("{absolute}[{pct:.2}%]"),
                    unknown: 0,
                    confidence: worst,
                    margin: plain.margin,
                    glyphs: plain.glyphs,
                },
                absolute: Some(absolute),
                percent: Some(pct),
                level: (level > 0).then_some(level),
                reconciled: true,
            };
        }
    }
    unresolved
}

/// Find the EXP readout anywhere in a region, by what it says.
///
/// Calibrated coordinates are a promise the client does not keep. The bar
/// moves when the window is resized, when the UI is rescaled, when the
/// client is run at another resolution — and on somebody else's machine it
/// was never in the same place to begin with. A tracker that has to be
/// re-taught its coordinates every time is a tracker that is broken most of
/// the time.
///
/// So the field is identified by its own structure rather than its address.
/// The EXP readout is the only text on screen shaped like
/// `<digits>[<digits>.<digits>%]`, and it is the only one that then passes
/// the arithmetic: the absolute number divided by the cost of some level
/// has to come out at the percentage printed beside it. A stray number that
/// merely looks similar fails that check. Nothing else on the screen can
/// pass it by accident.
///
/// Returns the region the readout occupies, ready to be read every frame
/// without searching again.
pub fn find_exp_field(frame: &Frame, profile: &Profile, table: &[u64], band: &Roi) -> Option<Roi> {
    // Rows cannot be found by projection here: outside the status bar the
    // frame is a game, and at any sensible threshold every row of it holds
    // ink. So the search slides a window of one text line's height down the
    // region and asks each one a cheap question — is there a percent sign
    // in it — before asking an expensive one.
    let tall = profile
        .atlas
        .values()
        .flatten()
        .map(|g| g.h)
        .max()
        .unwrap_or(14);
    let window_h = (tall + 6) as u32;
    let percents: Vec<&Glyph> = profile.atlas.get("%").map(|v| v.iter().collect()).unwrap_or_default();
    if percents.is_empty() {
        return None;
    }

    let mut y = band.y;
    while y + window_h <= band.y + band.h {
        let row = Roi { y, h: window_h, ..band.clone() };
        let cuts = segment(frame, &row, profile.min_gap);
        for (i, cut) in cuts.iter().enumerate() {
            // A percent sign is wide for its height and nothing else in a
            // status bar looks like one, which is what makes it the anchor.
            let hit = percents
                .iter()
                .any(|f| f.w.abs_diff(cut.glyph.w) <= 1 && cut.glyph.agreement(f) >= MIN_MATCH);
            if !hit {
                continue;
            }
            if let Some(roi) = field_around(frame, profile, table, &row, &cuts, i) {
                return Some(roi);
            }
        }
        y += 2;
    }
    None
}

/// Find the EXP readout in a captured frame, whatever size the window is.
///
/// This is the whole reading problem in one call: undo the window's
/// stretching, look for the readout's shape, and check it by its own
/// arithmetic. It returns the resampled plate it was found on along with
/// the region inside that plate, so the caller can read it, and the scaling
/// that worked, so the caller can try that one first next time.
///
/// `grids` are the scalings to try, likeliest first. An empty slice means
/// the profile's own render size followed by every other size this client
/// is known to use.
///
/// The bottom fifth of the window is searched before the rest: it is where
/// this client has always put the bar, and it is a fifth of the work.
pub fn locate(
    frame: &Frame,
    profile: &Profile,
    table: &[u64],
    grids: &[crate::unstretch::Scale],
) -> Option<(crate::unstretch::Plate, Roi, crate::unstretch::Scale)> {
    let mut tried: Vec<crate::unstretch::Scale> = grids.to_vec();
    if tried.is_empty() {
        tried.push(profile.scale(frame.w, frame.h));
        for s in crate::unstretch::candidates(frame.w, frame.h) {
            if !tried.contains(&s) {
                tried.push(s);
            }
        }
    }
    // Cheap bands for every grid before any expensive one for a single
    // grid. The status bar is at the bottom of the client, so the bottom
    // fifth answers this nearly always — and it is a fifth of the pixels.
    //
    // Interleaving the two (bottom fifth, then whole upper frame, then the
    // next grid's bottom fifth) meant a frame with no status bar in it paid
    // for a full-frame resample and glyph sweep per candidate grid before
    // finishing. Measured on a blank 2560x1440 frame, which is the cheapest
    // possible failure, that was about 200ms; a desktop window full of text
    // is worse, because every percent-sign-shaped smudge gets chased. This
    // ordering is the same search with the same result, done in the order
    // that fails fast.
    for pass in 0..2 {
        for scale in &tried {
            let (nw, nh) = scale.native(frame.w, frame.h);
            let (y, h) = if pass == 0 { (nh * 4 / 5, nh / 5) } else { (0, nh * 4 / 5) };
            let plate = crate::unstretch::plate(frame, *scale, 0, y, nw, h);
            if plate.w == 0 || plate.h == 0 {
                continue;
            }
            let band = profile.over("search", plate.w, plate.h);
            if let Some(roi) = find_exp_field(&plate.frame(), profile, table, &band) {
                return Some((plate, roi, *scale));
            }
        }
    }
    None
}

/// Grow a region outwards from a percent sign and see whether it is an EXP
/// readout.
///
/// The shape being looked for is `<digits>[<digits>.<digits>%]`: the digits
/// before the bracket are the EXP, the ones inside it are the same figure as
/// a share of the level. Everything here is a guess until the arithmetic
/// agrees, which is the only check that cannot be passed by coincidence.
fn field_around(
    frame: &Frame,
    profile: &Profile,
    table: &[u64],
    row: &Roi,
    cuts: &[Cut],
    percent_at: usize,
) -> Option<Roi> {
    let ch = |i: usize| classify(&cuts[i].glyph, &profile.atlas).0;

    // Back from the percent sign to the opening bracket, which must have a
    // decimal point somewhere after it.
    let mut open = percent_at;
    let mut saw_point = false;
    while open > 0 {
        open -= 1;
        match ch(open) {
            '[' => break,
            '.' => saw_point = true,
            c if c.is_ascii_digit() => {}
            // Anything else and this is not a bracketed percentage.
            _ => return None,
        }
        if percent_at - open > 8 {
            return None;
        }
    }
    if !saw_point || ch(open) != '[' {
        return None;
    }

    // And back again over the absolute number.
    let mut start = open;
    while start > 0 {
        let c = ch(start - 1);
        if c.is_ascii_digit() || c == ',' {
            start -= 1;
        } else {
            break;
        }
    }
    if start == open {
        return None;
    }

    let end = match cuts.get(percent_at + 1).map(|c| classify(&c.glyph, &profile.atlas).0) {
        Some(']') => percent_at + 1,
        _ => percent_at,
    };
    let x0 = cuts[start].dx;
    let x1 = cuts[end].dx + cuts[end].glyph.w;
    let found = Roi {
        name: "exp".into(),
        x: row.x + x0 as u32,
        y: row.y,
        w: (x1 - x0) as u32 + 2,
        h: row.h,
        threshold: row.threshold,
        invert: row.invert,
    };
    let exp = read_exp(frame, &found, profile, None, table);
    (exp.level.is_some() && exp.absolute.is_some()).then_some(found)
}

/// Read one region.
pub fn read(frame: &Frame, roi: &Roi, profile: &Profile) -> Reading {
    let cuts = segment(frame, roi, profile.min_gap);
    classify_cuts(&cuts, &profile.atlas, None)
}

/// Turn segmented glyphs into text against a given atlas.
///
/// `space_gap` is what separates the status bar from prose: a field is one
/// run of characters, while a sentence needs its word boundaries back or the
/// keyword classifier has nothing to match. Where it is given, a horizontal
/// gap at least that wide becomes a space.
pub fn classify_cuts(
    cuts: &[Cut],
    atlas: &std::collections::BTreeMap<String, Vec<Glyph>>,
    space_gap: Option<usize>,
) -> Reading {
    if cuts.is_empty() {
        return Reading { text: String::new(), unknown: 0, confidence: 0.0, margin: 0.0, glyphs: 0 };
    }
    let mut text = String::new();
    let mut confidence = 1.0f32;
    let mut margin = 1.0f32;
    let mut unknown = 0usize;
    let mut prev_end: Option<usize> = None;
    for c in cuts {
        if let (Some(gap), Some(end)) = (space_gap, prev_end) {
            if c.dx.saturating_sub(end) >= gap {
                text.push(' ');
            }
        }
        let (ch, score, m) = classify(&c.glyph, atlas);
        if ch == '?' {
            unknown += 1;
        }
        text.push(ch);
        confidence = confidence.min(score);
        margin = margin.min(m);
        prev_end = Some(c.dx + c.glyph.w);
    }
    Reading { text, unknown, confidence, margin, glyphs: cuts.len() }
}

/// The rows of a region that hold ink, grouped into lines.
///
/// Row projection rather than a fixed pitch: the chat box is resizable, can
/// be half empty, and scrolls by whole lines, so where the text actually is
/// beats where it ought to be. Returns (y, height) pairs relative to the
/// region.
pub fn line_bands(frame: &Frame, roi: &Roi, line_gap: usize, min_row_ink: usize) -> Vec<(u32, u32)> {
    let m = mask(frame, roi);
    let (w, h) = (roi.w as usize, roi.h as usize);
    if h == 0 || w == 0 {
        return Vec::new();
    }

    // Separate marks in a row: one per letter stroke for text, one for a
    // bright object showing through from the game behind.
    let runs_in = |y: usize| {
        let mut runs = 0usize;
        let mut prev = false;
        for x in 0..w {
            let on = m[y * w + x];
            if on && !prev {
                runs += 1;
            }
            prev = on;
        }
        runs
    };
    let any_ink = |y: usize| (0..w).any(|x| m[y * w + x]);

    let floor = min_row_ink.max(1);
    let core: Vec<bool> = (0..h).map(|y| runs_in(y) >= floor).collect();

    // Group the text rows, allowing a short break inside one line — the gap
    // between a lowercase x-height and the dots of an i or the tail of a y.
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut blanks = 0usize;
    for y in 0..=h {
        if y < h && core[y] {
            if start.is_none() {
                start = Some(y);
            }
            blanks = 0;
        } else if let Some(s) = start {
            blanks += 1;
            if blanks >= line_gap.max(1) || y == h {
                bands.push((s, y - blanks));
                start = None;
                blanks = 0;
            }
        }
    }

    // The first and last rows of a line hold only the tall letters, so they
    // have too few marks to be core rows — and cutting them off would change
    // the shape of every capital in the line. Grow each band back over rows
    // that have ink, but only a little, or a cursor underneath the line
    // would drag it into the next one.
    const GROW: usize = 4;
    let mut out = Vec::with_capacity(bands.len());
    for (i, &(s, e)) in bands.iter().enumerate() {
        let floor_y = bands.get(i.wrapping_sub(1)).map(|&(_, pe)| pe + 1).unwrap_or(0);
        let ceil_y = bands.get(i + 1).map(|&(ns, _)| ns).unwrap_or(h);
        let mut s2 = s;
        while s2 > floor_y && s - s2 < GROW && any_ink(s2 - 1) {
            s2 -= 1;
        }
        let mut e2 = e;
        while e2 + 1 < ceil_y && e2 - e < GROW && any_ink(e2 + 1) {
            e2 += 1;
        }
        out.push((s2 as u32, (e2 - s2 + 1) as u32));
    }
    out
}

/// Every line of chat currently in the band, top to bottom.
///
/// Lines whose glyphs the atlas does not know come back with '?' in them
/// rather than being dropped here; the classifier refuses them, which keeps
/// the decision about what is trustworthy in one place.
/// How a band of prose is turned into lines of text.
///
/// Shared by the chat box and by the lines the client floats over the game
/// world, because the two problems are the same one: find the rows that
/// hold text, pick a threshold for each row on its own, and read it against
/// a font that is not the status bar's.
pub struct Rows<'a> {
    pub atlas: &'a std::collections::BTreeMap<String, Vec<Glyph>>,
    pub space_gap: usize,
    pub line_gap: usize,
    pub min_row_ink: usize,
    pub adaptive: bool,
    pub min_match: f32,
}

/// Read every line of prose in a band.
pub fn read_rows(frame: &Frame, band: &Roi, o: &Rows) -> Vec<String> {
    line_bands(frame, band, o.line_gap, o.min_row_ink)
        .into_iter()
        .map(|(dy, h)| {
            let line = line_roi(frame, band, dy, h, o.adaptive);
            read_line(
                &mask(frame, &line),
                line.w as usize,
                h as usize,
                o.atlas,
                o.space_gap,
                o.min_match,
            )
        })
        .filter(|t| !t.trim().is_empty())
        .collect()
}

pub fn read_chat(frame: &Frame, chat: &Chat) -> Vec<String> {
    read_rows(
        frame,
        &chat.roi,
        &Rows {
            atlas: &chat.atlas,
            space_gap: chat.space_gap,
            line_gap: chat.line_gap,
            min_row_ink: chat.min_row_ink,
            adaptive: chat.adaptive,
            min_match: chat.min_match,
        },
    )
}

/// The binarized pixels of a region, row-major. Exposed for the calibration
/// tools, which read a line the same way the tracker does.
pub fn line_mask(frame: &Frame, roi: &Roi) -> Vec<bool> {
    mask(frame, roi)
}

/// The box a chat glyph is resampled into before it is compared.
const NORM_ROWS: usize = 20;
const NORM_COLS: usize = 14;

/// A glyph resampled into a fixed box, as per-cell coverage.
///
/// The compositor scales the whole frame, so a character's pixels depend on
/// where it sits — and chat lines are 31 and a quarter pixels apart, so the
/// same character is rasterised a different way on every line. A '2' that
/// is nineteen rows tall with gaps in its diagonal on one line is
/// seventeen solid rows on the next, which as a pixel comparison is a
/// seventy-three percent match: not a near miss, a different character.
/// The same happens across a line, a column at a time.
///
/// Teaching around it would mean every character in every vertical phase
/// and every horizontal one on top — hundreds of forms per character.
/// Resampling into a common box asks the question that actually matters:
/// is this the same shape.
///
/// Width is not lost by this, because only forms within a pixel of the
/// window's own width are ever compared against it. The box answers "same
/// shape", the width gate answers "same size", and an 'i' is still not an
/// 'm'.
fn scaled_box(g: &Glyph) -> Vec<f32> {
    let mut out = vec![0.0f32; NORM_ROWS * NORM_COLS];
    if g.h == 0 || g.w == 0 {
        return out;
    }
    for r in 0..NORM_ROWS {
        let y0 = r as f32 * g.h as f32 / NORM_ROWS as f32;
        let y1 = (r + 1) as f32 * g.h as f32 / NORM_ROWS as f32;
        for c in 0..NORM_COLS {
            let x0 = c as f32 * g.w as f32 / NORM_COLS as f32;
            let x1 = (c + 1) as f32 * g.w as f32 / NORM_COLS as f32;
            let (mut acc, mut wsum) = (0.0f32, 0.0f32);
            let mut y = y0.floor() as usize;
            while (y as f32) < y1 && y < g.h {
                let wy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
                let mut x = x0.floor() as usize;
                while (x as f32) < x1 && x < g.w {
                    let wx = ((x + 1) as f32).min(x1) - (x as f32).max(x0);
                    let wgt = wy.max(0.0) * wx.max(0.0);
                    if g.on(x, y) {
                        acc += wgt;
                    }
                    wsum += wgt;
                    x += 1;
                }
                y += 1;
            }
            out[r * NORM_COLS + c] = if wsum > 0.0 { acc / wsum } else { 0.0 };
        }
    }
    out
}

/// Agreement between two resampled glyphs.
///
/// Only meaningful between glyphs of a similar size: resampling throws
/// absolute size away, so a full stop and a lowercase 'l' both become a
/// solid block. The callers gate on width and height before asking.
fn agree_scaled(a: &[f32], b: &[f32]) -> f32 {
    let diff: f32 = a.iter().zip(b).map(|(p, q)| (p - q).abs()).sum();
    1.0 - diff / (NORM_ROWS * NORM_COLS) as f32
}

/// The threshold that best splits a region into two groups, by Otsu's
/// method: the value minimising the variance within each side of the split.
///
/// Used per line of chat, where "how bright is the text" has a different
/// answer every line and no fixed answer at all.
pub fn otsu(frame: &Frame, roi: &Roi) -> u8 {
    let mut hist = [0u32; 256];
    let mut total = 0u32;
    for y in roi.y..(roi.y + roi.h).min(frame.h) {
        for x in roi.x..(roi.x + roi.w).min(frame.w) {
            hist[frame.grey(x, y) as usize] += 1;
            total += 1;
        }
    }
    if total == 0 {
        return roi.threshold;
    }
    let sum: f64 = (0..256).map(|i| i as f64 * hist[i] as f64).sum();
    let (mut back, mut back_sum, mut best, mut best_var) = (0u32, 0f64, roi.threshold, -1f64);
    for t in 0..256u32 {
        back += hist[t as usize];
        if back == 0 {
            continue;
        }
        let fore = total - back;
        if fore == 0 {
            break;
        }
        back_sum += t as f64 * hist[t as usize] as f64;
        let mb = back_sum / back as f64;
        let mf = (sum - back_sum) / fore as f64;
        let var = back as f64 * fore as f64 * (mb - mf) * (mb - mf);
        if var > best_var {
            best_var = var;
            best = t as u8;
        }
    }
    best
}

/// One line of a prose band, with the threshold it should be read at.
pub fn line_roi(frame: &Frame, band: &Roi, dy: u32, h: u32, adaptive: bool) -> Roi {
    let line = Roi { y: band.y + dy, h, ..band.clone() };
    if !adaptive {
        return line;
    }
    // Never below the configured floor: a band of empty chat, or of plain
    // scenery, would otherwise have a threshold chosen for its background
    // noise and come back full of imaginary text.
    let t = otsu(frame, &line).max(band.threshold);
    Roi { threshold: t, ..line }
}

/// Ink columns of a line mask.
fn ink_columns(m: &[bool], w: usize, h: usize) -> Vec<bool> {
    (0..w).map(|x| (0..h).any(|y| m[y * w + x])).collect()
}

/// The slice of a line between two columns, trimmed to its ink the way a
/// taught form is.
fn window(m: &[bool], w: usize, h: usize, x0: usize, gw: usize) -> Option<Glyph> {
    let (mut y0, mut y1) = (usize::MAX, 0usize);
    for y in 0..h {
        for x in x0..(x0 + gw).min(w) {
            if m[y * w + x] {
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if y0 == usize::MAX {
        return None;
    }
    let gh = y1 - y0 + 1;
    let mut bits = String::with_capacity(gw * gh);
    for y in y0..=y1 {
        for x in x0..x0 + gw {
            bits.push(if x < w && m[y * w + x] { '1' } else { '0' });
        }
    }
    Some(Glyph { w: gw, h: gh, bits })
}

/// Read a line of proportional text.
///
/// The status bar can be cut into glyphs by looking for blank columns,
/// because it is one short field in a known layout. Prose cannot: at this
/// size the client's letters sometimes touch (the two t's in "Gatton" share
/// a crossbar and segment as one shape) and sometimes break apart (a thin
/// stroke falls below the threshold and one letter becomes two runs). Both
/// happen in the same line, so the run count can match the character count
/// while every pairing after the first mistake is wrong — which is how a
/// reader ends up taught that an 'e' is an 'f'.
///
/// So the atlas is matched against the raw mask instead, and where each
/// character begins is an output rather than an input. Every way of laying
/// the taught forms end to end across the line is a path; the best-scoring
/// one wins, found by a left-to-right pass that keeps only the best way of
/// reaching each column. A stretch of ink no form explains becomes a single
/// '?' rather than derailing everything after it.
pub fn read_line(
    m: &[bool],
    w: usize,
    h: usize,
    atlas: &std::collections::BTreeMap<String, Vec<Glyph>>,
    space_gap: usize,
    min_match: f32,
) -> String {
    read_line_detail(m, w, h, atlas, space_gap, min_match).0
}

/// The same read, plus where each character was found and how well it fitted.
/// For the calibration tools: when a line reads badly, the question is
/// always whether a glyph nearly matched or was nowhere near.
pub fn read_line_detail(
    m: &[bool],
    w: usize,
    h: usize,
    atlas: &std::collections::BTreeMap<String, Vec<Glyph>>,
    space_gap: usize,
    min_match: f32,
) -> (String, Vec<(usize, usize, char, f32)>) {
    let floor = if min_match > 0.0 { min_match } else { MIN_MATCH };
    if w == 0 || h == 0 || atlas.is_empty() {
        return (String::new(), Vec::new());
    }
    let ink = ink_columns(m, w, h);
    // Each taught form, resampled once for the whole line.
    let forms: Vec<(char, usize, usize, Vec<f32>)> = atlas
        .iter()
        .flat_map(|(ch, fs)| {
            let c = ch.chars().next().unwrap_or('?');
            fs.iter().map(move |f| (c, f.w, f.h, scaled_box(f)))
        })
        .collect();
    let widths: Vec<usize> = {
        let mut v: Vec<usize> = forms.iter().map(|(_, fw, ..)| *fw).collect();
        v.sort_unstable();
        v.dedup();
        v
    };

    /// Every path across the line explains the same columns, so a column is
    /// the unit everything is priced in: a blank column crossed is worth a
    /// whole point, and a column inside a match is worth how well that
    /// match fits.
    ///
    /// Both halves matter. Pricing matches per column but blanks at nothing
    /// hands free points to any wide character laid over a narrow one plus
    /// the space beside it — blank agrees with blank, so a mediocre wide
    /// match outscores a good narrow one, and the line reads as nonsense.
    /// Paying for the blank either way removes the bribe: covering space
    /// you did not need to cover now costs the difference between a perfect
    /// fit and the one you got.
    const BLANK_COLUMN: f32 = 1.0;

    /// Charged once per match, so that between two paths fitting equally
    /// well, the one inventing fewer characters wins. Half a column: enough
    /// to break a tie, small enough that a one-column 'i' matched perfectly
    /// is still worth more than giving up on it.
    const PER_GLYPH_COST: f32 = 0.5;

    let mut best: Vec<Option<f32>> = vec![None; w + 1];
    let mut from: Vec<Option<(usize, Option<char>, bool)>> = vec![None; w + 1];
    let mut fit_at: Vec<f32> = vec![0.0; w + 1];
    best[0] = Some(0.0);

    for x in 0..w {
        let Some(score) = best[x] else { continue };
        if !ink[x] {
            // Skip to the next ink, remembering whether the gap was wide
            // enough to be a space. Words have to survive: the classifier
            // works on keywords.
            let next = (x..w).find(|&i| ink[i]).unwrap_or(w);
            let space = next - x >= space_gap;
            let v = score + (next - x) as f32 * BLANK_COLUMN;
            if best[next].map(|b| v > b).unwrap_or(true) {
                best[next] = Some(v);
                from[next] = Some((x, None, space));
            }
            continue;
        }

        // Window per candidate width, shared by every form of that width.
        for &gw in &widths {
            if x + gw > w {
                continue;
            }
            // A taught form is trimmed to its own ink, so the stretch it is
            // compared against has to begin and end on ink too. Without
            // that, a wide character matches a narrow one plus the blank
            // beside it — blank agrees with blank — and since a match is
            // paid per column, swallowing the space scores better than
            // reading the line.
            if !ink[x + gw - 1] {
                continue;
            }
            let Some(win) = window(m, w, h, x, gw) else { continue };
            let wn = scaled_box(&win);
            let mut top: Option<(char, f32)> = None;
            // Within a pixel of the window's width: the same scaling that
            // changes a character's height by two rows changes its width by
            // a column, so insisting on an exact width would refuse the
            // right answer for the same reason.
            for (ch, _, _, f) in forms
                .iter()
                .filter(|(_, fw, fh, _)| fw.abs_diff(gw) <= 1 && fh.abs_diff(win.h) <= 3)
            {
                let s = agree_scaled(&wn, f);
                if s >= floor && top.map(|(_, b)| s > b).unwrap_or(true) {
                    top = Some((*ch, s));
                }
            }
            if let Some((ch, s)) = top {
                let v = score + gw as f32 * s - PER_GLYPH_COST;
                let end = x + gw;
                if best[end].map(|b| v > b).unwrap_or(true) {
                    best[end] = Some(v);
                    from[end] = Some((x, Some(ch), false));
                    fit_at[end] = s;
                }
            }
        }

        // Nothing fitted: give up on this run of ink as one unknown glyph
        // rather than on the rest of the line.
        let end = (x..w).find(|&i| !ink[i]).unwrap_or(w);
        if end > x && best[end].map(|b| score > b).unwrap_or(true) {
            best[end] = Some(score);
            from[end] = Some((x, Some('?'), false));
        }
    }

    // The line may end in blanks, so take the furthest column reached.
    let mut at = (0..=w).rev().find(|&i| best[i].is_some()).unwrap_or(0);
    let mut out: Vec<char> = Vec::new();
    let mut detail: Vec<(usize, usize, char, f32)> = Vec::new();
    while at > 0 {
        let Some((prev, ch, space)) = from[at] else { break };
        if let Some(c) = ch {
            out.push(c);
            detail.push((prev, at - prev, c, fit_at[at]));
        }
        if space {
            out.push(' ');
        }
        at = prev;
    }
    out.reverse();
    detail.reverse();
    (out.iter().collect::<String>().trim().to_string(), detail)
}

/// Group a line's runs into characters by the wide gaps between them.
///
/// For teaching only, and it is why teaching uses text typed with spaces
/// between the characters: a gap wide enough to be a space is the one
/// boundary the pixels state outright, so grouping on it cannot drift the
/// way pairing run-to-character does.
pub fn grouped_glyphs(frame: &Frame, roi: &Roi, min_gap: usize, space_gap: usize) -> Vec<Glyph> {
    let cuts = segment(frame, roi, min_gap);
    let mut groups: Vec<Vec<&Cut>> = Vec::new();
    for c in &cuts {
        let start_new = match groups.last().and_then(|g| g.last()) {
            Some(prev) => c.dx.saturating_sub(prev.dx + prev.glyph.w) >= space_gap,
            None => true,
        };
        if start_new {
            groups.push(vec![c]);
        } else {
            groups.last_mut().expect("just pushed").push(c);
        }
    }
    let m = mask(frame, roi);
    groups
        .iter()
        .filter_map(|g| {
            let x0 = g.first()?.dx;
            let x1 = g.last().map(|c| c.dx + c.glyph.w)?;
            window(&m, roi.w as usize, roi.h as usize, x0, x1 - x0)
        })
        .collect()
}

/// Digits only, as an integer. Separators and brackets are dropped.
///
/// Returns `None` if any glyph was unreadable. Stripping a `?` out would turn
/// `7?3` into `73` — off by a factor of ten and indistinguishable from a real
/// reading. A missing number is safe; a quietly wrong one is not.
pub fn digits(text: &str) -> Option<u64> {
    if text.contains('?') {
        return None;
    }
    let d: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.is_empty() { None } else { d.parse().ok() }
}

/// The `[12.34%]` part of an EXP readout, if present.
pub fn percent(text: &str) -> Option<f64> {
    if text.contains('?') {
        return None;
    }
    let start = text.find('[')?;
    let end = text[start..].find('%')? + start;
    text[start + 1..end]
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .parse()
        .ok()
}

/// The absolute EXP, i.e. the digits before the bracketed percentage.
pub fn exp_absolute(text: &str) -> Option<u64> {
    let head = match text.find('[') {
        Some(i) => &text[..i],
        None => text,
    };
    digits(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a one-line mask from ASCII art, '#' being ink.
    fn art(rows: &[&str]) -> (Vec<bool>, usize, usize) {
        let w = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let h = rows.len();
        let mut m = vec![false; w * h];
        for (y, r) in rows.iter().enumerate() {
            for (x, c) in r.chars().enumerate() {
                m[y * w + x] = c == '#';
            }
        }
        (m, w, h)
    }

    fn glyph(rows: &[&str]) -> Glyph {
        let (m, w, h) = art(rows);
        Glyph { w, h, bits: m.iter().map(|b| if *b { '1' } else { '0' }).collect() }
    }

    fn atlas(pairs: &[(char, &[&str])]) -> std::collections::BTreeMap<String, Vec<Glyph>> {
        let mut a = std::collections::BTreeMap::new();
        for (ch, rows) in pairs {
            a.insert(ch.to_string(), vec![glyph(rows)]);
        }
        a
    }

    /// Two characters whose shapes touch, which column segmentation would
    /// hand back as one glyph. The reader has to find the boundary itself.
    #[test]
    fn touching_letters_are_still_read_separately() {
        let a = atlas(&[('l', &["#", "#", "#"]), ('o', &["###", "#.#", "###"])]);
        let (m, w, h) = art(&["######", "##.#.#", "##.####"]);
        // "l" "l" "o" with no gap anywhere between them.
        let (m2, w2, h2) = art(&["#.#.###", "#.#.#.#", "#.#.###"]);
        let _ = (m, w, h);
        assert_eq!(read_line(&m2, w2, h2, &a, 3, 0.9), "llo");
    }

    /// A wide gap is a space and a narrow one is not: the chat classifier
    /// works on keywords, so losing word boundaries loses the events.
    #[test]
    fn wide_gaps_become_spaces() {
        let a = atlas(&[('l', &["#", "#", "#"])]);
        let (m, w, h) = art(&["#.#....#", "#.#....#", "#.#....#"]);
        assert_eq!(read_line(&m, w, h, &a, 3, 0.9), "ll l");
    }

    /// Ink the atlas cannot explain costs one character, not the line.
    #[test]
    fn unexplained_ink_is_one_unknown_not_a_derailment() {
        let a = atlas(&[('l', &["#", "#", "#"])]);
        // A shape with no full-height column, so no stem hides inside it.
        let (m, w, h) = art(&["#.##.#", "#....#", "#.##.#"]);
        assert_eq!(read_line(&m, w, h, &a, 9, 0.9), "l?l");
    }

    /// Teaching groups by the wide gaps, so a character that breaks into two
    /// runs is still taught as one glyph.
    #[test]
    fn teaching_groups_a_split_character_back_together() {
        // 'i': a dot and a stem with a blank row between, which also leaves
        // a blank column run of its own in some phases.
        let (m, w, h) = art(&["#.#....##", "#.#....##", "#.#....##"]);
        let px: Vec<u8> = m
            .iter()
            .flat_map(|on| {
                let v = if *on { 0u8 } else { 255u8 };
                [v, v, v, 255]
            })
            .collect();
        let frame = Frame { w: w as u32, h: h as u32, px: &px };
        let roi = Roi {
            name: "t".into(),
            x: 0,
            y: 0,
            w: w as u32,
            h: h as u32,
            threshold: 128,
            invert: true,
        };
        let gs = grouped_glyphs(&frame, &roi, 1, 3);
        assert_eq!(gs.len(), 2, "two characters, not three runs");
        assert_eq!(gs[0].w, 3, "the first keeps both of its runs");
    }

    #[test]
    fn parses_a_clean_reading() {
        assert_eq!(exp_absolute("763[0.26%]"), Some(763));
        assert_eq!(percent("763[0.26%]"), Some(0.26));
        assert_eq!(digits("HP[1364/1364]"), Some(13641364));
    }

    /// The important one. A digit the atlas has not been taught comes back as
    /// '?', and every parser must refuse the whole field rather than drop it:
    /// silently turning 7?3 into 73 is an order-of-magnitude error that looks
    /// exactly like a real reading.
    #[test]
    fn an_unknown_glyph_never_becomes_a_number() {
        assert_eq!(digits("7?3"), None);
        assert_eq!(exp_absolute("7?3[0.26%]"), None);
        assert_eq!(percent("763[0.?6%]"), None);
    }

    #[test]
    fn empty_and_junk_readings_are_none() {
        assert_eq!(digits(""), None);
        assert_eq!(digits("[]"), None);
        assert_eq!(percent("763"), None);
    }

    #[test]
    fn glyph_agreement_is_symmetric_and_bounded() {
        let a = Glyph { w: 2, h: 2, bits: "1001".into() };
        let b = Glyph { w: 2, h: 2, bits: "1001".into() };
        let c = Glyph { w: 2, h: 2, bits: "0110".into() };
        assert_eq!(a.agreement(&b), 1.0);
        assert_eq!(a.agreement(&c), 0.0);
        assert_eq!(c.agreement(&a), a.agreement(&c));
    }

    /// Different sizes pad rather than scale: these are bitmap fonts.
    #[test]
    fn differing_sizes_pad_to_a_common_box() {
        let small = Glyph { w: 1, h: 1, bits: "1".into() };
        let tall = Glyph { w: 1, h: 2, bits: "10".into() };
        assert_eq!(small.agreement(&tall), 1.0);
        let other = Glyph { w: 1, h: 2, bits: "11".into() };
        assert_eq!(small.agreement(&other), 0.5);
    }
}

#[cfg(test)]
mod exp_checks {
    use super::*;

    fn table() -> Vec<u64> {
        crate::expcurve::to_next()
    }

    #[test]
    fn a_number_that_fits_the_level_is_accepted() {
        // 149,705 of level 40's 294,216 is 50.88%, which is what the bar says.
        assert_eq!(agrees(149_705, 50.88, Some(40), &table()), Some(40));
    }

    #[test]
    fn a_clean_but_impossible_number_is_refused() {
        // The reading that cost a live session two billion EXP: well formed,
        // every glyph confident, and not a number that level 40 can show.
        assert_eq!(agrees(2_993_970_233, 50.88, Some(40), &table()), None);
    }

    #[test]
    fn the_bar_may_roll_over_into_the_next_level() {
        let t = table();
        let a = t[41] / 10;
        let p = ((a as f64 / t[41] as f64) * 10_000.0).round() / 100.0;
        assert_eq!(agrees(a, p, Some(40), &t), Some(41));
    }

    #[test]
    fn without_a_known_level_the_pair_must_identify_one() {
        assert_eq!(agrees(149_705, 50.88, None, &table()), Some(40));
        assert_eq!(agrees(149_705, 99.99, None, &table()), None);
    }
}
