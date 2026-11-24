//! System tray for "minimize to tray" close behaviour.
//!
//! The window close X is handled in the frontend (custom title bar). Choosing
//! "minimize to tray" hides the main window through a Rust command; choosing
//! "quit" exits the process so the existing `RunEvent::Exit` cleanup still
//! kills worker children. The tray menu offers a way back and an explicit quit
//! that routes through the frontend close guards (running tasks confirmation).

use crate::storage;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, menu::Menu, menu::MenuItem};

pub(crate) const TRAY_QUIT_EVENT: &str = "pymss://tray-quit";

/// Pure helper so locale selection can be unit-tested without a native tray.
pub(crate) fn prefers_chinese(locale: &str, system_language: Option<&str>) -> bool {
    let locale = locale.trim();
    let lowered = locale.to_lowercase();
    if lowered == "en" || lowered.starts_with("en_") || lowered.starts_with("en-") {
        return false;
    }
    if lowered.starts_with("zh") {
        return true;
    }
    system_language
        .map(|value| value.to_lowercase().contains("zh"))
        .unwrap_or(true)
}

fn labels(app: &AppHandle) -> (&'static str, &'static str) {
    let locale = storage::read_app_store(app, "app-settings")
        .ok()
        .and_then(|value| value.get("locale").and_then(|item| item.as_str()).map(str::to_string))
        .unwrap_or_else(|| "system".into());
    let language = ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .or_else(|| std::env::var("LANG").ok());
    if prefers_chinese(&locale, language.as_deref()) {
        ("显示主窗口", "退出 SoundFlow")
    } else {
        ("Show Window", "Quit SoundFlow")
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let (show_label, quit_label) = labels(app);
    let show_item = MenuItem::with_id(app, "show_window", show_label, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit_app", quit_label, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        show_main_window(app);
                    }
                }
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show_window" => show_main_window(app),
            "quit_app" => {
                // Bring the window back so the frontend confirmation dialog is visible,
                // then let the frontend run its close guards and call quit_application.
                show_main_window(app);
                let _ = app.emit(TRAY_QUIT_EVENT, ());
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    let tray = builder.build(app)?;
    // Keep the tray icon alive for the whole process lifetime.
    app.manage(tray);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::prefers_chinese;

    #[test]
    fn explicit_locale_wins_over_system_language() {
        assert!(prefers_chinese("zh-CN", Some("en-US")));
        assert!(!prefers_chinese("en", Some("zh_CN")));
        assert!(!prefers_chinese("en-US", None));
    }

    #[test]
    fn system_locale_follows_environment_with_chinese_default() {
        assert!(prefers_chinese("system", Some("zh_CN.UTF-8")));
        assert!(!prefers_chinese("system", Some("en_US.UTF-8")));
        assert!(prefers_chinese("system", None));
    }
}
