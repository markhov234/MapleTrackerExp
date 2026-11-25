/**
 * Grind-map rankings and mob stats for MapleStory Classic.
 *
 * EXP/hr figures are ranges players actually log on each map, from community
 * grind trackers. Two things about them matter when showing them to someone:
 *
 *   - They are solo figures. A party splits each kill, then adds the party
 *     bonus, so they do not transfer to a partied session.
 *   - The source calls them theoretical ceilings. Most players run a map a few
 *     levels above its monsters, so arriving at a bracket's first level earns
 *     less. `playedAt` records the level players usually run it at.
 *
 * So these are a reference to compare your own measured rate against, never a
 * promise. Anything the UI derives from them says "logged", not "you will get".
 *
 * Coverage is levels 1-60. The source's 61-70 bracket is not transcribed yet;
 * `forLevel` returns null above 60 and the UI must handle that.
 */
(function (root, factory) {
  if (typeof module === "object" && module.exports) module.exports = factory();
  else root.MAPS = factory();
})(typeof self !== "undefined" ? self : this, function () {
  "use strict";

  // est: true means the source marked the figure an estimate rather than logged.
  // portals: portals to walk to the nearest potion vendor. 0 is on the map.
  var BRACKETS = [
    { lo: 1, hi: 10, maps: [
      { n: "L Forest I", street: "Victoria Road", lo: 9000, hi: 9100, est: true, mob: "Ribbon Pig", mobLv: 10, portals: 3, why: "Broad mix from Snails up to Ribbon Pigs; mob levels climb with you." },
      { n: "Construction Site North of Kerning City", street: "Victoria Road", lo: 6000, hi: 6300, est: true, mob: "Dark Stump", mobLv: 11, portals: 3, why: "Wide lv 4-19 spread, Red Snails early and Dark Stumps late." },
      { n: "The Tree That Grew II", street: "Victoria Road", lo: 5100, hi: 5400, est: true, mob: "Slime", mobLv: 6, portals: 4, why: "Cleanest pure lv 5-11 grind, no outlevelled stragglers." }
    ]},
    { lo: 11, hi: 15, maps: [
      { n: "Tree Dungeon, Forest Up North IV", street: "Victoria Road", lo: 22000, hi: 30000, playedAt: 23, mob: "Green Mushroom", mobLv: 14, portals: 5, why: "Green Mushroom farm with a few low Slimes mixed in." },
      { n: "Line 1 <Area 1>", street: "Kerning City Subway", lo: 19000, hi: 32000, playedAt: 27, mob: "Bubbling", mobLv: 15, portals: 3, why: "Single-mob map at exactly lv 15, one of the cleanest grinds here." },
      { n: "West Rocky Mountain III", street: "Victoria Road", lo: 18000, hi: 18000, playedAt: 20, mob: "Green Mushroom", mobLv: 14, portals: 5, why: "Green Mushroom heavy with some Axe Stumps." }
    ]},
    { lo: 16, hi: 20, maps: [
      { n: "Dangerous Steam", street: "Dungeon", lo: 55000, hi: 66000, playedAt: 34, mob: "Stirge", mobLv: 20, portals: 3, why: "Stirge plus Horny and Zombie Mushroom, good variety." },
      { n: "Transfer Area", street: "Kerning City Subway", lo: 31000, hi: 50000, playedAt: 30, mob: "Stirge", mobLv: 20, portals: 4, why: "Single-mob Stirge map, tight layout, easy for mages and bowmen." },
      { n: "Tree Dungeon, Forest Up North III", street: "Victoria Road", lo: 25000, hi: 42000, playedAt: 26, mob: "Horny Mushroom", mobLv: 22, portals: 5, why: "The pick when both Kerning maps are crowded." }
    ]},
    { lo: 21, hi: 25, maps: [
      { n: "Deep Ant Tunnel I", street: "Dungeon", lo: 51000, hi: 80000, playedAt: 38, mob: "Zombie Mushroom", mobLv: 24, portals: 2, why: "Deepest Ant Tunnel map, broad spread up to Jr. Boogie." },
      { n: "Ant Tunnel Park", street: "Dungeon", lo: 51000, hi: 61000, playedAt: 36, mob: "Evil Eye", mobLv: 27, portals: 0, why: "Potions on the map itself. Adds Evil Eye to the tunnel mix." },
      { n: "Ant Tunnel III", street: "Dungeon", lo: 35000, hi: 64000, playedAt: 32, mob: "Zombie Mushroom", mobLv: 24, portals: 6, why: "Horny plus Zombie Mushroom and nothing else." }
    ]},
    { lo: 26, hi: 30, maps: [
      { n: "The Cave of Evil Eye III", street: "Dungeon", lo: 68000, hi: 85000, playedAt: 40, mob: "Evil Eye", mobLv: 27, portals: 3, why: "Near-pure Evil Eye, almost no off-spawns to dodge." },
      { n: "Deep Ant Tunnel I", street: "Dungeon", lo: 51000, hi: 80000, playedAt: 38, mob: "Zombie Mushroom", mobLv: 24, portals: 2, why: "Broadest spread; best if you stay on one map through 30." },
      { n: "Ant Tunnel Park", street: "Dungeon", lo: 51000, hi: 61000, playedAt: 36, mob: "Evil Eye", mobLv: 27, portals: 0, why: "Evil Eye dominant, potions on the map." }
    ]},
    { lo: 31, hi: 35, maps: [
      { n: "The Burnt Land V", street: "Warning Street", lo: 63000, hi: 104000, playedAt: 43, mob: "Fire Boar", mobLv: 32, portals: 8, why: "Fire Boar grind; the quests out here pay EXP too." },
      { n: "Line 2 <Area 1>", street: "Kerning City Subway", lo: 64000, hi: 73000, playedAt: 40, mob: "Jr. Wraith", mobLv: 34, portals: 5, why: "Subway density plus a perfect level match for 32-34." },
      { n: "Line 1 <Area 2>", street: "Kerning City Subway", lo: 56000, hi: 69000, playedAt: 38, mob: "Jr. Wraith", mobLv: 34, portals: 5, why: "Less popular than Line 2, so often quieter." }
    ]},
    { lo: 36, hi: 40, maps: [
      { n: "Monkey Forest II", street: "Victoria Road", lo: 109000, hi: 129000, playedAt: 45, mob: "Zombie Lupin", mobLv: 41, portals: 5, why: "Mixed Lupin and Zombie Lupin; stretches into lv 40+ as you level." },
      { n: "Lorang Lorang", street: "Florina Road", lo: 101000, hi: 124000, playedAt: 46, mob: "Lorang", mobLv: 38, portals: 2, why: "Quieter than the main Lorang spot when that is packed." },
      { n: "Forgotten Entrance III", street: "Shallow Passage", lo: 100000, hi: 100000, playedAt: 45, mob: "Glowshroom", mobLv: 39, portals: 6, why: "Glowshrooms and Raffles hold 33 of the 44 spawn points." },
      { n: "A Look-Out Shed Around the Beach", street: "Florina Road", lo: 75000, hi: 94000, playedAt: 43, mob: "Lorang", mobLv: 38, portals: 1, why: "Lorang density with Lupin support; nearly on top of the potions." }
    ]},
    { lo: 41, hi: 45, maps: [
      { n: "Tree Dungeon, Monkey Forest II", street: "Hidden Street", lo: 107000, hi: 141000, playedAt: 52, mob: "Zombie Lupin", mobLv: 41, portals: 6, why: "Hidden Street alternative that fewer players know about." },
      { n: "Dangerous Valley", street: "Hidden Street", lo: 105000, hi: 125000, playedAt: 50, mob: "Copper Drake", mobLv: 45, portals: 3, why: "Fire Boar and Copper Drake on one map." },
      { n: "Hanging Gardens of Rot", street: "Hidden Street", lo: 91000, hi: 91000, est: true, playedAt: 46, mob: "Aqumander", mobLv: 43, portals: 4, why: "29 Aqumander spawns plus Glowshrooms and Rafflesias." },
      { n: "Monkey Swamp III", street: "Hidden Street", lo: 72000, hi: 88000, playedAt: 43, mob: "Zombie Lupin", mobLv: 41, portals: 9, why: "Zombie Lupin density; low defence keeps damage respectable." }
    ]},
    { lo: 46, hi: 50, maps: [
      { n: "Forbidden Entrance", street: "Shallow Passage", lo: 133000, hi: 133000, playedAt: 52, mob: "Duskmander", mobLv: 50, portals: 3, why: "25 of 32 spawn points, and the shortest walk to pots in the bracket." },
      { n: "Line 2 <Area 3>", street: "Kerning City Subway", lo: 96000, hi: 142000, playedAt: 51, mob: "Wraith", mobLv: 48, portals: 7, why: "Pure Wraith at lv 48, nothing else spawns. Every kill is on-level." },
      { n: "Line 1 <Area 4>", street: "Kerning City Subway", lo: 95000, hi: 146000, playedAt: 51, mob: "Wraith", mobLv: 48, portals: 7, why: "Plays identically to Line 2 Area 3; pick whichever is quieter." },
      { n: "Line 2 <Area 2>", street: "Kerning City Subway", lo: 81000, hi: 114000, playedAt: 50, mob: "Wraith", mobLv: 48, portals: 6, why: "Best once you are 47-48 and can two-shot Wraiths." }
    ]},
    { lo: 51, hi: 55, maps: [
      { n: "Precipice of Darkness", street: "Deep Passage", lo: 116000, hi: 137000, playedAt: 59, mob: "Rotten Mushroom", mobLv: 56, portals: 11, why: "15 Rotten Mushroom spawns give over half the EXP." },
      { n: "Lorang and Clang", street: "Florina Road", lo: 94000, hi: 107000, playedAt: 51, mob: "Clang", mobLv: 49, portals: 3, why: "Clangs carry about 70% of the EXP, Lorangs mixed in." },
      { n: "Dangerous Croko II", street: "Warning Street", lo: 83000, hi: 83000, playedAt: 51, mob: "Croco", mobLv: 52, portals: 6, why: "Crocos carry four fifths of the EXP." }
    ]},
    { lo: 56, hi: 60, maps: [
      { n: "Sleepy Dungeon III", street: "Dungeon", lo: 145000, hi: 167000, playedAt: 59, mob: "Dark Stone Golem", mobLv: 58, portals: 5, why: "45 spawn points. Go here when V is crowded." },
      { n: "Another Sanctuary", street: "Deep Passage", lo: 116000, hi: 158000, playedAt: 65, mob: "Rotten Mushroom", mobLv: 56, portals: 13, why: "Rotten Mushrooms and Sporewoods give 91% of the EXP." },
      { n: "Sleepy Dungeon V", street: "Dungeon", lo: 98000, hi: 98000, playedAt: 57, mob: "Dark Stone Golem", mobLv: 58, portals: 5, why: "88 spawn points, the most in this bracket." }
    ]}
  ];

  /**
   * Mob stats, from the monster database.
   *
   * `hpPerExp` is the number that matters: multiply measured EXP/hour by it to
   * get damage per hour, with no kill count and no mob HP needed separately.
   *
   * Only mobs verified against the database are listed. Anything missing makes
   * the UI fall back to a clear rate rather than show an invented figure, so
   * adding entries here is purely additive.
   */
  var MOBS = {
    "Cold Eye": { level: 40, hp: 1894, exp: 85, hpPerExp: 1894 / 85 }
  };

  function bracketFor(level) {
    for (var i = 0; i < BRACKETS.length; i++) {
      if (level >= BRACKETS[i].lo && level <= BRACKETS[i].hi) return BRACKETS[i];
    }
    return null;
  }

  function mob(name) {
    return MOBS[name] || null;
  }

  /**
   * Effective DPS: damage per second actually landed on monsters that died.
   *
   * Overkill, misses and damage to mobs you did not finish are all excluded,
   * so this reads lower than a burst number off a damage meter. For deciding
   * where to grind that is the more useful of the two.
   */
  function effectiveDps(expPerHour, mobName) {
    var m = mob(mobName);
    if (!m || !(expPerHour > 0)) return null;
    return (expPerHour * m.hpPerExp) / 3600;
  }

  /**
   * Rank alternatives to where you are now.
   *
   * `measuredRate` is your own observed EXP/hour, which is truth. The map
   * figures are other players' logs, so a candidate is only offered when its
   * low end beats what you are already getting — no "this might be worse".
   *
   * `keep` is the list of maps currently on screen, and it exists because
   * "is this map better than what I am getting" is a question with a sharp
   * edge and a moving input. The measured rate is a trailing average that
   * changes every time a kill lands or an old one ages out of the window, so
   * a map sitting near the cut-off crosses it constantly, and the panel
   * blinks. A map already being offered is therefore held until it is
   * clearly not worth it, rather than dropped the instant it dips.
   */
  var ENTER = 0.05;
  var LEAVE = 0.02;

  function recommend(level, measuredRate, currentMapName, remainingExp, keep) {
    var b = bracketFor(level);
    if (!b) return { bracket: null, picks: [], reason: "no map data for level " + level };

    var held = {};
    (keep || []).forEach(function (n) { held[n] = true; });

    var picks = b.maps
      .filter(function (m) { return m.n !== currentMapName; })
      .map(function (m) {
        // saveMin comes from the map's LOW logged rate, so it is the
        // conservative end of the saving; saveMax from the high end.
        var out = { map: m, saveMin: null, saveMax: null, betterBy: null };
        if (measuredRate > 0) {
          out.betterBy = m.lo / measuredRate - 1;
          if (remainingExp > 0) {
            var now = remainingExp / measuredRate;
            out.saveMin = now - remainingExp / m.lo;
            out.saveMax = now - remainingExp / m.hi;
          }
        }
        return out;
      })
      .filter(function (p) {
        if (p.betterBy === null) return true;
        return p.betterBy > (held[p.map.n] ? LEAVE : ENTER);
      })
      .sort(function (a, b2) { return b2.map.lo - a.map.lo; });

    return { bracket: b, picks: picks, onRankedMap: b.maps.some(function (m) { return m.n === currentMapName; }) };
  }

  return {
    BRACKETS: BRACKETS,
    MOBS: MOBS,
    bracketFor: bracketFor,
    mob: mob,
    effectiveDps: effectiveDps,
    recommend: recommend,
    COVERAGE: { lo: 1, hi: 60 }
  };
});
