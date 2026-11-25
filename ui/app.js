/**
 * EXP Calc — UI layer.
 *
 * This file only reads inputs, calls the deterministic engine in exp.js, and
 * paints the result. It contains no EXP arithmetic of its own, so the
 * assistant and the panels can never disagree about a number.
 */
(function () {
  "use strict";

  var $ = function (id) { return document.getElementById(id); };
  var fmt = EXP.formatExp;

  var settings = {
    assistant: true,
    motion: false,
    compact: false,
    live: true
  };

  var state = { expMode: "abs", lastPlan: null, lastParty: null };

  /* ---------------- settings persistence ---------------- */

  function loadSettings() {
    try {
      var raw = localStorage.getItem("expcalc.settings");
      if (raw) {
        var s = JSON.parse(raw);
        Object.keys(settings).forEach(function (k) {
          if (typeof s[k] === "boolean") settings[k] = s[k];
        });
      }
    } catch (e) { /* private window or blocked storage: defaults are fine */ }
  }
  function saveSettings() {
    try { localStorage.setItem("expcalc.settings", JSON.stringify(settings)); } catch (e) {}
  }
  function applySettings() {
    document.documentElement.setAttribute("data-reduced-motion", String(settings.motion));
    $("s-assistant").setAttribute("aria-checked", String(settings.assistant));
    $("s-motion").setAttribute("aria-checked", String(settings.motion));
    $("s-compact").setAttribute("aria-checked", String(settings.compact));
    $("s-live").setAttribute("aria-checked", String(settings.live));
    $("assistant").hidden = !settings.assistant;
    if (!settings.assistant) $("main").setAttribute("data-assistant", "hidden");
    else if ($("main").getAttribute("data-assistant") === "hidden") $("main").setAttribute("data-assistant", "open");
  }

  /** Stat-tile formatting honours the compact setting; tables never do. */
  function stat(n) {
    if (n === null || n === undefined || !isFinite(n)) return "—";
    return settings.compact ? EXP.compact(n) : fmt(n);
  }

  /* ---------------- reading the form ---------------- */

  function num(id, dflt) {
    var v = parseFloat($(id).value);
    return isFinite(v) ? v : (dflt === undefined ? 0 : dflt);
  }

  function readInputs() {
    var level = Math.max(1, Math.min(99, Math.floor(num("f-level", 1))));
    var target = Math.max(2, Math.min(100, Math.floor(num("f-target", level + 1))));
    var levelCost = EXP.toNext[level];

    var expInto;
    if (state.expMode === "pct") {
      expInto = Math.round((Math.max(0, Math.min(100, num("f-exp", 0))) / 100) * levelCost);
    } else {
      expInto = Math.max(0, Math.min(levelCost, Math.floor(num("f-exp", 0))));
    }

    var members = Math.max(1, Math.min(6, Math.floor(num("f-party", 1))));
    var attackers = Math.max(1, Math.min(members, Math.floor(num("f-attackers", 1))));
    var levelSumRaw = parseFloat($("f-levelsum").value);
    var multiplier = Math.max(0.1, num("f-mult", 1));

    return {
      level: level,
      target: target,
      expInto: expInto,
      levelCost: levelCost,
      rate: Math.max(0, num("f-rate", 0)),
      mobExp: Math.max(0, num("f-mobexp", 0)),
      members: members,
      attackers: attackers,
      eligibleLevels: isFinite(levelSumRaw) && levelSumRaw > 0 ? levelSumRaw : level * members,
      damageShare: Math.max(0, Math.min(100, num("f-dmg", 100))) / 100,
      multiplier: multiplier
    };
  }

  /* ---------------- charts ---------------- */

  function vb(svg) {
    var p = svg.getAttribute("viewBox").split(/\s+/).map(Number);
    return { w: p[2], h: p[3] };
  }
  function el(name, attrs) {
    var n = document.createElementNS("http://www.w3.org/2000/svg", name);
    for (var k in attrs) n.setAttribute(k, attrs[k]);
    return n;
  }

  /**
   * Draws EXP-per-level on a log y axis.
   * `hot` optionally highlights a [from, to] level span.
   */
  function drawCurve(svg, hot, colorBands) {
    var size = vb(svg), W = size.w, H = size.h;
    var pad = { l: 48, r: 14, t: 14, b: 26 };
    var x0 = pad.l, x1 = W - pad.r, y0 = H - pad.b, y1 = pad.t;
    while (svg.firstChild) svg.removeChild(svg.firstChild);

    var lo = Math.log10(EXP.toNext[1]);        // 15
    var hi = Math.log10(EXP.toNext[99]);       // ~9.48e9
    var X = function (lv) { return x0 + ((lv - 1) / 98) * (x1 - x0); };
    var Y = function (v) { return y0 - ((Math.log10(v) - lo) / (hi - lo)) * (y0 - y1); };

    // y gridlines at decades
    for (var d = Math.ceil(lo); d <= Math.floor(hi); d++) {
      var yy = Y(Math.pow(10, d));
      svg.appendChild(el("line", { class: "axis", x1: x0, y1: yy, x2: x1, y2: yy }));
      var t = el("text", { x: x0 - 7, y: yy + 3, "text-anchor": "end" });
      t.textContent = EXP.compact(Math.pow(10, d));
      svg.appendChild(t);
    }
    // x labels
    [1, 20, 40, 60, 80, 99].forEach(function (lv) {
      var tx = el("text", { x: X(lv), y: y0 + 16, "text-anchor": "middle" });
      tx.textContent = lv;
      svg.appendChild(tx);
    });
    // band boundaries
    [50, 89].forEach(function (lv) {
      svg.appendChild(el("line", { class: "band-line", x1: X(lv), y1: y1, x2: X(lv), y2: y0 }));
    });

    function path(from, to) {
      var pts = [];
      for (var lv = from; lv <= to; lv++) pts.push((lv === from ? "M" : "L") + X(lv).toFixed(1) + " " + Y(EXP.toNext[lv]).toFixed(1));
      return pts.join(" ");
    }

    if (colorBands) {
      var bands = [[1, 50, "var(--c-band-quad)"], [50, 89, "var(--c-band-slow)"], [89, 99, "var(--c-band-fast)"]];
      bands.forEach(function (b) {
        svg.appendChild(el("path", { d: path(b[0], b[1]), fill: "none", stroke: b[2], "stroke-width": 2.2, "stroke-linejoin": "round" }));
      });
    } else {
      svg.appendChild(el("path", { class: "curve", d: path(1, 99) }));
    }

    if (hot && hot.to > hot.from) {
      var a = X(hot.from), b = X(Math.min(99, hot.to));
      svg.appendChild(el("rect", { class: "span", x: a, y: y1, width: Math.max(1, b - a), height: y0 - y1 }));
      svg.appendChild(el("path", { class: "curve-hot", d: path(hot.from, Math.min(99, hot.to)) }));
      svg.appendChild(el("circle", { class: "dot", cx: a, cy: Y(EXP.toNext[hot.from]), r: 4 }));
      var tl = Math.min(99, hot.to);
      svg.appendChild(el("circle", { class: "dot-hollow", cx: X(tl), cy: Y(EXP.toNext[tl]), r: 4 }));
    } else if (hot) {
      svg.appendChild(el("circle", { class: "dot", cx: X(hot.from), cy: Y(EXP.toNext[hot.from]), r: 4 }));
    }
  }

  /* ---------------- rendering ---------------- */

  function recalc() {
    var i = readInputs();

    var party = EXP.partyExpPerMob({
      mobExp: i.mobExp,
      eligibleMembers: i.members,
      attackers: i.attackers,
      yourLevel: i.level,
      eligibleLevels: i.eligibleLevels,
      damageShare: i.damageShare
    });
    var effPerMob = party.perMob * i.multiplier;

    var plan = EXP.plan({
      level: i.level,
      expInto: i.expInto,
      targetLevel: i.target,
      expPerHour: i.rate,
      expPerMob: effPerMob
    });

    state.lastPlan = plan;
    state.lastParty = party;
    state.lastInputs = i;

    // headline
    $("r-cap").textContent = "Time to level " + plan.target;
    $("r-time").textContent = EXP.formatHours(plan.hours);
    if (plan.finishAt) {
      $("r-sub").textContent = "At " + fmt(i.rate) + " EXP/hr, finishing around " +
        plan.finishAt.toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
    } else {
      $("r-sub").textContent = "Enter a measured EXP per hour to get a time estimate.";
    }

    // tiles
    $("r-total").textContent = stat(plan.totalNeeded);
    $("r-total-n").textContent = "from " + plan.level + " to " + plan.target;
    $("r-kills").textContent = plan.killsNeeded === null ? "—" : stat(plan.killsNeeded);
    $("r-kills-n").textContent = effPerMob > 0 ? "at " + fmt(Math.round(effPerMob)) + " per kill" : "set a mob EXP";
    $("r-permob").textContent = effPerMob > 0 ? stat(Math.round(effPerMob)) : "—";
    $("r-permob-n").textContent = party.solo
      ? (i.multiplier !== 1 ? "solo x" + i.multiplier : "solo")
      : "party share " + fmt(party.share) + " + " + Math.round(party.bonusPct * 100) + "% bonus";
    $("r-kph").textContent = plan.killsPerHour ? stat(Math.round(plan.killsPerHour)) : "—";

    // this level
    $("r-lv").textContent = plan.level;
    $("r-pct").textContent = (plan.levelProgress * 100).toFixed(2);
    $("r-rem").textContent = fmt(plan.thisLevelRemaining) + " EXP";
    $("r-bar").style.width = (plan.levelProgress * 100).toFixed(2) + "%";
    var band = EXP.bandFor(plan.level);
    $("r-band").textContent = "Lv " + plan.level + " band: " + band.detail;

    // unit hint on the exp field
    $("f-exp-unit").textContent = state.expMode === "pct" ? "0-100%" : "of " + fmt(i.levelCost);

    // chart + steps
    drawCurve($("chart-plan"), { from: plan.level, to: plan.target }, false);

    var tb = $("r-steps");
    tb.textContent = "";
    var shown = plan.steps.slice(0, 14);
    shown.forEach(function (s) {
      var tr = document.createElement("tr");
      tr.innerHTML = "<td>" + s.level + " → " + s.toLevel + "</td><td>" + fmt(s.cost) +
        "</td><td>" + fmt(s.cumulative) + "</td><td>" + EXP.formatHours(s.hours) +
        "</td><td class=\"band\">" + s.band + "</td>";
      tb.appendChild(tr);
    });
    $("r-steps-n").textContent = plan.steps.length > shown.length
      ? shown.length + " of " + plan.steps.length + " levels"
      : plan.steps.length + " level" + (plan.steps.length === 1 ? "" : "s");

    renderRecommendations();
  }

  function renderCurveView() {
    drawCurve($("chart-curve"), null, true);
    var tb = $("curve-rows");
    tb.textContent = "";
    var here = state.lastPlan ? state.lastPlan.level : 1;
    for (var lv = 1; lv <= 99; lv++) {
      var tr = document.createElement("tr");
      if (lv === here) tr.setAttribute("data-here", "true");
      tr.innerHTML = "<td>" + lv + "</td><td>" + fmt(EXP.toNext[lv]) + "</td><td>" +
        fmt(EXP.cumulative[lv]) + "</td><td class=\"band\">" + EXP.bandFor(lv).name + "</td>";
      tb.appendChild(tr);
    }
  }

  /* ---------------- assistant (deterministic, local) ---------------- */

  function rec(kind, title, body) {
    return { kind: kind, title: title, body: body };
  }

  function buildRecommendations() {
    var p = state.lastPlan, i = state.lastInputs, party = state.lastParty;
    var out = [];
    if (!p) return out;

    if (i.rate <= 0) {
      out.push(rec("warn", "No rate set", "Everything but the EXP totals needs a measured EXP per hour. Grind one clean hour and put the number in."));
    }
    if (p.target > 90 && p.level < 90) {
      var wall = EXP.expBetween(90, 0, p.target);
      out.push(rec("alert", "You are crossing the 90 wall",
        "Levels 90 and up cost 2.1× the one before. " + fmt(wall) + " of your " +
        fmt(p.totalNeeded) + " total sits above level 90 — " +
        Math.round((wall / p.totalNeeded) * 100) + "% of the whole plan."));
    }
    if (isFinite(p.hours) && p.hours > 80) {
      var interim = Math.min(p.target - 1, p.level + Math.max(1, Math.floor((p.target - p.level) / 3)));
      if (interim > p.level) {
        var ip = EXP.plan({ level: p.level, expInto: p.expInto, targetLevel: interim, expPerHour: i.rate, expPerMob: party.perMob * i.multiplier });
        out.push(rec("warn", "That is a long haul",
          EXP.formatHours(p.hours) + " at this rate. Level " + interim + " is a closer checkpoint at " +
          EXP.formatHours(ip.hours) + "."));
      }
    }
    if (!party.solo) {
      var solo = i.mobExp * i.multiplier;
      var eff = party.perMob * i.multiplier;
      var delta = solo > 0 ? Math.round(((eff - solo) / solo) * 100) : 0;
      out.push(rec("info", "Party maths",
        "Your share is " + fmt(party.share) + " plus a " + Math.round(party.bonusPct * 100) +
        "% size bonus, so " + fmt(Math.round(eff)) + " per kill against " + fmt(Math.round(solo)) +
        " solo — " + (delta >= 0 ? "+" : "") + delta + "%. Note the 20% damage term means party EXP does not arrive in clean multiples, so kill counts cannot be recovered from EXP while partied."));
    }
    if (i.multiplier !== 1) {
      out.push(rec("info", "Multiplier is on",
        "At ×" + i.multiplier + ", your normalised rate is " + fmt(Math.round(i.rate / i.multiplier)) +
        " EXP/hr. Compare maps on the normalised figure or a coupon session will flatter a bad map."));
    }
    if (p.level < 50 && p.target >= 50) {
      out.push(rec("info", "Level 50 in range",
        "Reaching 50 is under 0.04% of the climb to 100 — " + fmt(EXP.cumulative[50]) +
        " of " + fmt(EXP.cumulative[100]) + ". The curve stays gentle until 90."));
    }
    if (p.killsPerHour && p.killsPerHour > 0) {
      out.push(rec("info", "Implied pace",
        "Your rate works out at " + fmt(Math.round(p.killsPerHour)) + " kills/hr, about " +
        Math.round(p.killsPerHour / 60) + " a minute. If that is far off what you actually see, the mob EXP or the rate is wrong."));
    }
    return out;
  }

  function renderRecommendations() {
    if (!settings.assistant) return;
    var host = $("a-recs");
    host.textContent = "";
    var recs = buildRecommendations();
    if (!recs.length) {
      host.innerHTML = '<div class="rec"><span class="rt">Nothing to flag</span><span class="rb">The plan looks straightforward at this rate.</span></div>';
      return;
    }
    recs.forEach(function (r) {
      var d = document.createElement("div");
      d.className = "rec " + (r.kind === "alert" ? "alert" : r.kind === "warn" ? "warn" : "");
      var t = document.createElement("span"); t.className = "rt"; t.textContent = r.title;
      var b = document.createElement("span"); b.className = "rb"; b.textContent = r.body;
      d.appendChild(t); d.appendChild(b);
      host.appendChild(d);
    });
  }

  var PRESETS = [
    "How long to my target?",
    "How many kills?",
    "What does a party do?",
    "Why is level 90 so expensive?",
    "Is my rate any good?"
  ];

  /** Rule-based answers over the current plan. No model, no network. */
  function answer(q) {
    var p = state.lastPlan, i = state.lastInputs, party = state.lastParty;
    var s = q.toLowerCase();
    if (!p) return "Run a calculation first and I will have numbers to work from.";

    if (/how long|time|hours|when/.test(s)) {
      if (!isFinite(p.hours)) return "I need a measured EXP per hour before I can put a time on it.";
      return "Level " + p.level + " to " + p.target + " is " + fmt(p.totalNeeded) + " EXP. At " +
        fmt(i.rate) + " EXP/hr that is " + EXP.formatHours(p.hours) + ", finishing around " +
        p.finishAt.toLocaleString(undefined, { weekday: "short", hour: "numeric", minute: "2-digit" }) + ".";
    }
    if (/kill|mob|monster/.test(s)) {
      if (p.killsNeeded === null) return "Set a mob base EXP and I can turn the total into a kill count.";
      return fmt(p.killsNeeded) + " kills at " + fmt(Math.round(party.perMob * i.multiplier)) +
        " EXP each. That is " + fmt(Math.round(p.killsPerHour)) + " an hour at your current rate.";
    }
    if (/party|share|split|group/.test(s)) {
      if (party.solo) return "You are set to solo, so you get the mob's full EXP. Set eligible members above 1 and I will split it: 80% by level, 20% by damage, plus a size bonus of 10% per extra member up to 30%, and 5% more per extra member who hits it.";
      return "With " + i.members + " eligible and " + i.attackers + " hitting, your share is " +
        fmt(party.share) + " and the bonus is " + Math.round(party.bonusPct * 100) + "%, so " +
        fmt(party.perMob) + " per kill before multipliers.";
    }
    if (/90|wall|expensive|steep|curve/.test(s)) {
      return "Levels 1 to 89 follow the old pre-Big-Bang rates. From 90 each level costs 2.1× the one before, so level 90 needs " +
        fmt(EXP.toNext[90]) + " and level 99 needs " + fmt(EXP.toNext[99]) +
        ". Reaching 90 is only 0.6% of the total climb to 100.";
    }
    if (/rate|good|fast|slow|per hour/.test(s)) {
      if (i.rate <= 0) return "No rate set yet. Grind one clean hour, read the EXP gained, and enter it.";
      var lvPerHr = i.rate / EXP.toNext[p.level];
      return "At " + fmt(i.rate) + " EXP/hr you clear " + lvPerHr.toFixed(2) +
        " of level " + p.level + " per hour, so about " + EXP.formatHours(1 / lvPerHr) +
        " per level at this point in the curve." +
        (i.multiplier !== 1 ? " Normalised for your ×" + i.multiplier + " multiplier that is " + fmt(Math.round(i.rate / i.multiplier)) + " EXP/hr." : "");
    }
    if (/level|need|total|exp/.test(s)) {
      return "Level " + p.level + " costs " + fmt(p.levelCost) + " and you have " + fmt(p.expInto) +
        " banked, so " + fmt(p.thisLevelRemaining) + " to go. The whole run to " + p.target +
        " is " + fmt(p.totalNeeded) + ".";
    }
    return "I answer from the numbers on screen: time to target, kills needed, party splits, the level 90 curve, and whether your rate is reasonable. Try one of the chips above.";
  }

  function say(who, text) {
    var log = $("a-log");
    var d = document.createElement("div");
    d.className = "msg" + (who === "You" ? " you" : "");
    var w = document.createElement("div"); w.className = "who"; w.textContent = who;
    var b = document.createElement("div"); b.className = "bubble"; b.textContent = text;
    d.appendChild(w); d.appendChild(b);
    log.appendChild(d);
    $("a-scroll").scrollTop = $("a-scroll").scrollHeight;
  }

  /* ---------------- wiring ---------------- */

  function setAssistantOpen(open) {
    $("main").setAttribute("data-assistant", open ? "open" : "closed");
    var t = $("a-toggle");
    t.setAttribute("aria-expanded", String(open));
    t.setAttribute("aria-label", open ? "Collapse assistant" : "Expand assistant");
    t.style.transform = open ? "" : "rotate(180deg)";
  }

  function switchView(name) {
    [].forEach.call(document.querySelectorAll(".view"), function (v) {
      v.setAttribute("data-active", String(v.getAttribute("data-view") === name));
    });
    [].forEach.call(document.querySelectorAll(".nav-item"), function (b) {
      if (b.getAttribute("data-view") === name) b.setAttribute("aria-current", "page");
      else b.removeAttribute("aria-current");
    });
    if (name === "curve") renderCurveView();
  }

  function init() {
    loadSettings();
    applySettings();

    // nav, with arrow-key movement
    var navs = [].slice.call(document.querySelectorAll(".nav-item"));
    navs.forEach(function (b, idx) {
      b.addEventListener("click", function () { switchView(b.getAttribute("data-view")); });
      b.addEventListener("keydown", function (e) {
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        e.preventDefault();
        var next = navs[(idx + (e.key === "ArrowDown" ? 1 : navs.length - 1)) % navs.length];
        next.focus();
      });
    });

    // exp entry mode
    [].forEach.call($("f-expmode").querySelectorAll("button"), function (b) {
      b.addEventListener("click", function () {
        var mode = b.getAttribute("data-mode");
        if (mode === state.expMode) return;
        var i = readInputs();
        state.expMode = mode;
        [].forEach.call($("f-expmode").querySelectorAll("button"), function (o) {
          o.setAttribute("aria-pressed", String(o.getAttribute("data-mode") === mode));
        });
        // convert the value so the displayed position does not jump
        if (mode === "pct") {
          $("f-exp").value = (i.levelCost > 0 ? (i.expInto / i.levelCost) * 100 : 0).toFixed(2);
          $("f-exp-label").textContent = "Percent into level";
          $("f-exp").step = "0.01"; $("f-exp").max = "100";
        } else {
          $("f-exp").value = String(i.expInto);
          $("f-exp-label").textContent = "EXP banked";
          $("f-exp").step = "1"; $("f-exp").removeAttribute("max");
        }
        recalc();
      });
    });

    // inputs
    var ids = ["f-level", "f-target", "f-exp", "f-rate", "f-mobexp", "f-party", "f-attackers", "f-levelsum", "f-mult"];
    ids.forEach(function (id) {
      $(id).addEventListener("input", function () { if (settings.live) recalc(); });
    });
    $("f-dmg").addEventListener("input", function () {
      $("f-dmg-out").textContent = $("f-dmg").value;
      if (settings.live) recalc();
    });
    $("go").addEventListener("click", recalc);

    // settings switches
    [["s-assistant", "assistant"], ["s-motion", "motion"], ["s-compact", "compact"], ["s-live", "live"]]
      .forEach(function (pair) {
        $(pair[0]).addEventListener("click", function () {
          settings[pair[1]] = !settings[pair[1]];
          saveSettings();
          applySettings();
          recalc();
        });
      });

    // assistant collapse
    $("a-toggle").addEventListener("click", function () {
      setAssistantOpen($("main").getAttribute("data-assistant") === "closed");
    });

    // In a narrow window the panel floats over the content, so start it stowed
    // rather than covering the results the moment the app opens.
    if (window.matchMedia("(max-width: 1100px)").matches) setAssistantOpen(false);

    // preset chips
    PRESETS.forEach(function (q) {
      var c = document.createElement("button");
      c.className = "chip"; c.type = "button"; c.textContent = q;
      c.addEventListener("click", function () { say("You", q); say("Assistant", answer(q)); });
      $("a-chips").appendChild(c);
    });

    $("a-form").addEventListener("submit", function (e) {
      e.preventDefault();
      var v = $("a-input").value.trim();
      if (!v) return;
      $("a-input").value = "";
      say("You", v);
      say("Assistant", answer(v));
    });

    recalc();
    say("Assistant", "Loaded your curve. Level " + state.lastPlan.level + " needs " +
      fmt(state.lastPlan.thisLevelRemaining) + " more EXP. Ask me anything about the plan.");
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
