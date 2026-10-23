use std::os::unix::fs::DirBuilderExt;

use tauri::{AppHandle, Manager};

use crate::profile_backup::ProfileStore;
use crate::state::AppState;
use crate::{auth, config, ui};

pub fn backup_saved_text_in(path: &std::path::Path, language: crate::i18n::Language) -> String {
    crate::i18n::render(
        language,
        "backup.saved",
        &[("path", &path.display().to_string())],
    )
    .expect("validated backup destination message")
}

#[cfg(test)]
#[test]
fn saved_notice_translates_scope_without_reparsing_a_literal_destination() {
    let path = std::path::Path::new("/private/{profile}<b>");
    for (language, _) in crate::i18n::LANGUAGES {
        let body = backup_saved_text_in(path, language);
        assert!(body.contains("/private/{profile}<b>") && body.contains("node_modules"));
        assert!(!body.contains("{path}"));
        if language == crate::i18n::Language::Ru {
            assert!(body.contains("профил"));
        }
    }
}

pub fn open_backups(app: &AppHandle) {
    let path = config::profile_backup_dir();
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&path)
    {
        Ok(()) => crate::util::open_path(&path),
        Err(error) => ui::message(
            app,
            crate::i18n::text("backup.open_failed"),
            &error.to_string(),
            gtk::MessageType::Error,
        ),
    }
}

/// Read-only to the profile. A live snapshot is explicitly best-effort, not a
/// global/session backup. Restore is deliberately CLI-only and offline.
pub fn backup_web(app: &AppHandle) {
    if app.state::<AppState>().is_updating() {
        ui::message(
            app,
            crate::i18n::text("backup.busy_title"),
            crate::i18n::text("backup.busy"),
            gtk::MessageType::Warning,
        );
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = ProfileStore::new(&config::dsh_home(), &config::profile_backup_dir())
            .and_then(|store| store.create("web"));
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || match result {
            Ok(path) => {
                app_for_main.state::<AppState>().log(
                    crate::i18n::render(
                        crate::i18n::active_language(),
                        "backup.saved_log",
                        &[("path", &path.display().to_string())],
                    )
                    .expect("validated backup log message"),
                );
                ui::message(
                    &app_for_main,
                    crate::i18n::text("backup.saved_title"),
                    &backup_saved_text_in(&path, crate::i18n::active_language()),
                    gtk::MessageType::Info,
                );
            }
            Err(error) => ui::message(
                &app_for_main,
                crate::i18n::text("backup.failed_title"),
                &auth::redact(&format!("{error:#}")),
                gtk::MessageType::Error,
            ),
        });
    });
}
