//! Log the EXP number as fast as the capture allows, for a while.
//!
//! The question this answers: does the status bar move in steps you can
//! count? If a kill is worth a fixed amount, the gaps between readings are
//! multiples of it, and the number of kills follows from the total without
//! reading a word of chat. If the amounts vary — a party splitting EXP by
//! damage, several kinds of monster on the map — the gaps are a smear and
//! no count can be recovered from them.
//!
//! It is a measurement tool, not part of the app. Output is a CSV of
//! (seconds since start, absolute EXP) on stdout.
//!
//! Usage: sample-exp [seconds]
//!
//! Reads pixels through the same compositor capture as the tracker, and
//! nothing else: no process memory, no input, no network, no game files.

use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use maple_tracker::expcurve;
use maple_tracker::ocr::{self, Profile};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame as CapFrame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

const PROFILE_JSON: &str = include_str!("../../assets/profile.json");

/// Fast enough that two kills rarely land in one gap, which is the whole
/// point: a gap holding two kills looks like a monster worth twice as much.
const EVERY: Duration = Duration::from_millis(120);

struct Sampler {
    profile: Profile,
    table: Vec<u64>,
    tx: Sender<(f64, u64)>,
    start: Instant,
    last: Instant,
    until: Duration,
    seen: u64,
    field: Option<ocr::Roi>,
    /// The scaling that read the bar, kept so every later frame skips the
    /// search for it.
    grid: Option<maple_tracker::unstretch::Scale>,
}

impl GraphicsCaptureApiHandler for Sampler {
    type Flags = (Profile, Sender<(f64, u64)>, Duration);
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (profile, tx, until) = ctx.flags;
        let now = Instant::now();
        Ok(Sampler {
            profile,
            table: expcurve::to_next(),
            tx,
            start: now,
            last: now - EVERY,
            until,
            seen: 0,
            field: None,
            grid: None,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut CapFrame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let now = Instant::now();
        if now.duration_since(self.start) > self.until {
            control.stop();
            return Ok(());
        }
        if now.duration_since(self.last) < EVERY {
            return Ok(());
        }
        self.last = now;

        let mut scratch: Vec<u8> = Vec::new();
        let buffer = frame.buffer()?;
        let (w, h) = (buffer.width(), buffer.height());
        let px = buffer.as_nopadding_buffer(&mut scratch);
        let view = ocr::Frame { w, h, px };
        // Find the readout the same way the tracker does, so the sampler is
        // not the one tool that still believes the bar cannot move.
        if self.field.is_none() {
            if let Some((plate, roi, scale)) =
                ocr::locate(&view, &self.profile, &self.table, &[])
            {
                let (x, y) = plate.to_native(roi.x, roi.y);
                eprintln!(
                    "[sample] EXP field at ({x}, {y}) {}x{} on the client's grid",
                    roi.w, roi.h
                );
                self.grid = Some(scale);
                self.field = Some(ocr::Roi { x, y, ..roi });
            } else if now.duration_since(self.start) > Duration::from_secs(5) {
                eprintln!("[sample] no EXP bar in this {w}x{h} window");
                control.stop();
                return Ok(());
            }
        }
        let Some(roi) = self.field.clone() else { return Ok(()) };
        let scale = self.grid.unwrap_or_else(|| self.profile.scale(w, h));
        let plate = maple_tracker::unstretch::plate(
            &view,
            scale,
            roi.x.saturating_sub(1),
            roi.y.saturating_sub(1),
            roi.w + 2,
            roi.h + 2,
        );
        let local = ocr::Roi { x: roi.x - plate.x, y: roi.y - plate.y, ..roi.clone() };
        let exp = ocr::read_exp(&plate.frame(), &local, &self.profile, None, &self.table);
        self.seen += 1;
        if let Some(a) = exp.absolute {
            let _ = self.tx.send((now.duration_since(self.start).as_secs_f64(), a));
        }
        Ok(())
    }
}

fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(60);
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

    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        println!("t,exp");
        let mut last: Option<u64> = None;
        for (t, exp) in rx {
            if last != Some(exp) {
                println!("{t:.2},{exp}");
                last = Some(exp);
            }
        }
    });

    eprintln!("sampling for {secs}s...");
    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        (profile, tx, Duration::from_secs(secs)),
    );
    if let Err(e) = Sampler::start(settings) {
        eprintln!("capture ended: {e}");
    }
    let _ = reader.join();
}
