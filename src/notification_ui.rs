use crate::{config, notifications, state::AppState, ui, windows};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

pub fn preference_path() -> std::path::PathBuf {
    config::config_dir().join("notifications.json")
}
pub fn initialize(app: &AppHandle) {
    let state = app.state::<AppState>();
    let enabled = notifications::load_preference(&preference_path()).unwrap_or_else(|_| {
        state.log(crate::i18n::text("notice.preference_unreadable"));
        false
    });
    state.notifier.set_enabled(enabled);
    let handle = app.clone();
    state.notifier.set_activate(Arc::new(move || {
        let app = handle.clone();
        let _ = handle.run_on_main_thread(move || windows::show_main_or_setup(&app));
    }));
}
pub fn toggle(app: &AppHandle, requested: bool) -> bool {
    let state = app.state::<AppState>();
    // Disabling takes effect even if the preference cannot be persisted.
    if !requested {
        state.notifier.set_enabled(false);
    }
    match notifications::save_preference(&preference_path(), requested) {
        Ok(()) => {
            state.notifier.set_enabled(requested);
            requested
        }
        Err(_) => {
            ui::message(
                app,
                crate::i18n::text("notification.preference_title"),
                crate::i18n::text("notification.preference_failed"),
                gtk::MessageType::Warning,
            );
            state.notifier.enabled()
        }
    }
}
pub fn attach_tasks(window: &tauri::WebviewWindow, initial: url::Url) {
    let app = window.app_handle().clone();
    let _ = window.with_webview(move |platform| {
        let app_for_origin = app.clone();
        let origin = std::rc::Rc::new(move || {
            Some(
                app_for_origin
                    .state::<AppState>()
                    .server()
                    .map(|s| s.base_url)
                    .unwrap_or_else(|| initial.clone()),
            )
        });
        let app_for_activity = app.clone();
        let activity = std::rc::Rc::new(move || post(&app_for_activity, notifications::Kind::Task));
        if crate::task_notifications::attach(&platform.inner(), origin, activity).is_err() {
            app.state::<AppState>()
                .log(crate::i18n::text("notice.bridge_unavailable"));
        }
    });
}

pub fn post(app: &AppHandle, kind: notifications::Kind) {
    // Task activity is useful while away, not while already looking at the UI.
    if kind == notifications::Kind::Task
        && app
            .get_webview_window("main")
            .is_some_and(|window| window.is_focused().unwrap_or(false))
    {
        return;
    }
    app.state::<AppState>().notifier.post(kind);
}
