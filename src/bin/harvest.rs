//! Collect the frames a reader still needs to be taught from.
//!
//! Teaching a font needs frames that actually hold the text, and the text
//! this program is still missing — the floating EXP and meso lines, the
//! counts on the quick slot bar — only appears while somebody is playing.
//! Asking for that on demand is a poor trade: the player has to stop doing
//! what they were doing, and the sample ends up narrow anyway, all of it
//! from one map at one moment with one set of numbers.
//!
//! So this watches instead. It keeps a frame only when the frame is worth
//! keeping — when the band in question has something in it that was not
//! there a moment ago — and ignores the rest. Play normally for a while and
//! it leaves behind a varied set: different amounts, different maps,
//! different backgrounds.
//!
//!   harvest notices [seconds] [--every MS] [--max N]
//!
//! Frames land in `captures/harvest/`.
//!
//! Reads pixels through the same compositor capture as the tracker, and
//! nothing else: no process memory, no input, no network, no game files.

use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Profile};
use maple_tracker::unstretch;
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame as CapFrame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

const PROFILE_JSON: &str = include_str!("../../assets/profile.json");
const DIR: &str = "captures/harvest";

struct Shot {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

struct Harvester {
    profile: Profile,
    table: Vec<u64>,
    tx: Sender<Shot>,
    start: Instant,
    last: Instant,
    every: Duration,
    until: Duration,
    kept: usize,
    max: usize,
    grid: Option<unstretch::Scale>,
    /// What the watched band looked like when a frame was last kept, so the
    /// same screen is not saved a hundred times over.
    mark: Vec<u8>,
}

/// A coarse signature of the band being watched.
///
/// Coarse on purpose: the question is "has this changed enough to be worth
/// another frame", not "is it identical". Averaging blocks of it throws
/// away the shimmer and keeps the shape.
fn signature(f: &ocr::Frame) -> Vec<u8> {
    let (bw, bh) = (16u32, 8u32);
    let mut out = Vec::with_capacity((bw * bh) as usize);
    for by in 0..bh {
        for bx in 0..bw {
            let (x0, x1) = (bx * f.w / bw, (bx + 1) * f.w / bw);
            let (y0, y1) = (by * f.h / bh, (by + 1) * f.h / bh);
            let mut sum = 0u32;
            let mut n = 0u32;
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    sum += f.grey(x, y) as u32;
                    n += 1;
                }
            }
            out.push(if n > 0 { (sum / n) as u8 } else { 0 });
        }
    }
    out
}

fn moved(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return true;
    }
    a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 6).count() * 20 > a.len()
}

impl GraphicsCaptureApiHandler for Harvester {
    type Flags = (Profile, Sender<Shot>, Duration, Duration, usize);
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (profile, tx, every, until, max) = ctx.flags;
        let now = Instant::now();
        Ok(Harvester {
            profile,
            table: expcurve::to_next(),
            tx,
            start: now,
            last: now - every,
            every,
            until,
            kept: 0,
            max,
            grid: None,
            mark: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut CapFrame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let now = Instant::now();
        if now.duration_since(self.start) > self.until || self.kept >= self.max {
            control.stop();
            return Ok(());
        }
        if now.duration_since(self.last) < self.every {
            return Ok(());
        }
        self.last = now;

        let mut scratch: Vec<u8> = Vec::new();
        let buffer = frame.buffer()?;
        let (w, h) = (buffer.width(), buffer.height());
        let px = buffer.as_nopadding_buffer(&mut scratch);
        let view = ocr::Frame { w, h, px };

        let scale = match self.grid {
            Some(s) => s,
            None => match ocr::locate(&view, &self.profile, &self.table, &[]) {
                Some((_, _, s)) => {
                    eprintln!("[harvest] client grid found, stretched by {:.4}", s.sx);
                    self.grid = Some(s);
                    s
                }
                None => return Ok(()),
            },
        };
        let (nw, nh) = scale.native(w, h);

        // Cut the band this run is interested in, and ask whether it has
        // anything new in it.
        let Some(cfg) = self.profile.notices.as_ref() else { return Ok(()) };
        let r = cfg.band(nw, nh);
        let band = unstretch::plate_with(&view, scale, r.x, r.y, r.w, r.h, unstretch::warm);
        if band.w == 0 || band.h == 0 {
            return Ok(());
        }
        let bf = band.frame();

        // Something has to be lit at all, or this is an empty band and a
        // frame of nothing.
        let ink = (0..bf.h)
            .step_by(2)
            .flat_map(|y| (0..bf.w).step_by(2).map(move |x| (x, y)))
            .filter(|(x, y)| bf.grey(*x, *y) >= 90)
            .count();
        if ink < 40 {
            return Ok(());
        }

        let sig = signature(&bf);
        if !moved(&sig, &self.mark) {
            return Ok(());
        }
        self.mark = sig;
        self.kept += 1;
        let _ = self.tx.send(Shot { w, h, px: px.to_vec() });
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) != Some("notices") {
        eprintln!("usage: harvest notices [seconds] [--every MS] [--max N]");
        std::process::exit(2);
    }
    let value = |name: &str| {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)?.parse::<u64>().ok())
    };
    let secs = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(300u64);
    let every = Duration::from_millis(value("--every").unwrap_or(700));
    let max = value("--max").unwrap_or(60) as usize;

    let profile: Profile = match serde_json::from_str(PROFILE_JSON) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("bundled profile is invalid: {e}");
            std::process::exit(1);
        }
    };
    let Ok(windows) = Window::enumerate() else {
        eprintln!("could not enumerate windows");
        std::process::exit(1);
    };
    let Some(window) = windows
        .into_iter()
        .find(|w| w.is_valid() && w.title().map(|t| t == "MapleStory").unwrap_or(false))
    else {
        eprintln!("MapleStory is not running");
        std::process::exit(1);
    };

    let _ = std::fs::create_dir_all(DIR);
    let (tx, rx) = mpsc::channel::<Shot>();
    let writer = std::thread::spawn(move || {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut n = 0usize;
        for s in rx {
            n += 1;
            let path = format!("{DIR}/{stamp}-{n:03}.png");
            match image::RgbaImage::from_raw(s.w, s.h, s.px) {
                Some(img) => match img.save(&path) {
                    Ok(()) => println!("kept {path}"),
                    Err(e) => eprintln!("could not write {path}: {e}"),
                },
                None => eprintln!("frame {n} was the wrong size to save"),
            }
        }
        println!("\n{n} frame(s) in {DIR}");
    });

    eprintln!("[harvest] watching for up to {secs}s — play normally");
    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        (profile, tx, every, Duration::from_secs(secs), max),
    );
    if let Err(e) = Harvester::start(settings) {
        eprintln!("capture ended: {e}");
    }
    let _ = writer.join();
}
