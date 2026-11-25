var E = require("../ui/exp.js");
var MAPS = require("../ui/mapdata.js");
var fails = 0;
function ok(name, cond, extra) {
  if (!cond) { fails++; console.log("FAIL " + name + (extra ? "  " + extra : "")); }
  else console.log("PASS " + name + (extra ? "  " + extra : ""));
}

console.log("=== curve ===");
ok("lv5 = 135", E.toNext[5] === 135, "got " + E.toNext[5]);
ok("lv40 = 294,216", E.toNext[40] === 294216, "got " + E.toNext[40]);
ok("lv49 = 655,200", E.toNext[49] === 655200, "got " + E.toNext[49]);
ok("lv51 = 748,608", E.toNext[51] === 748608, "got " + E.toNext[51]);
ok("lv90 = 11,938,059", E.toNext[90] === 11938059, "got " + E.toNext[90]);
ok("lv99 = 9,482,161,548", E.toNext[99] === 9482161548, "got " + E.toNext[99]);
ok("cumulative to reach lv100 = 18,194,778,397",
   E.cumulative[100] === 18194778397, "got " + E.cumulative[100]);
ok("cumulative to reach lv1 is 0", E.cumulative[1] === 0);
ok("cumulative to reach lv2 is 15", E.cumulative[2] === 15);

console.log("\n=== expBetween ===");
ok("40(0) -> 41 equals lv40 cost", E.expBetween(40, 0, 41) === 294216);
ok("40(763) -> 41 subtracts banked exp", E.expBetween(40, 763, 41) === 293453);
ok("same level is zero", E.expBetween(40, 763, 40) === 0);
ok("target below level is zero", E.expBetween(40, 0, 30) === 0);
ok("40 -> 42 sums both levels",
   E.expBetween(40, 0, 42) === 294216 + 324240, "got " + E.expBetween(40, 0, 42));
ok("banked exp cannot exceed the level cost",
   E.expBetween(40, 99999999, 41) === 0, "got " + E.expBetween(40, 99999999, 41));

console.log("\n=== recommendation stability ===");
(function () {
  // A map sitting right at the cut-off is the case that made the panel
  // blink: the measured rate is a trailing average that moves constantly,
  // so without hysteresis the same map enters and leaves on every frame.
  var b = MAPS.bracketFor(40);
  ok("level 40 has map data", !!b && b.maps.length > 0);
  if (!b || !b.maps.length) return;

  var m = b.maps.slice().sort(function (x, y) { return y.lo - x.lo; })[0];

  // Just inside the entry bar (needs > 5% better), then just outside it.
  var rateIn = m.lo / 1.06;
  var rateOut = m.lo / 1.03;
  var has = function (r) {
    return (r.picks || []).some(function (p) { return p.map.n === m.n; });
  };

  ok("a clearly better map is offered",
     has(MAPS.recommend(40, rateIn, "", 1000, [])));
  ok("a marginal map is not offered unprompted",
     !has(MAPS.recommend(40, rateOut, "", 1000, [])));
  ok("a map already shown is held while it is still better",
     has(MAPS.recommend(40, rateOut, "", 1000, [m.n])));
  ok("a map already shown is dropped once it is not better",
     !has(MAPS.recommend(40, m.lo / 1.001, "", 1000, [m.n])));
  ok("the map you are on is never offered",
     !has(MAPS.recommend(40, rateIn, m.n, 1000, [m.n])));
})();


console.log("\n=== region theming ===");
(function () {
  var T = require("../ui/theme.js");

  ok("a zero rotation changes nothing", T.rotated("#A9CE61", 0) === "#a9ce61",
     "got " + T.rotated("#A9CE61", 0));
  ok("alpha survives a rotation",
     /^rgba\(\d+, \d+, \d+, 0\.12\)$/.test(T.rotated("rgba(169, 206, 97, 0.12)", 40)),
     "got " + T.rotated("rgba(169, 206, 97, 0.12)", 40));
  ok("an unparseable value is left alone", T.rotated("inherit", 40) === "inherit");

  // The guarantee the whole approach rests on: contrast comes from
  // lightness, and rotating hue must not touch it.
  var worst = 0, worstTok = "";
  ["#0D1B15", "#14261D", "#1C3427", "#A9CE61", "#C3E088", "#7FA343",
   "#A8B59C", "#74836B", "#4D5A47", "rgba(169, 206, 97, 0.30)"].forEach(function (base) {
    var before = T.toHsl(T.parse(base));
    for (var d = 0; d < 360; d += 15) {
      var after = T.toHsl(T.parse(T.rotated(base, d)));
      var drift = Math.abs(after.l - before.l);
      if (drift > worst) { worst = drift; worstTok = base + " @" + d; }
    }
  });
  ok("lightness holds across every hue", worst < 0.01,
     "worst drift " + worst.toFixed(4) + " at " + worstTok);

  // Shallow Passage measured 117 degrees; the stock accent is ~80.
  var delta = 117 - T.hueOf("#A9CE61");
  var tinted = T.rotated("#A9CE61", delta);
  ok("the accent lands on the region's hue",
     Math.abs(T.hueOf(tinted) - 117) < 2, "got " + T.hueOf(tinted).toFixed(1));

  ok("every themed token parses",
     T.TOKENS.length === 15 && T.TOKENS.every(function (t) { return /^--c-/.test(t); }));
  ok("semantic colours are not themed",
     T.TOKENS.indexOf("--c-good") < 0 && T.TOKENS.indexOf("--c-alert") < 0);
})();


console.log("\n=== party formula ===");
var solo = E.partyExpPerMob({ mobExp: 85, eligibleMembers: 1, attackers: 1, yourLevel: 40, damageShare: 1 });
ok("solo gets full mob exp, no bonus", solo.perMob === 85 && solo.bonusExp === 0, "perMob " + solo.perMob);

// 2 eligible, both hit, equal levels, equal damage.
// weighted = 0.8*(40/80) + 0.2*0.5 = 0.4 + 0.1 = 0.5 -> share = round(85*0.5) = 43  (42.5 rounds up)
// bonus = min(0.30, 0.10*1) + 0.05*1 = 0.15 -> bonusExp = round(43*0.15) = 6  (6.45 -> 6)
var duo = E.partyExpPerMob({ mobExp: 85, eligibleMembers: 2, attackers: 2, yourLevel: 40, eligibleLevels: 80, damageShare: 0.5 });
ok("duo share = 43 (half rounds up)", duo.share === 43, "got " + duo.share);
ok("duo bonus = 15%", Math.abs(duo.bonusPct - 0.15) < 1e-9, "got " + duo.bonusPct);
ok("duo bonusExp = 6", duo.bonusExp === 6, "got " + duo.bonusExp);
ok("duo total = 49", duo.perMob === 49, "got " + duo.perMob);

// Bonus table from the source: 6 eligible / 1 attacker = 30%; 6 eligible / 6 attackers = 55%.
var six1 = E.partyExpPerMob({ mobExp: 1000, eligibleMembers: 6, attackers: 1, yourLevel: 40, eligibleLevels: 240, damageShare: 1 });
var six6 = E.partyExpPerMob({ mobExp: 1000, eligibleMembers: 6, attackers: 6, yourLevel: 40, eligibleLevels: 240, damageShare: 1/6 });
ok("6 eligible / 1 attacker = 30% bonus", Math.abs(six1.bonusPct - 0.30) < 1e-9, "got " + six1.bonusPct);
ok("6 eligible / 6 attackers = 55% bonus", Math.abs(six6.bonusPct - 0.55) < 1e-9, "got " + six6.bonusPct);
ok("bonus caps the level part at 30%",
   Math.abs(E.partyExpPerMob({ mobExp: 100, eligibleMembers: 5, attackers: 1, yourLevel: 40, eligibleLevels: 200, damageShare: 1 }).bonusPct - 0.30) < 1e-9);
ok("share is never below 1", E.partyExpPerMob({ mobExp: 1, eligibleMembers: 6, attackers: 1, yourLevel: 1, eligibleLevels: 600, damageShare: 0 }).share === 1);

console.log("\n=== plan ===");
var p = E.plan({ level: 40, expInto: 763, targetLevel: 41, expPerHour: 100000, expPerMob: 85 });
ok("plan total = 293,453", p.totalNeeded === 293453, "got " + p.totalNeeded);
ok("plan hours ~2.9346", Math.abs(p.hours - 2.93453) < 1e-4, "got " + p.hours.toFixed(5));
ok("plan kills = ceil(293453/85) = 3453", p.killsNeeded === 3453, "got " + p.killsNeeded);
ok("kills per hour = 1176.47", Math.abs(p.killsPerHour - 100000 / 85) < 1e-6);
ok("one step for one level", p.steps.length === 1);
ok("finishAt is a future date", p.finishAt instanceof Date && p.finishAt > new Date());

var p2 = E.plan({ level: 40, expInto: 0, targetLevel: 50, expPerHour: 0, expPerMob: 0 });
ok("zero rate gives Infinity hours, not NaN", p2.hours === Infinity);
ok("zero rate gives no finish date", p2.finishAt === null);
ok("zero mob exp gives null kills", p2.killsNeeded === null);
ok("10 steps for 40->50", p2.steps.length === 10, "got " + p2.steps.length);
ok("steps accumulate to the total",
   p2.steps[p2.steps.length - 1].cumulative === p2.totalNeeded,
   p2.steps[p2.steps.length - 1].cumulative + " vs " + p2.totalNeeded);

console.log("\n=== guards ===");
ok("level 0 clamps to 1", E.plan({ level: 0, targetLevel: 5, expPerHour: 1 }).level === 1);
ok("level 999 clamps to 99", E.plan({ level: 999, targetLevel: 100, expPerHour: 1 }).level === 99);
ok("target below level+1 is bumped", E.plan({ level: 40, targetLevel: 10, expPerHour: 1 }).target === 41);
ok("target caps at 100", E.plan({ level: 98, targetLevel: 500, expPerHour: 1 }).target === 100);
ok("garbage exp is treated as 0", E.plan({ level: 40, expInto: NaN, targetLevel: 41, expPerHour: 1 }).expInto === 0);

console.log("\n=== formatting ===");
ok("formatHours 2.9345 -> 2h", E.formatHours(2.9345) === "2h 56m" || E.formatHours(2.9345) === "2h", "got " + E.formatHours(2.9345));
ok("formatHours Infinity -> dash", E.formatHours(Infinity) === "\u2014");
ok("compact 1.05e9 -> B", E.compact(1050000000).indexOf("B") > -1, "got " + E.compact(1050000000));
ok("formatExp adds separators", E.formatExp(293453) === "293,453", "got " + E.formatExp(293453));

console.log("\n" + (fails ? fails + " FAILED" : "ALL ENGINE TESTS PASS"));
process.exit(fails ? 1 : 0);
