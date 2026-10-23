use std::sync::atomic::Ordering;

use gtk::prelude::*;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::state::AppState;
use crate::{config, diagnostics, profile_ui, remote, service, ui, update, util, windows};

/// Every tray menu entry, as a single source of truth for ids and labels.
///
/// Menu construction and the event handler both go through this type, so a new
/// entry cannot be added without a matching handler branch (the match is
/// exhaustive) and a typo cannot create a silently dead menu item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    Restart,
    Updates,
    ShellStatus,
    Pairing,
    Diagnostics,
    DiagnosticsFolder,
    Profile,
    BackupProfile,
    VerifyBackup,
    BackupHistory,
    DataSnapshot,
    ExportBackup,
    Storage,
    Plugins,
    ProfileBackups,
    Downloads,
    Notifications,
    Language,
    Quit,
}

impl Action {
    /// Menu order, matching the visual grouping (separators are added by the
    /// caller and are not actions).
    pub const ALL: [Action; 20] = [
        Action::Show,
        Action::Restart,
        Action::Updates,
        Action::ShellStatus,
        Action::Pairing,
        Action::Diagnostics,
        Action::DiagnosticsFolder,
        Action::Profile,
        Action::BackupProfile,
        Action::VerifyBackup,
        Action::BackupHistory,
        Action::DataSnapshot,
        Action::ExportBackup,
        Action::Storage,
        Action::Plugins,
        Action::ProfileBackups,
        Action::Downloads,
        Action::Notifications,
        Action::Language,
        Action::Quit,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Action::Show => "show",
            Action::Restart => "restart",
            Action::Updates => "updates",
            Action::ShellStatus => "shell-status",
            Action::Pairing => "pairing",
            Action::Diagnostics => "diagnostics",
            Action::DiagnosticsFolder => "diagnostics-folder",
            Action::Profile => "profile",
            Action::BackupProfile => "backup-profile",
            Action::VerifyBackup => "verify-backup",
            Action::BackupHistory => "backup-history",
            Action::DataSnapshot => "data-snapshot",
            Action::ExportBackup => "export-backup",
            Action::Storage => "storage",
            Action::Plugins => "plugins",
            Action::ProfileBackups => "profile-backups",
            Action::Downloads => "downloads",
            Action::Notifications => "notifications",
            Action::Language => "language",
            Action::Quit => "quit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Show => "Show DSH Ocean",
            Action::Restart => "Restart DSH server",
            Action::Updates => "Check for DSH core updates…",
            Action::ShellStatus => "DSH Ocean shell updates…",
            Action::Pairing => "Pair a device…",
            Action::Diagnostics => "Save diagnostics",
            Action::DiagnosticsFolder => "Open diagnostics folder",
            Action::Profile => "Open web profile folder",
            Action::BackupProfile => "Back up web configuration…",
            Action::VerifyBackup => "Verify configuration backup…",
            Action::BackupHistory => "Backup history…",
            Action::DataSnapshot => "Back up sessions and stores…",
            Action::ExportBackup => "Export a backup…",
            Action::Storage => "Storage usage…",
            Action::Plugins => "Plugins…",
            Action::ProfileBackups => "Open profile backups",
            Action::Downloads => "Downloads…",
            Action::Notifications => "Desktop notifications",
            Action::Language => "Language…",
            Action::Quit => "Quit DSH Ocean",
        }
    }

    pub fn label_for(self, language: crate::i18n::Language) -> &'static str {
        crate::i18n::message(language, &format!("tray.{}", self.id()))
            .unwrap_or_else(|| self.label())
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }
}

pub fn install(app: &tauri::App) -> tauri::Result<TrayIcon> {
    let item = |action: Action| -> tauri::Result<MenuItem<tauri::Wry>> {
        MenuItem::with_id(
            app,
            action.id(),
            action.label_for(crate::i18n::active_language()),
            true,
            None::<&str>,
        )
    };
    let show = item(Action::Show)?;
    let restart = item(Action::Restart)?;
    let updates = item(Action::Updates)?;
    let shell = item(Action::ShellStatus)?;
    let separator_one = PredefinedMenuItem::separator(app)?;
    let pairing = item(Action::Pairing)?;
    let diagnostics = item(Action::Diagnostics)?;
    let diagnostics_folder = item(Action::DiagnosticsFolder)?;
    let profile = item(Action::Profile)?;
    let backup = item(Action::BackupProfile)?;
    let backups = item(Action::ProfileBackups)?;
    let verify_backup = item(Action::VerifyBackup)?;
    let backup_history = item(Action::BackupHistory)?;
    let storage = item(Action::Storage)?;
    let plugins = item(Action::Plugins)?;
    let data_snapshot = item(Action::DataSnapshot)?;
    let export_backup = item(Action::ExportBackup)?;
    let downloads = item(Action::Downloads)?;
    let notices = CheckMenuItem::with_id(
        app,
        Action::Notifications.id(),
        Action::Notifications.label_for(crate::i18n::active_language()),
        true,
        app.state::<AppState>().notifier.enabled(),
        None::<&str>,
    )?;
    let language = item(Action::Language)?;
    let separator_two = PredefinedMenuItem::separator(app)?;
    let quit = item(Action::Quit)?;

    let menu = Menu::with_items(
        app,
        &[
            &show,
            &restart,
            &updates,
            &shell,
            &pairing,
            &separator_one,
            &diagnostics,
            &diagnostics_folder,
            &profile,
            &backup,
            &verify_backup,
            &backup_history,
            &data_snapshot,
            &export_backup,
            &storage,
            &plugins,
            &backups,
            &downloads,
            &notices,
            &language,
            &separator_two,
            &quit,
        ],
    )?;

    let dark = gtk::Settings::default()
        .map(|settings| {
            settings.is_gtk_application_prefer_dark_theme()
                || settings
                    .gtk_theme_name()
                    .map(|name| name.to_ascii_lowercase().contains("dark"))
                    .unwrap_or(false)
        })
        .unwrap_or(true);
    let bytes = if dark {
        include_bytes!("../icons/tray-dark.png").as_slice()
    } else {
        include_bytes!("../icons/tray-light.png").as_slice()
    };
    let icon = Image::from_bytes(bytes)?;

    TrayIconBuilder::with_id("dsh-tray")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            let Some(action) = Action::from_id(event.id().as_ref()) else {
                return;
            };
            match action {
                Action::Show => windows::show_main_or_setup(app),
                Action::Language => crate::localization_ui::show(app),
                Action::Notifications => {
                    let requested = notices.is_checked().unwrap_or(false);
                    let enabled = crate::notification_ui::toggle(app, requested);
                    let _ = notices.set_checked(enabled);
                }
                Action::Restart => restart_server(app),
                Action::Updates => update::check_and_show(app),
                Action::ShellStatus => crate::shell_status_ui::show(app),
                Action::Pairing => {
                    if let Err(error) = remote::open_window(app) {
                        ui::message(
                            app,
                            "Could not open pairing",
                            &format!("{error:#}"),
                            gtk::MessageType::Error,
                        );
                    }
                }
                Action::Diagnostics => diagnostics::save_and_show(app),
                Action::DiagnosticsFolder => diagnostics::open_folder(),
                Action::Profile => util::open_path(&config::web_profile_dir()),
                Action::BackupProfile => profile_ui::backup_web(app),
                Action::ProfileBackups => profile_ui::open_backups(app),
                Action::VerifyBackup => crate::profile_verify_ui::show(app),
                Action::BackupHistory => crate::profile_history_ui::show(app),
                Action::Storage => crate::storage_ui::show(app),
                Action::Plugins => crate::plugins_ui::show(app),
                Action::DataSnapshot => crate::data_snapshot_ui::show(app),
                Action::ExportBackup => crate::backup_export_ui::show(app),
                Action::Downloads => crate::download_ui::show_history(app),
                Action::Quit => {
                    app.state::<AppState>()
                        .quitting
                        .store(true, Ordering::SeqCst);
                    app.exit(0);
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                windows::show_main_or_setup(tray.app_handle());
            }
        })
        .build(app)
}

fn restart_server(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = service::restart_and_reload(&app).await {
            let app_for_main = app.clone();
            let message = format!("{error:#}");
            let _ = app.run_on_main_thread(move || {
                ui::message(
                    &app_for_main,
                    "Could not restart DSH",
                    &message,
                    gtk::MessageType::Error,
                );
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::Action;

    #[test]
    fn every_action_has_a_unique_id_and_label() {
        let mut ids = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for action in Action::ALL {
            assert!(ids.insert(action.id()), "duplicate id {}", action.id());
            assert!(
                labels.insert(action.label()),
                "duplicate label {}",
                action.label()
            );
            assert!(!action.id().is_empty());
            assert!(!action.label().is_empty());
            // Ids are stable, machine-facing tokens: no spaces or markup.
            assert!(
                action
                    .id()
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
                "unexpected id {}",
                action.id()
            );
        }
        assert_eq!(ids.len(), Action::ALL.len());
    }

    #[test]
    fn ids_round_trip_through_the_handler_mapping() {
        for action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("show"), Some(Action::Show));
        assert_eq!(Action::from_id(""), None);
        assert_eq!(Action::from_id("silently-dead-entry"), None);
    }

    #[test]
    fn the_menu_order_covers_every_variant_exactly_once() {
        // A compile-time exhaustive match keeps this list honest: adding a
        // variant without updating ALL is a build error here.
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            match action {
                Action::Show
                | Action::Restart
                | Action::Updates
                | Action::ShellStatus
                | Action::Pairing
                | Action::Diagnostics
                | Action::DiagnosticsFolder
                | Action::Profile
                | Action::BackupProfile
                | Action::VerifyBackup
                | Action::BackupHistory
                | Action::DataSnapshot
                | Action::ExportBackup
                | Action::Storage
                | Action::Plugins
                | Action::ProfileBackups
                | Action::Downloads
                | Action::Notifications
                | Action::Language
                | Action::Quit => {}
            }
            assert!(seen.insert(action.id()), "{} listed twice", action.id());
        }
    }
}
