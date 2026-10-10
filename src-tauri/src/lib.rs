mod commands;
mod engine;
mod error;
mod model;
mod source_local;
mod source_org;
mod source_status;
mod store;
mod timefmt;

use std::sync::Mutex;

use tauri::Manager;

use crate::commands::Inner;
use crate::store::Store;

const IS_DESKTOP: bool = !cfg!(any(target_os = "android", target_os = "ios"));

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    use tauri::Emitter as _;

    let show = MenuItem::with_id(app, "crr-show", "打开提醒器", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "crr-check", "立即检查", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "crr-quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &check, &quit])?;
    let builder = TrayIconBuilder::with_id("crr-tray")
        .menu(&menu)
        .tooltip("Codex 额度重置提醒")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "crr-show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
            "crr-check" => {
                let _ = app.emit("crr-tray-check", ());
            }
            "crr-quit" => {
                app.exit(0);
            }
            _ => {}
        });
    match app.default_window_icon() {
        Some(icon) => {
            let _tray = builder.icon(icon.clone()).build(app)?;
        }
        None => {
            let _tray = builder.build(app)?;
        }
    }
    Ok(())
}

fn fallback_config_dir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push("codex-reset-reminder");
    p
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let base = tauri::Builder::default().plugin(tauri_plugin_notification::init());

    // autostart 插件在 android/ios 目标上没有 Rust API（实测安卓编译报
    // unresolved import ManagerExt、cannot find init），只能桌面端注册。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let base = base.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::AppleScript,
        None,
    ));

    let builder = base
        .setup(|app| {
            let dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| fallback_config_dir());
            let store = Store::new(dir);
            let settings = store.load_settings();
            let state = store.load_state();
            app.manage(Mutex::new(Inner {
                store,
                settings,
                state,
            }));

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            if let Err(e) = setup_tray(app) {
                eprintln!("[crr] 托盘不可用：{e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if !IS_DESKTOP || window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 关窗口只隐藏，保持提醒在后台运行。
                api.prevent_close();
                let _ = window.hide();
            }
        });

    builder
        .invoke_handler(tauri::generate_handler![
            commands::crr_local,
            commands::crr_check,
            commands::crr_get_settings,
            commands::crr_set_settings,
            commands::crr_reset_history,
            commands::crr_platform,
            commands::crr_window
        ])
        .run(tauri::generate_context!())
        .expect("Codex 重置提醒器启动失败");
}
