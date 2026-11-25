//! Find the EXP readout in a frame without being told where it is.
//!
//! The check on [`maple_tracker::ocr::find_exp_field`]: point it at any
//! capture and it should come back with the same numbers the status bar is
//! showing, whatever the window size or wherever the client has put its
//! furniture.
//!
//! Usage: find-exp <frame.png> [more.png ...] [--whole] [--thr N]
//!
//! By default it searches the bottom fifth of the frame, which is where
//! every client of this game puts the bar and is twenty times less work.
//! `--whole` searches the entire frame.

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Frame, Profile, Roi};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let whole = args.iter().any(|a| a == "--whole");
    let thr = args
        .iter()
        .position(|a| a == "--thr")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<u8>().ok());
    let frames: Vec<&String> = args
        .iter()
        .filter(|a| !a.starts_with("--") && a.ends_with(".png"))
        .collect();
    if frames.is_empty() {
        eprintln!("usage: find-exp <frame.png> [...] [--whole] [--thr N]");
        std::process::exit(2);
    }

    let profile: Profile = match std::fs::read_to_string("assets/profile.json")
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not load assets/profile.json: {e}");
            std::process::exit(1);
        }
    };
    let table = expcurve::to_next();
    let hint = profile.roi("exp").cloned();

    for path in frames {
        let img = match image::open(path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                println!("{path:<28} could not open: {e}");
                continue;
            }
        };
        let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };
        let band = Roi {
            name: "search".into(),
            x: 0,
            y: if whole { 0 } else { frame.h * 4 / 5 },
            w: frame.w,
            h: if whole { frame.h } else { frame.h / 5 },
            threshold: thr.or(hint.as_ref().map(|r| r.threshold)).unwrap_or(140),
            invert: false,
        };

        if args.iter().any(|a| a == "--why") {
            for (dy, h) in ocr::line_bands(&frame, &band, 2, 4) {
                let row = Roi { y: band.y + dy, h, ..band.clone() };
                let cuts = ocr::segment(&frame, &row, profile.min_gap);
                let t = ocr::classify_cuts(&cuts, &profile.atlas, None).text;
                if cuts.len() >= 4 {
                    println!("  y {:>5} h {:>3} cuts {:>4}  {t:?}", row.y, h, cuts.len());
                }
            }
        }

        let began = std::time::Instant::now();
        let found = ocr::find_exp_field(&frame, &profile, &table, &band);
        let took = began.elapsed();

        match found {
            Some(roi) => {
                let exp = ocr::read_exp(&frame, &roi, &profile, None, &table);
                println!(
                    "{path:<28} ({:>4},{:>4}) {:>3}x{:<3}  {:<22} level {:?}  [{:?}]",
                    roi.x,
                    roi.y,
                    roi.w,
                    roi.h,
                    exp.reading.text,
                    exp.level,
                    took
                );
            }
            None => println!("{path:<28} not found  [{took:?}]"),
        }
    }
}
