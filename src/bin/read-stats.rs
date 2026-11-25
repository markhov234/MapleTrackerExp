//! Read the live status bar from a captured frame and derive the session view.
//!
//! This is the end-to-end check on the OCR path: pixels in, a level and an
//! EXP position out, with no hand-entered numbers anywhere.
//!
//! Usage: read-stats [frame.png] [profile.json]

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Frame, Profile};

fn main() {
    // `--level N` tells the reconciler which level to check the percentage
    // against, the way the live tracker's session does.
    let flags: Vec<String> = std::env::args().skip(1).collect();
    let mut args = flags
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && flags.get(i.wrapping_sub(1)).map(|p| p != "--level").unwrap_or(true))
        .map(|(_, a)| a.clone());
    let frame_path = args.next().unwrap_or_else(|| "captures/frame.png".into());
    let profile_path = args.next().unwrap_or_else(|| "assets/profile.json".into());

    let raw = match std::fs::read_to_string(&profile_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not read {profile_path}: {e}");
            eprintln!("run build-atlas first");
            std::process::exit(1);
        }
    };
    let profile: Profile = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{profile_path} is not a valid profile: {e}");
            std::process::exit(1);
        }
    };

    let img = match image::open(&frame_path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {frame_path}: {e}");
            std::process::exit(1);
        }
    };
    let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };

    println!("frame   : {frame_path}  {}x{}", frame.w, frame.h);
    println!(
        "profile : {profile_path}  {} chars, {} forms, client renders {}x{}",
        profile.atlas.len(),
        profile.glyph_count(),
        profile.render_w,
        profile.render_h
    );

    let table = expcurve::to_next();
    let Some((plate, roi, scale)) = ocr::locate(&frame, &profile, &table, &[]) else {
        eprintln!("\nno EXP readout found in this frame");
        std::process::exit(2);
    };
    let (gx, gy) = plate.to_native(roi.x, roi.y);
    let (nw, nh) = scale.native(frame.w, frame.h);
    let view = plate.frame();
    println!(
        "grid    : stretched by {:.4} -> client {nw}x{nh}, readout at ({gx}, {gy}) {}x{}",
        scale.sx, roi.w, roi.h
    );

    let r = ocr::read(&view, &roi, &profile);
    println!("\n{:<22} {:>7} {:>8}  {}", "READ", "CONF", "MARGIN", "GLYPHS");
    println!("{}", "-".repeat(52));
    println!(
        "{:<22} {:>6.1}% {:>7.1}%  {}",
        if r.text.is_empty() { "(no ink)".into() } else { r.text.clone() },
        r.confidence * 100.0,
        r.margin * 100.0,
        r.glyphs
    );

    // Why a glyph was refused is the thing you actually need when a field
    // stops reading: the score alone does not say which two characters were
    // tied.
    if r.unknown > 0 {
        for (i, c) in ocr::segment(&view, &roi, profile.min_gap).iter().enumerate() {
            let all = ocr::all_scores(&c.glyph, &profile.atlas);
            let shown: Vec<String> =
                all.iter().map(|(ch, s)| format!("{ch} {:.1}%", s * 100.0)).collect();
            println!(
                "  cut {i:>2} at x+{:<4} {}x{}  {}",
                c.dx, c.glyph.w, c.glyph.h, shown.join("  ")
            );
        }
    }
    let exp_text = r.text;

    if exp_text.is_empty() {
        eprintln!("\nno EXP reading — nothing further to derive");
        std::process::exit(2);
    }

    let mut absolute = ocr::exp_absolute(&exp_text);
    let mut pct = ocr::percent(&exp_text);

    // Where the pixels left a glyph ambiguous, the percentage can still
    // settle it — the same reconciliation the live tracker does.
    if absolute.is_none() || pct.is_none() {
        {
            let level = std::env::args()
                .skip_while(|a| a != "--level")
                .nth(1)
                .and_then(|v| v.parse::<usize>().ok());
            let e = ocr::read_exp(&view, &roi, &profile, level, &table);
            if e.absolute.is_some() {
                println!(
                    "\nreconciled against the percentage: {:?}  (conf {:.1}%)",
                    e.reading.text,
                    e.reading.confidence * 100.0
                );
                absolute = e.absolute;
                pct = e.percent;
            }
        }
    }

    println!("\nparsed  : absolute {:?}  percent {:?}", absolute, pct);

    let (Some(abs), Some(p)) = (absolute, pct) else {
        eprintln!("could not parse both halves of the EXP field");
        std::process::exit(2);
    };

    match expcurve::infer_level(abs, p, &table) {
        Some(level) => {
            let cost = table[level];
            let remaining = cost.saturating_sub(abs);
            println!("\nDERIVED FROM PIXELS ONLY");
            println!("  level        {level}");
            println!("  EXP          {} of {}", fmt(abs), fmt(cost));
            println!("  progress     {:.2}%", abs as f64 / cost as f64 * 100.0);
            println!("  to level {}   {}", level + 1, fmt(remaining));
            println!("\n  (level identified from the EXP-to-percent ratio, not a second region)");
        }
        None => {
            println!("\ncould not identify the level from {abs} at {p}% — ambiguous or 0.00%");
            std::process::exit(3);
        }
    }
}

fn fmt(n: u64) -> String {
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
