//! Teach the minimap's title, and read the region's colour from its badge.
//!
//! The badge beside "MINI MAP" belongs to the region rather than the map, so
//! it is what the overlay should take its colours from. Finding it starts
//! from the title, which is one fixed template and answers two questions at
//! once: where the panel is, and whether it is open at all.
//!
//!   minimap-atlas show  <frame.png> [...]   the title bar, on the client grid
//!   minimap-atlas teach <frame.png> [...] --at X Y W H
//!                                          store that rectangle as the title
//!   minimap-atlas check <frame.png> [...]   find it, and report the colour
//!
//! Reads PNG captures and a profile. Touches nothing else.

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Frame, Glyph, Profile};
use maple_tracker::region::{self, Minimap};
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

struct Shot {
    name: String,
    client: (u32, u32),
    plate: Plate,
}

/// Every capture, resampled onto the client's own grid.
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
        // Colour-preserving: the title is matched on brightness and the
        // badge is read for its hue, so this plate has to carry both.
        out.push(Shot {
            name,
            client: (nw, nh),
            plate: unstretch::plate_rgb(&frame, scale, 0, 0, nw, nh),
        });
    }
    out
}

fn render(f: &Frame, x0: u32, y0: u32, w: u32, h: u32, thr: u8) {
    for y in y0..(y0 + h).min(f.h) {
        let row: String = (x0..(x0 + w).min(f.w))
            .map(|x| if f.grey(x, y) < thr { '#' } else { '.' })
            .collect();
        println!("  {y:>3} {row}");
    }
}

fn show(shots: &[Shot], cfg: &Minimap) {
    for s in shots {
        let f = s.plate.frame();
        println!(
            "\n=== {} — client {}x{}, title ink below grey {} ===",
            s.name, s.client.0, s.client.1, cfg.threshold
        );
        render(&f, 0, 0, 70, 24, cfg.threshold);
    }
}

fn teach(shots: &[Shot], cfg: &mut Minimap, at: (u32, u32, u32, u32)) {
    let (x, y, w, h) = at;
    let mut added = 0usize;
    for s in shots {
        let f = s.plate.frame();
        let mut bits = String::with_capacity((w * h) as usize);
        for j in 0..h {
            for i in 0..w {
                bits.push(if f.grey(x + i, y + j) < cfg.threshold { '1' } else { '0' });
            }
        }
        let g = Glyph { w: w as usize, h: h as usize, bits };
        let ink = g.bits.bytes().filter(|b| *b == b'1').count();
        if ink * 6 < (w * h) as usize {
            println!("{:<22} barely any ink in that rectangle — skipped", s.name);
            continue;
        }
        if cfg.anchor.iter().any(|f| f.bits == g.bits) {
            println!("{:<22} same as a form already taught", s.name);
            continue;
        }
        for row in 0..h as usize {
            let line: String = (0..w as usize)
                .map(|c| if g.on_at(c, row) { '#' } else { '.' })
                .collect();
            println!("      {line}");
        }
        cfg.anchor.push(g);
        added += 1;
        println!("{:<22} taught ({ink} ink pixels)", s.name);
    }
    println!("\n{added} new form(s); the title now has {}", cfg.anchor.len());
}

fn check(shots: &[Shot], cfg: &Minimap) {
    if cfg.anchor.is_empty() {
        eprintln!("the title has not been taught yet; run `show` then `teach --at X Y W H`");
        std::process::exit(1);
    }
    for s in shots {
        let f = s.plate.frame();
        let began = std::time::Instant::now();
        let found = region::find_anchor(&f, cfg);
        let r = region::region(&f, cfg);
        let took = began.elapsed();
        match (found, r) {
            (Some((x, y)), Some(r)) => println!(
                "{:<22} title at ({x:>3},{y:>3})   hue {:6.1}deg   chroma {:5.1}   {:4} px  [{:?}]",
                s.name, r.hue, r.chroma, r.pixels, took
            ),
            (Some((x, y)), None) => println!(
                "{:<22} title at ({x:>3},{y:>3})   badge not colourful enough  [{took:?}]",
                s.name
            ),
            _ => println!("{:<22} no minimap open  [{took:?}]", s.name),
        }
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
        eprintln!("usage: minimap-atlas <show|teach|check> <frame.png> [...]");
        std::process::exit(2);
    }
    let mut cfg = profile.minimap.clone().unwrap_or_default();
    if let Some(v) = value("--grey").and_then(|v| v.parse().ok()) {
        cfg.threshold = v;
    }
    for (name, slot) in [("--dx", 0), ("--dy", 1), ("--w", 2), ("--h", 3)] {
        if let Some(v) = value(name).and_then(|v| v.parse::<i32>().ok()) {
            match slot {
                0 => cfg.dx = v,
                1 => cfg.dy = v,
                2 => cfg.w = v.max(1) as u32,
                _ => cfg.h = v.max(1) as u32,
            }
        }
    }

    match mode.as_str() {
        "show" => show(&shots, &cfg),
        "check" => check(&shots, &cfg),
        "teach" => {
            let at = args.iter().position(|a| a == "--at").and_then(|i| {
                Some((
                    args.get(i + 1)?.parse().ok()?,
                    args.get(i + 2)?.parse().ok()?,
                    args.get(i + 3)?.parse().ok()?,
                    args.get(i + 4)?.parse().ok()?,
                ))
            });
            let Some(at) = at else {
                eprintln!("teach needs --at X Y W H, the title's rectangle on the client grid");
                std::process::exit(2);
            };
            teach(&shots, &mut cfg, at);
            profile.minimap = Some(cfg);
            save(&profile);
        }
        other => {
            eprintln!("unknown mode {other:?}; expected show, teach or check");
            std::process::exit(2);
        }
    }
}
