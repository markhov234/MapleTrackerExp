//! Reading the game's chat log.
//!
//! The status bar states a total; the chat log states the individual events
//! that moved it. "You received EXP (+27)" is one kill, with its exact value.
//! That is the only honest route to a kill count: while partied, the EXP
//! gained per kill includes a continuous damage-share term, so no amount of
//! arithmetic on the status bar can recover how many monsters died.
//!
//! Three problems, in order:
//!
//!   * **The log scrolls.** A band of lines is re-read a couple of times a
//!     second and most of what it holds was already counted. New lines are
//!     found by overlapping the band against the previous read, the way a
//!     diff finds appended text — see [`Log::ingest`].
//!   * **The text is prose.** The status bar is digits in a known layout;
//!     a chat line is a sentence whose wording varies by client version and
//!     locale. Classification is therefore by keyword against a table of
//!     rules rather than by a fixed format, and the rules are ordered,
//!     because "You received EXP" and "You received an item" share a verb.
//!   * **A misread line must not become a number.** A line containing an
//!     unreadable glyph anywhere near its digits is dropped rather than
//!     guessed at, exactly as the status-bar reader drops a field.
//!   * **An unreadable number does not unmake the event.** That a monster
//!     died is established by the sentence, which is one fixed phrase; how
//!     much EXP it paid is established by the digits, which are separate
//!     evidence. So an EXP line whose amount cannot be read still counts as
//!     a kill and contributes nothing to the EXP or the average. The
//!     alternative — discarding the line whole — reports zero kills on a
//!     map where hundreds died, which is the more misleading of the two.
//!
//! Nothing here touches the game: it is string handling over text that the
//! OCR produced from pixels.

use serde::Serialize;

/// Something the log said happened.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Event {
    /// EXP from a kill. `bonus` marks the party / holiday / event lines that
    /// accompany a kill rather than being one: counting those as kills would
    /// double every monster in a party.
    ///
    /// `amount` is `None` when the sentence read but its digits did not. The
    /// kill still happened; only its value is unknown.
    Exp { amount: Option<u64>, bonus: bool },
    /// Mesos picked up.
    Meso { amount: u64 },
    /// An item picked up, by name, with how many.
    Item { name: String, count: u64 },
    /// A consumable used.
    Potion { name: String },
    LevelUp,
}

/// How a line is recognised and what number to take from it.
///
/// Kept as data rather than as a chain of `if`s so a client that words a
/// line differently is a table entry, not a patch. Order is significant:
/// the first rule whose keywords all appear wins.
struct Rule {
    kind: Kind,
    /// Every one of these must appear in the lowercased line.
    needs: &'static [&'static str],
    /// None of these may appear.
    not: &'static [&'static str],
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Exp,
    BonusExp,
    Meso,
    Item,
    Potion,
    LevelUp,
}

/// The phrasings seen in the Classic client, most specific first.
///
/// "bonus exp" has to be tested before plain EXP and "received an item"
/// before anything else keyed on "received", or the general rule eats the
/// specific one.
const RULES: &[Rule] = &[
    Rule { kind: Kind::LevelUp, needs: &["level up"], not: &[] },
    Rule { kind: Kind::BonusExp, needs: &["exp", "party"], not: &[] },
    Rule { kind: Kind::BonusExp, needs: &["bonus exp"], not: &[] },
    Rule { kind: Kind::BonusExp, needs: &["exp", "bonus"], not: &[] },
    // "received" as well as "exp", and not merely the absence of "bonus"
    // and "party". Absence is weak evidence when a glyph the font has not
    // been taught reads as '?': a party line whose own keyword was the
    // unreadable word would otherwise fall through to here and be counted
    // as a second monster. The kill line names its own verb; require it.
    Rule { kind: Kind::Exp, needs: &["received", "exp"], not: &["bonus", "party"] },
    Rule { kind: Kind::Exp, needs: &["experience"], not: &["bonus", "party"] },
    Rule { kind: Kind::Meso, needs: &["meso"], not: &[] },
    Rule { kind: Kind::Potion, needs: &["used a"], not: &[] },
    Rule { kind: Kind::Item, needs: &["obtained"], not: &["meso"] },
    Rule { kind: Kind::Item, needs: &["received an item"], not: &[] },
    Rule { kind: Kind::Item, needs: &["picked up"], not: &["meso"] },
];

/// What one line of chat means, if anything.
///
/// Chat is mostly other players talking. A line that is not an event is not
/// an error, so this returns `None` for the overwhelming majority of lines.
pub fn classify(line: &str) -> Option<Event> {
    let lower = line.to_ascii_lowercase();
    let rule = RULES.iter().find(|r| {
        r.needs.iter().all(|k| lower.contains(k)) && !r.not.iter().any(|k| lower.contains(k))
    })?;
    // An unreadable glyph is tolerated in the prose and never in the part
    // that becomes a number or a name. Dropping a '?' out of "(+2?7)" would
    // give 27 — a wrong count that looks exactly like a real one — while a
    // '?' in the sentence around it costs nothing, since the line was
    // already classified by the keywords that did read.
    match rule.kind {
        Kind::LevelUp => Some(Event::LevelUp),
        Kind::Exp => Some(Event::Exp { amount: number(&lower), bonus: false }),
        Kind::BonusExp => Some(Event::Exp { amount: number(&lower), bonus: true }),
        Kind::Meso => number(&lower).map(|amount| Event::Meso { amount }),
        Kind::Potion => item_name(line)
            .filter(|n| !n.contains('?'))
            .map(|name| Event::Potion { name }),
        Kind::Item => item_name(line).filter(|n| !n.contains('?')).map(|name| Event::Item {
            name,
            count: number(&lower).unwrap_or(1).max(1),
        }),
    }
}

/// The quantity in a line.
///
/// A `+`-prefixed number wins, because that is how the client writes the
/// amount gained and any other number in the line is incidental. Commas are
/// thousands separators in every one of these messages.
/// An unreadable glyph touching a number makes that number unusable: '?'
/// beside a digit is a digit, and a missing one changes the value by an
/// order of magnitude.
fn number(lower: &str) -> Option<u64> {
    let bytes: Vec<char> = lower.chars().collect();
    let mut plus: Option<u64> = None;
    let mut first: Option<u64> = None;
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut digits = String::new();
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == ',') {
            if bytes[i].is_ascii_digit() {
                digits.push(bytes[i]);
            }
            i += 1;
        }
        let Ok(v) = digits.parse::<u64>() else { continue };
        let touches_unknown = (start > 0 && bytes[start - 1] == '?')
            || bytes.get(i).map(|c| *c == '?').unwrap_or(false);
        if touches_unknown {
            return None;
        }
        let signed = start > 0 && bytes[start - 1] == '+';
        if signed && plus.is_none() {
            plus = Some(v);
        }
        if first.is_none() {
            first = Some(v);
        }
    }
    plus.or(first)
}

/// The item's name out of a pickup line.
///
/// The client wraps names in a word the surrounding phrase does not use —
/// everything after the last colon, or inside the brackets. Falling back to
/// the whole line would turn one misparsed sentence into a permanent entry
/// in the item list, so an unrecognised shape yields nothing.
fn item_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if let (Some(a), Some(b)) = (trimmed.find('['), trimmed.rfind(']')) {
        if b > a + 1 {
            return Some(trimmed[a + 1..b].trim().to_string());
        }
    }
    if let Some(i) = trimmed.rfind(':') {
        let tail = trimmed[i + 1..].trim();
        let tail = tail.trim_end_matches(|c: char| c == '.' || c.is_ascii_digit() || c == 'x');
        let tail = tail.trim();
        if !tail.is_empty() {
            return Some(tail.to_string());
        }
    }
    // "You have obtained a Leather." / "You have used a White Potion."
    for marker in [" obtained a ", " obtained an ", " used a ", " used an ", " picked up a "] {
        if let Some(i) = trimmed.to_ascii_lowercase().find(marker) {
            let tail = trimmed[i + marker.len()..].trim().trim_end_matches('.').trim();
            if !tail.is_empty() {
                return Some(tail.to_string());
            }
        }
    }
    None
}

/// The scrolling log, remembered between reads so the same line is counted
/// once.
#[derive(Default)]
pub struct Log {
    /// The lines the band held last time, in order.
    seen: Vec<String>,
}

impl Log {
    pub fn new() -> Self {
        Log::default()
    }

    /// Hand in the lines the band currently holds; get back the ones that are
    /// new since the last call.
    ///
    /// The band is a window onto an append-only list, so the previous read's
    /// tail is the current read's head: the longest such overlap is where the
    /// old content ends. Identical repeated lines ("You received EXP (+27)",
    /// over and over) are why this matches the longest overlap rather than
    /// the first — the shortest match would discard real kills.
    ///
    /// The first read is dropped entirely. Those lines were on screen before
    /// the tracker looked, so counting them would credit the session with
    /// kills it did not see.
    ///
    /// When nothing overlaps, the log scrolled further than the band is tall
    /// between reads and the lines in between are simply gone. Everything on
    /// screen is taken as new, which under-counts rather than invents.
    pub fn ingest(&mut self, lines: Vec<String>) -> Vec<String> {
        let first_read = self.seen.is_empty();
        if lines.is_empty() {
            return Vec::new();
        }
        let mut overlap = 0usize;
        let max = self.seen.len().min(lines.len());
        for k in (1..=max).rev() {
            if self.seen[self.seen.len() - k..] == lines[..k] {
                overlap = k;
                break;
            }
        }
        let fresh: Vec<String> = lines[overlap..].to_vec();
        self.seen = lines;
        if first_read { Vec::new() } else { fresh }
    }

    /// Lines in, events out. The convenience the tracker actually calls.
    pub fn events(&mut self, lines: Vec<String>) -> Vec<Event> {
        self.ingest(lines).iter().filter_map(|l| classify(l)).collect()
    }

    /// The same, for a band that also catches things that are not messages.
    ///
    /// The chat box holds nothing but lines somebody wrote. The lines the
    /// client floats over the game world are different on two counts: the
    /// band sees scenery as well, and the client fades each message until it
    /// stops reading. Both produce entries that are not messages, and both
    /// would otherwise change the shape of the list between one frame and
    /// the next — which is exactly what the overlap matching reads to tell a
    /// new message from one already counted. A meso line was counted twice
    /// this way, because the faded line above it stopped being recognisable
    /// and the list appeared to have shifted.
    ///
    /// Dropping unrecognised lines first makes a message that has become
    /// unreadable look precisely like one that has scrolled away, which is
    /// the safe way to be wrong: a kill can be missed, never invented.
    pub fn recognised(&mut self, lines: Vec<String>) -> Vec<Event> {
        self.events(lines.into_iter().filter(|l| classify(l).is_some()).collect())
    }
}

/// Running counts over everything the log has reported this session.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Tally {
    /// Monsters killed: one per base EXP line. Party bonus lines accompany a
    /// kill rather than being one, so they are not counted here.
    pub kills: u64,
    /// The subset of those kills whose EXP was legible. The average has to
    /// divide by this rather than by `kills`, or every kill the font could
    /// not price would drag the figure down as if the monster paid nothing.
    pub kills_valued: u64,
    /// EXP credited by the chat log, which is the sum of base and bonus.
    /// Kept apart from the status bar's figure: when the two disagree, the
    /// difference is lines the band missed, and that is worth being able to
    /// see rather than hiding.
    pub exp_from_chat: u64,
    pub exp_bonus: u64,
    pub mesos: u64,
    /// Item name -> how many picked up, highest first when rendered.
    pub items: std::collections::BTreeMap<String, u64>,
    pub potions: std::collections::BTreeMap<String, u64>,
}

impl Tally {
    pub fn apply(&mut self, e: &Event) {
        match e {
            Event::Exp { amount, bonus } => {
                if !*bonus {
                    self.kills += 1;
                }
                if let Some(a) = amount {
                    self.exp_from_chat += a;
                    if *bonus {
                        self.exp_bonus += a;
                    } else {
                        self.kills_valued += 1;
                    }
                }
            }
            Event::Meso { amount } => self.mesos += amount,
            Event::Item { name, count } => {
                *self.items.entry(name.clone()).or_default() += count;
            }
            Event::Potion { name } => {
                *self.potions.entry(name.clone()).or_default() += 1;
            }
            Event::LevelUp => {}
        }
    }

    /// Mean EXP per kill, which is the figure a kill count is worth having.
    pub fn exp_per_kill(&self) -> Option<f64> {
        (self.kills_valued > 0).then(|| self.exp_from_chat as f64 / self.kills_valued as f64)
    }

    /// The `n` most-picked-up items, most first.
    pub fn top(map: &std::collections::BTreeMap<String, u64>, n: usize) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = map.iter().map(|(k, c)| (k.clone(), *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    /// The failure that made a meso line count twice: the EXP line above it
    /// faded until it no longer read, and the list looked like it had
    /// scrolled rather than lost a line.
    #[test]
    fn a_line_that_fades_out_does_not_recount_the_one_below_it() {
        let mut log = Log::default();
        let clean = || {
            vec![
                "You received EXP (+107)".to_string(),
                "You have gained mesos (+95)".to_string(),
            ]
        };
        assert!(log.recognised(clean()).is_empty(), "the first read is what was already there");
        assert!(log.recognised(clean()).is_empty(), "nothing new on an identical frame");

        let faded = vec![
            "????u n?? ?????? E i?? ???0??".to_string(),
            "You have gained mesos (+95)".to_string(),
        ];
        assert!(log.recognised(faded).is_empty(), "the meso line was already counted");
    }

    /// Scenery read as text must not disturb the list either.
    #[test]
    fn junk_between_messages_is_ignored() {
        let mut log = Log::default();
        log.recognised(vec!["You received EXP (+107)".to_string()]);
        let evs = log.recognised(vec![
            "? ?? i???".to_string(),
            "You received EXP (+107)".to_string(),
            "You have gained mesos (+95)".to_string(),
        ]);
        assert_eq!(evs.len(), 1, "only the meso line is new: {evs:?}");
        assert!(matches!(evs[0], Event::Meso { amount: 95 }));
    }

    /// Two kills of the same monster really do print the same line twice.
    #[test]
    fn the_same_message_twice_is_two_events() {
        let mut log = Log::default();
        log.recognised(vec!["You received EXP (+107)".to_string()]);
        let evs = log.recognised(vec![
            "You received EXP (+107)".to_string(),
            "You received EXP (+107)".to_string(),
        ]);
        assert_eq!(evs.len(), 1, "one more kill: {evs:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kill_line_is_exp_and_a_party_line_is_not() {
        assert_eq!(classify("You received EXP (+27)"), Some(Event::Exp { amount: Some(27), bonus: false }));
        assert_eq!(
            classify("Bonus EXP for PARTY (+9)"),
            Some(Event::Exp { amount: Some(9), bonus: true })
        );
    }

    /// The reason the kill counter exists at all: a party splits EXP, so the
    /// bonus line must not be mistaken for a second monster.
    #[test]
    fn a_party_bonus_does_not_count_as_a_kill() {
        let mut t = Tally::default();
        for e in [
            Event::Exp { amount: Some(27), bonus: false },
            Event::Exp { amount: Some(9), bonus: true },
        ] {
            t.apply(&e);
        }
        assert_eq!(t.kills, 1);
        assert_eq!(t.exp_from_chat, 36);
        assert_eq!(t.exp_bonus, 9);
    }

    #[test]
    fn mesos_and_items_are_told_apart() {
        assert_eq!(classify("You have gained 1,230 mesos."), Some(Event::Meso { amount: 1230 }));
        assert_eq!(
            classify("You have obtained a Leather."),
            Some(Event::Item { name: "Leather".into(), count: 1 })
        );
        assert_eq!(
            classify("You have used a White Potion."),
            Some(Event::Potion { name: "White Potion".into() })
        );
    }

    /// Player chat is the bulk of the log and must stay out of the totals.
    #[test]
    fn ordinary_chatter_is_not_an_event() {
        assert_eq!(classify("Gatton : [Lion's Fang] B> +2.34 Clean Lions fang plz offer"), None);
        assert_eq!(classify("Umbrella : [Snail] Buying +4 or +5 clean rower"), None);
    }

    /// The same refusal the status-bar parsers make, as far as the *number*
    /// goes: dropping the '?' would silently turn 2?7 into 27. A meso line
    /// is nothing but its number, so it is refused whole; an EXP line also
    /// says a monster died, and that part read cleanly.
    #[test]
    fn an_unreadable_digit_is_never_counted() {
        assert_eq!(
            classify("You received EXP (+2?7)"),
            Some(Event::Exp { amount: None, bonus: false })
        );
        assert_eq!(
            classify("You received EXP (+?27)"),
            Some(Event::Exp { amount: None, bonus: false })
        );
        assert_eq!(classify("You have gained 1,2?0 mesos."), None);
    }

    /// The hazard the leniency opens, and the guard against it: a party
    /// bonus line whose own keyword is the unreadable part must not fall
    /// through and be counted as a monster of its own.
    #[test]
    fn a_party_line_with_an_unreadable_keyword_is_not_a_kill() {
        assert_eq!(classify("?onus EXP ?or ?AR?Y (+9)"), None);
        assert_eq!(classify("Bonus EXP ?or ?AR?Y (+9)"),
                   Some(Event::Exp { amount: Some(9), bonus: true }));
    }

    /// The point of the above: the kill lands, the EXP does not, and the
    /// average is not dragged down by a monster it could not price.
    #[test]
    fn a_kill_whose_exp_is_unreadable_still_counts_as_a_kill() {
        let mut t = Tally::default();
        t.apply(&classify("You received EXP (+2?7)").unwrap());
        t.apply(&classify("You received EXP (+50)").unwrap());
        assert_eq!(t.kills, 2, "both monsters died");
        assert_eq!(t.exp_from_chat, 50, "only one of them could be priced");
        assert_eq!(t.exp_per_kill(), Some(50.0), "the average divides by what it knows");
    }

    /// The opposite case, and the common one: the log is prose, so a letter
    /// the atlas has not seen yet lands somewhere harmless far more often
    /// than it lands on a digit. Refusing the whole line for that would
    /// throw away kills over a smudge in a word that was never read for
    /// meaning.
    #[test]
    fn an_unreadable_letter_away_from_the_number_is_tolerated() {
        assert_eq!(
            classify("Y?u received EXP (+27)"),
            Some(Event::Exp { amount: Some(27), bonus: false })
        );
    }

    /// Names are different: a misread one would sit in the item list as its
    /// own entry forever.
    #[test]
    fn an_unreadable_item_name_is_refused() {
        assert_eq!(classify("You have obtained a Lea?her."), None);
    }

    #[test]
    fn only_lines_appended_since_the_last_read_are_new() {
        let mut log = Log::new();
        let first = vec!["a".to_string(), "b".into(), "c".into()];
        assert!(log.ingest(first).is_empty(), "what was already on screen is not ours");
        let second = vec!["b".to_string(), "c".into(), "d".into()];
        assert_eq!(log.ingest(second), vec!["d".to_string()]);
        let same = vec!["b".to_string(), "c".into(), "d".into()];
        assert!(log.ingest(same).is_empty(), "an unchanged band adds nothing");
    }

    /// Repeated identical lines are the normal case while grinding, and the
    /// shortest overlap would silently discard kills.
    #[test]
    fn repeated_identical_lines_each_count_once() {
        let mut log = Log::new();
        let e = "You received EXP (+27)".to_string();
        log.ingest(vec![e.clone(), e.clone(), e.clone()]);
        let next = vec![e.clone(), e.clone(), e.clone(), e.clone()];
        assert_eq!(log.ingest(next).len(), 1, "one new line, not four and not none");
    }

    #[test]
    fn a_band_that_scrolled_clean_through_takes_everything() {
        let mut log = Log::new();
        log.ingest(vec!["a".into(), "b".into()]);
        assert_eq!(log.ingest(vec!["x".into(), "y".into()]).len(), 2);
    }

    #[test]
    fn exp_per_kill_needs_a_kill() {
        let mut t = Tally::default();
        assert_eq!(t.exp_per_kill(), None);
        t.apply(&Event::Exp { amount: Some(30), bonus: false });
        t.apply(&Event::Exp { amount: Some(10), bonus: true });
        assert_eq!(t.exp_per_kill(), Some(40.0));
    }
}
