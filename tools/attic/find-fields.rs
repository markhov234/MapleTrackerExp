//! Locate text fields inside a band of the frame.
//!
//! Calibration otherwise means eyeballing coordinates off a scaled
//! screenshot, which is slow and wrong. This segments a band twice: once with
//! a wide gap so each run is a whole field, then once per field with a normal
//! gap to count its glyphs. The output is ready to paste into a profile.
//!
//! Usage:
//!   find-fields <frame.png> <x> <y> <w> <h> [--thr N] [--invert] [--gap N]

use maple_tracker::ocr::{segment, Frame, Roi};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 5 {
        eprintln!("usage: find-fields <frame.png> <x> <y> <w> <h> [--thr N] [--invert] [--gap N]");
        std::process::exit(2);
    }
    let path = &a[0];
    let num = |i: usize| a[i].parse::<u32>().unwrap_or(0);
    let (x, y, w, h) = (num(1), num(2), num(3), num(4));

    let flag = |name: &str, dflt: u32| -> u32 {
        a.iter()
            .position(|s| s == name)
            .and_then(|i| a.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(dflt)
    };
    let thr = flag("--thr", 150) as u8;
    let field_gap = flag("--gap", 12) as usize;
    let invert = a.iter().any(|s| s == "--invert");

    let img = match image::open(path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {path}: {e}");
            std::process::exit(1);
        }
    };
    let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };

    let band = Roi {
        name: "band".into(),
        x,
        y,
        w,
        h,
        threshold: thr,
        invert,
    };

    println!(
        "band    : ({x}, {y}) {w}x{h}  thr {thr}{}  field gap {field_gap}",
        if invert { " inverted" } else { "" }
    );

    // Pass one: wide gap, so each run is a whole field rather than a glyph.
    let fields = segment(&frame, &band, field_gap);
    if fields.is_empty() {
        println!("no ink in this band");
        return;
    }
    println!("\n{} field(s)\n", fields.len());
    println!("{:>4}  {:<28}  {:>6}  {}", "#", "ROI (x, y, w, h)", "GLYPHS", "widths");
    println!("{}", "-".repeat(78));

    for (i, f) in fields.iter().enumerate() {
        // The cut is trimmed vertically, so recover its absolute box.
        let fx = x + f.dx as u32;
        let fw = f.glyph.w as u32;

        // Pass two: normal gap inside this field, to count its glyphs.
        let inner = Roi {
            name: format!("f{i}"),
            x: fx,
            y,
            w: fw,
            h,
            threshold: thr,
            invert,
        };
        let glyphs = segment(&frame, &inner, 1);

        // Tighten vertically across the whole field.
        let (mut top, mut bot) = (u32::MAX, 0u32);
        for g in &glyphs {
            let gy = y; // segment already trims each glyph; recompute from mask
            let _ = gy;
        }
        // Simple vertical trim over the field box.
        for row in 0..h {
            let mut any = false;
            for col in 0..fw {
                let l = {
                    let px = fx + col;
                    let py = y + row;
                    if px >= frame.w || py >= frame.h {
                        0
                    } else {
                        let i = ((py * frame.w + px) * 4) as usize;
                        let (r, g, b) = (
                            frame.px[i] as f32,
                            frame.px[i + 1] as f32,
                            frame.px[i + 2] as f32,
                        );
                        (r * 0.299 + g * 0.587 + b * 0.114) as u8
                    }
                };
                let lit = if invert { l < thr } else { l >= thr };
                if lit {
                    any = true;
                    break;
                }
            }
            if any {
                top = top.min(row);
                bot = bot.max(row);
            }
        }
        let (fy, fh) = if top == u32::MAX {
            (y, h)
        } else {
            (y + top, bot - top + 1)
        };

        let widths: Vec<String> = glyphs
            .iter()
            .map(|g| format!("{}x{}", g.glyph.w, g.glyph.h))
            .collect();
        println!(
            "{:>4}  ({:>4}, {:>4}, {:>3}, {:>2})         {:>6}  {}",
            i,
            fx,
            fy,
            fw,
            fh,
            glyphs.len(),
            widths.join(" ")
        );
    }
    println!(
        "\nPaste a row's box into build-atlas as a Roi, with what it actually reads."
    );
}
