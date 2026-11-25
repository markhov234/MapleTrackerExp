//! Where the app says what it is doing, when there is nobody watching.
//!
//! A release build has no console — it is a `windows` subsystem binary, so
//! `eprintln!` goes nowhere at all. That is right for the person using it,
//! who wants an overlay and not a terminal, and wrong for the moment the
//! overlay says "looking for the EXP bar…" on somebody else's machine and
//! there is no way to find out why.
//!
//! So the same lines go to a file beside the session history, where they can
//! be read after the fact or sent on. The file is capped and rolled over
//! rather than grown without limit: a tracker left running all weekend must
//! not quietly fill a disk.
//!
//! Nothing here records anything about the game or the player — only what
//! this program did: which window it looked at, what the status bar read,
//! where it failed.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<File>> = OnceLock::new();

/// Lines are short; a megabyte is thousands of them, far more than any
/// diagnosis needs, and small enough to attach to a message.
const MAX_BYTES: u64 = 1024 * 1024;

/// Start writing to `<dir>/exp-calc.log`.
///
/// Failing to open it is not worth stopping for: the app's job is to read a
/// status bar, and it can do that with no log at all. Calling this twice, or
/// never, is also fine — [`write`] simply has nowhere to put the line.
pub fn open(dir: &Path) {
    let path = dir.join("exp-calc.log");
    // One generation of history is enough to cover "it broke, I restarted
    // it, now tell me what happened the first time".
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("exp-calc.log.old"));
    }
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = FILE.set(Mutex::new(f));
    }
    write(format_args!("--- started, logging to {} ---", path.display()));
}

/// Record one line, timestamped.
///
/// Also goes to stderr, which a debug build has and a release build does
/// not. Both are best-effort: a tracker that panicked because it could not
/// write a log would be a poor trade.
pub fn write(args: std::fmt::Arguments) {
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    eprintln!("{args}");
    if let Some(f) = FILE.get() {
        if let Ok(mut f) = f.lock() {
            let _ = writeln!(f, "{when} {args}");
            let _ = f.flush();
        }
    }
}

/// Say what the app is doing, to the log file and to stderr.
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => { $crate::notes::write(format_args!($($arg)*)) };
}
