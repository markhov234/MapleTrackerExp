# EXP Calc

A small always-on-top overlay for MapleStory. It shows your EXP rate, how
long until the next level, and what the session has earned so far.

It reads the game's status bar **off the screen**, the same way you do.

## Installing

1. Download `EXP Calc_0.1.0_x64-setup.exe`.
2. Windows will say **"Windows protected your PC"**. That is SmartScreen
   noticing the installer has no code-signing certificate, which costs a few
   hundred pounds a year — it is not a report of anything found in the file.
   Click **More info**, then **Run anyway**.
3. It installs for your user only, so there is no administrator prompt, and
   it uninstalls from *Settings → Apps* like anything else.

Windows 10 or 11, 64-bit. If the machine is missing the WebView2 runtime
(most are not — it ships with Windows 11 and recent Windows 10), the
installer fetches it.

## Using it

Start **EXP Calc** from the Start menu, then start MapleStory. The panel
finds the game on its own and begins counting.

- **Ctrl+Shift+E** hides and shows the panel.
- Drag the header to move it; the button in the corner shrinks it to a bar.
- Right-click the tray icon to quit.

It finds the EXP bar by its shape, not by a position someone measured, so it
does not need to be told where your status bar is, and it keeps working if
you resize the client or play on a different monitor.

## What it does not do

This matters more than the feature list, because tools in this space often
do cross the line and accounts get banned for it. EXP Calc:

- **does not read the game's memory** — no `OpenProcess`, no
  `ReadProcessMemory`;
- **does not send the game any input** — no keystrokes, no clicks, no macros,
  nothing automated;
- **does not touch the network** — no packets captured, read or sent;
- **does not touch any game file.**

What it does is ask Windows for a picture of the game window, several times a
second, and read the numbers printed on it. It also registers one global
hotkey, the one that shows and hides the panel — a single registration with
Windows, not a keyboard hook, so it cannot see any other key you press.

## If something goes wrong

The panel says what it is doing: *looking for the EXP bar…* means it has not
found the status bar yet — the usual cause is a window covering it.

There is a log at:

```
%APPDATA%\com.maplertracker.expcalc\exp-calc.log
```

It records which window was looked at and what the status bar read, nothing
about your account or your character. Session history lives in the same
folder.

## Building it yourself

```
npx @tauri-apps/cli@2 build
```

The installer lands in `target/release/bundle/nsis/`.

The development tools — teaching the font, inspecting captures, listing
windows — are behind a feature flag so that a release build does not compile
them:

```
cargo run --features tools --bin native-atlas -- check captures/*.png
```
