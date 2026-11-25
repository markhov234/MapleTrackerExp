/**
 * Rotating the overlay's palette onto a region's own hue.
 *
 * The hue comes off the badge in the minimap's header — the game artists'
 * colour for that region — so the panel can follow the player from Henesys
 * to Kerning City without anybody writing a palette for either.
 *
 * The one rule here is that **only the hue moves**. Each token keeps the
 * saturation and lightness it was designed with, which is what makes this
 * safe to do automatically: contrast is a function of lightness, so text
 * that reads against one region's colours reads against all of them. A
 * scheme that picked whole colours per region could not promise that.
 *
 * Semantic colours are not in the list. Red has to keep meaning red.
 */
(function (root, factory) {
  if (typeof module === "object" && module.exports) module.exports = factory();
  else root.THEME = factory();
})(typeof self !== "undefined" ? self : this, function () {
  "use strict";

  /** The tokens that carry the theme's colour. */
  var TOKENS = [
    "--c-bg", "--c-surface", "--c-elevated",
    "--c-accent", "--c-accent-soft", "--c-accent-softer", "--c-accent-line",
    "--c-accent-deep", "--c-accent-text",
    "--c-border", "--c-border-strong", "--c-divider",
    "--c-text-muted", "--c-text-dim", "--c-text-faint"
  ];

  /** `#abc`, `#aabbcc`, `rgb(...)` and `rgba(...)` — what the tokens use. */
  function parse(v) {
    v = (v || "").trim();
    var m = v.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
    if (m) {
      var x = m[1];
      if (x.length === 3) x = x[0] + x[0] + x[1] + x[1] + x[2] + x[2];
      return {
        r: parseInt(x.slice(0, 2), 16),
        g: parseInt(x.slice(2, 4), 16),
        b: parseInt(x.slice(4, 6), 16),
        a: 1, hex: true
      };
    }
    m = v.match(/^rgba?\(([^)]+)\)$/i);
    if (!m) return null;
    var p = m[1].split(",").map(function (t) { return parseFloat(t); });
    if (p.length < 3 || p.some(isNaN)) return null;
    return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1, hex: false };
  }

  function toHsl(c) {
    var r = c.r / 255, g = c.g / 255, b = c.b / 255;
    var mx = Math.max(r, g, b), mn = Math.min(r, g, b), d = mx - mn;
    var h = 0;
    if (d) {
      if (mx === r) h = ((g - b) / d) % 6;
      else if (mx === g) h = (b - r) / d + 2;
      else h = (r - g) / d + 4;
      h *= 60;
      if (h < 0) h += 360;
    }
    var l = (mx + mn) / 2;
    return { h: h, s: d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1)), l: l };
  }

  function toRgb(h, s, l) {
    h = ((h % 360) + 360) % 360;
    var c = (1 - Math.abs(2 * l - 1)) * s;
    var x = c * (1 - Math.abs((h / 60) % 2 - 1));
    var m = l - c / 2;
    var p = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x]
          : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
    return p.map(function (v) { return Math.round((v + m) * 255); });
  }

  /** The hue of a colour, in degrees; 0 for anything unparseable. */
  function hueOf(value) {
    var c = parse(value);
    return c ? toHsl(c).h : 0;
  }

  /**
   * The same colour, turned `delta` degrees around the wheel.
   *
   * Returns the input untouched if it cannot be parsed, so an unexpected
   * token value leaves the panel looking as it did rather than blank.
   */
  function rotated(value, delta) {
    var c = parse(value);
    if (!c) return value;
    var hsl = toHsl(c);
    var rgb = toRgb(hsl.h + delta, hsl.s, hsl.l);
    if (c.hex) {
      return "#" + rgb.map(function (v) {
        return ("0" + Math.max(0, Math.min(255, v)).toString(16)).slice(-2);
      }).join("");
    }
    return "rgba(" + rgb[0] + ", " + rgb[1] + ", " + rgb[2] + ", " + c.a + ")";
  }

  return { TOKENS: TOKENS, parse: parse, toHsl: toHsl, hueOf: hueOf, rotated: rotated };
});
