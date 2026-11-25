//! The MapleStory Classic level curve.
//!
//! Not a shipped table: four piecewise formulas generate all 99 levels, and
//! they were checked against every published value and the cumulative total
//! before being written here. The same curve exists in `ui/exp.js` for the
//! frontend; `tests/engine.test.js` covers that copy.

/// Level 99 is the last level with a requirement; the cap is 100.
pub const LAST_LEVEL: usize = 99;

/// EXP to advance FROM `level` to `level + 1`. `prev` is the requirement for
/// `level - 1`, which the recursive bands need.
///
/// The recursive bands multiply by an integer and divide last on purpose:
/// `prev as f64 * 1.0548` can land a hair under the true value and truncate to
/// the wrong integer.
fn step(level: usize, prev: u64) -> u64 {
    match level {
        1..=5 => (((level * level) / 2) as u64 + 15) * level as u64,
        6..=50 => {
            let q = ((level * level) / 3) as u64;
            (q + 19) * q
        }
        51..=89 => (prev * 10548) / 10000,
        90..=99 => (prev * 21) / 10,
        _ => 0,
    }
}

/// `to_next[l]` is the EXP needed to go from level `l` to `l + 1`.
/// Index 0 is unused; index 100 is 0.
pub fn to_next() -> Vec<u64> {
    let mut v = vec![0u64; 101];
    let mut prev = 0u64;
    for lv in 1..=LAST_LEVEL {
        let s = step(lv, prev);
        v[lv] = s;
        prev = s;
    }
    v
}

/// Identify the level from an absolute EXP value and the displayed percentage.
///
/// The status bar shows both, and their ratio is the level's total. The
/// percentage carries only two decimals, so this is unusable when it reads
/// 0.00% — early in a level, or at high levels where a whole percent is
/// enormous. Callers must handle `None` rather than guess.
///
/// Once the level is known, it is tracked by watching EXP wrap downward, so
/// this only has to succeed once per session.
pub fn infer_level(absolute: u64, percent: f64, table: &[u64]) -> Option<usize> {
    if absolute == 0 || percent <= 0.0 {
        return None;
    }
    let implied = absolute as f64 / (percent / 100.0);

    // Accept only an unambiguous answer: the best candidate must also be the
    // only one whose own rounded percentage matches what is on screen.
    let mut hits: Vec<(usize, f64)> = Vec::new();
    for lv in 1..=LAST_LEVEL {
        let cost = table[lv];
        if cost == 0 || absolute > cost {
            continue;
        }
        let shown = ((absolute as f64 / cost as f64) * 10000.0).round() / 100.0;
        if (shown - percent).abs() < 0.005 {
            hits.push((lv, (cost as f64 - implied).abs()));
        }
    }
    if hits.len() == 1 {
        return Some(hits[0].0);
    }
    // Several levels round to the same displayed percentage: take the closest
    // to the implied total only when it is clearly nearer than the rest.
    hits.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    match (hits.first(), hits.get(1)) {
        (Some(best), Some(next)) if best.1 * 4.0 < next.1 => Some(best.0),
        (Some(best), None) => Some(best.0),
        _ => None,
    }
}

/// Total EXP from `(level, into)` up to `target`.
pub fn between(level: usize, into: u64, target: usize, table: &[u64]) -> u64 {
    if target <= level || level == 0 || level > LAST_LEVEL {
        return 0;
    }
    let mut total = table[level].saturating_sub(into);
    for lv in (level + 1)..target.min(LAST_LEVEL + 1) {
        total += table[lv];
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_matches_published_values() {
        let t = to_next();
        assert_eq!(t[5], 135);
        assert_eq!(t[40], 294_216);
        assert_eq!(t[49], 655_200);
        assert_eq!(t[51], 748_608);
        assert_eq!(t[90], 11_938_059);
        assert_eq!(t[99], 9_482_161_548);
        let cumulative: u64 = (1..=99).map(|l| t[l]).sum();
        assert_eq!(cumulative, 18_194_778_397);
    }

    #[test]
    fn infers_the_level_from_the_status_bar() {
        let t = to_next();
        // The real captured frame: 763 EXP showing as 0.26%.
        assert_eq!(infer_level(763, 0.26, &t), Some(40));
    }

    #[test]
    fn refuses_to_guess_without_a_percentage() {
        let t = to_next();
        assert_eq!(infer_level(500, 0.0, &t), None);
        assert_eq!(infer_level(0, 1.5, &t), None);
    }

    #[test]
    fn between_subtracts_banked_exp() {
        let t = to_next();
        assert_eq!(between(40, 763, 41, &t), 293_453);
        assert_eq!(between(40, 0, 42, &t), 294_216 + 324_240);
        assert_eq!(between(40, 0, 40, &t), 0);
    }
}
