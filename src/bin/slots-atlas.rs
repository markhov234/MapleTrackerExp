//! Calibrate and test the quick slot bar.
//!
//!   slots-atlas find  <frame.png> [...]   where the bar is
//!   slots-atlas icons <frame.png>         write the eight slots as PNGs
//!   slots-atlas count <frame.png>         draw each slot's count
//!   slots-atlas save  <frame.png>         store the settings in the profile
//!
//! Grid tweaks while calibrating: --left --top --pitch --slot --cw --ch
//! --blue --edge.
//!
//! Reads PNG captures and a profile. Touches nothing else.

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Frame, Profile};
use maple_tracker::slots::{self, Slots};
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
            plate: unstretch::plate_rgb(&frame, scale, 0, 0, nw, nh),
        });
    }
    out
}

fn find(shots: &[Shot], cfg: &Slots) {
    for s in shots {
        let f = s.plate.frame();
        let began = std::time::Instant::now();
        let p = slots::find_panel(&f, cfg);
        let took = began.elapsed();
        match p {
            Some(p) => println!(
                "{:<22} client {}x{}  bar at ({:>4},{:>4}) {}x{}  [{:?}]",
                s.name, s.client.0, s.client.1, p.x, p.y, p.w, p.h, took
            ),
            None => println!("{:<22} client {}x{}  no quick slot bar  [{took:?}]",
                             s.name, s.client.0, s.client.1),
        }
    }
}

/// Write every slot out as a PNG, so the grid can be judged by eye rather
/// than argued about in numbers.
fn icons(shots: &[Shot], cfg: &Slots, dir: &str) {
    let Some(s) = shots.first() else { return };
    let f = s.plate.frame();
    let Some(p) = slots::find_panel(&f, cfg) else {
        println!("no quick slot bar in {}", s.name);
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    for i in 0..cfg.cols * cfg.rows {
        let Some(r) = slots::slot_rect(&p, cfg, i) else { continue };
        let mut img = image::RgbaImage::new(r.w, r.h);
        for y in 0..r.h {
            for x in 0..r.w {
                let (cr, cg, cb) = f.rgb(r.x + x, r.y + y);
                img.put_pixel(x, y, image::Rgba([cr, cg, cb, 255]));
            }
        }
        let out = format!("{dir}/slot{i}.png");
        match img.save(&out) {
            Ok(()) => println!("slot {i} at ({:>4},{:>4}) {}x{} -> {out}", r.x, r.y, r.w, r.h),
            Err(e) => eprintln!("could not write {out}: {e}"),
        }
    }
}

fn count(shots: &[Shot], cfg: &Slots) {
    let Some(s) = shots.first() else { return };
    let f = s.plate.frame();
    let Some(p) = slots::find_panel(&f, cfg) else {
        println!("no quick slot bar in {}", s.name);
        return;
    };
    for i in 0..cfg.cols * cfg.rows {
        let Some(r) = slots::count_rect(&p, cfg, i) else { continue };
        let m = slots::count_mask(&f, &r, cfg);
        let ink = m.iter().filter(|v| **v).count();
        println!("\nslot {i}  ({:>4},{:>4}) {}x{}  {ink} ink px", r.x, r.y, r.w, r.h);
        for y in 0..r.h as usize {
            let row: String = (0..r.w as usize)
                .map(|x| if m[y * r.w as usize + x] { '#' } else { '.' })
                .collect();
            println!("   {row}");
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
        eprintln!("usage: slots-atlas <find|icons|count|save> <frame.png> [...]");
        std::process::exit(2);
    }
    let mut cfg = profile.slots.clone().unwrap_or_default();
    let num = |v: Option<String>| v.and_then(|t| t.parse::<u32>().ok());
    if let Some(v) = num(value("--left")) { cfg.left = v; }
    if let Some(v) = num(value("--top")) { cfg.top = v; }
    if let Some(v) = num(value("--pitch")) { cfg.pitch_x = v; cfg.pitch_y = v; }
    if let Some(v) = num(value("--slot")) { cfg.slot = v; }
    if let Some(v) = num(value("--cw")) { cfg.count_w = v; }
    if let Some(v) = num(value("--ch")) { cfg.count_h = v; }
    if let Some(v) = num(value("--blue")) { cfg.count_blue = v.min(255) as u8; }
    if let Some(v) = num(value("--edge")) { cfg.edge = v.min(255) as u8; }

    match mode.as_str() {
        "find" => find(&shots, &cfg),
        "icons" => icons(&shots, &cfg, &value("--out").unwrap_or_else(|| "ui/slots".into())),
        "count" => count(&shots, &cfg),
        "save" => {
            profile.slots = Some(cfg);
            match serde_json::to_string_pretty(&profile)
                .map_err(|e| e.to_string())
                .and_then(|t| std::fs::write(profile_path(), t).map_err(|e| e.to_string()))
            {
                Ok(()) => println!("written to {}", profile_path()),
                Err(e) => eprintln!("could not write: {e}"),
            }
        }
        other => {
            eprintln!("unknown mode {other:?}; expected find, icons, count or save");
            std::process::exit(2);
        }
    }
}
