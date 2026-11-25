//! Calibrate the chat log and teach it its font.
//!
//! The status-bar atlas is nineteen characters because a status bar is
//! digits. Chat is prose, so this needs most of a keyboard — and guessing
//! which glyph is which from captured pixels is exactly the mistake the rest
//! of this project refuses to make. So the text is supplied: type a line you
//! know into the game's chat, point this at it, and every glyph is taught
//! against a character you are certain of.
//!
//! Usage:
//!   chat-atlas box   <frame.png> <x> <y> <w> <h> [--thr N] [--dark] [--gap N]
//!                    [--space N] [--linegap N]
//!   chat-atlas find  <frame.png> [--thr N] [--dark]
//!   chat-atlas read  <frame.png>
//!   chat-atlas teach <frame.png> <line> "<the characters on that line>"
//!
//! `box` records where the chat is, `find` helps you locate it, `read` shows
//! what the current atlas makes of each line, and `teach` learns the glyphs
//! of one line whose text you have typed yourself.
//!
//! Teaching wants the characters typed **with spaces between them**:
//!
//!     a b c d e f g h i j k l m n o p q r s t u v w x y z
//!     A B C D E F G H I J K L M N O P Q R S T U V W X Y Z
//!     0 1 2 3 4 5 6 7 8 9 + - . , : ! ? ( ) [ ] ' / %
//!
//! Type one of those into the game's chat, capture a frame, and teach it.
//! The spaces are not decoration: in running text the client's letters
//! sometimes touch and sometimes break in two, so which pixels belong to
//! which character is a guess — and a single wrong guess mis-teaches every
//! character after it. A space is a gap the pixels state outright.

use maple_tracker::ocr::{self, Chat, Frame, Profile, Roi};

/// The profile to read and write.
///
/// Overridable so a dry run against a synthetic frame cannot overwrite the
/// profile the app ships with, which is hours of teaching.
fn profile_path() -> String {
    std::env::var("MAPLE_PROFILE").unwrap_or_else(|_| "assets/profile.json".into())
}

fn load() -> Profile {
    let path = profile_path();
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not load {path}: {e}");
            eprintln!("run build-atlas first");
            std::process::exit(1);
        }
    }
}

fn save(profile: &Profile) {
    let path = profile_path();
    match serde_json::to_string_pretty(profile)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string()))
    {
        Ok(()) => println!("wrote   : {path}"),
        Err(e) => {
            eprintln!("could not write {path}: {e}");
            std::process::exit(1);
        }
    }
}

fn open(path: &str) -> image::RgbaImage {
    match image::open(path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("could not open {path}: {e}");
            std::process::exit(1);
        }
    }
}

/// Positional arguments, with flag values excluded.
///
/// A flag's value looks exactly like a positional argument, so filtering on
/// the leading dashes alone silently reads `--thr 100` as the x coordinate.
fn positionals(a: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for (i, s) in a.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        if s.starts_with("--") {
            // Value-taking flags are the ones that are not bare switches.
            skip = !matches!(s.as_str(), "--light");
            continue;
        }
        let _ = i;
        out.push(s);
    }
    out
}

fn flag(a: &[String], name: &str, dflt: u32) -> u32 {
    a.iter()
        .position(|s| s == name)
        .and_then(|i| a.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(dflt)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(|s| s.as_str()) {
        Some("box") => box_mode(&a[1..]),
        Some("find") => find_mode(&a[1..]),
        Some("read") => read_mode(&a[1..]),
        Some("teach") => teach_mode(&a[1..]),
        _ => {
            eprintln!("usage: chat-atlas <box|find|read|teach> ...  (see the top of this file)");
            std::process::exit(2);
        }
    }
}

/// Record where the chat box is.
///
/// The chat plate is light with dark text, the opposite of the status bar,
/// so `--dark` (dark text on a bright plate) is the default here and the
/// flag turns it off.
fn box_mode(a: &[String]) {
    let pos = positionals(a);
    if pos.len() < 5 {
        eprintln!("usage: chat-atlas box <frame.png> <x> <y> <w> <h> [--thr N] [--light]");
        std::process::exit(2);
    }
    let num = |i: usize| pos[i].parse::<u32>().unwrap_or(0);
    let mut profile = load();
    let keep = profile.chat.take();
    let chat = Chat {
        enabled: a.iter().any(|s| s == "--enable"),
        roi: Roi {
            name: "chat".into(),
            x: num(1),
            y: num(2),
            w: num(3),
            h: num(4),
            threshold: flag(a, "--thr", 140) as u8,
            invert: !a.iter().any(|s| s == "--light"),
        },
        min_gap: flag(a, "--gap", 1) as usize,
        space_gap: flag(a, "--space", 4) as usize,
        line_gap: flag(a, "--linegap", 2) as usize,
        min_row_ink: flag(a, "--rowink", 12) as usize,
        adaptive: !a.iter().any(|s| s == "--fixed"),
        min_match: flag(a, "--match", 85) as f32 / 100.0,
        // Teaching is cumulative: moving the box must not throw away the
        // font, which is the expensive part to rebuild.
        atlas: keep.map(|c| c.atlas).unwrap_or_default(),
    };
    println!(
        "chat    : ({}, {}) {}x{}  thr {}{}  gap {} space {} linegap {}",
        chat.roi.x,
        chat.roi.y,
        chat.roi.w,
        chat.roi.h,
        chat.roi.threshold,
        if chat.roi.invert { " dark-on-light" } else { "" },
        chat.min_gap,
        chat.space_gap,
        chat.line_gap
    );
    println!("atlas   : {} characters kept", chat.atlas.len());
    profile.chat = Some(chat);
    save(&profile);
}

/// Where in the frame the lines of text are, for a band you are guessing at.
fn find_mode(a: &[String]) {
    let pos = positionals(a);
    if pos.is_empty() {
        eprintln!("usage: chat-atlas find <frame.png> [x y w h] [--thr N] [--light]");
        std::process::exit(2);
    }
    let img = open(pos[0]);
    let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };
    let num = |i: usize, d: u32| pos.get(i).and_then(|v| v.parse().ok()).unwrap_or(d);
    let roi = Roi {
        name: "band".into(),
        x: num(1, 0),
        y: num(2, 0),
        w: num(3, frame.w),
        h: num(4, frame.h),
        threshold: flag(a, "--thr", 140) as u8,
        invert: !a.iter().any(|s| s == "--light"),
    };
    let gap = flag(a, "--linegap", 2) as usize;
    let rowink = flag(a, "--rowink", 12) as usize;
    println!("band    : ({}, {}) {}x{}  rowink {rowink}", roi.x, roi.y, roi.w, roi.h);
    for (dy, h) in ocr::line_bands(&frame, &roi, gap, rowink) {
        let line = Roi { y: roi.y + dy, h, ..roi.clone() };
        let cuts = ocr::segment(&frame, &line, 1);
        println!(
            "  y {:>5}  h {:>3}  {:>4} glyphs  x {}..{}",
            roi.y + dy,
            h,
            cuts.len(),
            cuts.first().map(|c| c.dx).unwrap_or(0),
            cuts.last().map(|c| c.dx + c.glyph.w).unwrap_or(0)
        );
    }
}

fn read_mode(a: &[String]) {
    let path = a.first().map(|s| s.as_str()).unwrap_or("captures/frame.png");
    let profile = load();
    let Some(chat) = profile.chat.as_ref() else {
        eprintln!("the profile has no chat box yet — run `chat-atlas box` first");
        std::process::exit(1);
    };
    let img = open(path);
    let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };
    // The chat regions are still absolute pixels in the captured frame,
    // unlike the status bar, which is now found by its shape on the client's
    // own grid. Until the chat font is taught that way too, a frame of an
    // unexpected size puts those regions somewhere else.
    if let Some(c) = profile.chat.as_ref() {
        if frame.w < c.roi.x + c.roi.w || frame.h < c.roi.y + c.roi.h {
            println!(
                "WARNING: frame is {}x{}, too small for the taught chat region at ({}, {}) {}x{}",
                frame.w, frame.h, c.roi.x, c.roi.y, c.roi.w, c.roi.h
            );
        }
    }
    let bands = ocr::line_bands(&frame, &chat.roi, chat.line_gap, chat.min_row_ink);
    println!("{} line(s) in the band\n", bands.len());
    for (i, (dy, h)) in bands.iter().enumerate() {
        let line = ocr::line_roi(&frame, &chat.roi, *dy, *h, chat.adaptive);
        let text = ocr::read_line(
            &ocr::line_mask(&frame, &line),
            line.w as usize,
            line.h as usize,
            &chat.atlas,
            chat.space_gap,
            chat.min_match,
        );
        println!("[{i:>2}] {text:?}");
        if a.iter().any(|s| s == "--why") {
            let (_, d) = ocr::read_line_detail(
                &ocr::line_mask(&frame, &line),
                line.w as usize,
                line.h as usize,
                &chat.atlas,
                chat.space_gap,
                chat.min_match,
            );
            let shown: Vec<String> = d
                .iter()
                .map(|(x, w, c, s)| format!("{c}@{x}w{w}:{:.0}", s * 100.0))
                .collect();
            println!("     thr {} | {}", line.threshold, shown.join(" "));
        }
        if let Some(e) = maple_tracker::chat::classify(&text) {
            println!("     -> {e:?}");
        }
    }
}

/// Teach one line's glyphs against text you supplied.
fn teach_mode(a: &[String]) {
    let pos = positionals(a);
    if pos.len() < 3 {
        eprintln!("usage: chat-atlas teach <frame.png> <line> \"<the text on that line>\"");
        std::process::exit(2);
    }
    let img = open(pos[0]);
    let frame = Frame { w: img.width(), h: img.height(), px: img.as_raw() };
    let index: usize = pos[1].parse().unwrap_or(0);
    let truth = pos[2];

    let mut profile = load();
    let Some(chat) = profile.chat.clone() else {
        eprintln!("the profile has no chat box yet — run `chat-atlas box` first");
        std::process::exit(1);
    };
    // `--at y h` names the line's rows outright, for teaching from a frame
    // where finding them automatically is not the thing being tested, and
    // `--from x` skips a prefix the typed text does not include — the
    // client stamps every line with a timestamp and your character name.
    let line = if a.iter().any(|s| s == "--at") {
        let y = flag(a, "--at", chat.roi.y);
        let h = a
            .iter()
            .position(|s| s == "--at")
            .and_then(|i| a.get(i + 2))
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(24);
        Roi { y, h, ..chat.roi.clone() }
    } else {
        let bands = ocr::line_bands(&frame, &chat.roi, chat.line_gap, chat.min_row_ink);
        let Some(&(dy, h)) = bands.get(index) else {
            eprintln!("line {index} is not in the band — it holds {} line(s)", bands.len());
            std::process::exit(1);
        };
        Roi { y: chat.roi.y + dy, h, ..chat.roi.clone() }
    };
    let line = match a.iter().position(|s| s == "--from") {
        Some(_) => {
            let from = flag(a, "--from", line.x);
            Roi { w: line.w.saturating_sub(from.saturating_sub(line.x)), x: from, ..line }
        }
        None => line,
    };
    // `--to x` stops short of whatever else is on the row. The chat window
    // is translucent, so the mouse pointer sitting over it is ink like any
    // other and would be taught as the last character of the line.
    let line = match a.iter().position(|s| s == "--to") {
        Some(_) => {
            let to = flag(a, "--to", line.x + line.w);
            Roi { w: to.saturating_sub(line.x).min(line.w), ..line }
        }
        None => line,
    };
    // Teach at the threshold this line will be read at, or the forms are
    // taught from strokes a different weight than the ones they must match.
    let line = if chat.adaptive {
        let t = ocr::otsu(&frame, &line).max(chat.roi.threshold);
        println!("thresh  : {t} (adaptive)");
        Roi { threshold: t, ..line }
    } else {
        line
    };
    let glyphs = ocr::grouped_glyphs(&frame, &line, chat.min_gap, chat.space_gap);

    // One whitespace-separated token in, one glyph out. A mismatch means the
    // grouping disagrees with what was typed, and teaching on would pair the
    // wrong character with every glyph after the discrepancy.
    let want: Vec<&str> = truth.split_whitespace().collect();
    println!("groups  : {} glyphs, expected {}", glyphs.len(), want.len());
    if glyphs.len() != want.len() {
        print!("widths  :");
        for g in &glyphs {
            print!(" {}x{}", g.w, g.h);
        }
        println!();
        eprintln!("MISMATCH: type the characters with spaces between them, or adjust");
        eprintln!("--space (too small merges two characters, too large splits one).");
        std::process::exit(1);
    }

    let chat = profile.chat.as_mut().expect("checked above");
    let mut learned = 0usize;
    for (glyph, token) in glyphs.iter().zip(want.iter()) {
        // A token is one character. Anything longer was typed as a word and
        // its pixels are several characters stuck together.
        let mut cs = token.chars();
        let (Some(ch), None) = (cs.next(), cs.next()) else {
            eprintln!("{token:?} is not a single character — teach one at a time");
            std::process::exit(1);
        };
        let forms = chat.atlas.entry(ch.to_string()).or_default();
        if forms.iter().any(|f| f.bits == glyph.bits && f.w == glyph.w) {
            continue;
        }
        forms.push(glyph.clone());
        learned += 1;
    }
    let total: usize = chat.atlas.values().map(|v| v.len()).sum();
    println!("learned : {learned} new form(s)");
    println!("atlas   : {} characters, {total} forms", chat.atlas.len());
    save(&profile);
}
