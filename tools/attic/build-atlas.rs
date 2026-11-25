//! Teach the digit atlas from a captured frame, and write the OCR profile.
//!
//! You give it a region and what that region actually says. It segments the
//! region into glyphs, checks the count matches your string, and stores each
//! glyph's bitmap under its character. Teaching further frames adds the other
//! sub-pixel forms the DPI upscale produces, which only improves matching.
//!
//! Usage: build-atlas [frame.png] [out.json]

use maple_tracker::ocr::{segment, Frame, Glyph, Profile, Roi};
use std::collections::BTreeMap;

/// The status-bar EXP field: light digits and green brackets on a dark plate.
///
/// Threshold 140 rather than 190 on purpose. The digits are bright enough for
/// either, but the green brackets sit near 150 luma and vanish at 190 — and
/// the parser needs `[` and `]` to tell the absolute EXP from the percentage.
fn default_rois() -> Vec<Roi> {
    vec![
        // Wide enough for the field to grow. Both halves get longer as you
        // play — the EXP number gains digits and separators, and the
        // percentage goes from "0.26%" to "10.07%" to "100.00%" — and a box
        // sized to today's value silently clips tomorrow's. It stops short of
        // 1589, where the next UI element begins.
        Roi {
            name: "exp".into(),
            x: 1400,
            y: 1384,
            w: 185,
            h: 18,
            threshold: 140,
            invert: false,
        },
        // HP and MP are taught purely for digit coverage: one EXP string only
        // contains five of the ten digits, and a tracker that cannot read a 4
        // or an 8 is useless. Their boxes come from `find-fields`.
        //
        // HP's box starts past the bar graphic that sits at the field's left
        // edge, which is not a character and has nothing to teach.
        Roi {
            name: "hp".into(),
            x: 931,
            y: 1383,
            w: 162,
            h: 18,
            threshold: 140,
            invert: false,
        },
        Roi {
            name: "mp".into(),
            x: 1146,
            y: 1383,
            w: 151,
            h: 18,
            threshold: 140,
            invert: false,
        },
    ]
}

/// What each region reads on `captures/frame.png`.
///
/// The bold `HP` / `MP` labels are taught alongside the numbers. They cost
/// nothing, and including them keeps each truth string a literal description
/// of the whole box rather than a hand-trimmed slice of it.
const TRUTH: &[(&str, &str)] = &[
    ("exp", "763[0.26%]"),
    ("hp", "HP[1364/1364]"),
    ("mp", "MP[1138/1138]"),
];

/// Say plainly which digits the atlas still cannot read.
///
/// A single frame's status bar only contains some of them, so a fresh atlas
/// is always partial. Naming the gap is the difference between a known
/// limitation and a mystery.
fn report_coverage(profile: &Profile) {
    let missing: Vec<char> = "0123456789"
        .chars()
        .filter(|d| !profile.atlas.contains_key(&d.to_string()))
        .collect();
    if missing.is_empty() {
        println!("digits  : all ten taught");
    } else {
        let list: Vec<String> = missing.iter().map(|c| c.to_string()).collect();
        println!("digits  : MISSING {}", list.join(" "));
        println!("          Any reading containing one of those fails rather than guessing.");
        println!("          Capture a frame showing one and teach it:");
        println!("            capture-spike \"MapleStory\" teach.png");
        println!("            find-fields captures/teach.png 900 1378 700 26 --thr 140");
        println!("            build-atlas --add captures/teach.png <x> <y> <w> <h> 140 \"<text>\"");
    }
}

/// Teach one extra region into the existing profile.
///
/// Additive on purpose: the atlas is built up across frames as digits appear,
/// and nothing already learned should be lost to add one more form.
fn add_mode(argv: &[String]) {
    let invert = argv.iter().any(|s| s == "--invert");
    let positional: Vec<&String> = argv.iter().filter(|s| !s.starts_with("--")).collect();
    if positional.len() < 7 {
        eprintln!("usage: build-atlas --add <frame.png> <x> <y> <w> <h> <thr> \"<text>\" [--invert]");
        std::process::exit(2);
    }
    let frame_path = positional[0];
    let num = |i: usize| positional[i].parse::<u32>().unwrap_or(0);
    let truth = positional[6];

    let profile_path = "assets/profile.json";
    let mut profile: Profile = match std::fs::read_to_string(profile_path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not load {profile_path}: {e}");
            eprintln!("run build-atlas with no arguments first to create it");
            std::process::exit(1);
        }
    };

    let img = match image::open(frame_path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {frame_path}: {e}");
            std::process::exit(1);
        }
    };
    let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };

    let roi = Roi {
        name: "adhoc".into(),
        x: num(1),
        y: num(2),
        w: num(3),
        h: num(4),
        threshold: num(5) as u8,
        invert,
    };
    let cuts = segment(&frame, &roi, profile.min_gap);
    let chars: Vec<char> = truth.chars().filter(|c| !c.is_whitespace()).collect();
    println!("segments: {} glyphs, expected {}", cuts.len(), chars.len());
    print!("widths  :");
    for c in &cuts {
        print!(" {}x{}", c.glyph.w, c.glyph.h);
    }
    println!();

    if cuts.len() != chars.len() {
        eprintln!("MISMATCH: adjust the box, the threshold, or --invert until the counts agree.");
        std::process::exit(1);
    }

    let mut learned = 0usize;
    for (cut, ch) in cuts.iter().zip(chars.iter()) {
        // A position the percentage could not pin down teaches nothing. It
        // is left for a frame where the arithmetic settles it.
        if *ch == '?' {
            continue;
        }
        let forms = profile.atlas.entry(ch.to_string()).or_default();
        if forms.iter().any(|f| f.bits == cut.glyph.bits && f.w == cut.glyph.w) {
            continue;
        }
        forms.push(cut.glyph.clone());
        learned += 1;
    }
    println!("learned : {learned} new form(s)");
    println!("atlas   : {} characters, {} forms", profile.atlas.len(), profile.glyph_count());
    report_coverage(&profile);

    match serde_json::to_string_pretty(&profile)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(profile_path, s).map_err(|e| e.to_string()))
    {
        Ok(()) => println!("wrote   : {profile_path}"),
        Err(e) => {
            eprintln!("could not write {profile_path}: {e}");
            std::process::exit(1);
        }
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Teach the EXP region without being told what it says.
///
/// The field carries the same fact twice — an absolute EXP and a percentage
/// of the level — so once the level is known either half predicts the other.
/// Read the absolute, compute the percentage the client must be showing, and
/// the whole string becomes ground truth.
///
/// This matters because the atlas is never finished by hand. Every new EXP
/// value shifts the glyphs to a different sub-pixel offset, and the DPI
/// upscale rasterises each offset differently, so forms keep appearing
/// indefinitely. Teaching them one screenshot at a time does not scale;
/// harvesting them from frames the player already produced does.
///
/// Safety: the absolute must read cleanly, and the predicted string must
/// segment into exactly as many glyphs as it has characters. A wrong level
/// fails that check rather than poisoning the atlas.
fn learn_mode(argv: &[String]) {
    let positional: Vec<&String> = argv.iter().filter(|s| !s.starts_with("--")).collect();
    if positional.len() < 2 {
        eprintln!("usage: build-atlas --learn <frame.png> <level>");
        std::process::exit(2);
    }
    let frame_path = positional[0];
    let level: usize = positional[1].parse().unwrap_or(0);

    let profile_path = "assets/profile.json";
    let mut profile: Profile = match std::fs::read_to_string(profile_path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not load {profile_path}: {e}");
            std::process::exit(1);
        }
    };
    let table = maple_tracker::expcurve::to_next();
    if level == 0 || level > maple_tracker::expcurve::LAST_LEVEL {
        eprintln!("level must be 1..{}", maple_tracker::expcurve::LAST_LEVEL);
        std::process::exit(2);
    }
    let cost = table[level];

    let img = match image::open(frame_path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {frame_path}: {e}");
            std::process::exit(1);
        }
    };
    let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };
    let Some(roi) = profile.roi("exp").cloned() else {
        eprintln!("profile has no exp region");
        std::process::exit(1);
    };

    let reading = maple_tracker::ocr::read(&frame, &roi, &profile);
    println!("{frame_path}: read \"{}\"", reading.text);

    // Split the field into its two halves without requiring either to be
    // clean, so a broken half can be recovered from the intact one.
    let head_end = reading.text.find('[');
    let pct_end = reading.text.find('%');
    let (head, pct_str) = match (head_end, pct_end) {
        (Some(i), Some(j)) if j > i => (
            reading.text[..i].to_string(),
            reading.text[i + 1..j].to_string(),
        ),
        _ => (reading.text.clone(), String::new()),
    };

    let expected: String;
    if !head.contains('?') {
        // Forward: the absolute is solid, so the percentage is determined.
        let absolute: u64 = head
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        if absolute == 0 {
            eprintln!("no absolute EXP to anchor on");
            std::process::exit(1);
        }
        let pct = (absolute as f64 / cost as f64 * 10000.0).round() / 100.0;
        expected = format!("{}[{:.2}%]", group(absolute), pct);
        println!("level {level} costs {} -> expects \"{expected}\"", group(cost));
    } else if !pct_str.is_empty() && !pct_str.contains('?') {
        // Reverse: some digits are unreadable, but the percentage is clean.
        // Only some numbers round to what is on screen, so enumerate the
        // unknown digits and keep the assignments that do.
        //
        // One unknown usually pins down exactly one number. Several rarely
        // do — the percentage is quoted to a hundredth, which is tens of EXP
        // wide, so the last digit is often free. That does not make the
        // frame useless: every surviving assignment may still agree on the
        // *leading* digits, and a position every solution agrees on is
        // ground truth whether or not the whole number is. Those glyphs are
        // taught; the genuinely free ones are left for a frame that pins
        // them.
        let Ok(shown) = pct_str.parse::<f64>() else {
            eprintln!("could not parse the percentage {pct_str:?}");
            std::process::exit(1);
        };
        let holes = head.matches('?').count();
        if holes > 3 {
            eprintln!("{holes} unreadable digits is too many to enumerate; try another frame");
            std::process::exit(1);
        }
        let combos = 10usize.pow(holes as u32);
        let mut hits: Vec<Vec<char>> = Vec::new();
        for mut idx in 0..combos {
            let mut digits: Vec<char> = Vec::with_capacity(holes);
            for _ in 0..holes {
                digits.push((b'0' + (idx % 10) as u8) as char);
                idx /= 10;
            }
            let mut candidate = head.clone();
            for d in &digits {
                candidate = candidate.replacen('?', &d.to_string(), 1);
            }
            let n: u64 = match candidate
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
            {
                Ok(v) => v,
                Err(_) => continue,
            };
            if n > cost {
                continue;
            }
            let implied = (n as f64 / cost as f64 * 10000.0).round() / 100.0;
            if (implied - shown).abs() < 0.001 {
                hits.push(digits);
            }
        }
        if hits.is_empty() {
            eprintln!("nothing makes \"{head}\" round to {shown:.2}% — wrong level?");
            std::process::exit(1);
        }

        // A position is certain when every surviving assignment agrees.
        let mut certain: Vec<Option<char>> = Vec::with_capacity(holes);
        for i in 0..holes {
            let first = hits[0][i];
            certain.push(hits.iter().all(|h| h[i] == first).then_some(first));
        }
        let pinned = certain.iter().filter(|c| c.is_some()).count();
        println!(
            "{} assignment(s) round to {shown:.2}% at level {level}; {pinned} of {holes} digit(s) are the same in all of them",
            hits.len()
        );
        if pinned == 0 {
            eprintln!("none of the unreadable digits is pinned down; try another frame");
            std::process::exit(1);
        }

        let mut filled = head.clone();
        for c in &certain {
            let sub = c.map(|d| d.to_string()).unwrap_or_else(|| "?".into());
            filled = filled.replacen('?', &sub, 1);
        }
        expected = format!("{filled}[{shown:.2}%]");
    } else {
        eprintln!("too much of the field is unreadable to recover it; try another frame");
        std::process::exit(1);
    }

    let cuts = segment(&frame, &roi, profile.min_gap);
    let chars: Vec<char> = expected.chars().collect();
    if cuts.len() != chars.len() {
        eprintln!(
            "segmented {} glyphs but the prediction has {} characters — wrong level, or the box clips the field",
            cuts.len(),
            chars.len()
        );
        std::process::exit(1);
    }

    let mut learned = 0usize;
    for (cut, ch) in cuts.iter().zip(chars.iter()) {
        // A position the percentage could not pin down teaches nothing. It
        // is left for a frame where the arithmetic settles it.
        if *ch == '?' {
            continue;
        }
        let forms = profile.atlas.entry(ch.to_string()).or_default();
        if forms.iter().any(|f| f.bits == cut.glyph.bits && f.w == cut.glyph.w) {
            continue;
        }
        forms.push(cut.glyph.clone());
        learned += 1;
    }
    println!("learned {learned} new form(s); atlas now {} characters, {} forms",
        profile.atlas.len(), profile.glyph_count());
    report_coverage(&profile);

    if let Err(e) = serde_json::to_string_pretty(&profile)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(profile_path, s).map_err(|e| e.to_string()))
    {
        eprintln!("could not write {profile_path}: {e}");
        std::process::exit(1);
    }
    println!("wrote {profile_path}");
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(|s| s == "--add").unwrap_or(false) {
        add_mode(&argv[1..]);
        return;
    }
    if argv.first().map(|s| s == "--learn").unwrap_or(false) {
        learn_mode(&argv[1..]);
        return;
    }

    let mut args = std::env::args().skip(1);
    let frame_path = args.next().unwrap_or_else(|| "captures/frame.png".into());
    let out_path = args.next().unwrap_or_else(|| "assets/profile.json".into());

    let img = match image::open(&frame_path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {frame_path}: {e}");
            std::process::exit(1);
        }
    };
    let (fw, fh) = (img.width(), img.height());
    let frame = Frame { w: fw, h: fh, px: img.as_raw() };
    println!("frame   : {frame_path}  {fw}x{fh}");

    let rois = default_rois();
    let min_gap = 1usize;

    // Start from whatever has already been learned. Region boxes are always
    // taken fresh from the defaults, so widening one does not cost you every
    // glyph form taught from later frames.
    let mut atlas: BTreeMap<String, Vec<Glyph>> = match std::fs::read_to_string(&out_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Profile>(&s).ok())
    {
        Some(existing) => {
            println!(
                "merging : {} characters, {} forms already known",
                existing.atlas.len(),
                existing.glyph_count()
            );
            existing.atlas
        }
        None => BTreeMap::new(),
    };
    let mut failed = false;

    for (name, truth) in TRUTH {
        let Some(roi) = rois.iter().find(|r| &r.name == name) else {
            eprintln!("no region named {name}");
            failed = true;
            continue;
        };
        let cuts = segment(&frame, roi, min_gap);
        let chars: Vec<char> = truth.chars().filter(|c| !c.is_whitespace()).collect();

        println!(
            "\nregion  : {name} ({}, {}) {}x{} thr {}{}",
            roi.x,
            roi.y,
            roi.w,
            roi.h,
            roi.threshold,
            if roi.invert { " inverted" } else { "" }
        );
        println!("segments: {} glyphs, expected {}", cuts.len(), chars.len());
        print!("widths  :");
        for c in &cuts {
            print!(" {}x{}", c.glyph.w, c.glyph.h);
        }
        println!();

        if cuts.len() != chars.len() {
            eprintln!(
                "MISMATCH in {name}: segmented {} but '{}' has {} characters.",
                cuts.len(),
                truth,
                chars.len()
            );
            eprintln!("Adjust the region bounds, the threshold, or min_gap until they agree.");
            failed = true;
            continue;
        }

        let mut learned = 0usize;
        for (cut, ch) in cuts.iter().zip(chars.iter()) {
            let key = ch.to_string();
            let forms = atlas.entry(key).or_default();
            // Skip a form we already have; teaching more frames should add
            // new sub-pixel variants, not duplicates.
            if forms.iter().any(|f| {
                f.w == cut.glyph.w && f.h == cut.glyph.h && f.bits == cut.glyph.bits
            }) {
                continue;
            }
            forms.push(cut.glyph.clone());
            learned += 1;
        }
        println!("learned : {learned} new form(s)");
    }

    if failed {
        std::process::exit(1);
    }

    let profile = Profile {
        version: 1,
        frame_w: fw,
        frame_h: fh,
        min_gap,
        rois,
        atlas,
        // A rebuild of the status-bar profile does not know where the chat
        // box is; `chat-atlas box` adds that separately.
        chat: None,
        native: None,
    };

    println!(
        "\natlas   : {} characters, {} forms",
        profile.atlas.len(),
        profile.glyph_count()
    );
    print!("chars   :");
    for k in profile.atlas.keys() {
        print!(" {k}");
    }
    println!();
    report_coverage(&profile);

    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(&profile)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(&out_path, s).map_err(|e| e.to_string()))
    {
        Ok(()) => println!("wrote   : {out_path}"),
        Err(e) => {
            eprintln!("could not write {out_path}: {e}");
            std::process::exit(1);
        }
    }
}
