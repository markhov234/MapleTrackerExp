//! Teach the status-bar font as the client draws it.
//!
//! The window stretches the client's image by some fraction, and a bitmap
//! font read through that stretch is not one font but a different rendering
//! per sub-pixel phase — a '2' that is nineteen rows tall with gaps on one
//! line and seventeen solid rows on the next. An atlas taught that way needs
//! a form per character per phase and is never quite finished. Taught on the
//! client's own grid, the same font is one form per character and is
//! finished the first time it is seen.
//!
//! The text usually does not have to be typed in. Once the profile can read
//! anything at all it is its own oracle: it says what the readout reads, and
//! the only question left is what those characters look like. `--text` is
//! for the cold start, and for a character the font has never been shown.
//!
//!   native-atlas discover <frame.png> [...]
//!       Which resolution the client renders at, and at which grey level,
//!       measured rather than assumed.
//!
//!   native-atlas teach <frame.png> [...] [--text S] [--at X Y W H]
//!                                        [--render WxH] [--grey N]
//!       Write the font into the profile. `--at` gives the readout's place
//!       in captured pixels, needed only when nothing can read yet.
//!
//!   native-atlas check <frame.png> [...]
//!       Read every frame through the profile and report what came back.
//!
//! Reads PNG captures and a profile. Touches nothing else.

use std::collections::BTreeMap;

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Frame, Glyph, Profile, Roi};
use maple_tracker::unstretch::{self, Scale};

const PROFILE: &str = "assets/profile.json";

fn profile_path() -> String {
    std::env::var("MAPLE_PROFILE").unwrap_or_else(|_| PROFILE.into())
}

fn load() -> Profile {
    let path = profile_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("could not read {path}: {e}");
        std::process::exit(1);
    });
    serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("{path} is not a profile: {e}");
        std::process::exit(1);
    })
}

struct Capture {
    name: String,
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Capture {
    fn frame(&self) -> Frame<'_> {
        Frame { w: self.w, h: self.h, px: &self.px }
    }
}

fn captures(args: &[String]) -> Vec<Capture> {
    let mut out = Vec::new();
    for path in args.iter().filter(|a| a.ends_with(".png")) {
        match image::open(path) {
            Ok(img) => {
                let img = img.to_rgba8();
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                out.push(Capture { name, w: img.width(), h: img.height(), px: img.into_raw() });
            }
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
    out
}

/// What is already known about one capture: where the readout is, in the
/// captured frame's own pixels, and what it says.
///
/// Captured pixels rather than the client's, deliberately. The region has to
/// survive being re-cut at a different candidate scale, and only a position
/// on the frame that actually arrived means the same thing under all of them.
struct Known {
    idx: usize,
    region: Roi,
    text: String,
}

/// Establish what each capture holds, by whichever route is available.
///
/// The profile is its own oracle wherever it can already read — which, once
/// anything has been taught, is everywhere. Typed text is the cold start:
/// with no font there is nothing to recognise the readout with, so its place
/// has to be given as well, once.
fn known(caps: &[Capture], profile: &Profile, text: Option<&str>, at: Option<Roi>) -> Vec<Known> {
    let table = expcurve::to_next();
    let mut out = Vec::new();
    for (idx, cap) in caps.iter().enumerate() {
        if let (Some(region), Some(t)) = (&at, text) {
            out.push(Known { idx, region: region.clone(), text: t.to_string() });
            continue;
        }
        let Some((plate, roi, scale)) = ocr::locate(&cap.frame(), profile, &table, &[]) else {
            eprintln!("{:<22} nothing here reads as an EXP bar; left out", cap.name);
            continue;
        };
        let (nx, ny) = plate.to_native(roi.x, roi.y);
        let region = Roi {
            name: "field".into(),
            x: (nx as f64 * scale.sx) as u32,
            y: (ny as f64 * scale.sy) as u32,
            w: (roi.w as f64 * scale.sx).ceil() as u32,
            h: (roi.h as f64 * scale.sy).ceil() as u32,
            threshold: profile.threshold,
            invert: false,
        };
        let found = ocr::read_exp(&plate.frame(), &roi, profile, None, &table).reading.text;
        match text {
            // Typed text with no place given: the profile still had to find
            // the readout, but whoever typed it knows what it says better
            // than a font with a hole in it does.
            Some(t) => out.push(Known { idx, region, text: t.to_string() }),
            None if found.contains('?') => {
                eprintln!("{:<22} unsure of {found:?}; left out", cap.name)
            }
            None => out.push(Known { idx, region, text: found }),
        }
    }
    out
}

/// The readout of one capture, resampled onto a candidate client grid.
///
/// Only the readout, never the whole strip. The strip also holds the gauges,
/// the icons and the frame around them, and those are the same pixels in
/// every capture — counting them would reward leaving the stretch in place,
/// since unscaled chrome repeats perfectly while stretched text does not.
fn field_glyphs(cap: &Capture, region: &Roi, s: Scale, threshold: u8, min_gap: usize) -> Vec<Glyph> {
    let frame = cap.frame();
    let (x0, y0) = s.to_native(region.x, region.y);
    let (x1, y1) = s.to_native(region.x + region.w, region.y + region.h);
    let plate = unstretch::plate(
        &frame,
        s,
        x0.saturating_sub(1),
        y0.saturating_sub(1),
        (x1 - x0) + 3,
        (y1 - y0) + 3,
    );
    if plate.w == 0 || plate.h == 0 {
        return Vec::new();
    }
    let local = Roi {
        name: "field".into(),
        x: 0,
        y: 0,
        w: plate.w,
        h: plate.h,
        threshold,
        invert: false,
    };
    ocr::segment(&plate.frame(), &local, min_gap).into_iter().map(|c| c.glyph).collect()
}

fn key(g: &Glyph) -> (usize, usize, String) {
    (g.w, g.h, g.bits.clone())
}

struct Row {
    nw: u32,
    nh: u32,
    thr: u8,
    agreed: usize,
    total: usize,
    distinct: usize,
}

/// Which render size, and which grey level, make the font collapse.
///
/// The right scale is not a matter of opinion: at it, the same character cut
/// from different frames at different positions is the same bitmap, and at
/// every other scale it is not. Counting distinct bitmaps per glyph read is
/// therefore a measurement, and it does not need a font to make it — which
/// matters, because the font is what is being built.
///
/// The grey level has to be found at the same time, because averaging pixels
/// together narrows the range they span: a serif one client-pixel wide is
/// half background by the time it reaches the plate, and a cut-off chosen
/// for the captured image clips it away. That is not cosmetic. Shorn of its
/// arms, '[' is a bare column and so is ']', and two characters that share a
/// bitmap are two characters the reader can no longer tell apart.
fn discover(caps: &[Capture], profile: &Profile, found: &[Known]) -> Option<(u32, u32, u8)> {
    let first = caps.first()?;
    if found.len() < 2 {
        eprintln!("need the readout in at least two captures to tell the scales apart");
        return None;
    }

    // A candidate grid is judged on whether it cuts each readout into
    // exactly the characters known to be there. Counting glyphs instead
    // would reward whichever grid shatters the font worst, since a '5'
    // broken into three pieces counts three times and each piece repeats
    // obligingly.
    let mut rows: Vec<Row> = Vec::new();
    for s in unstretch::candidates(first.w, first.h) {
        let (nw, nh) = s.native(first.w, first.h);
        for thr in (100u8..=170).step_by(5) {
            let mut seen: BTreeMap<(usize, usize, String), usize> = BTreeMap::new();
            let (mut agreed, mut total) = (0usize, 0usize);
            for k in found {
                let cap = &caps[k.idx];
                if cap.w != first.w || cap.h != first.h {
                    continue;
                }
                let gs = field_glyphs(cap, &k.region, s, thr, profile.min_gap);
                if gs.len() != k.text.chars().count() {
                    continue;
                }
                agreed += 1;
                for g in gs {
                    *seen.entry(key(&g)).or_default() += 1;
                    total += 1;
                }
            }
            rows.push(Row { nw, nh, thr, agreed, total, distinct: seen.len() });
        }
    }

    // Cutting the readouts correctly is a gate, not the ranking. A grid that
    // reads one fewer but whose shapes repeat three times as often is the
    // one that has actually found the font; a grid that cuts every readout
    // into the right number of pieces and then finds all of them different
    // has only reproduced the problem being solved.
    let reuse = |r: &Row| r.total as f64 / r.distinct.max(1) as f64;
    let enough = (found.len() * 4 / 5).max(2);
    let mut best_per_size: BTreeMap<(u32, u32), &Row> = BTreeMap::new();
    for row in &rows {
        if row.agreed < enough {
            continue;
        }
        let e = best_per_size.entry((row.nw, row.nh)).or_insert(row);
        if reuse(row) > reuse(e) {
            *e = row;
        }
    }
    if best_per_size.is_empty() {
        println!("no grid cuts {enough} of {} readouts into the characters read there", found.len());
        return None;
    }

    println!(
        "{:<14} {:>7} {:>6} {:>8} {:>8} {:>9} {:>7}",
        "render", "scale", "grey", "fields", "glyphs", "distinct", "reuse"
    );
    let mut best: Option<(f64, &Row)> = None;
    for row in best_per_size.values() {
        let r = reuse(row);
        println!(
            "{:<14} {:>7.4} {:>6} {:>8} {:>8} {:>9} {:>7.2}",
            format!("{}x{}", row.nw, row.nh),
            first.w as f64 / row.nw as f64,
            row.thr,
            format!("{}/{}", row.agreed, found.len()),
            row.total,
            row.distinct,
            r
        );
        if best.map(|(b, _)| r > b).unwrap_or(true) {
            best = Some((r, row));
        }
    }
    let (r, row) = best?;
    println!(
        "\nclient renders at {}x{}, text above grey {} (each bitmap reused {r:.2} times)",
        row.nw, row.nh, row.thr
    );
    Some((row.nw, row.nh, row.thr))
}

/// Pair what is known to be written with what the plate shows.
fn teach(caps: &[Capture], profile: &mut Profile, found: &[Known], grid: (u32, u32, u8)) {
    let (nw, nh, threshold) = grid;
    let mut atlas: BTreeMap<String, Vec<Glyph>> = BTreeMap::new();
    let mut taught = 0usize;

    for k in found {
        let cap = &caps[k.idx];
        let s = Scale { sx: cap.w as f64 / nw as f64, sy: cap.h as f64 / nh as f64 };
        let glyphs = field_glyphs(cap, &k.region, s, threshold, profile.min_gap);
        let chars: Vec<char> = k.text.chars().collect();
        // Where the two disagree about how many characters there are, the
        // frame is skipped rather than guessed at: a glyph taught under the
        // wrong name is a wrong number for as long as the profile lives.
        if glyphs.len() != chars.len() {
            println!(
                "{:<22} {:?} is {} characters but the plate cuts into {} — skipped",
                cap.name,
                k.text,
                chars.len(),
                glyphs.len()
            );
            continue;
        }
        for (ch, g) in chars.iter().zip(glyphs) {
            let forms = atlas.entry(ch.to_string()).or_default();
            if !forms.iter().any(|f| key(f) == key(&g)) {
                forms.push(g);
            }
            taught += 1;
        }
        println!("{:<22} {:?} ({} characters)", cap.name, k.text, chars.len());
    }

    if atlas.is_empty() {
        println!("\nnothing taught; profile untouched");
        return;
    }

    // Two characters with the same bitmap are two characters the reader can
    // only guess between, and it will guess silently. Better no font at all
    // than one that reads a bracket as a one.
    let mut owners: BTreeMap<(usize, usize, String), Vec<&String>> = BTreeMap::new();
    for (ch, fs) in &atlas {
        for f in fs {
            owners.entry(key(f)).or_default().push(ch);
        }
    }
    let clashes: Vec<_> = owners.values().filter(|v| v.len() > 1).collect();
    if !clashes.is_empty() {
        println!("\nthese characters share a bitmap and cannot be told apart:");
        for v in clashes {
            println!("  {}", v.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(" = "));
        }
        println!("profile untouched; try another grey level with --grey N");
        return;
    }

    // Characters the profile knew and this run did not see are kept: an
    // evening that happened never to show a '0' should not delete the '0'.
    let mut kept = 0usize;
    for (ch, fs) in &profile.atlas {
        if !atlas.contains_key(ch) {
            atlas.insert(ch.clone(), fs.clone());
            kept += 1;
        }
    }

    let forms: usize = atlas.values().map(|v| v.len()).sum();
    println!("\n{taught} glyphs read -> {} characters, {forms} forms", atlas.len());
    if kept > 0 {
        println!("({kept} character(s) not seen this run kept from the profile)");
    }
    for (ch, fs) in &atlas {
        println!("  {ch:?} {} form(s)", fs.len());
        if fs.len() == 1 {
            let f = &fs[0];
            for y in 0..f.h {
                let row: String = (0..f.w)
                    .map(|x| if f.bits.as_bytes()[y * f.w + x] == b'1' { '#' } else { '.' })
                    .collect();
                println!("      {row}");
            }
        }
    }

    profile.render_w = nw;
    profile.render_h = nh;
    profile.threshold = threshold;
    profile.atlas = atlas;
    let path = profile_path();
    match serde_json::to_string_pretty(&profile)
        .map_err(|e| e.to_string())
        .and_then(|t| std::fs::write(&path, t).map_err(|e| e.to_string()))
    {
        Ok(()) => println!("\nwritten to {path}"),
        Err(e) => eprintln!("\ncould not write {path}: {e}"),
    }
}

/// Read every capture through the profile and say what came back.
fn check(caps: &[Capture], profile: &Profile) {
    if profile.atlas.is_empty() {
        eprintln!("the profile has no font yet; run `teach` first");
        std::process::exit(1);
    }
    let table = expcurve::to_next();
    for cap in caps {
        let began = std::time::Instant::now();
        let hit = ocr::locate(&cap.frame(), profile, &table, &[]);
        let took = began.elapsed();
        match hit {
            Some((plate, roi, scale)) => {
                let exp = ocr::read_exp(&plate.frame(), &roi, profile, None, &table);
                let (gx, gy) = plate.to_native(roi.x, roi.y);
                let (nw, nh) = scale.native(cap.w, cap.h);
                println!(
                    "{:<22} {:>9} /{:.4} -> {nw}x{nh}  ({gx:>4},{gy:>4}) {:<18} level {:<5} conf {:.3} margin {:.3}  [{:?}]",
                    cap.name,
                    format!("{}x{}", cap.w, cap.h),
                    scale.sx,
                    exp.reading.text,
                    exp.level.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
                    exp.reading.confidence,
                    exp.reading.margin,
                    took
                );
            }
            None => println!(
                "{:<22} {:>9} not found  [{took:?}]",
                cap.name,
                format!("{}x{}", cap.w, cap.h)
            ),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_default();
    let caps = captures(&args);
    if caps.is_empty() {
        eprintln!("usage: native-atlas <discover|teach|check> <frame.png> [...]");
        std::process::exit(2);
    }
    let value =
        |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mut profile = load();

    if mode == "check" {
        check(&caps, &profile);
        return;
    }

    let text = value("--text");
    let at = args.iter().position(|a| a == "--at").and_then(|i| {
        let n = |k: usize| args.get(i + k)?.parse::<u32>().ok();
        Some(Roi {
            name: "field".into(),
            x: n(1)?,
            y: n(2)?,
            w: n(3)?,
            h: n(4)?,
            threshold: profile.threshold,
            invert: false,
        })
    });
    if at.is_some() && text.is_none() {
        eprintln!("--at says where the readout is; --text has to say what it reads");
        std::process::exit(2);
    }
    let found = known(&caps, &profile, text.as_deref(), at);
    if found.is_empty() {
        eprintln!("nothing to go on: no capture could be read, and none was described");
        eprintln!("for a first font, give one: --text \"145,837[49.57%]\" --at X Y W H");
        std::process::exit(1);
    }

    match mode.as_str() {
        "discover" => {
            discover(&caps, &profile, &found);
        }
        "teach" => {
            let explicit = value("--render").and_then(|v| {
                let (a, b) = v.split_once('x')?;
                let grey = value("--grey").and_then(|g| g.parse().ok()).unwrap_or(135);
                Some((a.parse().ok()?, b.parse().ok()?, grey))
            });
            let grid = match explicit {
                Some(g) => Some(g),
                None => discover(&caps, &profile, &found).map(|(w, h, thr)| {
                    (w, h, value("--grey").and_then(|g| g.parse().ok()).unwrap_or(thr))
                }),
            };
            if let Some(grid) = grid {
                println!();
                teach(&caps, &mut profile, &found, grid);
            }
        }
        other => {
            eprintln!("unknown mode {other:?}; expected discover, teach or check");
            std::process::exit(2);
        }
    }
}
