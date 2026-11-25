//! Teach and test the lines the client floats over the game world.
//!
//! "You received EXP (+107)" and "You have gained mesos (+95)" are not in
//! the chat log. They are drawn onto the scene itself, low and to the right,
//! above the status bar, and they fade out over a couple of seconds. They
//! are also the only place kills and mesos appear at all, which is why every
//! tile that depends on them has been showing a dash.
//!
//! Everything here works on the client's own pixel grid, never the captured
//! one. Unstretched, this is an ordinary bitmap font; stretched, it is a
//! different rendering per sub-pixel phase and not worth teaching.
//!
//!   notice-atlas show  <frame.png> [...]   what the band holds, line by line
//!   notice-atlas read  <frame.png> [...]   read it with the taught font
//!   notice-atlas teach <frame.png> --at Y H --text "a b c"
//!                                          teach one line, characters spaced
//!   notice-atlas events <frame.png> [...]  what the lines are understood as
//!
//! Reads PNG captures and a profile. Touches nothing else.

use std::collections::BTreeMap;

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Frame, Notices, Profile, Roi};
use maple_tracker::unstretch::{self, Plate};

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

fn save(p: &Profile) {
    let path = profile_path();
    match serde_json::to_string_pretty(p)
        .map_err(|e| e.to_string())
        .and_then(|t| std::fs::write(&path, t).map_err(|e| e.to_string()))
    {
        Ok(()) => println!("written to {path}"),
        Err(e) => eprintln!("could not write {path}: {e}"),
    }
}

/// A sensible starting region: the right half of the frame, from just below
/// the middle to just above the status bar. Deliberately generous — the
/// lines that matter are picked out by what they say, not by where they are.
fn default_notices() -> Notices {
    Notices {
        enabled: false,
        x0: 0.45,
        y0: 0.55,
        x1: 1.0,
        y1: 0.93,
        threshold: 100,
        min_gap: 1,
        space_gap: 3,
        line_gap: 2,
        min_row_ink: 6,
        min_match: 0.88,
        atlas: BTreeMap::new(),
    }
}

struct Shot {
    name: String,
    plate: Plate,
}

impl Shot {
    fn frame(&self) -> Frame<'_> {
        self.plate.frame()
    }
}

/// Every capture, resampled onto the client's grid.
///
/// The scaling is confirmed the same way the tracker confirms it — by
/// finding a readout whose number divides into the percentage beside it —
/// and only falls back to the profile's recorded size when no status bar is
/// in the frame to check against.
fn shots(args: &[String], profile: &Profile) -> Vec<Shot> {
    let table = expcurve::to_next();
    let mut out = Vec::new();
    for path in args.iter().filter(|a| a.ends_with(".png")) {
        let Ok(img) = image::open(path) else {
            eprintln!("{path}: could not open");
            continue;
        };
        let img = img.to_rgba8();
        let (w, h) = (img.width(), img.height());
        let px = img.into_raw();
        let frame = Frame { w, h, px: &px };
        let scale = match ocr::locate(&frame, profile, &table, &[]) {
            Some((_, _, s)) => s,
            None => profile.scale(w, h),
        };
        let (nw, nh) = scale.native(w, h);
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        // Measured by warm light, not brightness: the floating lines are
        // drawn over the map, and lit grass is as bright as text.
        out.push(Shot {
            name,
            plate: unstretch::plate_with(&frame, scale, 0, 0, nw, nh, unstretch::warm),
        });
    }
    out
}

fn render(frame: &Frame, roi: &Roi) -> Vec<String> {
    (0..roi.h)
        .map(|y| {
            (0..roi.w)
                .map(|x| {
                    let (px, py) = (roi.x + x, roi.y + y);
                    if px < frame.w && py < frame.h && frame.grey(px, py) >= roi.threshold {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect()
}

/// Show what the band holds, so the region and the thresholds can be judged
/// rather than guessed at.
fn show(shots: &[Shot], cfg: &Notices, draw: bool) {
    for s in shots {
        let f = s.frame();
        let band = cfg.band(f.w, f.h);
        println!(
            "\n=== {} — client {}x{}, band ({},{}) {}x{} ===",
            s.name, f.w, f.h, band.x, band.y, band.w, band.h
        );
        let rows = ocr::line_bands(&f, &band, cfg.line_gap, cfg.min_row_ink);
        if rows.is_empty() {
            println!("  no rows of text found");
            continue;
        }
        for (dy, h) in rows {
            let line = ocr::line_roi(&f, &band, dy, h, true);
            // Trim to the ink, because the band is far wider than the text
            // and right-aligned lines would otherwise print as mostly dots.
            let m = ocr::line_mask(&f, &line);
            let w = line.w as usize;
            let cols: Vec<bool> = (0..w)
                .map(|x| (0..h as usize).any(|y| m[y * w + x]))
                .collect();
            let first = cols.iter().position(|c| *c);
            let last = cols.iter().rposition(|c| *c);
            let (Some(a), Some(b)) = (first, last) else { continue };
            let tight =
                Roi { x: line.x + a as u32, w: (b - a + 1) as u32, ..line.clone() };
            println!(
                "  y {:>4} h {:>2}  x {:>4}..{:<4}  grey {:>3}  ({} px wide)",
                line.y,
                h,
                tight.x,
                tight.x + tight.w,
                line.threshold,
                tight.w
            );
            if draw {
                for row in render(&f, &tight) {
                    println!("      {row}");
                }
            }
        }
    }
}

fn read(shots: &[Shot], cfg: &Notices) {
    if cfg.atlas.is_empty() {
        eprintln!("no font taught yet; use `show` to find the lines, then `teach`");
        std::process::exit(1);
    }
    for s in shots {
        let f = s.frame();
        println!("\n=== {} ===", s.name);
        for line in ocr::read_notices(&f, cfg) {
            println!("  {line:?}");
        }
    }
}

fn events(shots: &[Shot], cfg: &Notices) {
    let mut log = maple_tracker::chat::Log::default();
    for s in shots {
        let f = s.frame();
        let lines = ocr::read_notices(&f, cfg);
        let evs = log.recognised(lines.clone());
        println!("\n=== {} ===", s.name);
        for l in &lines {
            println!("  read  {l:?}");
        }
        for e in &evs {
            println!("  event {e:?}");
        }
    }
}

/// Teach one line from what it says.
///
/// The chat font had to be taught from characters typed with spaces between
/// them, because there was no way to know where one letter ended and the
/// next began. This font does not need that, and cannot have it — nobody
/// can make the client float a spaced-out message. Instead the line is cut
/// on blank columns and the cuts are matched against the typed text.
///
/// That only works if the cuts and the characters correspond exactly, so
/// the count is checked before anything is stored, and a mismatch prints
/// the cuts and stores nothing. A glyph taught under the wrong name is a
/// wrong kill count for as long as the profile lives.
fn teach(shots: &[Shot], cfg: &mut Notices, at: (u32, u32), text: &str) {
    let Some(s) = shots.first() else {
        eprintln!("no capture");
        return;
    };
    let f = s.frame();
    let band = cfg.band(f.w, f.h);
    let line = ocr::line_roi(&f, &Roi { y: at.0, h: at.1, ..band }, 0, at.1, true);
    let cuts = ocr::segment(&f, &line, cfg.min_gap);
    let glyphs: Vec<ocr::Glyph> = cuts.iter().map(|c| c.glyph.clone()).collect();
    // Spaces are in the line as gaps, not as ink, so they are not cut.
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if glyphs.len() != chars.len() {
        println!(
            "typed {} characters but the line cuts into {} — nothing stored",
            chars.len(),
            glyphs.len()
        );
        for (i, c) in cuts.iter().enumerate() {
            println!(
                "  cut {i:>2} at x+{:<4} {}x{}{}",
                c.dx,
                c.glyph.w,
                c.glyph.h,
                chars.get(i).map(|ch| format!("   would be {ch:?}")).unwrap_or_default()
            );
        }
        return;
    }
    let mut added = 0usize;
    for (ch, g) in chars.iter().zip(glyphs) {
        let forms = cfg.atlas.entry(ch.to_string()).or_default();
        if !forms.iter().any(|f| (f.w, f.h, &f.bits) == (g.w, g.h, &g.bits)) {
            forms.push(g);
            added += 1;
        }
    }
    let total: usize = cfg.atlas.values().map(|v| v.len()).sum();
    println!(
        "{added} new form(s); font now {} characters, {total} forms",
        cfg.atlas.len()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_default();
    let value =
        |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mut profile = load();
    let shots = shots(&args, &profile);
    if shots.is_empty() {
        eprintln!("usage: notice-atlas <show|read|teach|events> <frame.png> [...]");
        std::process::exit(2);
    }
    let mut cfg = profile.notices.clone().unwrap_or_else(default_notices);
    // Let the region be nudged from the command line while it is being found.
    for (name, slot) in [("--x0", 0), ("--y0", 1), ("--x1", 2), ("--y1", 3)] {
        if let Some(v) = value(name).and_then(|v| v.parse::<f32>().ok()) {
            match slot {
                0 => cfg.x0 = v,
                1 => cfg.y0 = v,
                2 => cfg.x1 = v,
                _ => cfg.y1 = v,
            }
        }
    }
    if let Some(v) = value("--grey").and_then(|v| v.parse().ok()) {
        cfg.threshold = v;
    }
    if let Some(v) = value("--ink").and_then(|v| v.parse().ok()) {
        cfg.min_row_ink = v;
    }

    match mode.as_str() {
        "show" => show(&shots, &cfg, args.iter().any(|a| a == "--draw")),
        "read" => read(&shots, &cfg),
        "events" => events(&shots, &cfg),
        "teach" => {
            let at = args.iter().position(|a| a == "--at").and_then(|i| {
                Some((args.get(i + 1)?.parse().ok()?, args.get(i + 2)?.parse().ok()?))
            });
            let (Some(at), Some(text)) = (at, value("--text")) else {
                eprintln!("teach needs --at Y H and --text \"Y o u   h a v e\"");
                std::process::exit(2);
            };
            teach(&shots, &mut cfg, at, &text);
            profile.notices = Some(cfg);
            save(&profile);
        }
        other => {
            eprintln!("unknown mode {other:?}; expected show, read, teach or events");
            std::process::exit(2);
        }
    }
}
