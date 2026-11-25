//! The live loop: capture the game window, read the status bar, push the
//! session state to the overlay.
//!
//! Runs on its own thread and survives the game not being there — if the
//! window is missing it reports that and keeps looking, so starting the
//! tracker before the game is a normal thing to do.
//!
//! Frames arrive at display rate but are only read a couple of times a
//! second. That is deliberate and sufficient: the matcher costs microseconds,
//! but EXP resolution does not improve with sampling rate, and 2 Hz was
//! verified to recover per-kill quanta just as well as 10 Hz.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame as CapFrame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use crate::chat;
use crate::expcurve;
use crate::history::History;
use crate::ocr::{self, Profile};
use crate::session::Session;

/// Window titles to look for, in priority order. Title rather than process
/// name: the game's anticheat refuses process queries from an unelevated
/// caller, though it does not affect compositor-level capture.
const TITLES: &[&str] = &["MapleStory Classic", "MapleStory"];

/// How often the status bar is actually read.
const READ_EVERY: Duration = Duration::from_millis(450);

/// How long to wait before looking for the game again.
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// How often the in-progress session is written to disk.
///
/// Often enough that a crash or a kill costs seconds rather than an evening,
/// rarely enough that it is not a write per frame. The checkpoint is a rename
/// over a temporary file, so an interrupted one leaves the previous checkpoint
/// rather than a truncated line.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(30);

/// The event the overlay listens for.
pub const EVENT: &str = "session";

struct Reader {
    app: AppHandle,
    profile: Arc<Profile>,
    table: Vec<u64>,
    /// Shared with the app, not owned here. The capture handler is rebuilt
    /// every time the game window comes and goes, and a session that reset
    /// when the client restarted would lose the evening's total to a
    /// channel change.
    session: Arc<Mutex<Session>>,
    history: Arc<History>,
    /// Set once a frame arrives at the size the profile was taught on, which
    /// is how the search above learns it picked the right window.
    matched: Arc<AtomicBool>,
    /// The scrollback as it looked last read, so a line is counted once.
    log: chat::Log,
    /// The same, for the lines the client floats over the game world. Kept
    /// apart from the chat log because they are two different lists that
    /// scroll independently; sharing one would make each look like the
    /// other had jumped.
    notices: chat::Log,
    /// Where the EXP readout actually is, once found.
    ///
    /// Not where the profile says: the bar moves when the window is
    /// resized, when the client is run at another resolution, and it was
    /// never in the same place on somebody else's machine. The profile's
    /// region is a hint for the threshold and a first guess at the search
    /// area, nothing more.
    field: Option<ocr::Roi>,
    /// The scaling that last produced a reading, tried first next time.
    ///
    /// Not simply the one the profile recorded. That one is a good guess —
    /// it is what this client was doing when the font was taught — but a
    /// client told to render at another resolution is doing something else,
    /// and the font is the same bitmap font either way. So the recorded
    /// scaling is the first guess, the rest are tried behind it, and
    /// whichever reads is the one kept.
    grid: Option<crate::unstretch::Scale>,
    /// The frame size last seen, so a resized window starts looking again
    /// instead of reading where the bar used to be.
    last_size: (u32, u32),
    /// When the field was last looked for, so a screen without a status bar
    /// does not turn into a search every frame.
    last_search: Instant,
    last_checkpoint: Instant,
    reads: u64,
    /// Whether the last frame produced a number, so the log records the
    /// moment it stopped and the moment it came back rather than every
    /// frame in between.
    was_ok: bool,
    last_trouble: Option<Instant>,
    /// When the readout last produced a number, so a momentarily empty
    /// region is ridden out rather than announced.
    last_good: Option<Instant>,
    last_region: Instant,
    /// The last badge hue written to the log, so a stable region is not
    /// announced every couple of seconds.
    last_hue: Option<f32>,
}

/// How long a readout that has been working is given to come back before the
/// panel admits it cannot read the bar.
const GRACE: Duration = Duration::from_secs(2);

/// How often an unreadable bar is worth mentioning again while it stays
/// unreadable.
const TROUBLE_EVERY: Duration = Duration::from_secs(30);

/// How often the field is looked for while it cannot be read.
///
/// A whole-frame search costs about a tenth of a second. Once a second is
/// often enough to pick the bar up when the player opens a window over it
/// or the client rearranges itself, and rare enough that a frame with no
/// status bar in it — a loading screen, the world map — costs nothing much.
const SEARCH_EVERY: Duration = Duration::from_secs(1);

/// How long a window gets to show an EXP bar before the search moves on.
///
/// Only used when the client's own grid is known, where any window size is
/// worth a look and so something has to decide when a look has failed. Long
/// enough to survive a loading screen, short enough that cycling through a
/// handful of candidate windows is quick.
const GIVE_UP: Duration = Duration::from_secs(4);

/// How often to look at the minimap's badge.
///
/// Finding the panel is a search over the whole corner and costs about
/// eighteen milliseconds. You cross a region boundary a handful of times an
/// hour, so a couple of seconds is far more often than the answer changes.
const REGION_EVERY: Duration = Duration::from_secs(2);

impl Reader {
    /// Send a snapshot to the overlay.
    fn emit(&mut self, snap: crate::session::Snapshot) {
        if let Err(e) = self.app.emit(EVENT, snap) {
            crate::note!("[tracker] emit failed: {e}");
        }
    }

    /// The scalings worth trying for a frame of this size, likeliest first.
    fn grids(&self, w: u32, h: u32) -> Vec<crate::unstretch::Scale> {
        let mut out: Vec<crate::unstretch::Scale> = Vec::new();
        let mut add = |s: crate::unstretch::Scale| {
            if !out.contains(&s) {
                out.push(s);
            }
        };
        if let Some(s) = self.grid {
            add(s);
        }
        add(self.profile.scale(w, h));
        for s in crate::unstretch::candidates(w, h) {
            add(s);
        }
        out
    }

    /// Find the readout, and remember the scaling that worked.
    ///
    /// Nothing here is calibrated to this machine: [`ocr::locate`] tries
    /// scalings until one yields a readout whose number divides into the
    /// percentage printed beside it, and no wrong scaling can fake that. So
    /// the window may be any size, on any monitor, and the profile reads it.
    fn find_field(&mut self, view: &ocr::Frame) -> Option<ocr::Roi> {
        let grids = self.grids(view.w, view.h);
        let (plate, roi, scale) = ocr::locate(view, &self.profile, &self.table, &grids)?;
        if self.grid != Some(scale) {
            let (nw, nh) = scale.native(view.w, view.h);
            crate::note!(
                "[tracker] {}x{} window is the client's {nw}x{nh} stretched by {:.4}",
                view.w, view.h, scale.sx
            );
            self.grid = Some(scale);
        }
        let (x, y) = plate.to_native(roi.x, roi.y);
        Some(ocr::Roi { x, y, ..roi })
    }

    /// Read a region found earlier, on the grid it was found on.
    ///
    /// Only the field itself is resampled here, a hundred pixels by ten, so
    /// the per-frame cost of working on the client's grid is negligible —
    /// the expensive resampling belongs to the search, which is rare.
    fn read_field(
        &self,
        view: &ocr::Frame,
        roi: &ocr::Roi,
        known_level: Option<usize>,
    ) -> ocr::ExpReading {
        let scale = self.grid.unwrap_or_else(|| self.profile.scale(view.w, view.h));
        let plate = crate::unstretch::plate(
            view,
            scale,
            roi.x.saturating_sub(1),
            roi.y.saturating_sub(1),
            roi.w + 2,
            roi.h + 2,
        );
        let local = ocr::Roi { x: roi.x - plate.x, y: roi.y - plate.y, ..roi.clone() };
        ocr::read_exp(&plate.frame(), &local, &self.profile, known_level, &self.table)
    }
}

impl Reader {
    fn new(
        app: AppHandle,
        profile: Arc<Profile>,
        session: Arc<Mutex<Session>>,
        history: Arc<History>,
        matched: Arc<AtomicBool>,
    ) -> Reader {
        Reader {
            app,
            profile,
            table: expcurve::to_next(),
            session,
            history,
            matched,
            log: chat::Log::new(),
            notices: chat::Log::new(),
            field: None,
            grid: None,
            last_size: (0, 0),
            last_search: Instant::now() - SEARCH_EVERY,
            last_checkpoint: Instant::now(),
            reads: 0,
            was_ok: true,
            last_trouble: None,
            last_good: None,
            last_region: Instant::now() - REGION_EVERY,
            last_hue: None,
        }
    }

    /// Read one frame. Called on the worker thread, never on the capture
    /// callback: everything in here is tens to hundreds of milliseconds of
    /// pixel work, and doing it in the callback holds a frame the Windows
    /// compositor wants back.
    fn step(&mut self, w: u32, h: u32, px: &[u8]) {
        let now = Instant::now();
        let view = ocr::Frame { w, h, px };

        // A resized window redraws its status bar somewhere else, and may
        // redraw it at another internal resolution too, so both the region
        // and the scaling have to be found again rather than carried over.
        if self.last_size != (w, h) {
            if self.last_size != (0, 0) {
                crate::note!("[tracker] window is now {w}x{h}; looking for the EXP bar again");
                self.field = None;
                self.grid = None;
            }
            self.last_size = (w, h);
        }

        // Which region the player is in, taken from the badge beside the
        // minimap's title. The badge belongs to the region rather than the
        // map, so it changes when you cross into Kerning City and not when
        // you walk through a door — and being the artists' own colour for
        // the place, it is the theme rather than a key into a table of
        // themes somebody had to write.
        if let Some(cfg) = self.profile.minimap.as_ref().filter(|m| m.enabled) {
            if now.duration_since(self.last_region) >= REGION_EVERY {
                self.last_region = now;
                let scale = self.grid.unwrap_or_else(|| self.profile.scale(w, h));
                let corner = crate::unstretch::plate_rgb(
                    &view,
                    scale,
                    0,
                    0,
                    cfg.search_w + 100,
                    cfg.search_h + 60,
                );
                if corner.w > 0 && corner.h > 0 {
                    let open = crate::region::find_anchor(&corner.frame(), cfg).is_some();
                    self.session.lock().unwrap().observe_minimap(open);
                    if let Some(r) = crate::region::region(&corner.frame(), cfg) {
                        self.session.lock().unwrap().observe_region(r.hue);
                        if self.last_hue.map(|h: f32| (h - r.hue).abs() > 8.0).unwrap_or(true) {
                            self.last_hue = Some(r.hue);
                            crate::note!(
                                "[tracker] region badge reads {:.0} degrees (chroma {:.0}, {} px)",
                                r.hue,
                                r.chroma,
                                r.pixels
                            );
                        }
                    }
                }
            }
        }

        // The floating lines, low and to the right, which are the only place
        // kills and mesos appear. Read on the client's own grid like
        // everything else, and only over the band they occupy — a couple of
        // hundred pixels by a hundred, so the resampling is nothing.
        if let Some(cfg) = self.profile.notices.as_ref().filter(|c| c.enabled) {
            let scale = self.grid.unwrap_or_else(|| self.profile.scale(w, h));
            let (nw, nh) = scale.native(w, h);
            let band = cfg.band(nw, nh);
            let plate = crate::unstretch::plate_with(
                &view,
                scale,
                band.x,
                band.y,
                band.w,
                band.h,
                crate::unstretch::warm,
            );
            if plate.w > 0 && plate.h > 0 {
                let local = ocr::Roi { x: 0, y: 0, w: plate.w, h: plate.h, ..band };
                let lines = ocr::read_rows(&plate.frame(), &local, &cfg.rows());
                let events = self.notices.recognised(lines);
                if !events.is_empty() {
                    self.session.lock().unwrap().observe_chat(&events);
                }
            }
        }

        if let Some(cfg) = self.profile.chat.as_ref().filter(|c| c.enabled) {
            let events = self.log.events(ocr::read_chat(&view, cfg));
            if !events.is_empty() {
                self.session.lock().unwrap().observe_chat(&events);
            }
        }

        if self.profile.atlas.is_empty() {
            let snap = self.session.lock().unwrap().snapshot("profile has no font");
            self.emit(snap);
            return;
        }

        // The level the session already believes it is on turns the
        // percentage into a check on the absolute number, which is what lets
        // the reader settle a glyph the pixels leave ambiguous.
        let known_level = self.session.lock().unwrap().level();

        // Read where the field was last seen; go looking only when that
        // fails. Searching every frame would cost a tenth of a second each
        // time for an answer that almost never changes.
        let mut exp = self
            .field
            .as_ref()
            .map(|r| self.read_field(&view, r, known_level));
        if exp.as_ref().map(|e| e.absolute.is_none()).unwrap_or(true)
            && now.duration_since(self.last_search) >= SEARCH_EVERY
        {
            self.last_search = now;
            if let Some(found) = self.find_field(&view) {
                if self.field.as_ref().map(|r| (r.x, r.y, r.w)) != Some((found.x, found.y, found.w)) {
                    crate::note!(
                        "[tracker] EXP field at ({}, {}) {}x{}",
                        found.x, found.y, found.w, found.h
                    );
                }
                self.matched.store(true, Ordering::Relaxed);
                exp = Some(self.read_field(&view, &found, known_level));
                self.field = Some(found);
            }
        }
        let Some(exp) = exp else {
            let snap = self
                .session
                .lock()
                .unwrap()
                .snapshot("looking for the EXP bar\u{2026}");
            self.emit(snap);
            return;
        };
        // Always set by here: a reading only exists where a region was.
        let roi = self.field.clone().unwrap_or_else(|| self.profile.over("exp", 0, 0));
        let roi = &roi;
        let reading = exp.reading.clone();
        let absolute = exp.absolute;

        // What goes in the log, now that there is a log rather than a
        // terminal somebody is watching.
        //
        // The reader runs several times a second, so a status bar with a
        // window over it would write thousands of identical lines. Only
        // changes are worth recording: the read that stopped working, and
        // the one that started again. Between them, one line every so often
        // is enough to show the thing is still alive and still stuck.
        self.reads += 1;
        let ok = absolute.is_some();
        if ok {
            self.last_good = Some(now);
        }
        // A frame or two with nothing in the region at all is not a problem
        // worth reporting. Watching this run live, the readout comes back
        // empty — no glyphs, not a misread — for about two hundred
        // milliseconds every several seconds, and reads perfectly either
        // side of it. Something in the client redraws that strip. Saying
        // "status bar not readable" each time would flicker the panel and
        // fill the log over something the next frame fixes.
        let blip = !ok
            && reading.glyphs == 0
            && self.last_good.map(|t| now.duration_since(t) < GRACE).unwrap_or(false);
        if blip {
            let snap = self.session.lock().unwrap().snapshot("tracking");
            self.emit(snap);
            return;
        }
        let stale = self
            .last_trouble
            .map(|t| now.duration_since(t) >= TROUBLE_EVERY)
            .unwrap_or(true);
        if !ok && (self.was_ok || stale) {
            self.last_trouble = Some(now);
            crate::note!(
                "[tracker] {w}x{h} readout at ({},{}) came back {:?} — conf {:.1}%, {} unknown of {} glyphs",
                roi.x,
                roi.y,
                reading.text,
                reading.confidence * 100.0,
                reading.unknown,
                reading.glyphs
            );
        } else if ok && !self.was_ok {
            self.last_trouble = None;
            crate::note!("[tracker] readout is legible again: {:?}", reading.text);
        }
        self.was_ok = ok;

        let status = match absolute {
            Some(a) => {
                let inferred = exp.level;
                let mut session = self.session.lock().unwrap();
                session.observe(inferred, a, reading.confidence, now);
                let measuring = session.snapshot("").measuring;
                drop(session);
                if reading.confidence < 0.9 && !exp.reconciled {
                    "low confidence — recalibrate"
                } else if measuring {
                    "measuring your pace\u{2026}"
                } else {
                    "tracking"
                }
            }
            // Distinguish "a digit the atlas has never been taught" from
            // "nothing legible there", because only the first is fixable by
            // teaching and the user needs to be told which it is.
            None if reading.unknown > 0 => "unreadable digit — teach the atlas",
            // An unreadable bar is otherwise normal: a window can cover it.
            None => "status bar not readable",
        };

        // Checkpoint before emitting: if this process is killed the next
        // instant, the session on disk should be the one just measured.
        if now.duration_since(self.last_checkpoint) >= CHECKPOINT_EVERY {
            self.last_checkpoint = now;
            let rec = self.session.lock().unwrap().record();
            self.history.checkpoint(&rec);
        }

        let snap = self.session.lock().unwrap().snapshot(status);
        self.emit(snap);
    }

    /// The game window went away — a client exit, a crash, or a channel
    /// change. The session is not over, so it is checkpointed rather than
    /// filed: capture restarts as soon as the window is back and the totals
    /// carry straight on.
    fn closed(&mut self) {
        let rec = self.session.lock().unwrap().record();
        self.history.checkpoint(&rec);
        let snap = self.session.lock().unwrap().snapshot("game window closed");
        self.emit(snap);
    }
}

/// One frame, copied out of the compositor's buffer so it can be let go of.
struct Shot {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// The capture callback, and deliberately nothing else.
///
/// Windows Graphics Capture calls `on_frame_arrived` on its own thread while
/// holding a frame that belongs to the desktop compositor, and it wants it
/// back. Reading the status bar is tens of milliseconds on a good frame and
/// a few hundred on a frame that has no status bar in it, because then every
/// candidate grid gets swept. Spending that in the callback stalls the
/// compositor's queue, and what the user sees is their whole desktop
/// hitching for a second or two whenever this program starts up and walks
/// through candidate windows.
///
/// So the callback does the least it can: copy the pixels, hand them to a
/// worker, return. A frame that arrives while the worker is still busy is
/// dropped rather than queued — the reader samples twice a second by
/// design, and a stale frame is worth nothing.
struct Capture {
    tx: std::sync::mpsc::SyncSender<Shot>,
    /// Set by the reader once it has actually read an EXP bar here.
    matched: Arc<AtomicBool>,
    started: Instant,
    last_read: Instant,
    warned: bool,
    dropped: u64,
}

impl GraphicsCaptureApiHandler for Capture {
    type Flags = (
        AppHandle,
        Arc<Profile>,
        Arc<Mutex<Session>>,
        Arc<History>,
        Arc<AtomicBool>,
    );
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (app, profile, session, history, matched) = ctx.flags;
        // Capacity one: there is never a reason to hold two frames, and a
        // bounded channel is what makes dropping the policy rather than
        // growing a queue until memory runs out.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Shot>(1);
        let mut reader = Reader::new(app, profile, session, history, Arc::clone(&matched));
        std::thread::spawn(move || {
            for shot in rx {
                reader.step(shot.w, shot.h, &shot.px);
            }
            // The channel closed, which means the capture ended: the window
            // was closed, changed channel, or was the wrong window.
            reader.closed();
        });
        Ok(Capture {
            tx,
            matched,
            started: Instant::now(),
            last_read: Instant::now() - READ_EVERY,
            warned: false,
            dropped: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut CapFrame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let now = Instant::now();

        // Deciding whether this is really the game window.
        //
        // Matching the frame against a size the profile was taught on used
        // to be the test, because the regions were absolute pixels and a
        // different size meant they pointed at the wrong place. It was never
        // a good test of what the window *was*, only a cheap one, and it
        // failed the common case outright: resize the client and the tracker
        // stopped seeing the game.
        //
        // There is a far better test now. Is there a region shaped like an
        // EXP readout, and does the number in it divide into the percentage
        // printed beside it? Size is a coincidence; that is a fact about the
        // window, and nothing but the game's status bar produces it. So any
        // size is accepted, and the window that gets dropped is the one that
        // has not produced a reading within a few seconds.
        if !self.matched.load(Ordering::Relaxed) && now.duration_since(self.started) > GIVE_UP {
            if !self.warned {
                self.warned = true;
                crate::note!(
                    "[tracker] no EXP bar in this window after {GIVE_UP:?} — trying the next one"
                );
            }
            control.stop();
            return Ok(());
        }

        if now.duration_since(self.last_read) < READ_EVERY {
            return Ok(());
        }
        // Claim the slot before the copy, not after. Otherwise a worker that
        // is still busy makes every single frame pay for a full-frame memcpy
        // just to be refused.
        self.last_read = now;

        let mut scratch: Vec<u8> = Vec::new();
        let buffer = frame.buffer()?;
        let (w, h) = (buffer.width(), buffer.height());
        let px = buffer.as_nopadding_buffer(&mut scratch);
        match self.tx.try_send(Shot { w, h, px: px.to_vec() }) {
            Ok(()) => {}
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                self.dropped += 1;
                if self.dropped % 40 == 1 {
                    crate::note!(
                        "[tracker] reader is behind; {} frame(s) dropped so far",
                        self.dropped
                    );
                }
            }
            // The worker is gone, so there is nothing to capture for.
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => control.stop(),
        }
        Ok(())
    }

    /// Nothing to do here: dropping the sender ends the worker's loop, and
    /// the worker checkpoints the session itself.
    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Every window that might be the game, best first.
///
/// Title matching cannot settle this on its own. A browser tab called
/// "… | MapleStory Classic World - YouTube" matches "MapleStory Classic"
/// better than the client, which is titled plainly "MapleStory" — and that is
/// not a hypothetical, it is what this app captured until the capture itself
/// was made to check. So the title only ranks the candidates: exact names
/// first, substrings after, and the frame size decides which one is real.
///
/// Windows belonging to this process are skipped. The overlay's own title is
/// "EXP Calc", but it renders pages about MapleStory and has matched before.
fn candidate_windows() -> Vec<(u32, Window, String)> {
    let Ok(windows) = Window::enumerate() else { return Vec::new() };
    let mine = std::process::id();
    let mut ranked: Vec<(u32, Window, String)> = Vec::new();
    for w in windows {
        if !w.is_valid() {
            continue;
        }
        if w.process_id().map(|p| p == mine).unwrap_or(false) {
            continue;
        }
        let Ok(title) = w.title() else { continue };
        let lower = title.to_ascii_lowercase();
        let rank = TITLES
            .iter()
            .position(|p| title.eq_ignore_ascii_case(p))
            .map(|i| i as u32)
            .or_else(|| {
                TITLES
                    .iter()
                    .position(|p| lower.contains(&p.to_ascii_lowercase()))
                    .map(|i| 100 + i as u32)
            });
        let Some(rank) = rank else { continue };
        // Too small to be a game client, so not worth a capture session.
        //
        // Every candidate costs a D3D11 device, a capture session, and up
        // to GIVE_UP seconds of frames before it is abandoned. Starting and
        // tearing those down in a row is itself work the compositor feels,
        // and a browser tab or a chat window named after the game can never
        // pass the EXP-bar test anyway. This is a cheap pre-filter, not the
        // identification: that is still the arithmetic.
        let size = w.width().ok().zip(w.height().ok());
        match size {
            Some((cw, ch)) if cw >= 640 && ch >= 480 => {}
            Some((cw, ch)) => {
                crate::note!("[tracker] skipping {title:?}: {cw}x{ch} is too small to be the client");
                continue;
            }
            // A window whose size cannot be read is almost certainly
            // minimised or gone. Let it through: being wrong here only
            // costs one abandoned capture, and refusing a real client
            // would be worse.
            None => {}
        }
        ranked.push((rank, w, title));
    }
    ranked.sort_by_key(|(r, ..)| *r);
    for (r, _, t) in &ranked {
        crate::note!("[tracker] candidate (rank {r}): {t:?}");
    }
    ranked
}

/// A window whose title *is* the game's name, rather than merely mentioning
/// it. The difference decides what to tell the user when nothing captures:
/// a browser tab about MapleStory means the game is not running, while the
/// real client at an unexpected size means the profile needs recalibrating.
fn had_exact_match(ranked: &[(u32, Window, String)]) -> bool {
    ranked.iter().any(|(r, ..)| *r < 100)
}

/// Start tracking on a background thread. Returns immediately.
///
/// The session is created here and handed back, so it belongs to the app
/// rather than to one capture attempt: the game can close, crash or change
/// channel and the totals survive it. The same handle is what the roll-up
/// command reads for the in-progress figures.
pub fn spawn(app: AppHandle, profile: Profile, history: Arc<History>) -> Arc<Mutex<Session>> {
    let profile = Arc::new(profile);
    let session = Arc::new(Mutex::new(Session::new(expcurve::to_next())));
    let handle = Arc::clone(&session);
    std::thread::spawn(move || {
        loop {
            let candidates = candidate_windows();
            if candidates.is_empty() {
                // Re-sent on every retry, not announced once. The thread starts
                // before the webview finishes loading, so a single early event
                // is delivered to nobody and the panel would sit on its
                // placeholder data forever.
                let waiting = session.lock().unwrap().snapshot("waiting for MapleStory");
                match app.emit(EVENT, waiting) {
                    Ok(()) => crate::note!("[tracker] no game window; emitted waiting state"),
                    Err(e) => crate::note!("[tracker] emit failed: {e}"),
                }
                std::thread::sleep(RETRY_AFTER);
                continue;
            }

            // Try them in order. A window of the wrong size stops its own
            // capture after one frame, so a wrong guess costs milliseconds
            // and the next candidate gets a turn; the right one blocks here
            // until the game goes away.
            let exact = had_exact_match(&candidates);
            let mut captured = false;
            for (_, window, title) in candidates {
                crate::note!("[tracker] trying {title:?}");
                let matched = Arc::new(AtomicBool::new(false));
                let settings = Settings::new(
                    window,
                    CursorCaptureSettings::WithoutCursor,
                    DrawBorderSettings::WithoutBorder,
                    SecondaryWindowSettings::Default,
                    MinimumUpdateIntervalSettings::Default,
                    DirtyRegionSettings::Default,
                    ColorFormat::Rgba8,
                    (
                        app.clone(),
                        Arc::clone(&profile),
                        Arc::clone(&session),
                        Arc::clone(&history),
                        Arc::clone(&matched),
                    ),
                );
                if let Err(e) = Capture::start(settings) {
                    crate::note!("[tracker] capture of {title:?} ended: {e}");
                }
                if matched.load(Ordering::Relaxed) {
                    crate::note!("[tracker] {title:?} was the game; it has now gone away");
                    captured = true;
                    break;
                }
            }
            if !captured {
                // Nothing captured at the calibrated size. If one of the
                // candidates was actually called "MapleStory", the client is
                // running at a size the profile was not taught on and the
                // user needs to recalibrate. If they were all just windows
                // mentioning the game — a browser tab, this app's own docs —
                // then the game simply is not running, and saying
                // "recalibrate" would send the user to fix something that is
                // not broken.
                let msg = if exact {
                    "the game is a different size than calibrated — recalibrate"
                } else {
                    "waiting for MapleStory"
                };
                let snap = session.lock().unwrap().snapshot(msg);
                let _ = app.emit(EVENT, snap);
            }
            std::thread::sleep(RETRY_AFTER);
        }
    });
    handle
}
