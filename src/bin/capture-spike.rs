//! Phase 1 spike: find the game window and capture one frame to PNG.
//!
//! Boundaries this deliberately respects, and must keep respecting:
//!   - no process memory access (no OpenProcess / ReadProcessMemory)
//!   - no input synthesis (no SendInput / keybd_event)
//!   - no network or packet inspection
//!   - no reads of game files
//! It uses Windows.Graphics.Capture, the same compositor-level API behind OBS
//! and Discord screen share, and nothing else.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::encoder::ImageFormat;
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

/// Window titles to look for, in priority order. Title matching rather than
/// process name: the game's anticheat blocks process queries from an
/// unelevated caller, so `process_name()` returns an error for its window.
const TITLE_CANDIDATES: &[&str] = &["MapleStory", "MapleStory Classic"];

/// How long to wait for the compositor to deliver a frame before giving up.
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// What the capture thread reports back to main.
enum Report {
    Saved { path: PathBuf, width: u32, height: u32, elapsed_ms: u128 },
    Failed(String),
}

struct Spike {
    tx: Sender<Report>,
    out: PathBuf,
    start: Instant,
}

impl GraphicsCaptureApiHandler for Spike {
    type Flags = (Sender<Report>, PathBuf);
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (tx, out) = ctx.flags;
        Ok(Self { tx, out, start: Instant::now() })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // One frame is all the spike needs.
        let buffer = frame.buffer()?;
        let (width, height) = (buffer.width(), buffer.height());
        drop(buffer);

        let report = match frame.save_as_image(&self.out, ImageFormat::Png) {
            Ok(()) => Report::Saved {
                path: self.out.clone(),
                width,
                height,
                elapsed_ms: self.start.elapsed().as_millis(),
            },
            Err(e) => Report::Failed(format!("frame arrived but saving failed: {e}")),
        };
        let _ = self.tx.send(report);

        control.stop();
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        let _ = self.tx.send(Report::Failed("capture item closed".into()));
        Ok(())
    }
}

fn find_window() -> Option<(Window, String)> {
    // An explicit title argument wins, so the tool can be pointed at any
    // window (including our own overlay) rather than only the game.
    let arg = std::env::args().nth(1);
    let owned: Vec<&str> = match arg.as_deref() {
        Some(t) => vec![t],
        None => TITLE_CANDIDATES.to_vec(),
    };
    for title in owned {
        if let Ok(w) = Window::from_contains_name(title) {
            if w.is_valid() {
                let actual = w.title().unwrap_or_else(|_| (*title).to_string());
                return Some((w, actual));
            }
        }
    }
    None
}

fn main() {
    let Some((window, title)) = find_window() else {
        eprintln!("No game window found. Looked for: {}", TITLE_CANDIDATES.join(", "));
        eprintln!("Start the client, then re-run.");
        std::process::exit(1);
    };

    let width = window.width().unwrap_or(0);
    let height = window.height().unwrap_or(0);
    println!("window : \"{title}\"  {width}x{height}");
    match window.process_name() {
        Ok(p) => println!("process: {p}"),
        Err(e) => println!("process: unavailable ({e}) - expected, anticheat blocks the query"),
    }

    let out_dir = PathBuf::from("captures");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("could not create {}: {e}", out_dir.display());
        std::process::exit(1);
    }
    // Optional second argument names the output file, so capturing another
    // window does not clobber the saved game frame.
    let out = out_dir.join(std::env::args().nth(2).unwrap_or_else(|| "frame.png".into()));

    let (tx, rx) = mpsc::channel::<Report>();

    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        (tx, out.clone()),
    );

    // `start` takes over its thread, so run it on a worker and bound the wait.
    // Without this, a capture the compositor never services would hang forever.
    std::thread::spawn(move || {
        if let Err(e) = Spike::start(settings) {
            eprintln!("capture session error: {e}");
        }
    });

    println!("waiting up to {}s for a frame...", FRAME_TIMEOUT.as_secs());
    match rx.recv_timeout(FRAME_TIMEOUT) {
        Ok(Report::Saved { path, width, height, elapsed_ms }) => {
            println!("\nCAPTURE OK");
            println!("  frame  : {width}x{height}");
            println!("  arrived: {elapsed_ms} ms");
            println!("  saved  : {}", path.display());
            println!("\n-> load this PNG into the calibrator to cut the digit atlas");
        }
        Ok(Report::Failed(why)) => {
            eprintln!("\nCAPTURE FAILED: {why}");
            std::process::exit(1);
        }
        Err(RecvTimeoutError::Timeout) => {
            eprintln!("\nCAPTURE FAILED: no frame in {}s", FRAME_TIMEOUT.as_secs());
            eprintln!("The window enumerated but delivered no frames.");
            std::process::exit(1);
        }
        Err(RecvTimeoutError::Disconnected) => {
            eprintln!("\nCAPTURE FAILED: capture thread stopped before sending a frame");
            std::process::exit(1);
        }
    }
}
