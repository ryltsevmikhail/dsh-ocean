use std::os::unix::fs::DirBuilderExt;

use tauri::{AppHandle, Manager};

use crate::profile_backup::ProfileStore;
use crate::state::AppState;
use crate::{auth, config, ui};

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
            "Could not open backups",
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
            "Update in progress",
            "Wait for the runtime update before creating a profile snapshot.",
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
                app_for_main.state::<AppState>().log(format!("Web profile configuration snapshot saved at {}", path.display()));
                ui::message(&app_for_main, "Profile configuration backed up", &format!("Saved privately to:\n{}\n\nScope: web profile configuration only. Global credentials, sessions and node_modules are excluded. Profile files may themselves contain secrets.\n\nA live snapshot is best-effort; stop other writers for stronger consistency. Restore requires the offline CLI command and retains the replaced profile.", path.display()), gtk::MessageType::Info);
            }
            Err(error) => ui::message(&app_for_main, "Profile backup failed", &auth::redact(&format!("{error:#}")), gtk::MessageType::Error),
        });
    });
}
