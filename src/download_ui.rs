use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gtk::prelude::*;
use tauri::{AppHandle, Manager, WebviewWindow};
use url::Url;

use crate::download_engine::{self, SharedHistory};
use crate::downloads::{History, Status};
use crate::state::AppState;
use crate::{config, ui};

fn history(app: &AppHandle) -> SharedHistory {
    let state = app.state::<AppState>();
    let mut slot = state.downloads.lock().expect("download history init mutex");
    slot.get_or_insert_with(|| {
        let history =
            History::open(&config::state_dir().join("downloads.json")).unwrap_or_else(|_| {
                state.log(crate::i18n::text("notice.history_unavailable"));
                History::default()
            });
        Arc::new(Mutex::new(history))
    })
    .clone()
}
pub fn attach(window: &WebviewWindow, initial: Url) {
    let app = window.app_handle().clone();
    let records = history(&app);
    let window_for_dialog = window.clone();
    let _ = window.with_webview(move |platform| {
        let app_for_origin = app.clone();
        let origin = Rc::new(move || {
            Some(
                app_for_origin
                    .state::<AppState>()
                    .server()
                    .map(|s| s.base_url)
                    .unwrap_or_else(|| initial.clone()),
            )
        });
        let chooser = Rc::new(move |name: &str, source: &str| {
            let parent = window_for_dialog.gtk_window().ok();
            let folder = window_for_dialog.app_handle().path().download_dir().ok();
            let dialog = crate::download_chooser::build(
                parent.as_ref().map(|window| window.upcast_ref()),
                name,
                source,
                folder.as_deref(),
            );
            crate::download_chooser::run(&dialog)
        });
        let app_for_terminal = app.clone();
        let terminal = Arc::new(move |entry: crate::downloads::Entry| {
            if entry.status == Status::Saved {
                crate::notification_ui::post(
                    &app_for_terminal,
                    crate::notifications::Kind::DownloadSaved,
                );
                app_for_terminal
                    .state::<AppState>()
                    .log(crate::i18n::text("notice.download_saved"));
            }
            if entry.status == Status::Failed {
                crate::notification_ui::post(
                    &app_for_terminal,
                    crate::notifications::Kind::DownloadFailed,
                );
                let app = app_for_terminal.clone();
                let _ = app_for_terminal.run_on_main_thread(move || {
                    let mut message = entry
                        .message
                        .unwrap_or_else(|| crate::i18n::text("downloads.failed_text").into());
                    if let Some(path) = entry.recovery {
                        message.push_str(
                            &crate::i18n::render(
                                crate::i18n::active_language(),
                                "downloads.staging",
                                &[("path", &path.display().to_string())],
                            )
                            .expect("validated download message"),
                        );
                    }
                    ui::message(
                        &app,
                        crate::i18n::text("downloads.failed"),
                        &crate::auth::redact(&message),
                        gtk::MessageType::Warning,
                    );
                });
            }
        });
        let cancel = download_engine::attach(&platform.inner(), origin, chooser, records, terminal);
        *app.state::<AppState>()
            .download_cancel
            .lock()
            .expect("download cancellation mutex") = Some(cancel);
    });
}

pub fn show_history(app: &AppHandle) {
    let records = history(app);
    let parent = app
        .get_webview_window("main")
        .and_then(|w| w.gtk_window().ok());
    let control = app
        .state::<AppState>()
        .download_cancel
        .lock()
        .expect("download cancellation mutex")
        .clone();
    let cancel = control
        .map(|control| Rc::new(move |ids| control.request(ids)) as crate::download_history::Cancel);
    let window = crate::download_history::build_with_cancel(
        parent.as_ref().map(|window| window.upcast_ref()),
        records,
        cancel,
    );
    window.dialog.show_all();
}
