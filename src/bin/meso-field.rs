//! Check the meso plate reader against a captured frame.
//!
//!   meso-field check <frame.png> [...]   list every plate and what it reads
//!   meso-field show  <frame.png>         draw the plates found, as ink maps
//!   meso-field teach <frame.png> --total 983,602
//!                                        cut the inventory's own digits
//!   meso-field save  <frame.png>         turn the reader on in the profile
//!
//! Tweaks while checking: --plate --ink --min-w --min-h --max-h --gap --from-y
//!
//! Reads PNG captures and a profile. Touches nothing else.

use maple_tracker::expcurve;
use maple_tracker::mesos::{self, Mesos};
use maple_tracker::ocr::{self, Frame, Profile};
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

struct Shot {
    name: String,
    client: (u32, u32),
    plate: Plate,
}

/// Every capture, resampled onto the client's own grid — the meso digits
/// are the same bitmap font as the status bar, and only exact on that grid.
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
        out.push(Shot {
            name,
            client: (nw, nh),
            // Colour-preserving: the plate is found by how bright all three
            // channels are, not by luma.
            plate: unstretch::plate_rgb(&frame, scale, 0, 0, nw, nh),
        });
    }
    out
}

fn check(shots: &[Shot], cfg: &Mesos) {
    for s in shots {
        let f = s.plate.frame();
        let began = std::time::Instant::now();
        let all = mesos::plates(&f, cfg);
        let good = mesos::candidates(&f, cfg);
        let took = began.elapsed();
        println!(
            "\n=== {} — client {}x{} — {} plate(s), {} readable  [{:?}]",
            s.name,
            s.client.0,
            s.client.1,
            all.len(),
            good.len(),
            took
        );
        for pl in &all {
            let hit = good.iter().find(|(q, _)| *q == *pl);
            match hit {
                Some((_, v)) => println!(
                    "   ({:>4},{:>4}) {:>4}x{:<3}  {}",
                    pl.x,
                    pl.y,
                    pl.w,
                    pl.h,
                    fmt(*v)
                ),
                None => println!(
                    "   ({:>4},{:>4}) {:>4}x{:<3}  — nothing meso-shaped on it",
                    pl.x, pl.y, pl.w, pl.h
                ),
            }
        }
    }
}

fn fmt(v: u64) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Draw what the reader sees on each plate, so a misread can be judged by
/// eye rather than argued about in numbers.
fn show(shots: &[Shot], cfg: &Mesos) {
    let Some(s) = shots.first() else { return };
    let f = s.plate.frame();
    for pl in mesos::plates(&f, cfg) {
        println!("\nplate ({}, {}) {}x{}  ('#' = ink)", pl.x, pl.y, pl.w, pl.h);
        let roi = maple_tracker::ocr::Roi {
            name: "mesos".into(),
            x: pl.x, y: pl.y, w: pl.w, h: pl.h,
            threshold: cfg.ink, invert: true,
        };
        let cuts = ocr::segment(&f, &roi, cfg.min_gap);
        let mut line = String::new();
        for c in &cuts {
            let r = ocr::classify_cuts(std::slice::from_ref(c), &cfg.atlas, None);
            if r.unknown > 0 {
                line.push_str(&format!("[?{}x{}]", c.glyph.w, c.glyph.h));
            } else {
                line.push_str(&r.text);
            }
        }
        println!("  cuts {}: {}", cuts.len(), line);
        if std::env::args().any(|a| a == "--cuts") {
            for c in &cuts {
                println!("   cut dx={} {}x{}", c.dx, c.glyph.w, c.glyph.h);
                for gy in 0..c.glyph.h {
                    let row: String = (0..c.glyph.w)
                        .map(|gx| if c.glyph.on_at(gx, gy) { '#' } else { '.' })
                        .collect();
                    println!("     {row}");
                }
            }
        }
        for y in pl.y..(pl.y + pl.h).min(f.h) {
            let row: String = (pl.x..(pl.x + pl.w).min(f.w))
                .map(|x| if f.grey(x, y) < cfg.ink { '#' } else { '.' })
                .collect();
            println!("  {row}");
        }
    }
}

fn write(profile: &mut Profile, cfg: Mesos) {
    profile.mesos = Some(cfg);
    match serde_json::to_string_pretty(profile)
        .map_err(|e| e.to_string())
        .and_then(|t| std::fs::write(profile_path(), t).map_err(|e| e.to_string()))
    {
        Ok(()) => println!("written to {}", profile_path()),
        Err(e) => eprintln!("could not write: {e}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_default();
    let value =
        |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mut profile = load();
    let shots = shots(&args, &profile);
    if shots.is_empty() {
        eprintln!("usage: meso-field <check|show|save> <frame.png> [...]");
        std::process::exit(2);
    }
    let mut cfg = profile.mesos.clone().unwrap_or_default();
    let num = |v: Option<String>| v.and_then(|t| t.parse::<u32>().ok());
    if let Some(v) = num(value("--plate")) {
        cfg.plate = v.min(255) as u8;
    }
    if let Some(v) = num(value("--ink")) {
        cfg.ink = v.min(255) as u8;
    }
    if let Some(v) = num(value("--min-w")) {
        cfg.min_w = v;
    }
    if let Some(v) = num(value("--min-h")) {
        cfg.min_h = v;
    }
    if let Some(v) = num(value("--max-h")) {
        cfg.max_h = v;
    }
    if let Some(v) = num(value("--gap")) {
        cfg.ink_gap = v;
    }
    if let Some(v) = value("--from-y").and_then(|t| t.parse::<f32>().ok()) {
        cfg.from_y = v;
    }

    match mode.as_str() {
        "check" => check(&shots, &cfg),
        "show" => show(&shots, &cfg),
        "teach" => {
            let Some(total) = value("--total") else {
                eprintln!("teach needs --total <the number on screen, commas and all>");
                std::process::exit(2);
            };
            for s in &shots {
                match mesos::teach(&s.plate.frame(), &cfg, &total) {
                    Ok(atlas) => {
                        let before = cfg.atlas.len();
                        cfg.atlas = atlas;
                        println!(
                            "{}: taught {} new, {} of 11 known",
                            s.name,
                            cfg.atlas.len() - before,
                            cfg.atlas.len()
                        );
                    }
                    Err(e) => println!("{}: {e}", s.name),
                }
            }
            let left = cfg.untaught();
            if left.is_empty() {
                println!("all eleven known — `meso-field save` will switch the reader on");
            } else {
                let list: Vec<String> = left.iter().map(|c| format!("{c:?}")).collect();
                println!("still missing: {}", list.join(" "));
            }
            write(&mut profile, cfg);
        }
        "save" => {
            let left = cfg.untaught();
            if !left.is_empty() {
                let list: Vec<String> = left.iter().map(|c| format!("{c:?}")).collect();
                eprintln!(
                    "refusing to switch it on: {} not taught yet — a total holding one of                      those would read as nothing at all",
                    list.join(" ")
                );
                std::process::exit(1);
            }
            cfg.enabled = true;
            write(&mut profile, cfg);
            println!("reader enabled");
        }
        other => {
            eprintln!("unknown mode {other:?}; expected check, show, teach or save");
            std::process::exit(2);
        }
    }
}
