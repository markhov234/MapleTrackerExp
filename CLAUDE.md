# EXP Calc

A Windows desktop overlay that watches a MapleStory window and reports EXP,
level, rate, time-to-level, kills and loot for a play session. Tauri 2 shell,
Rust core, plain HTML/JS panel.

It reads the game **by looking at it**. Nothing else.

## The boundary (read this before adding anything)

This program may:

- capture pixels of the game window through the Windows compositor
  (`windows-capture`), the same path a screen recorder uses;
- receive one registered global hotkey.

It must **never**:

- read or write game process memory (`OpenProcess`, `ReadProcessMemory`);
- synthesise input (`SendInput`, `keybd_event`, `PostMessage` to the client);
- capture, proxy or inspect network traffic;
- read, patch or ship game files.

Every feature in here has been built inside that line, including the ones
where it was genuinely inconvenient. If a feature seems to need crossing it,
the answer is that the feature doesn't happen — not that the line moves. Say
so plainly rather than building a version that quietly cheats.

The same rule governs the third-party program that was reverse engineered for
reference (see "Prior art" below): **static analysis only, never run, never
redistributed.**

## How it reads the screen

Three ideas do most of the work. Understanding them is most of understanding
the codebase.

### 1. Unstretch (`src/unstretch.rs`)

The client renders at a fixed internal resolution and Windows scales the
window up. A 2560x1440 window is often the client's own 1280x720 doubled. Read
pixels straight off that and the bitmap font is a blurred mess that only
matches templates approximately.

So: resample back onto the client's own grid first, and the font becomes
*exact*. Two invariants that cost real time to find —

- the grid is anchored at the **frame origin**, not at the region being read;
- resampling happens on **grey/colour values, before thresholding**. Shrinking
  an already-thresholded bitmap averages serifs away; it once collapsed `[`
  and `]` into the same glyph.

`Profile::scale(w, h)` derives the scale, `candidates()` offers the plausible
grids, and the right one is whichever verifies (below). There are no hardcoded
coordinates and no per-resolution calibration: the client was taught at
1366x768 and reads fine at 1280x720 with no changes.

Plates come in flavours because different measurements want different
channels: `plate()` (luma), `plate_with(f)` (any measure — `warm` = `min(R,G)`
separates the pale-grey and gold notice text from green foliage), `plate_rgb()`
(colour preserved, for the region badge and slot icons).

### 2. Find things by content, verify by arithmetic

The EXP field is located by **shape** — `<digits>[<digits>.<digits>%]` — and
then confirmed by the identity that must hold between the absolute EXP and the
percentage at that level. `ocr::locate()` is the shared entry point and also
answers "is this even the right window?": the test is not "the frame is
2560x1440", it is "a readout exists whose number divides into its percentage".

**The worst bug in this project's history came from skipping that check.** The
reader has a clean path (every glyph recognised) and a reconciling path
(some unknown). The arithmetic check lived only in the reconciling path. An
unstretch improvement made the clean path always taken, so the only check
became unreachable, and the session reported **2,993,820,528 EXP gained**.
`agrees()` now guards both paths and `Session::observe` caps a delta at the
level's own total. Four tests cover it, including that literal number. Don't
remove them.

### 3. A half-taught reader must say so, not guess

Each reader is gated on an `enabled` flag in the profile and refuses to
produce numbers from glyphs it hasn't been taught. `native-atlas teach`
**refuses to write** an atlas where two characters share a bitmap. Zero kills
because nothing died and zero kills because nothing is being counted must not
look alike — hence `Snapshot::counting`.

The one deliberate exception, added last: an EXP notice whose *sentence* reads
but whose *digits* don't still counts as a kill, with `amount: None`. That a
monster died and what it paid are separate evidence; losing the second
shouldn't destroy the first. `exp_per_kill` divides by `kills_valued` so
unpriced kills don't drag the average down. See the module docs in
`src/chat.rs`.

### 4. The capture callback does almost nothing

Windows Graphics Capture calls `on_frame_arrived` on its own thread while
holding a frame the desktop compositor wants back. Reading the status bar is
tens of milliseconds on a good frame and a few hundred on one with no status
bar in it, because then every candidate grid gets swept. Doing that in the
callback stalls the compositor's queue, and what the user sees is **their
whole desktop hitching** for a second or two whenever the app starts and
walks through candidate windows. That was a real, reported bug.

So `Capture` copies the pixels, hands them to a worker over a channel of
capacity **one**, and returns. `Reader` does everything else on its own
thread. A frame that arrives while the reader is busy is dropped, which is
correct: the reader samples twice a second by design and a stale frame is
worth nothing. Do not move analysis back into the callback, and do not grow
that channel — a queue there just converts a stall into latency plus memory.

Two supporting cuts: `candidate_windows` skips anything smaller than
640x480, because every candidate costs a D3D11 device and up to `GIVE_UP`
seconds of frames; and `ocr::locate` sweeps the cheap bottom-fifth band for
*every* grid before any full-frame band for *any* grid.

## Layout

| File | What it holds |
|---|---|
| `src/unstretch.rs` | the grid, scales, plates |
| `src/ocr.rs` | `Profile`, glyph cutting and classification, `locate()`, `read_rows()` |
| `src/chat.rs` | turning OCR'd lines into events and tallies (`classify`, `Log`, `Tally`) |
| `src/tracker.rs` | `Capture` (the callback) and `Reader` (the analysis), on two threads |
| `src/session.rs` | live state and `Snapshot`, the single struct the UI receives |
| `src/history.rs` | `sessions.jsonl` + `current.json`, range totals, and the weekly `review` |
| `src/region.rs` | minimap badge -> region hue |
| `src/mesos.rs` | the purse, read off the inventory's own total |
| `src/avatar.rs` | find the player's own sprite; shared by the overlay and `find-character` |
| `src/expcurve.rs` | EXP-to-next-level table |
| `src/notes.rs` | file logging; a release build has no console, so `note!` is the only way to see anything |
| `ui/overlay.html` | the panel |
| `ui/theme.js` | region hue rotation — **only hue moves**, so contrast can't drift |
| `ui/tokens.css` | the palette, dark and light; no component rule may hardcode a colour |
| `ui/mapdata.js` | map recommendations (hysteresis + throttle; it used to flicker) |
| `assets/profile.json` | everything taught: atlas, thresholds, band geometry, per-reader `enabled` |

`assets/profile.json` is `include_str!`'d into the binary. Editing it means
rebuilding.

## Build, run, install

```powershell
cargo test                       # 64 tests
cargo build --release            # -> target/release/maple-tracker.exe
npx -y @tauri-apps/cli@2 build   # -> the installer, below
```

The bundler writes `target/release/EXP Calc.exe` and
`target/release/bundle/nsis/EXP Calc_0.1.0_x64-setup.exe`. **`cargo build`
alone does not refresh `EXP Calc.exe`** — three test cycles were once wasted
on a stale copy of it. Install silently with `/S`; it lands in
`%LOCALAPPDATA%\EXP Calc` for the current user only, no admin.

Log and data: `%APPDATA%\com.maplertracker.expcalc\` —
`exp-calc.log`, `sessions.jsonl`, `current.json`. An uninstall can take that
folder with it, so **back up `sessions.jsonl`, not just `current.json`**,
before any reinstall. (It was lost once that way.)

### The workbench

Calibration tools are behind a feature flag so a release build can't carry
them: `cargo run --features tools --bin <name>`.

- `list-windows`, `find-character`, `capture-spike`, `read-stats`, `sample-exp`
- `native-atlas` — teach the status-bar font
- `notice-atlas` — teach the floating EXP/meso lines (`show`, `teach --at Y H --text "..."`, `read`, `events`)
- `minimap-atlas` — the region badge
- `harvest` — **watches the live game and saves only frames where the watched
  band changed** (`harvest notices 600`, `harvest slots 600`). Built because
  teaching needs frames with real text in them, and asking the player to pose
  for them yields a narrow sample from one map at one moment.

`captures/` is working data and is gitignored: it is full-screen shots of a
live game, so it carries the player's own and other players' character names.

## Where things stand

Working and verified live: EXP, level, rate, time-to-level, session history,
region theming from the minimap badge (reads ~117 deg), quick slot icons shown
in the panel, kill counting switched on.

**Known gaps, roughly in order of what's worth doing next.**

1. **Finish the notice font.** 26 characters taught; missing digits
   **2 3 4 6 8** and letters **B f A R T**. Until then every kill counts but
   EXP-per-kill only averages the legible ones, and mesos from notices are
   mostly dropped. Run `harvest notices 600` while actually fighting, then
   `notice-atlas teach`. This is the cheapest unblock left.
   - `B A R T f` matter specifically for `Bonus EXP for PARTY (+n)`. Until
     they're taught that line reads as `?onus EXP ?or ?AR?Y`, which is why the
     kill rule requires the literal word `received` rather than merely the
     absence of `bonus`/`party` — otherwise a party bonus double-counts every
     monster. Don't loosen that rule before teaching those letters.
2. **Switch the meso reader on.** `src/mesos.rs` reads the inventory's
   meso total — one number on a white plate — instead of adding up floating
   lines, which makes the figure immune to the half-taught notice font and
   gives the session's meso change as a difference of two readings rather
   than an accumulation. Sales fall out of it: what the purse gained beyond
   what was picked up off the ground, less what was spent.

   It is written, unit-tested against plates painted from the real atlas,
   and **disabled**, because no capture with the inventory open exists yet.
   On a live frame with the window shut it finds nine bright plates and
   reads a meso total from none of them, which is the right answer. To
   finish: capture a frame with the inventory open, run
   `meso-field check` on it, and if the digits read, `meso-field save`.
   If they do not, the inventory draws its numbers in a different bitmap
   from the status bar and needs its own eleven glyphs taught.

   Note the inversion. Every other reader here looks for light text on a
   dark client; this is dark text on a white plate, so the ROI carries
   `invert: true` and the threshold means the opposite of usual.

3. **Potions used (HP and MP).** Not done, and the route to it changed.
   There *was* a quick-slot reader that found the slot panel and cut the
   eight icons out of it; it was removed at the user's request, because the
   icons took panel space and the counts — the part that would actually
   answer "how many potions did I drink" — were never read. If this is
   picked up again, `git log` has the whole module. What was learned and is
   worth not relearning: the counts are white digits with a 1px black
   outline, about 4x8 native px, drawn over both a white panel background
   and coloured icons, so the **outline** is the discriminator and the blue
   channel is not (the background is white too; that assumption cost a
   cycle). Telling HP from MP means watching which bar jumps when a count
   drops, which needs HP/MP reading restored — lost when `rois` and the
   `H`/`M`/`P`/`/` glyphs were dropped in the unstretch rewrite.
4. **Monster kill attribution and images.** The Monsters tab has no data path
   at all — `S.monsters` is never populated from any snapshot field. The
   floating lines never name the monster, so identity needs a nametag reader.
   Images cannot be cropped from the screen; see "Prior art".
5. Smaller: mesos spent isn't readable (could be estimated), the Inter font
   isn't bundled, map data stops at level 60.

### The panel

Two numbers are promoted above everything else — time to the next level and
EXP per hour — because those are what somebody reads mid-fight, without
stopping. Keep that hierarchy: if a third thing becomes "important", it goes
in the grid, not the hero.

Below them there are **four** figures and only four: played, monsters
killed, mesos picked up, mesos from sales. The panel used to carry twelve,
including DPS, deaths, party bonus, EXP per kill and kills to level, plus
roll-ups for today / month / all time. They went because the person using
this asked for them to go: "simple is better", and "I don't want to know
more than the next level". Resist refilling that grid.

The recommendation card stays — it was removed once by mistake and asked
for back. It carries the only outbound link in the app, to the MeowDB map
database, because that card is the one place a link means anything: the
panel has just said a map is worth the trip.

Opening that link goes through `open_map_database`, a command that takes
**no URL**. A command that opens whatever address the page hands it is a
command that opens whatever address anything reaching the page hands it,
and this webview renders text read off the game screen. If a second link is
ever needed, add a second no-argument command, not a parameter.

### The portrait

The panel's character picture is cut off the player's own screen, at launch
and every five and a half hours after. The trick is that the client draws
the character's name twice in the same bitmap font — on the status bar and
on the label under the sprite — so the status bar's rendering *is* the
search template, and no atlas or text recognition is needed. The box above
the match is flood-filled inward from its border to key the background out.

The hard part is not finding the name, it is finding the right one. The
same string appears in chat, in a buddy list, on a shop sign, in another
player's message, and those match the template just as well. Five
plausibility checks separate them (`judge`), and the strongest signal is
`side_ink`: a character's own label stands alone on its plate, while the
same name in chat has other text hard against it. Every bound is set a long
way from what the real sprite measures, because a missing portrait for one
more minute costs nothing and a confidently wrong one is ugly.

`find-character` is the workbench over the same module — it prints every
score and writes the PNG. Its `--selftest` currently fails: it expects
`captures/frame.png` to be the 2560x1440 reference it was tuned on, and the
file on disk has since been replaced with a 1920x1032 capture. `captures/`
is gitignored, so that baseline cannot live in the repo as things stand.
Either commit a dedicated reference frame or point the self-test at one.

### Colour

Two themes in `ui/tokens.css` — dark by default because the panel sits over
a game, light for reading the week back afterwards; the header's gear
switches them and the choice is kept in `localStorage`. They share an
identity rather than inverting: phthalo green and jade are structure in
both, and only the accent changes, antique gold on obsidian in dark and
phthalo green on ivory in light.

Corners are near-square (`--r-sm: 2px`, and the pill is 3px, not 999px).
With the rounding gone the panel separates its parts with rules and
alignment instead: an accent edge down the left of each figure, one heavy
top edge on the hero. If something needs to stand out, give it an edge, not
a radius.

That last part is a deliberate departure from the palette as specified,
which lists Champagne as the light-mode accent. Champagne on ivory is about
1.5:1 — fine for a fill, unreadable for the hero figure, which is the one
thing on this panel that has to be legible across a desk. So champagne
fills and green writes. The 60/30/10 split holds either way.

**No component rule may hardcode a colour.** Light mode broke the first
time precisely because `.panel` and six other rules carried a literal dark
`rgba()` that no theme could reach. If a new surface needs a shade that is
not in the tokens, add the token.

Under the figures, `review` (one Tauri command, `History::review`) returns
seven day buckets for the current week plus the previous session, so the
panel can answer "is tonight better than last night". The live session folds
into the buckets and the week total, but deliberately **not** into `last`:
"last session" means the one before this one. A day with nothing in it keeps
its column — a rest day and a day that was never recorded are different
things, and a chart that drops empty days hides that.

Nothing in `ui/` may be a screenshot of a real game session. `demo-bg.jpg`
was one, and it carried the player's character name, their meso balance and
a dozen other players' names into a public repository.

## Prior art

A third-party tracker ("Meso Pal") was reverse engineered **statically** for
reference — never run, never redistributed; its own README forbids
redistribution. Worth knowing, so the same ground isn't covered twice:

- It has **no quick-slot reader and no potion-use counter.** Potions appear
  only in a translation name table. Reading the slots is novel work; there is
  no reference implementation to lean on.
- It counts kills from the same floating `You received EXP (+` lines, and its
  schema is `minute_rollups(character, minute, map, monster, kills, ...)` with
  `monster INTEGER`, with queries that explicitly handle `monster = 0` —
  **unknown monster**. Kills are counted whether or not the monster is
  identified. That pattern is what justified the `amount: None` kill above.
- **Monster pictures are bundled game assets** (`game-data/mob-sprites.bin`,
  `sprites/NN.png`, `backdrops/<mapId>.png`), keyed by monster id, with
  identity from a nametag reader. Nothing clever is cropped off the screen.
  Bundling or fetching assets here is an open decision, not a solved problem.

## Working notes

- Measure before diagnosing. Guesses in this codebase have a poor record: the
  slot icons resending every 2s was blamed on two wrong causes before
  measurement showed 7 of 8 slots byte-identical and the 8th differing by 4.6%
  because its count was ticking. `differs()` now excludes the count band.
- Thresholds compared against moving inputs need hysteresis. The map
  recommendation flickered because `betterBy > 0.05` sat right on a trailing
  average.
- `grep -c` exits 1 when it finds nothing, which silently breaks `&&` chains.
- Prefer a test that pins the real failure (a literal bad number, a real
  misread line) over a test that restates the implementation.
