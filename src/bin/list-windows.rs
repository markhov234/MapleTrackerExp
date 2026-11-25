//! What the capture API can actually see.
//!
//! The tracker picks its target from `Window::enumerate`, and that list is
//! not the same as the one Task Manager shows: the API drops tool windows,
//! child windows and anything hidden. When the game is running and the
//! tracker still cannot find it, this is the first thing to check — it is
//! the difference between "we chose wrong" and "it was never offered".

use windows_capture::window::Window;

fn main() {
    let windows = match Window::enumerate() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("enumerate failed: {e}");
            std::process::exit(1);
        }
    };
    println!("{} capturable window(s)\n", windows.len());
    for w in &windows {
        let title = w.title().unwrap_or_else(|e| format!("<title failed: {e}>"));
        if title.trim().is_empty() {
            continue;
        }
        let size = match (w.width(), w.height()) {
            (Ok(a), Ok(b)) => format!("{a}x{b}"),
            _ => "?".into(),
        };
        let pid = w
            .process_id()
            .map(|p| p.to_string())
            .unwrap_or_else(|e| format!("<{e}>"));
        println!("{size:>12}  pid {pid:<24} {title:?}");
    }
}
