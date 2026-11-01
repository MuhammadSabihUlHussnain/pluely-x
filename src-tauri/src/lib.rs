// TEMPORARY window spike harness (Step 0). Replaced by the real app wiring afterwards.
mod error;
mod window;

use std::io::Write;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn spike_log_line(msg: &str) {
    if let Ok(path) = std::env::var("PLUELYX_SPIKE_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let fg = window::win32::foreground_window();
            let _ = writeln!(f, "{} fg={:#x}", msg, fg);
        }
    }
}

#[tauri::command]
async fn spike_log(msg: String) {
    spike_log_line(&msg);
}

#[tauri::command]
async fn spike_cycle(app: tauri::AppHandle, n: u32) -> Result<(), String> {
    let w = app.get_webview_window("main").ok_or("no main")?;
    for i in 0..n {
        w.hide().map_err(|e| e.to_string())?;
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        w.show().map_err(|e| e.to_string())?;
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let hwnd = w.hwnd().map_err(|e| e.to_string())?;
        let f = window::win32::ex_style_flags(hwnd);
        spike_log_line(&format!("cycle {} ex={:#x} tool={} app={} noact={}", i, f.raw, f.tool_window, f.app_window, f.no_activate));
    }
    spike_log_line("cycle done");
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![spike_log, spike_cycle])
        .setup(|app| {
            let w = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Pluely X")
                .inner_size(440.0, 560.0)
                .position(200.0, 120.0)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .content_protected(true)
                .resizable(true)
                .focusable(false)
                .focused(false)
                .visible(false)
                .build()?;
            let hwnd = w.hwnd()?;
            let use_subclass = std::env::var("PLUELYX_SPIKE_SUBCLASS").map(|v| v != "0").unwrap_or(true);
            if use_subclass {
                let ok = window::win32::install_overlay_subclass(hwnd);
                spike_log_line(&format!("subclass installed={}", ok));
            } else {
                spike_log_line("subclass DISABLED (baseline)");
            }
            w.show()?;
            let f = window::win32::ex_style_flags(hwnd);
            spike_log_line(&format!("startup hwnd={:#x} ex={:#x} tool={} app={} noact={}", hwnd.0 as isize, f.raw, f.tool_window, f.app_window, f.no_activate));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
