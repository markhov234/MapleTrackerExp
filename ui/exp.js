/**
 * Deterministic MapleStory Classic EXP engine.
 *
 * No network, no randomness, no AI. Every number the UI shows comes from here,
 * so the assistant panel can be switched off without changing a single result.
 *
 * The level curve is computed from four piecewise formulas rather than a
 * shipped table. All 99 levels and the cumulative total were verified against
 * the published Classic table before this was written.
 */
(function (root, factory) {
  if (typeof module === "object" && module.exports) module.exports = factory();
  else root.EXP = factory();
})(typeof self !== "undefined" ? self : this, function () {
  "use strict";

  var LEVEL_CAP = 100; // level 99 is the last level with a requirement

  /**
   * EXP required to advance FROM `level` to `level + 1`.
   * `prev` is the requirement for `level - 1` (needed by the recursive bands).
   *
   * Integer-safe on purpose: `prev * 1.0548` in binary floating point can land
   * a hair below the true value and truncate to the wrong integer, so the
   * recursive bands multiply by an integer and divide last.
   */
  function expToNext(level, prev) {
    if (level >= 1 && level <= 5) return (Math.floor((level * level) / 2) + 15) * level;
    if (level >= 6 && level <= 50) {
      var q = Math.floor((level * level) / 3);
      return (q + 19) * q;
    }
    if (level >= 51 && level <= 89) return Math.floor((prev * 10548) / 10000);
    if (level >= 90 && level <= 99) return Math.floor((prev * 21) / 10);
    return 0;
  }

  // toNext[l] = EXP to go from l to l+1. cumulative[l] = total EXP to REACH l.
  var toNext = [0];
  var cumulative = [0, 0];
  (function build() {
    var prev = 0, cum = 0;
    for (var lv = 1; lv <= 99; lv++) {
      var v = expToNext(lv, prev);
      toNext[lv] = v;
      cum += v;
      cumulative[lv + 1] = cum;
      prev = v;
    }
    toNext[100] = 0;
  })();

  /** Which formula band a level falls in — shown in the EXP Curve view. */
  function bandFor(level) {
    if (level <= 5) return { name: "quadratic", detail: "(floor(L2 / 2) + 15) x L" };
    if (level <= 50) return { name: "quadratic", detail: "q = floor(L2 / 3), then (q + 19) x q" };
    if (level <= 89) return { name: "geometric 1.0548x", detail: "trunc(previous x 1.0548)" };
    return { name: "geometric 2.1x", detail: "trunc(previous x 2.1)" };
  }

  function clampLevel(l) {
    l = Math.floor(Number(l) || 1);
    return Math.max(1, Math.min(99, l));
  }

  /** Total EXP to get from (level, expInto) up to targetLevel. */
  function expBetween(level, expInto, targetLevel) {
    level = clampLevel(level);
    targetLevel = Math.max(level, Math.min(100, Math.floor(Number(targetLevel) || level)));
    expInto = Math.max(0, Math.min(toNext[level], Number(expInto) || 0));
    if (targetLevel <= level) return 0;
    var total = toNext[level] - expInto;
    for (var l = level + 1; l < targetLevel; l++) total += toNext[l];
    return total;
  }

  /**
   * Effective EXP for one monster under Classic party rules.
   *
   *   Pool  = MobEXP x PartyDamage / MobMaxHP
   *   Share = max(1, round(Pool x (0.8 x YourLevel / EligibleLevels
   *                              + 0.2 x YourDamage / PartyDamage)))
   *   Bonus = min(0.30, 0.10 x (Eligible - 1)) + 0.05 x (Attackers - 1)
   *   Total = Share + (Bonus > 0 ? max(1, round(Share x Bonus)) : 0)
   *
   * Solo (eligible = 1) collapses to the mob's full EXP with no bonus.
   * Halves round up, which is what Math.round does for positive values.
   */
  function partyExpPerMob(o) {
    var mobExp = Math.max(0, Number(o.mobExp) || 0);
    var eligible = Math.max(1, Math.floor(Number(o.eligibleMembers) || 1));
    var attackers = Math.max(1, Math.min(eligible, Math.floor(Number(o.attackers) || 1)));
    var yourLevel = clampLevel(o.yourLevel);
    // Sum of eligible members' levels, including yours.
    var eligibleLevels = Math.max(yourLevel, Number(o.eligibleLevels) || yourLevel * eligible);
    // Your fraction of the party's counted damage, 0..1.
    var dmgShare = Math.max(0, Math.min(1, Number(o.damageShare)));
    if (!isFinite(dmgShare)) dmgShare = 1 / eligible;

    if (eligible === 1) {
      return { perMob: mobExp, share: mobExp, bonusExp: 0, bonusPct: 0, solo: true };
    }

    var pool = mobExp; // party lands the killing blow: PartyDamage === MobMaxHP
    var weighted = 0.8 * (yourLevel / eligibleLevels) + 0.2 * dmgShare;
    var share = Math.max(1, Math.round(pool * weighted));
    var bonusPct = Math.min(0.3, 0.1 * (eligible - 1)) + 0.05 * (attackers - 1);
    var bonusExp = bonusPct > 0 ? Math.max(1, Math.round(share * bonusPct)) : 0;

    return {
      perMob: share + bonusExp,
      share: share,
      bonusExp: bonusExp,
      bonusPct: bonusPct,
      solo: false
    };
  }

  /**
   * Full plan from the current position to a target level.
   * `expPerHour` is the measured rate; everything downstream is division.
   */
  function plan(o) {
    var level = clampLevel(o.level);
    var expInto = Math.max(0, Math.min(toNext[level], Number(o.expInto) || 0));
    var target = Math.max(level + 1, Math.min(100, Math.floor(Number(o.targetLevel) || level + 1)));
    var expPerHour = Math.max(0, Number(o.expPerHour) || 0);
    var perMob = Math.max(0, Number(o.expPerMob) || 0);

    var totalNeeded = expBetween(level, expInto, target);
    var thisLevelRemaining = toNext[level] - expInto;
    var hours = expPerHour > 0 ? totalNeeded / expPerHour : Infinity;

    // Per-level breakdown, with the running hour count at each step.
    var steps = [], acc = 0;
    for (var l = level; l < target; l++) {
      var cost = l === level ? thisLevelRemaining : toNext[l];
      acc += cost;
      steps.push({
        level: l,
        toLevel: l + 1,
        cost: cost,
        cumulative: acc,
        hours: expPerHour > 0 ? acc / expPerHour : Infinity,
        band: bandFor(l).name
      });
    }

    return {
      level: level,
      target: target,
      expInto: expInto,
      levelCost: toNext[level],
      levelProgress: toNext[level] > 0 ? expInto / toNext[level] : 0,
      thisLevelRemaining: thisLevelRemaining,
      totalNeeded: totalNeeded,
      expPerHour: expPerHour,
      hours: hours,
      killsNeeded: perMob > 0 ? Math.ceil(totalNeeded / perMob) : null,
      killsPerHour: perMob > 0 && expPerHour > 0 ? expPerHour / perMob : null,
      finishAt: isFinite(hours) ? new Date(Date.now() + hours * 3600 * 1000) : null,
      steps: steps
    };
  }

  /** Format a duration in hours as "3d 4h 12m". */
  function formatHours(h) {
    if (!isFinite(h)) return "—";
    if (h <= 0) return "0m";
    var mins = Math.round(h * 60);
    var d = Math.floor(mins / 1440);
    var hr = Math.floor((mins % 1440) / 60);
    var m = mins % 60;
    var out = [];
    if (d) out.push(d + "d");
    if (hr) out.push(hr + "h");
    if (m && !d) out.push(m + "m");
    return out.join(" ") || "0m";
  }

  function formatExp(n) {
    if (n === null || n === undefined || !isFinite(n)) return "—";
    return Math.round(n).toLocaleString("en-US");
  }

  /** Compact form for axis labels: 12.3M, 1.05B. */
  function compact(n) {
    var abs = Math.abs(n);
    if (abs >= 1e9) return (n / 1e9).toFixed(abs >= 1e10 ? 0 : 2) + "B";
    if (abs >= 1e6) return (n / 1e6).toFixed(abs >= 1e7 ? 0 : 1) + "M";
    if (abs >= 1e3) return (n / 1e3).toFixed(abs >= 1e4 ? 0 : 1) + "K";
    return String(Math.round(n));
  }

  return {
    LEVEL_CAP: LEVEL_CAP,
    toNext: toNext,
    cumulative: cumulative,
    expToNext: expToNext,
    bandFor: bandFor,
    expBetween: expBetween,
    partyExpPerMob: partyExpPerMob,
    plan: plan,
    formatHours: formatHours,
    formatExp: formatExp,
    compact: compact
  };
});
