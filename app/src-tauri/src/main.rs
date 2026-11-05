//! SimLab.app: a window on the local engine. Starts `simlab-engine serve`
//! (bundled next to this binary) unless one is already running, shows a
//! splash until it answers, then loads the app. Quitting stops the engine
//! it started, so nothing runs when SimLab is closed.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::webview::NewWindowResponse;
use tauri::{Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder};

const PORT: u16 = 7311;

struct Engine(Mutex<Option<Child>>);

/// SimLab's own pages: the splash and the local engine. Anything else (a
/// GitHub release, a docs link) opens in the Mac's default browser instead.
fn is_simlab(url: &Url) -> bool {
    match url.scheme() {
        "tauri" | "about" | "data" | "blob" => true,
        "http" | "https" => matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "tauri.localhost")),
        _ => false,
    }
}

fn open_outside(url: &Url) {
    if matches!(url.scheme(), "http" | "https" | "mailto") {
        let _ = Command::new("/usr/bin/open").arg(url.as_str()).spawn();
    }
}

fn engine_up() -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], PORT).into(), Duration::from_millis(200)).is_ok()
}

fn start_engine() -> Result<Option<Child>, String> {
    if engine_up() {
        return Ok(None); // already running (e.g. started by Claude through MCP)
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let engine = exe.with_file_name("simlab-engine");
    let log_dir = std::env::var("HOME").map(|h| format!("{h}/Library/Logs/SimLab")).unwrap_or_else(|_| "/tmp".into());
    let _ = std::fs::create_dir_all(&log_dir);
    let log = std::fs::File::create(format!("{log_dir}/engine.log")).map_err(|e| e.to_string())?;
    let child = Command::new(&engine)
        .args(["serve", "--port", &PORT.to_string(), "--exit-with-parent"])
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", engine.display()))?;
    Ok(Some(child))
}

fn main() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("SimLab")
                .inner_size(1440.0, 900.0)
                .min_inner_size(900.0, 600.0)
                // let the page receive dropped files (drag an STL onto it)
                .disable_drag_drop_handler()
                .on_navigation(|url| {
                    let ok = is_simlab(url);
                    if !ok {
                        open_outside(url);
                    }
                    ok
                })
                // links with target="_blank" (e.g. "Download 0.2.0")
                .on_new_window(|url, _| {
                    open_outside(&url);
                    NewWindowResponse::Deny
                })
                .build()?;
            let child = match start_engine() {
                Ok(c) => c,
                Err(e) => {
                    let _ = window.eval(&format!("document.getElementById('err').textContent = {:?}", e));
                    None
                }
            };
            app.manage(Engine(Mutex::new(child)));
            std::thread::spawn(move || {
                let t0 = Instant::now();
                while t0.elapsed() < Duration::from_secs(60) {
                    if engine_up() {
                        let url = format!("http://127.0.0.1:{PORT}/");
                        let _ = window.navigate(url.parse().unwrap());
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = window.eval(
                    "document.getElementById('err').textContent = 'The engine did not start. If you downloaded SimLab, macOS may still be holding it back: run   xattr -dr com.apple.quarantine /Applications/SimLab.app   in Terminal, then open SimLab again. Log: ~/Library/Logs/SimLab/engine.log'",
                );
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("SimLab failed to start");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<Engine>() {
                if let Some(mut child) = state.0.lock().unwrap().take() {
                    // ask nicely (SIGTERM lets it finish writing), then make sure
                    let _ = Command::new("/bin/kill").arg(child.id().to_string()).status();
                    let t0 = Instant::now();
                    while t0.elapsed() < Duration::from_secs(3) {
                        if let Ok(Some(_)) = child.try_wait() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let _ = child.kill();
                }
            }
        }
    });
}
