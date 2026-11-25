//! EXP Calc — overlay shell.
//!
//! A frameless, always-on-top, transparent window that sits over the game,
//! toggled by a global hotkey or the tray icon.
//!
//! On the input boundary, which is the line that separates a screen reader
//! from a bot: this registers ONE hotkey through `RegisterHotKey`, which asks
//! Windows to notify us when that exact combination is pressed. It is not a
//! `WH_KEYBOARD_LL` hook — we never see any other keystroke, and we never
//! synthesise one. Nothing here sends input to the game.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{LogicalSize, Manager, PhysicalPosition, WebviewWindow};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

const OVERLAY: &str = "overlay";
const MARGIN: i32 = 18;

/// The OCR profile is compiled in, so the app has no runtime file to find and
/// cannot start up half-configured. Rebuild after running `build-atlas`.
const PROFILE_JSON: &str = include_str!("../assets/profile.json");

const FULL: (f64, f64) = (424.0, 780.0);
const MINI: (f64, f64) = (276.0, 78.0);

/// Shrink to the compact bar, or grow back.
///
/// Tracking is unaffected either way — the capture loop and the session
/// totals do not care whether the panel is big, small or hidden. Minimising
/// changes how much of the session you can see, never whether it is counted.
///
/// The top-right corner is pinned across the resize so the panel shrinks
/// toward wherever the user parked it instead of jumping.
#[tauri::command]
fn set_mini(window: WebviewWindow, mini: bool) -> Result<(), String> {
    let (w, h) = if mini { MINI } else { FULL };
    let before = window.outer_size().map_err(|e| e.to_string())?;
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let scale = window.scale_factor().unwrap_or(1.0);

    window
        .set_size(LogicalSize::new(w, h))
        .map_err(|e| e.to_string())?;

    let right_edge = pos.x + before.width as i32;
    let new_width = (w * scale).round() as i32;
    window
        .set_position(PhysicalPosition::new(right_edge - new_width, pos.y))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The `×` in the header: hide to the tray, do not quit.
///
/// Quitting from `×` was the wrong default for a tracker. The session totals
/// and the rate window live in memory, so an accidental click did not just
/// close a window, it threw away the measurement — and the rate needs 90
/// seconds of baseline before it says anything at all. Hiding keeps the
/// capture loop running, so the numbers go on accumulating while the panel is
/// out of the way, and the tray icon brings it straight back.
///
/// Hidden rather than minimised deliberately: the app lives in the tray, and a
/// minimised window would leave a taskbar button sitting there for a panel
/// that is nowhere on screen.
#[tauri::command]
fn hide_app(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

/// What the app keeps across the whole run: the live session and the history
/// it will be filed into.
struct Tracked {
    session: std::sync::Arc<std::sync::Mutex<maple_tracker::session::Session>>,
    history: std::sync::Arc<maple_tracker::history::History>,
}

/// Add up a span of history — today, this week, this month, or everything.
///
/// The live session is passed in alongside the filed ones, so today's figure
/// moves while you are playing instead of only when you quit.
#[tauri::command]
fn range_totals(
    state: tauri::State<'_, Tracked>,
    range: String,
) -> Result<maple_tracker::history::Totals, String> {
    let range = maple_tracker::history::Range::parse(&range)
        .ok_or_else(|| format!("unknown range {range:?}"))?;
    let current = state.session.lock().map_err(|_| "session lock poisoned")?.record();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(state.history.totals(range, Some(&current), now))
}

/// The week in review, and the session before this one.
///
/// One call rather than seven range roll-ups: the panel wants the whole
/// picture at once, and the file is read once to build it.
#[tauri::command]
fn review(
    state: tauri::State<'_, Tracked>,
) -> Result<maple_tracker::history::Review, String> {
    let current = state.session.lock().map_err(|_| "session lock poisoned")?.record();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(state.history.review(Some(&current), now))
}

/// Open the map database in the user's own browser.
///
/// Takes no URL on purpose. A command that opens whatever address the page
/// hands it is a command that opens whatever address anything that can
/// reach the page hands it, and this webview renders text read off the
/// game screen. There is exactly one page worth linking to, so the command
/// knows it and the page does not get a say.
#[tauri::command]
fn open_map_database() -> Result<(), String> {
    const URL: &str = "https://meowdb.com/msclassic/maps";
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Through the shell's own URL handler rather than `cmd /c start`,
        // which would involve a command interpreter for no reason.
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", URL])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("not supported on this platform".into())
    }
}

/// Quit for real, from the tray's Quit item or the panel's footer button.
///
/// The tray icon is removed first. A tray icon is owned by the app handle and
/// Windows only drops it from the notification area when its owner goes away
/// cleanly, so leaving it to process teardown can strand a ghost icon that
/// only disappears when the user hovers it.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    shut_down(&app);
}

/// File the session, remove the tray icon, then end the process.
///
/// Filing first, and on every exit path, because an unfiled session only
/// exists as a checkpoint — recoverable on the next start, but it would show
/// up as a stray extra session rather than as the one just played.
fn shut_down(app: &tauri::AppHandle) {
    if let Some(t) = app.try_state::<Tracked>() {
        if let Ok(session) = t.session.lock() {
            t.history.finish(&session.record());
        }
    }
    app.remove_tray_by_id("tray");
    app.exit(0);
}

/// Show or hide the panel.
///
/// Showing deliberately does not call `set_focus`. Pulling focus off the game
/// the moment someone taps the hotkey mid-fight would get them killed; the
/// window takes focus on its own when they actually click it.
fn toggle(win: &WebviewWindow) {
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        let _ = win.show();
    }
}

/// Take the grey line off the window.
///
/// `decorations: false` does not mean "no frame" on Windows. The window keeps
/// `WS_THICKFRAME` and `WS_CAPTION` so that resizing and snapping still work —
/// confirmed on the live window, style `0x14CC0000` — and Windows 11 draws its
/// 1px border for that frame regardless of the title bar being hidden. On a
/// dark overlay sitting over the game it reads as a grey rectangle around the
/// panel, which is exactly what it is.
///
/// DWM will skip it if asked. `DWMWA_BORDER_COLOR` takes the sentinel
/// `DWMWA_COLOR_NONE`, and the corner preference stops the compositor rounding
/// a window whose own radius the panel already draws. Both are no-ops on older
/// Windows, and a failure here is cosmetic, so nothing is unwrapped.
#[cfg(windows)]
fn strip_system_border(win: &WebviewWindow) {
    use std::ffi::c_void;
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const c_void, size: u32) -> i32;
    }
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    const DWMWCP_DONOTROUND: u32 = 1;

    let Ok(hwnd) = win.hwnd() else { return };
    let h = hwnd.0 as isize;
    for (attr, value) in [
        (DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE),
        (DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND),
    ] {
        unsafe {
            DwmSetWindowAttribute(h, attr, &value as *const u32 as *const c_void, 4);
        }
    }
}

#[cfg(not(windows))]
fn strip_system_border(_win: &WebviewWindow) {}

/// Park the panel in the top-right of whichever monitor it opened on.
fn place_top_right(win: &WebviewWindow) {
    let Ok(Some(monitor)) = win.current_monitor() else { return };
    let Ok(size) = win.outer_size() else { return };
    let screen = monitor.size();
    let pos = monitor.position();
    let scale = monitor.scale_factor();
    let margin = (MARGIN as f64 * scale) as i32;
    let x = pos.x + screen.width as i32 - size.width as i32 - margin;
    let y = pos.y + margin;
    let _ = win.set_position(PhysicalPosition::new(x, y));
}

fn main() {
    // Ctrl+Shift+E. One combination, registered with the OS.
    let hotkey = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyE);

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![set_mini, hide_app, quit_app, range_totals, review, open_map_database])
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcut(hotkey)
                .expect("could not register Ctrl+Shift+E")
                .with_handler(move |app, triggered, event| {
                    if event.state == ShortcutState::Pressed && triggered == &hotkey {
                        if let Some(win) = app.get_webview_window(OVERLAY) {
                            toggle(&win);
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            let win = app
                .get_webview_window(OVERLAY)
                .expect("overlay window missing from tauri.conf.json");
            strip_system_border(&win);
            place_top_right(&win);

            let show = MenuItem::with_id(app, "show", "Show / hide", true, Some("Ctrl+Shift+E"))?;
            let reposition = MenuItem::with_id(app, "snap", "Snap to top right", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit EXP Calc", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &reposition, &quit])?;

            TrayIconBuilder::with_id("tray")
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .expect("bundle icon missing"),
                )
                .tooltip("EXP Calc — Ctrl+Shift+E")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window(OVERLAY) {
                            toggle(&win);
                        }
                    }
                    "snap" => {
                        if let Some(win) = app.get_webview_window(OVERLAY) {
                            let _ = win.show();
                            place_top_right(&win);
                        }
                    }
                    "quit" => shut_down(app),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(win) = tray.app_handle().get_webview_window(OVERLAY) {
                            toggle(&win);
                        }
                    }
                })
                .build(app)?;

            // Start watching the game. This never blocks startup and does not
            // need the game to be running yet — it reports "waiting for
            // MapleStory" and keeps looking.
            // History lives beside the app's own data, not in the install
            // directory: Program Files is not writable by the user the app
            // runs as, and a tracker that silently fails to record is worse
            // than one that does not try.
            let dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            maple_tracker::notes::open(&dir);
            let history = std::sync::Arc::new(maple_tracker::history::History::open(&dir));
            maple_tracker::note!("[history] {}", dir.display());

            match serde_json::from_str::<maple_tracker::ocr::Profile>(PROFILE_JSON) {
                Ok(profile) => {
                    let session = maple_tracker::tracker::spawn(
                        app.handle().clone(),
                        profile,
                        std::sync::Arc::clone(&history),
                    );
                    app.manage(Tracked { session, history });
                }
                Err(e) => maple_tracker::note!("bundled OCR profile is invalid, tracking disabled: {e}"),
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("EXP Calc failed to start");
}
