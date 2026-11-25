//! Locate the player's character sprite in a captured frame, automatically.
//!
//! The trick is that the client renders the character's name twice in the same
//! bitmap font at the same size: once on the bottom status bar, and once on the
//! label directly beneath the sprite. So no letter atlas and no text
//! recognition is needed — the name as drawn on the status bar IS the search
//! template. Find where else that exact bitmap appears, and the sprite is the
//! box sitting directly above it.
//!
//! Pipeline:
//!   1. binarize a generous slice of the status bar, trim to the ink
//!   2. slide that template over the frame, skipping the bar it came from
//!   3. crop the box above each strong match
//!   4. flood-fill the background away from the borders inward
//!   5. keep the candidate that actually looks like a sprite
//!
//! Step 5 matters because the name can be drawn elsewhere: in chat, in a buddy
//! list, in another player's message, on a shop sign. Those matches score just
//! as well on text, so the text score alone cannot pick the right one. What
//! separates them is what sits above the tag: a sprite keys down to a
//! mid-sized, many-coloured island, whereas UI chrome keys to nearly nothing,
//! to nearly everything, or to one flat hue.
//!
//! Everything here reads pixels. Nothing touches the game.
//!
//! Usage: find-character [frame.png] [out.png]
//!        find-character --at X Y        score the template at one position
//!        find-character --selftest      regression check against captures/
use image::RgbaImage;
use maple_tracker::avatar::{
    below, binarize, judge, side_at, key_sprite, locate, probes, side_ink, sprite_score, trim, Fail, Geom, Sprite,
    View, MAX_HUE_DOMINANCE, MAX_KEPT_FRAC, MAX_SIDE_INK, MIN_COLOURS, MIN_KEPT_FRAC, PLATE, REF_H,
    REF_W,
};

/// The pipeline itself lives in `maple_tracker::avatar`, because the overlay
/// runs it too — at launch and every few hours, to keep the panel's portrait
/// the character you are actually playing. This binary is the workbench over
/// it: one frame from disk, every score printed, and a regression check.
fn view(img: &RgbaImage) -> View<'_> {
    View { w: img.width(), h: img.height(), px: img.as_raw() }
}

/// Back into something `image` can write out.
fn to_png(s: &Sprite) -> RgbaImage {
    RgbaImage::from_raw(s.w, s.h, s.rgba.clone()).expect("sprite buffer matches its dimensions")
}

// ---------------------------------------------------------------------------
// Self-test
// ---------------------------------------------------------------------------

/// Regression check against the committed test frame. Fails loudly, with a
/// non-zero exit, if a retuned constant breaks detection.
///
/// `captures/frame.png` is 2560x1440, a level 40 Bandit.
fn selftest() -> i32 {
    const FRAME: &str = "captures/frame.png";
    let img = match image::open(FRAME) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("selftest FAIL: could not open {FRAME}: {e}");
            eprintln!("  (run it from the crate root, where captures/ lives)");
            return 1;
        }
    };
    assert_frame_dims(&img);

    let found = match locate(&view(&img), |m| println!("{m}")) {
        Ok(f) => f,
        Err(e) => {
            let what = match &e {
                Fail::Layout(m) => format!("layout rejected: {m}"),
                Fail::NoTag { best } => format!("no name tag found (best {best:?})"),
                Fail::NotASprite(r) => format!("no plausible sprite: {r:?}"),
            };
            eprintln!("selftest FAIL: {what}");
            return 1;
        }
    };

    let mut bad: Vec<String> = Vec::new();
    let mut check = |ok: bool, msg: String| {
        if !ok {
            bad.push(msg);
        }
    };

    let (ex, ey) = (799u32, 542u32);
    let (w, h) = (found.best.keyed.sprite.w, found.best.keyed.sprite.h);
    let kept = found.best.keyed.kept;
    let score = found.best.text;

    check(
        found.distinct == 1,
        format!("expected exactly 1 distinct match, got {}", found.distinct),
    );
    check(
        found.best.x.abs_diff(ex) <= 2 && found.best.y.abs_diff(ey) <= 2,
        format!(
            "expected tag within 2 px of ({ex}, {ey}), got ({}, {})",
            found.best.x, found.best.y
        ),
    );
    check(score > 0.90, format!("expected text score > 0.90, got {score:.4}"));
    check(
        w.abs_diff(124) <= 6 && h.abs_diff(143) <= 6,
        format!("expected sprite near 124x143, got {w}x{h}"),
    );
    check(kept > 8000, format!("expected more than 8000 kept px, got {kept}"));

    // The plausibility checks must not be sitting on top of the real answer.
    let k = &found.best.keyed;
    check(
        k.kept_frac > MIN_KEPT_FRAC * 2.0 && k.kept_frac < MAX_KEPT_FRAC - 0.1,
        format!(
            "kept fraction {:.3} is uncomfortably close to a plausibility bound \
             ({MIN_KEPT_FRAC}..{MAX_KEPT_FRAC})",
            k.kept_frac
        ),
    );
    check(
        k.colours >= MIN_COLOURS * 2,
        format!("colour count {} leaves no margin over MIN_COLOURS {MIN_COLOURS}", k.colours),
    );
    check(
        k.hue_dom < MAX_HUE_DOMINANCE - 0.1,
        format!("hue dominance {:.2} leaves no margin under {MAX_HUE_DOMINANCE}", k.hue_dom),
    );
    check(
        found.best.side < MAX_SIDE_INK / 2.0,
        format!(
            "side ink {:.3} leaves no margin under {MAX_SIDE_INK} — the real tag \
             should be standing alone on its plate",
            found.best.side
        ),
    );

    // Derived geometry must reproduce the hand-measured reference exactly.
    let g = Geom::derive(REF_W, REF_H);
    check(
        g.name == (684, 1407, 200, 24),
        format!("reference name box drifted: {:?}", g.name),
    );
    check(
        (g.ui_floor, g.sprite_w, g.sprite_h, g.sprite_gap) == (1360, 124, 150, 8),
        format!(
            "reference sprite geometry drifted: {:?}",
            (g.ui_floor, g.sprite_w, g.sprite_h, g.sprite_gap)
        ),
    );
    // And it must scale, not just hit the reference.
    let half = Geom::derive(REF_W / 2, REF_H / 2);
    check(
        half.sprite_w == 62 && half.name.0 == 640 - 298 && half.name.1 == 720 - 17,
        format!("half-scale geometry looks wrong: {:?} {}", half.name, half.sprite_w),
    );

    if bad.is_empty() {
        println!("\nselftest OK — tag ({}, {}) {:.2}%, sprite {w}x{h}, {kept} px", found.best.x, found.best.y, score * 100.0);
        0
    } else {
        eprintln!("\nselftest FAILED: {} problem(s)", bad.len());
        for b in &bad {
            eprintln!("  - {b}");
        }
        1
    }
}

fn assert_frame_dims(img: &RgbaImage) {
    let (w, h) = img.dimensions();
    if (w, h) != (REF_W, REF_H) {
        eprintln!("selftest note: test frame is {w}x{h}, expected {REF_W}x{REF_H}");
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// Score the template at one position, bypassing the probe early-reject, to
/// separate "template is wrong" from "probes are too strict".
fn diagnose_at(img: &RgbaImage, ax: usize, ay: usize) {
    let v = view(img);
    let (fw, fh) = img.dimensions();
    let g = Geom::derive(fw, fh);
    let (nx, ny, nw, nh) = g.name;
    let raw = binarize(&v, nx, ny, nw, nh);
    let Some((tmpl, _, _)) = trim(&raw) else {
        eprintln!("no ink in the name box");
        return;
    };
    let frame = binarize(&v, 0, 0, fw, g.ui_floor.min(fh));
    let plate = below(&v, 0, 0, fw, g.ui_floor.min(fh), PLATE);
    if ax + tmpl.w > frame.w || ay + tmpl.h > frame.h {
        eprintln!("({ax}, {ay}) + template {}x{} falls outside the search area", tmpl.w, tmpl.h);
        return;
    }
    let mut agree = 0usize;
    let mut on_hit = 0usize;
    let mut on_total = 0usize;
    for ty in 0..tmpl.h {
        for tx in 0..tmpl.w {
            let f = frame.at(ax + tx, ay + ty);
            let t = tmpl.at(tx, ty);
            if f == t {
                agree += 1;
            }
            if t {
                on_total += 1;
                if f {
                    on_hit += 1;
                }
            }
        }
    }
    let total = (tmpl.w * tmpl.h) as f32;
    println!(
        "score at ({ax}, {ay}): overall {:.1}%  ink-recall {:.1}% ({on_hit}/{on_total})",
        agree as f32 / total * 100.0,
        on_hit as f32 / on_total as f32 * 100.0
    );
    let pr = probes(&tmpl);
    let bad = pr
        .iter()
        .filter(|&&(px, py, want)| frame.at(ax + px, ay + py) != want)
        .count();
    println!("probes failing here: {bad} of {}", pr.len());

    let side = side_at(&frame, &plate, ax, ay, &tmpl, &g);
    println!("side ink beside the tag: {side:.3}");
    let profile: Vec<String> = [2u32, 4, 8, 16, 24, 32, 48]
        .iter()
        .map(|&r| {
            format!(
                "{r}:{:.2}",
                side_ink(&frame, &plate, ax, ay, tmpl.w, tmpl.h, g.sprite_gap as usize, r as usize)
            )
        })
        .collect();
    println!("  by reach: {}", profile.join("  "));

    if std::env::args().any(|a| a == "--map") {
        println!("  ink map of the tag band ('#' = bright, '.' = dark):");
        let x_from = ax.saturating_sub(60);
        let x_to = (ax + tmpl.w + 60).min(frame.w);
        for y in ay..(ay + tmpl.h).min(frame.h) {
            let row: String = (x_from..x_to)
                .map(|x| if frame.at(x, y) { '#' } else { '.' })
                .collect();
            println!("  {row}");
        }
    }

    if let Some((x0, y0, w, h)) = g.sprite_box(fw, ax as u32, ay as u32, tmpl.w as u32) {
        match key_sprite(&v, x0, y0, w, h) {
            None => println!("sprite box ({x0}, {y0}) {w}x{h}: keys out entirely"),
            Some(k) => println!(
                "sprite box ({x0}, {y0}) {w}x{h}: kept {} ({:.0}%)  colours {}  \
                 hue-dom {:.2}  sprite-score {:.3}  verdict {}",
                k.kept,
                k.kept_frac * 100.0,
                k.colours,
                k.hue_dom,
                sprite_score(&k, side),
                match judge(&k, side) {
                    Ok(()) => "plausible".to_string(),
                    Err(w) => format!("rejected — {w}"),
                }
            ),
        }
    } else {
        println!("no room for a sprite box above ({ax}, {ay})");
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.iter().any(|a| a == "--selftest") {
        std::process::exit(selftest());
    }

    let positional: Vec<&String> = argv.iter().filter(|a| !a.starts_with("--")).collect();
    let at = argv.iter().position(|a| a == "--at");
    // `--at X Y`: the two numbers are not input paths.
    let skip_tail = if at.is_some() { 2 } else { 0 };
    let paths: Vec<&String> = positional[..positional.len().saturating_sub(skip_tail)].to_vec();

    let frame_path: String = paths.first().map(|s| s.to_string()).unwrap_or_else(|| "captures/frame.png".into());
    // Not `ui/`. The panel's placeholder portrait lives there and is
    // tracked, and this writes a cut of the player's real character over
    // whatever it is pointed at — which is how a live sprite once ended up
    // staged for commit. `captures/` is gitignored for exactly this.
    let out_path: String =
        paths.get(1).map(|s| s.to_string()).unwrap_or_else(|| "captures/character.png".into());

    let img = match image::open(&frame_path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {frame_path}: {e}");
            std::process::exit(1);
        }
    };
    // `--native` puts the workbench on the same grid the overlay searches,
    // using the overlay's own resampler rather than an imitation of it.
    // Without this the two disagree about what the frame even looks like,
    // and the workbench's verdict stops meaning anything.
    let img = if argv.iter().any(|a| a == "--native") {
        let (w, h) = img.dimensions();
        let profile: maple_tracker::ocr::Profile =
            serde_json::from_str(&std::fs::read_to_string("assets/profile.json").unwrap())
                .expect("the profile parses");
        let raw = img.into_raw();
        let frame = maple_tracker::ocr::Frame { w, h, px: &raw };
        let scale = profile.scale(w, h);
        let (nw, nh) = scale.native(w, h);
        let plate = maple_tracker::unstretch::plate_rgb(&frame, scale, 0, 0, nw, nh);
        println!("native  : {w}x{h} -> {}x{}", plate.w, plate.h);
        RgbaImage::from_raw(plate.w, plate.h, plate.pixels().to_vec())
            .expect("the plate is rgba")
    } else {
        img
    };
    let (fw, fh) = img.dimensions();
    println!("frame   : {frame_path}  {fw}x{fh}");

    if let Some(i) = at {
        let (Some(ax), Some(ay)) = (
            argv.get(i + 1).and_then(|v| v.parse::<usize>().ok()),
            argv.get(i + 2).and_then(|v| v.parse::<usize>().ok()),
        ) else {
            eprintln!("--at needs two integers: --at X Y");
            std::process::exit(1);
        };
        diagnose_at(&img, ax, ay);
        return;
    }

    let found = match locate(&view(&img), |m| println!("{m}")) {
        Ok(f) => f,
        Err(e) => {
            match &e {
                Fail::Layout(m) => eprintln!("\n{m}"),
                Fail::NoTag { best } => {
                    eprintln!("\nNo name tag found. The character may be off-screen or hidden.");
                    if let Some((x, y, s)) = best {
                        eprintln!("Best near-miss: ({x}, {y}) at {:.1}%.", s * 100.0);
                    }
                    eprintln!("Fall back to the generic avatar and retry on a later frame.");
                }
                Fail::NotASprite(r) => {
                    eprintln!("\nFound the name, but nothing above it looks like a character:");
                    for (x, y, why) in r {
                        eprintln!("  ({x}, {y}): {why}");
                    }
                    eprintln!("The name was probably drawn in chat or a list, not under a sprite.");
                }
            }
            std::process::exit(e.code());
        }
    };

    for (x, y, why) in &found.rejected {
        println!("rejected: ({x}, {y}) — {why}");
    }
    for (x, y, text, sprite) in &found.runners_up {
        println!(
            "also ok : ({x}, {y}) text {:.1}% sprite-score {:.3}",
            text * 100.0,
            sprite
        );
    }

    let c = &found.best;
    let (tw, th) = (c.keyed.sprite.w, c.keyed.sprite.h);
    println!(
        "\ntag     : ({}, {}) at {:.1}%  sprite-score {:.3}",
        c.x,
        c.y,
        c.text * 100.0,
        c.sprite
    );

    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = to_png(&c.keyed.sprite).save(&out_path) {
        eprintln!("could not write {out_path}: {e}");
        std::process::exit(1);
    }
    println!("kept    : {} px, trimmed to {tw}x{th}\nwrote   : {out_path}", c.keyed.kept);
    // One machine-readable line, for the self-test and for callers.
    println!(
        "result  : tag={},{} score={:.4} distinct={} sprite={tw}x{th} kept={}",
        c.x, c.y, c.text, found.distinct, c.keyed.kept
    );
}
