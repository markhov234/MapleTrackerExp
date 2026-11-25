//! Session state derived from a stream of status-bar readings.
//!
//! Everything here is a consequence of one observable: the absolute EXP
//! number on the status bar, sampled over time. Level comes from the
//! EXP-to-percent ratio, rate from a trailing window of gains, and idle time
//! from the absence of gains. No user input, no configuration.

use serde::Serialize;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Gains older than this stop counting toward the displayed rate. Long enough
/// to ride out a slow patch, short enough that the figure still answers
/// "how fast am I going *now*".
const RATE_WINDOW: Duration = Duration::from_secs(600);

/// No EXP for this long and the session is treated as idle: the clock stops
/// and the pause does not drag the rate down. Town trips and AFK look the
/// same from outside, and both should be excluded.
const IDLE_AFTER: Duration = Duration::from_secs(60);

/// No rate is reported until there is at least this much to measure over.
///
/// Ten seconds of grinding can imply anything from a tenth to ten times your
/// real pace, and the overlay turns a rate into "time to level" — so an early
/// sample does not produce a slightly rough estimate, it produces a confident
/// sentence that is simply wrong. Better to say nothing for a minute and a
/// half than to say "1d 1h" and be out by a factor of five.
const MIN_BASELINE: Duration = Duration::from_secs(90);

/// Unix seconds now. Before the epoch is not a real case; 0 is a safe answer.
fn now_wall() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The character's own sprite, cut off the game screen.
#[derive(Clone, Debug, Serialize)]
pub struct Avatar {
    pub w: u32,
    pub h: u32,
    /// Raw RGBA8 in base64, background keyed to transparent. Sent as pixels
    /// rather than an encoded image so nothing has to decode it: the panel
    /// paints it straight onto a canvas.
    pub rgba: String,
}

/// What the overlay renders. Serialised straight to the webview.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// The region's own colour, in degrees of hue, when the minimap is open.
    ///
    /// `None` means we cannot see which region this is, and the panel should
    /// keep whatever colours it already has. Absence is not "go back to
    /// green" — a theme that resets because a window was closed would be
    /// worse than one that never changed.
    pub region_hue: Option<f32>,
    /// Whether the minimap is expanded far enough to show its badge.
    ///
    /// Collapsed, the panel still names the map — "Shallow Passage: Cave
    /// Fairy Store" — but the badge is not drawn, and the badge is the
    /// colour. So this is not a failure to report; it is something the
    /// player can fix in one click, and saying so is better than a theme
    /// that silently never changes.
    pub minimap_open: bool,
    /// Only set on the snapshot that carries a newly cut portrait. It is
    /// tens of kilobytes and changes a few times a day, so the panel keeps
    /// the last one it was given.
    pub avatar: Option<Avatar>,
    pub ok: bool,
    /// Why there is no reading, when `ok` is false.
    pub status: String,
    pub level: Option<usize>,
    pub exp_into: u64,
    pub level_cost: u64,
    pub remaining: u64,
    pub progress: f64,
    pub exp_gained: u64,
    pub rate: f64,
    pub seconds_to_level: Option<f64>,
    pub active_secs: u64,
    pub idle: bool,
    /// Gathering a long enough baseline to quote a rate.
    pub measuring: bool,
    pub confidence: f32,

    // --- From the chat log, when it is calibrated. ---
    /// True once the log has produced at least one event, which is what the
    /// panel keys on: zero kills because nothing was killed and zero kills
    /// because the log is not being read are different states, and only one
    /// of them is a number.
    pub counting: bool,
    pub kills: u64,
    /// EXP the log accounted for. Below `exp_gained` by whatever scrolled
    /// past between reads.
    pub exp_from_chat: u64,
    pub exp_bonus: u64,
    pub mesos: u64,
    /// The character's whole purse, as the inventory window prints it.
    /// `None` until that window has been open long enough to read twice.
    pub mesos_held: Option<u64>,
    /// How much the purse has moved since the first reading this session.
    /// Signed: an evening of shopping is a real answer, not a bug.
    pub mesos_made: Option<i64>,
    /// What the purse gained beyond what was picked up off the ground —
    /// sales, less whatever was spent. Only offered when both halves are
    /// actually being counted, because a subtraction with an uncounted
    /// term is a guess wearing a number's clothes.
    pub mesos_sold: Option<i64>,
    pub exp_per_kill: Option<f64>,
    /// Kills left at the current average, which is the question a kill
    /// count is actually asked in service of.
    pub kills_to_level: Option<u64>,
    pub mesos_per_hour: f64,
    pub items: Vec<(String, u64)>,
    pub potions: Vec<(String, u64)>,
}

pub struct Session {
    table: Vec<u64>,
    level: Option<usize>,
    /// The level this session opened at, for "levels gained" in the history.
    level_start: Option<usize>,
    /// Wall-clock start, unix seconds. The rest of this type runs on
    /// `Instant`, which is the right clock for measuring elapsed time and the
    /// wrong one for writing down when something happened: it does not
    /// survive a reboot and cannot be compared across runs.
    started_wall: i64,
    exp_into: u64,
    exp_gained: u64,
    /// (when, cumulative gained), trimmed to RATE_WINDOW.
    samples: VecDeque<(Instant, u64)>,
    last_seen: Option<(Instant, u64)>,
    last_gain: Option<Instant>,
    active: Duration,
    idle: bool,
    confidence: f32,
    /// What the chat log has reported. Empty and untouched when the log is
    /// not calibrated.
    chat: crate::chat::Tally,
    /// The hue last agreed on by two readings in a row.
    region_hue: Option<f32>,
    /// A reading not yet confirmed by a second one.
    region_pending: Option<f32>,
    minimap_open: bool,
    chat_seen: bool,
    /// The purse when the inventory was first read this session, and the
    /// most recent reading.
    meso_start: Option<u64>,
    meso_now: Option<u64>,
}

impl Session {
    pub fn new(table: Vec<u64>) -> Self {
        Session {
            table,
            level: None,
            level_start: None,
            started_wall: now_wall(),
            exp_into: 0,
            exp_gained: 0,
            samples: VecDeque::new(),
            last_seen: None,
            last_gain: None,
            active: Duration::ZERO,
            idle: false,
            confidence: 0.0,
            chat: crate::chat::Tally::default(),
            region_hue: None,
            region_pending: None,
            minimap_open: false,
            chat_seen: false,
            meso_start: None,
            meso_now: None,
        }
    }

    /// Feed one status-bar reading.
    ///
    /// `inferred` is the level the EXP-to-percent ratio suggests, which is
    /// absent whenever the percentage reads 0.00%. It is trusted on the first
    /// reading and otherwise only used to correct drift, because the wrap
    /// detection below is the more reliable signal once running.
    pub fn observe(&mut self, inferred: Option<usize>, absolute: u64, confidence: f32, now: Instant) {
        self.confidence = confidence;
        if self.level.is_none() {
            self.level = inferred;
            self.level_start = inferred;
        }

        let Some(level) = self.level else {
            self.exp_into = absolute;
            return;
        };

        if let Some((prev_t, prev_exp)) = self.last_seen {
            let dt = now.saturating_duration_since(prev_t);

            let delta: u64 = if absolute >= prev_exp {
                absolute - prev_exp
            } else {
                // EXP fell. Either a level-up wrapped it, or the character
                // died and lost some. A level-up is the only one that also
                // leaves the new value small relative to the old, and the
                // inferred level confirms it when available.
                let levelled = inferred.map(|l| l > level).unwrap_or(absolute * 4 < prev_exp);
                if levelled && level < crate::expcurve::LAST_LEVEL {
                    let carried = self.table[level].saturating_sub(prev_exp);
                    self.level = Some(level + 1);
                    carried + absolute
                } else {
                    0 // death: real, but not progress. Not counted as a gain.
                }
            };

            // Defence in depth. The reader refuses a number that fails the
            // bar's own arithmetic, so a delta this large should no longer
            // be reachable — but a session total is cumulative and kept, so
            // a single bad frame is permanent damage, and the cost of being
            // wrong here is not symmetric. You cannot gain more than a whole
            // level between two frames an eighth of a second apart; a
            // genuine level-up is the branch above, and it is bounded too.
            if delta > self.table.get(level).copied().unwrap_or(u64::MAX) {
                self.last_seen = Some((now, absolute));
                return;
            }

            if delta > 0 {
                self.exp_gained += delta;
                self.last_gain = Some(now);
                self.idle = false;
            } else if self
                .last_gain
                .map(|g| now.saturating_duration_since(g) > IDLE_AFTER)
                .unwrap_or(false)
            {
                self.idle = true;
            }

            // The clock only runs while EXP is moving.
            if !self.idle {
                self.active += dt;
            }

            self.samples.push_back((now, self.exp_gained));
            while let Some(&(t, _)) = self.samples.front() {
                if now.saturating_duration_since(t) > RATE_WINDOW {
                    self.samples.pop_front();
                } else {
                    break;
                }
            }
        } else {
            self.last_gain = Some(now);
            self.samples.push_back((now, 0));
        }

        self.exp_into = absolute;
        self.last_seen = Some((now, absolute));
    }

    /// The purse, read off the inventory window.
    ///
    /// The first confirmed reading is the session's baseline and is never
    /// revised: everything after it is measured against that. A session
    /// that starts with the inventory closed simply has no meso figures
    /// until it is opened, which is honest — there is nothing to compare
    /// against before then.
    pub fn observe_purse(&mut self, held: u64) {
        self.meso_start.get_or_insert(held);
        self.meso_now = Some(held);
    }

    /// Fold in what the chat log reported since the last read.
    ///
    /// Deliberately separate from `observe`: the status bar is the authority
    /// on how much EXP was gained, and the log is the authority on what
    /// produced it. Letting the log adjust the EXP total would mean a missed
    /// line quietly lowering a number the bar states outright.
    pub fn observe_chat(&mut self, events: &[crate::chat::Event]) {
        for e in events {
            self.chat_seen = true;
            self.chat.apply(e);
        }
    }

    /// The level the session believes it is on, for the reader to lean on.
    pub fn level(&self) -> Option<usize> {
        self.level
    }

    /// This session as a history record, for checkpointing and filing.
    ///
    /// `active_secs` rather than wall duration is the honest denominator: the
    /// clock only runs while EXP is moving, so a week's rate computed from
    /// these is a grinding rate and not an "app was open" rate.
    pub fn record(&self) -> crate::history::Record {
        crate::history::Record {
            started: self.started_wall,
            ended: now_wall(),
            exp_gained: self.exp_gained,
            active_secs: self.active.as_secs(),
            level_start: self.level_start,
            level_end: self.level,
            kills: self.chat.kills,
            mesos: self.chat.mesos,
        }
    }

    /// EXP per hour over the trailing window.
    pub fn rate(&self) -> f64 {
        let (Some(&(t0, e0)), Some(&(t1, e1))) = (self.samples.front(), self.samples.back()) else {
            return 0.0;
        };
        let span = t1.saturating_duration_since(t0);
        if span < MIN_BASELINE {
            return 0.0;
        }
        (e1 - e0) as f64 / span.as_secs_f64() * 3600.0
    }

    /// True while there are samples but not yet enough of them to quote a rate.
    fn measuring(&self) -> bool {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, _)), Some(&(t1, _))) => t1.saturating_duration_since(t0) < MIN_BASELINE,
            _ => false,
        }
    }

    /// Take a region reading, and commit it only once a second agrees.
    ///
    /// Crossing into Kerning City should change the panel at once — that is
    /// the whole feature — so this does not average anything. What it does
    /// refuse is a single odd frame: a badge half-covered by a cursor, or a
    /// map still fading in. Two readings that agree with each other and
    /// disagree with what is on screen is a region change; one is noise.
    pub fn observe_minimap(&mut self, open: bool) {
        self.minimap_open = open;
    }

    pub fn observe_region(&mut self, hue: f32) {
        let near = |a: f32, b: f32| {
            let d = (a - b).abs() % 360.0;
            d.min(360.0 - d) <= 18.0
        };
        if self.region_hue.map(|h| near(h, hue)).unwrap_or(false) {
            self.region_pending = None;
            return;
        }
        match self.region_pending {
            Some(p) if near(p, hue) => {
                self.region_hue = Some(hue);
                self.region_pending = None;
            }
            _ => self.region_pending = Some(hue),
        }
    }

    pub fn snapshot(&self, status: &str) -> Snapshot {
        let rate = self.rate();
        let (cost, remaining, progress) = match self.level {
            Some(l) if l <= crate::expcurve::LAST_LEVEL => {
                let c = self.table[l];
                (c, c.saturating_sub(self.exp_into), if c > 0 { self.exp_into as f64 / c as f64 } else { 0.0 })
            }
            _ => (0, 0, 0.0),
        };
        Snapshot {
            region_hue: self.region_hue,
            // Attached by the tracker on the one snapshot that carries a
            // freshly cut portrait; the session itself does not hold it.
            avatar: None,
            minimap_open: self.minimap_open,
            ok: self.level.is_some(),
            status: status.to_string(),
            level: self.level,
            exp_into: self.exp_into,
            level_cost: cost,
            remaining,
            progress,
            exp_gained: self.exp_gained,
            rate,
            seconds_to_level: if rate > 0.0 && remaining > 0 {
                Some(remaining as f64 / rate * 3600.0)
            } else {
                None
            },
            active_secs: self.active.as_secs(),
            idle: self.idle,
            measuring: self.measuring(),
            confidence: self.confidence,
            counting: self.chat_seen,
            mesos_held: self.meso_now,
            mesos_made: match (self.meso_start, self.meso_now) {
                (Some(a), Some(b)) => Some(b as i64 - a as i64),
                _ => None,
            },
            // Picked up is only being counted once the log has said
            // something; before that this subtraction would report every
            // meso as a sale.
            mesos_sold: match (self.meso_start, self.meso_now, self.chat_seen) {
                (Some(a), Some(b), true) => Some((b as i64 - a as i64) - self.chat.mesos as i64),
                _ => None,
            },
            kills: self.chat.kills,
            exp_from_chat: self.chat.exp_from_chat,
            exp_bonus: self.chat.exp_bonus,
            mesos: self.chat.mesos,
            exp_per_kill: self.chat.exp_per_kill(),
            // Projected from what kills have actually been worth this
            // session, so a party split is already in the figure.
            kills_to_level: self
                .chat
                .exp_per_kill()
                .filter(|v| *v > 0.0)
                .map(|v| (remaining as f64 / v).ceil() as u64),
            mesos_per_hour: if self.active.as_secs() > 0 {
                self.chat.mesos as f64 / self.active.as_secs_f64() * 3600.0
            } else {
                0.0
            },
            items: crate::chat::Tally::top(&self.chat.items, 8),
            potions: crate::chat::Tally::top(&self.chat.potions, 4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expcurve;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    /// What the purse is for: a difference of two readings, not a running
    /// sum, and sales as whatever it gained beyond what was picked up.
    #[test]
    fn the_purse_reports_a_difference_and_backs_out_what_was_picked_up() {
        let mut s = Session::new(expcurve::to_next());

        // Nothing at all until the inventory has been seen. A session that
        // starts with it closed has no baseline to measure against, and
        // guessing one would make the first reading look like profit.
        let snap = s.snapshot("");
        assert_eq!(snap.mesos_held, None);
        assert_eq!(snap.mesos_made, None);
        assert_eq!(snap.mesos_sold, None);

        s.observe_purse(984_239);
        // Still no sales figure: the log has said nothing, so "picked up"
        // is zero for want of counting rather than because nothing dropped.
        assert_eq!(s.snapshot("").mesos_sold, None);

        s.observe_chat(&[crate::chat::Event::Meso { amount: 300 }]);
        s.observe_purse(984_447);

        let snap = s.snapshot("");
        assert_eq!(snap.mesos_held, Some(984_447));
        assert_eq!(snap.mesos_made, Some(208), "the difference, not an accumulation");
        // Up 208 while 300 was picked up off the ground: 92 went out the
        // door, which is spending, not a sale.
        assert_eq!(snap.mesos_sold, Some(-92));

        // The baseline is never revised, however many times it is read.
        s.observe_purse(984_802);
        assert_eq!(s.snapshot("").mesos_made, Some(563));
    }

    #[test]
    fn tracks_gains_and_rate() {
        let t = expcurve::to_next();
        let mut s = Session::new(t);
        let base = Instant::now();
        s.observe(Some(40), 763, 1.0, base);
        // 1000 EXP every 10s = 360,000/hr, run past the minimum baseline
        for i in 1..=12 {
            s.observe(Some(40), 763 + 1000 * i, 1.0, at(base, 10 * i as u64));
        }
        assert_eq!(s.exp_gained, 12_000);
        let r = s.rate();
        assert!((r - 360_000.0).abs() < 1.0, "rate was {r}");
        let snap = s.snapshot("ok");
        assert_eq!(snap.level, Some(40));
        assert_eq!(snap.remaining, 294_216 - 12_763);
        assert!(!snap.measuring);
    }

    /// A few seconds of samples must not produce a rate. The overlay turns
    /// rate into a time-to-level sentence, so a noisy early figure reads as a
    /// confident wrong answer rather than a rough one.
    #[test]
    fn quotes_no_rate_until_the_baseline_is_long_enough() {
        let t = expcurve::to_next();
        let mut s = Session::new(t);
        let base = Instant::now();
        s.observe(Some(40), 1000, 1.0, base);
        s.observe(Some(40), 1500, 1.0, at(base, 10));
        assert_eq!(s.rate(), 0.0, "10s is not a baseline");
        let snap = s.snapshot("ok");
        assert!(snap.measuring);
        assert_eq!(snap.seconds_to_level, None, "no rate means no projection");
    }

    #[test]
    fn a_level_up_is_a_gain_not_a_loss() {
        let t = expcurve::to_next();
        let cost40 = t[40];
        let mut s = Session::new(t);
        let base = Instant::now();
        s.observe(Some(40), cost40 - 100, 1.0, base);
        // wraps past the level boundary with 50 into level 41
        s.observe(Some(41), 50, 1.0, at(base, 10));
        assert_eq!(s.exp_gained, 150, "should carry the remainder plus the new exp");
        assert_eq!(s.snapshot("ok").level, Some(41));
    }

    #[test]
    fn death_does_not_count_as_progress() {
        let t = expcurve::to_next();
        let mut s = Session::new(t);
        let base = Instant::now();
        s.observe(Some(40), 200_000, 1.0, base);
        s.observe(Some(40), 190_000, 1.0, at(base, 10));
        assert_eq!(s.exp_gained, 0);
        assert_eq!(s.snapshot("ok").level, Some(40));
    }

    #[test]
    fn goes_idle_without_gains_and_stops_the_clock() {
        let t = expcurve::to_next();
        let mut s = Session::new(t);
        let base = Instant::now();
        s.observe(Some(40), 1000, 1.0, base);
        s.observe(Some(40), 2000, 1.0, at(base, 10));
        let active_before = s.active;
        for i in 1..=10 {
            s.observe(Some(40), 2000, 1.0, at(base, 10 + 20 * i));
        }
        assert!(s.idle, "should have gone idle");
        // The clock ran for the first gap only, not the long silence.
        assert!(
            s.active < active_before + Duration::from_secs(90),
            "idle time should not accumulate, got {:?}",
            s.active
        );
    }

    #[test]
    fn waits_for_a_level_before_deriving_anything() {
        let t = expcurve::to_next();
        let mut s = Session::new(t);
        s.observe(None, 500, 1.0, Instant::now());
        let snap = s.snapshot("waiting");
        assert!(!snap.ok);
        assert_eq!(snap.level, None);
    }
}
