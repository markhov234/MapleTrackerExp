//! Everything the tracker has measured before now.
//!
//! The live session answers "how am I doing right now". This answers "how did
//! this week go", which needs the sessions to outlive the process. Two files
//! in the app's data directory:
//!
//!   * `sessions.jsonl` — one finished session per line, append-only.
//!   * `current.json` — the session in progress, rewritten on a checkpoint.
//!
//! Append-only for the finished ones because a tracker that corrupts a month
//! of history while writing today's line is worse than useless, and a line
//! that fails to parse costs exactly one session rather than the file. The
//! live session is separate for the same reason: it is the only thing being
//! overwritten, and it is the only thing that can be lost.
//!
//! Crash recovery falls out of that split. If `current.json` is there at
//! startup, the previous run did not shut down cleanly — the session is real
//! and already measured, so it is filed as finished rather than discarded.
//!
//! Nothing here touches the game. It is arithmetic over timestamps.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{Datelike, Local, TimeZone};
use serde::{Deserialize, Serialize};

/// One session, as it will be read back months later.
///
/// Wall-clock seconds, not the monotonic `Instant` the live session runs on:
/// `Instant` is meaningless across a reboot, and these have to survive one.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Record {
    /// Unix seconds, UTC. Roll-ups convert to local time at the boundary.
    pub started: i64,
    pub ended: i64,
    pub exp_gained: u64,
    /// Seconds the EXP was actually moving. Town trips are not counted, so
    /// this is the denominator a rate should use.
    pub active_secs: u64,
    pub level_start: Option<usize>,
    pub level_end: Option<usize>,
    /// Monsters killed, from the chat log. Defaulted rather than required:
    /// sessions recorded before the log was read have no figure, and a
    /// missing count must not make an old line unparseable.
    #[serde(default)]
    pub kills: u64,
    #[serde(default)]
    pub mesos: u64,
}

impl Record {
    /// Levels gained, when both ends are known.
    fn levels(&self) -> usize {
        match (self.level_start, self.level_end) {
            (Some(a), Some(b)) if b > a => b - a,
            _ => 0,
        }
    }
}

/// Which span of history to add up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    Session,
    Today,
    Week,
    Month,
    All,
}

impl Range {
    pub fn parse(s: &str) -> Option<Range> {
        Some(match s {
            "session" => Range::Session,
            "today" => Range::Today,
            "week" => Range::Week,
            "month" => Range::Month,
            "all" => Range::All,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Range::Session => "session",
            Range::Today => "today",
            Range::Week => "week",
            Range::Month => "month",
            Range::All => "all",
        }
    }

    /// The earliest start time that still counts, in unix seconds.
    ///
    /// Local midnight, local Monday, local first-of-month: the player reads
    /// these off a wall clock, so UTC would put the boundary in the middle of
    /// an evening's grinding for most of the world. `Session` and `All` have
    /// no lower bound of their own — the caller handles the first, and the
    /// second is the whole file.
    fn since(self, now: i64) -> i64 {
        let now = match Local.timestamp_opt(now, 0).single() {
            Some(t) => t,
            None => return 0,
        };
        let midnight = |d: chrono::NaiveDate| {
            d.and_hms_opt(0, 0, 0)
                .and_then(|n| Local.from_local_datetime(&n).single())
                .map(|t| t.timestamp())
                .unwrap_or(0)
        };
        match self {
            Range::Today => midnight(now.date_naive()),
            // ISO weeks start on Monday, which is what "this week" means to a
            // player looking at a weekly reset.
            Range::Week => {
                let back = now.weekday().num_days_from_monday() as i64;
                midnight(now.date_naive() - chrono::Duration::days(back))
            }
            Range::Month => midnight(now.date_naive().with_day(1).unwrap_or(now.date_naive())),
            Range::Session | Range::All => 0,
        }
    }
}

/// An added-up span, ready for the panel.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Totals {
    pub range: String,
    pub exp: u64,
    pub active_secs: u64,
    pub sessions: usize,
    /// EXP per hour over active time only.
    pub rate: f64,
    pub levels: usize,
    pub kills: u64,
    pub mesos: u64,
    /// Start of the oldest session counted, unix seconds. 0 when empty.
    pub since: i64,
}

/// The on-disk history, plus an in-memory copy of the finished sessions.
pub struct History {
    dir: PathBuf,
    done: Mutex<Vec<Record>>,
}

const DONE: &str = "sessions.jsonl";
const CURRENT: &str = "current.json";

impl History {
    /// Open (and if necessary create) the history in `dir`.
    ///
    /// Never fails in a way that stops the app: history is a nice-to-have
    /// beside live tracking, and a read-only disk should cost the roll-ups,
    /// not the overlay.
    pub fn open(dir: impl AsRef<Path>) -> History {
        let dir = dir.as_ref().to_path_buf();
        let _ = fs::create_dir_all(&dir);
        let h = History { dir, done: Mutex::new(Vec::new()) };
        h.recover();
        let loaded = h.read_done();
        *h.done.lock().unwrap() = loaded;
        h
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// A leftover `current.json` means the last run was killed rather than
    /// quit. The session it describes really happened, so file it.
    fn recover(&self) {
        let p = self.path(CURRENT);
        let Ok(text) = fs::read_to_string(&p) else { return };
        if let Ok(rec) = serde_json::from_str::<Record>(&text) {
            // A checkpoint that never recorded anything is noise, not history.
            if rec.exp_gained > 0 || rec.active_secs > 0 {
                self.append(&rec);
            }
        }
        let _ = fs::remove_file(&p);
    }

    /// Read the finished sessions, skipping anything unparseable.
    ///
    /// A half-written final line is the normal shape of a crash during an
    /// append, and losing one session is the correct price for it. Refusing
    /// to load the file at all would throw away the other months.
    fn read_done(&self) -> Vec<Record> {
        let Ok(text) = fs::read_to_string(self.path(DONE)) else { return Vec::new() };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Record>(l).ok())
            .collect()
    }

    fn append(&self, rec: &Record) {
        let Ok(line) = serde_json::to_string(rec) else { return };
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(self.path(DONE)) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Save the in-progress session over the last checkpoint.
    ///
    /// Written to a temporary file and renamed, so a crash mid-write leaves
    /// the previous checkpoint intact instead of a truncated one.
    pub fn checkpoint(&self, rec: &Record) {
        let Ok(text) = serde_json::to_string(rec) else { return };
        let tmp = self.path("current.tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, self.path(CURRENT));
        }
    }

    /// File the session as finished and clear the checkpoint.
    pub fn finish(&self, rec: &Record) {
        if rec.exp_gained == 0 && rec.active_secs == 0 {
            let _ = fs::remove_file(self.path(CURRENT));
            return;
        }
        self.append(rec);
        self.done.lock().unwrap().push(rec.clone());
        let _ = fs::remove_file(self.path(CURRENT));
    }

    /// Add up a range. `current` is the live session, which is not on disk yet
    /// and must still be counted — otherwise today's figure jumps only when
    /// you quit.
    pub fn totals(&self, range: Range, current: Option<&Record>, now: i64) -> Totals {
        let mut t = Totals { range: range.label().to_string(), ..Default::default() };
        let since = range.since(now);

        let mut add = |r: &Record| {
            t.exp += r.exp_gained;
            t.active_secs += r.active_secs;
            t.levels += r.levels();
            t.kills += r.kills;
            t.mesos += r.mesos;
            t.sessions += 1;
            if t.since == 0 || r.started < t.since {
                t.since = r.started;
            }
        };

        if range == Range::Session {
            if let Some(r) = current {
                add(r);
            }
        } else {
            // A session is counted in the span it started in. Splitting one
            // that runs through midnight would mean apportioning EXP we never
            // measured per-hour, which is a guess dressed as a number.
            for r in self.done.lock().unwrap().iter() {
                if r.started >= since {
                    add(r);
                }
            }
            if let Some(r) = current {
                if r.started >= since {
                    add(r);
                }
            }
        }

        if t.active_secs > 0 {
            t.rate = t.exp as f64 / t.active_secs as f64 * 3600.0;
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(started: i64, exp: u64, active: u64) -> Record {
        Record { started, ended: started + active as i64, exp_gained: exp, active_secs: active, ..Default::default() }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("maple-hist-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn totals_are_exp_over_active_time() {
        let h = History::open(temp_dir("rate"));
        let now = Local::now().timestamp();
        h.finish(&rec(now - 7200, 100_000, 3600));
        let t = h.totals(Range::All, None, now);
        assert_eq!(t.exp, 100_000);
        assert_eq!(t.sessions, 1);
        assert!((t.rate - 100_000.0).abs() < 1.0, "rate was {}", t.rate);
    }

    /// The live session is not on disk, and leaving it out would mean today's
    /// number only moved when you quit the app.
    #[test]
    fn the_live_session_counts_toward_today() {
        let h = History::open(temp_dir("live"));
        let now = Local::now().timestamp();
        let live = rec(now - 600, 50_000, 600);
        let t = h.totals(Range::Today, Some(&live), now);
        assert_eq!(t.exp, 50_000);
        assert_eq!(t.sessions, 1);
        // Session shows only the live one, never the filed ones.
        h.finish(&rec(now - 7200, 9_999, 60));
        assert_eq!(h.totals(Range::Session, Some(&live), now).exp, 50_000);
    }

    /// Boundaries are local, and a session before them does not count.
    #[test]
    fn yesterday_is_not_today() {
        let h = History::open(temp_dir("bounds"));
        let now = Local::now().timestamp();
        h.finish(&rec(now - 60 * 60 * 24 * 3, 7_000, 600)); // three days back
        h.finish(&rec(now - 300, 3_000, 300));
        assert_eq!(h.totals(Range::Today, None, now).exp, 3_000, "only today");
        assert_eq!(h.totals(Range::All, None, now).exp, 10_000, "all of it");
        let month = h.totals(Range::Month, None, now).exp;
        assert!(month == 3_000 || month == 10_000, "month was {month}");
    }

    /// A truncated last line is what a crash during an append looks like. It
    /// should cost that one session, not the whole file.
    #[test]
    fn a_corrupt_line_does_not_lose_the_file() {
        let dir = temp_dir("corrupt");
        let h = History::open(&dir);
        let now = Local::now().timestamp();
        h.finish(&rec(now - 600, 1_000, 600));
        let mut f = fs::OpenOptions::new().append(true).open(dir.join(DONE)).unwrap();
        writeln!(f, "{{\"started\":123,\"exp_gai").unwrap();
        drop(f);
        let reopened = History::open(&dir);
        assert_eq!(reopened.totals(Range::All, None, now).sessions, 1);
    }

    /// A checkpoint left behind means the last run was killed. The session
    /// happened; it should be in the history on the next start.
    #[test]
    fn an_abandoned_checkpoint_is_recovered() {
        let dir = temp_dir("recover");
        let h = History::open(&dir);
        let now = Local::now().timestamp();
        h.checkpoint(&rec(now - 900, 42_000, 900));
        drop(h);
        assert!(dir.join(CURRENT).exists());
        let reopened = History::open(&dir);
        assert_eq!(reopened.totals(Range::All, None, now).exp, 42_000);
        assert!(!dir.join(CURRENT).exists(), "the checkpoint should be cleared");
    }

    /// An app opened and closed without grinding should not litter the file.
    #[test]
    fn an_empty_session_is_not_filed() {
        let dir = temp_dir("empty");
        let h = History::open(&dir);
        let now = Local::now().timestamp();
        h.finish(&rec(now - 60, 0, 0));
        assert_eq!(h.totals(Range::All, None, now).sessions, 0);
    }
}
