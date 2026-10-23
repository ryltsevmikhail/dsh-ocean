#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::Ordering;

use dsh_ocean::{
    bootstrap, commands, config, remote, service, single_instance, state, systemd, tray, windows,
};
use tauri::{Manager, RunEvent, WindowEvent};

fn main() {
    if handle_cli() {
        return;
    }

    let app = tauri::Builder::default()
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_setup_state,
            commands::prepare_service,
            commands::recover_runtime,
            commands::start_bootstrap,
            commands::cancel_bootstrap,
            commands::save_diagnostics,
            commands::open_diagnostics_folder,
            commands::close_pairing,
            commands::restart_service,
            commands::quit_app,
            remote::get_remote_state,
            commands::start_pairing_proxy,
            commands::stop_pairing_proxy,
            commands::pairing_proxy_status,
        ])
        .setup(|app| {
            if !single_instance::acquire(app.handle().clone()) {
                app.handle().exit(0);
                return Ok(());
            }
            let language = match dsh_ocean::localization::load(
                &config::config_dir().join("localization.json"),
            ) {
                Ok(preference) => dsh_ocean::i18n::resolve_language(
                    Some(preference.code()),
                    std::env::var("LC_ALL").ok().as_deref(),
                    std::env::var("LC_MESSAGES").ok().as_deref(),
                    std::env::var("LANG").ok().as_deref(),
                ),
                Err(error) => {
                    eprintln!("dsh-ocean: language preference unavailable; using English: {error}");
                    dsh_ocean::i18n::Language::En
                }
            };
            let _ = dsh_ocean::i18n::initialize(language);
            dsh_ocean::notification_ui::initialize(app.handle());
            match tray::install(app) {
                Ok(tray) => app.state::<state::AppState>().set_tray(tray),
                Err(error) => eprintln!("dsh-ocean: tray unavailable: {error}"),
            }
            windows::create_setup(app)?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = service::prepare_and_open(&handle).await;
            });
            service::start_health_monitor(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.app_handle().state::<state::AppState>();
                if !state.quitting.load(Ordering::SeqCst) {
                    api.prevent_close();
                    if windows::close_exits_app(window.label(), config::load().close_to_tray) {
                        state.quitting.store(true, Ordering::SeqCst);
                        window.app_handle().exit(0);
                    } else {
                        // Default: preserve the desktop session in the tray.
                        // Closing an auxiliary window must never quit the app.
                        let _ = window.hide();
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building DSH Ocean");

    app.run(|app_handle, event| match event {
        RunEvent::ExitRequested { api, .. } => {
            let state = app_handle.state::<state::AppState>();
            match state.updates.request_exit() {
                dsh_ocean::native_updates::Exit::Allow => (),
                dsh_ocean::native_updates::Exit::Waiting => api.prevent_exit(),
                dsh_ocean::native_updates::Exit::Wait(control) => {
                    api.prevent_exit();
                    state.quitting.store(true, Ordering::SeqCst);
                    let handle = app_handle.clone();
                    gtk::glib::timeout_add_local(
                        std::time::Duration::from_millis(100),
                        move || {
                            if control.snapshot().finished {
                                handle.exit(0);
                                gtk::glib::ControlFlow::Break
                            } else {
                                gtk::glib::ControlFlow::Continue
                            }
                        },
                    );
                }
            }
        }
        RunEvent::Exit => app_handle.state::<state::AppState>().shutdown(),
        _ => (),
    });
}

fn handle_cli() -> bool {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| {
            argument.into_string().unwrap_or_else(|_| {
                eprintln!("dsh-ocean: arguments must be valid UTF-8; use --help");
                std::process::exit(2);
            })
        })
        .collect();
    if args.is_empty() {
        return false;
    }

    match args[0].as_str() {
        "--backup-profile"
        | "--verify-profile-backup"
        | "--restore-profile"
        | "--list-profile-backups"
        | "--prune-profile-backups"
        | "--backup-data"
        | "--verify-data-backup"
        | "--export-backup"
        | "--verify-export"
        | "--verify-backups"
        | "--import-export"
        | "--list-backup-staging"
        | "--collect-backup-staging"
        | "--prepare-exposure"
        | "--pairing-proxy"
        | "--list-data-backups"
        | "--restore-data"
        | "--prune-data-backups" => {
            match dsh_ocean::profile_cli::run(&args) {
                Ok(value) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).expect("serialize profile result")
                    );
                    if args[0] == "--verify-backups"
                        && value["failedCount"].as_u64().is_some_and(|count| count > 0)
                    {
                        std::process::exit(1);
                    }
                }
                Err(error) => {
                    eprintln!(
                        "dsh-ocean: {}",
                        dsh_ocean::auth::redact(&format!("{error:#}"))
                    );
                    std::process::exit(1);
                }
            }
            true
        }
        "--update-settings"
        | "--configure-updates"
        | "--set-update-channel"
        | "--check-shell-update"
        | "--managed-shell-status"
        | "--enroll-shell"
        | "--install-managed-shell"
        | "--rollback-managed-shell"
        | "--recover-managed-shell"
        | "--verify-runtime-release"
        | "--accept-runtime-release"
        | "--check-runtime-update"
        | "--import-runtime-release"
        | "--check-runtime-bundle"
        | "--download-runtime-release" => {
            let result = if matches!(
                args[0].as_str(),
                "--verify-runtime-release"
                    | "--accept-runtime-release"
                    | "--check-runtime-update"
                    | "--import-runtime-release"
                    | "--check-runtime-bundle"
                    | "--download-runtime-release"
            ) {
                dsh_ocean::runtime_cli::run(&args)
            } else {
                dsh_ocean::update_cli::run(&args)
            };
            match result {
                Ok(value) => println!(
                    "{}",
                    serde_json::to_string_pretty(&value).expect("serialize updater result")
                ),
                Err(error) => {
                    eprintln!(
                        "dsh-ocean: {}",
                        dsh_ocean::auth::redact(&format!("{error:#}"))
                    );
                    std::process::exit(1);
                }
            }
            true
        }
        "--verify-release"
        | "--stage-release"
        | "--download-release"
        | "--accept-release"
        | "--release-state"
        | "--reset-release-state"
        | "--apply-release"
        | "--rollback-shell"
        | "--recover-shell" => {
            match dsh_ocean::release_cli::run(&args) {
                Ok(value) => println!(
                    "{}",
                    serde_json::to_string_pretty(&value).expect("serialize release verification")
                ),
                Err(error) => {
                    eprintln!(
                        "dsh-ocean: {}",
                        dsh_ocean::auth::redact(&format!("{error:#}"))
                    );
                    std::process::exit(1);
                }
            }
            true
        }
        "--draft-release" => {
            match dsh_ocean::release_draft_cli::run(&args) {
                Ok(value) => {
                    eprintln!("dsh-ocean: unsigned release draft only; signing and independent verification are required");
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value)
                            .expect("serialize unsigned release draft")
                    );
                }
                Err(error) => {
                    eprintln!(
                        "dsh-ocean: {}",
                        dsh_ocean::auth::redact(&format!("{error:#}"))
                    );
                    std::process::exit(1);
                }
            }
            true
        }
        "--runtime-maintenance" => {
            let mode = match dsh_ocean::runtime_maintenance::parse(&args) {
                Ok(mode) => mode,
                Err(error) => {
                    eprintln!(
                        "dsh-ocean: {}",
                        dsh_ocean::auth::redact(&format!("{error:#}"))
                    );
                    eprintln!(
                        "usage: dsh-ocean --runtime-maintenance <check|gc|rollback> [--confirm-maintenance]"
                    );
                    std::process::exit(2);
                }
            };
            match dsh_ocean::runtime_maintenance::run(mode) {
                Ok(code) => std::process::exit(code),
                Err(error) => {
                    eprintln!(
                        "dsh-ocean: {}",
                        dsh_ocean::auth::redact(&format!("{error:#}"))
                    );
                    std::process::exit(1);
                }
            }
        }
        "--pairing-plan" => {
            if args.len() != 1 {
                eprintln!("dsh-ocean: --pairing-plan accepts no arguments");
                std::process::exit(1);
            }
            let base = config::server_url();
            let plan = dsh_ocean::pairing_plan::plan(
                &base,
                config::server_port(&base),
                dsh_ocean::pairing_plan::detect_lan_address(),
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&plan).expect("serialize pairing plan")
            );
            true
        }
        "--shell-status" => {
            if args.len() != 1 {
                eprintln!("dsh-ocean: --shell-status accepts no arguments");
                std::process::exit(1);
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&dsh_ocean::shell_status::detect())
                    .expect("serialize shell update guidance")
            );
            true
        }
        "--version" | "-V" => {
            println!("dsh-ocean {}", env!("CARGO_PKG_VERSION"));
            true
        }
        "--help" | "-h" => {
            println!(
                "DSH Ocean desktop client\n\n\
                 Usage: dsh-ocean [OPTIONS]\n\n\
                 Options:\n  \
                   --version       print the client version\n  \
                   --shell-status  print read-only shell installation hints and update guidance\n  \
                   --pairing-plan  print a read-only phone pairing/exposure plan (changes nothing)\n  \
                   --prepare-exposure [IP]  stage a candidate LAN unit for you to apply\n  \
                   --pairing-proxy --ttl-seconds N [--port P] [--check]  short-lived LAN forwarder\n  \
                   --runtime-maintenance <check|gc|rollback> [--confirm-maintenance]  run the embedded runtime maintenance mode (rollback needs confirmation)\n  \
                   --runtime       print detected DSH/Node runtime information\n  \
                   --print-unit    print the generated dsh-web.service unit\n  \
                   --doctor [--json] [--strict]  read-only diagnostics; --strict exits 1 on a blocking problem\n  \
                   --backup-profile [NAME]  snapshot profile configuration (default web)\n  \
                   --verify-profile-backup SNAPSHOT  verify snapshot integrity\n  \
                   --list-profile-backups  read-only snapshot inventory (manifests only)\n  \
                   --prune-profile-backups --keep N [--profile NAME] --confirm-prune  delete older snapshots\n  \
                   --backup-data [DIR]  snapshot sessions and shared stores (no restore yet)\n  \
                   --verify-data-backup SNAPSHOT  verify a whole-data snapshot\n  \
                   --list-data-backups [DIR]  list whole-data snapshots (metadata only)\n  \
                   --export-backup SNAPSHOT DEST.tar  export a verified snapshot as one tar\n  \
                   --verify-export ARCHIVE.tar  re-check an exported archive\n  \
                   --verify-backups [DIR]  verify every snapshot in the backup root\n  \
                   --import-export ARCHIVE.tar DEST_DIR  extract a verified snapshot copy\n  \
                   --list-backup-staging  list abandoned snapshot staging directories\n  \
                   --collect-backup-staging --older-than-minutes N --confirm-collect  remove them\n  \
                   --restore-data SNAPSHOT --confirm-offline  replace sessions/stores; retained copies kept\n  \
                   --prune-data-backups --keep N --confirm-prune  delete older whole-data snapshots\n  \
                   --restore-profile NAME SNAPSHOT --confirm-offline  restore configuration\n  \
                   --verify-runtime-release MANIFEST SIGNATURE TRUSTED_KEY --channel stable|next --target TARGET --min-sequence N --current-runtime-version VERSION  verify complete runtime metadata (no execution)\n  \
                   --accept-runtime-release ... --confirm-accept yes  remember a verified runtime floor separately from shell\n  \
                   --download-runtime-release ... --output ARCHIVE --confirm-download yes  download only authenticated complete runtime bytes, no execution or floor change\n  \
                   --import-runtime-release ... --artifact ARCHIVE --prefix DIR --confirm-import yes  import complete signed bytes without executing/selecting runtime\n  \
                   --check-runtime-bundle ... --artifact ARCHIVE --prefix DIR --confirm-check yes  execute authenticated native candidate checks in disposable state (no activation)\n  \
                   --check-runtime-update --current-runtime-version VERSION  check configured signed runtime source without installation\n  \
                   --managed-shell-status  inspect explicitly enrolled destination (no network)\n  \
                   --enroll-shell --confirm-managed yes  opt into a separate managed user destination\n  \
                   --install-managed-shell [--kind appimage|tar.gz] --confirm-install yes  authenticate configured release, download and apply to enrolled destination\n  \
                   --rollback-managed-shell --confirm-rollback yes  select verified retained managed version\n  \
                   --recover-managed-shell --confirm-recovery yes  recover interrupted managed selector transaction\n  \
                   --update-settings  show updater channels/trust fingerprints (no network or keys)\n  \
                   --configure-updates CONFIG --confirm-shell-key SHA256 [--confirm-runtime-key SHA256] [--confirm-replace-trust yes]  provision independently authenticated public trust\n  \
                   --set-update-channel shell|runtime stable|next  change only the selected updater channel\n  \
                   --check-shell-update [--kind appimage|tar.gz]  authenticate configured feed metadata only\n  \
                   --verify-release MANIFEST SIGNATURE TRUSTED_KEY --channel stable|next --target TARGET --kind KIND --min-sequence N [--artifact FILE]  offline verification only\n  \
                   --accept-release ...  the same checks, then remember the accepted sequence locally\n  \
                   --release-state  print remembered per-channel release sequences\n  \
                   --reset-release-state --confirm-reset  forget them (allows older signed manifests again)\n  \
                   --apply-release ...  verify and activate a local artifact into a private prefix (no download)\n  \
                   --rollback-shell --prefix DIR --confirm-rollback yes  reactivate the retained previous version\n  \
                   --recover-shell --prefix DIR --confirm-recovery yes  restore verified selectors after an interrupted shell activation\n  \
                   --stage-release MANIFEST SIGNATURE TRUSTED_KEY [policy options] --artifact SOURCE --output NEW_FILE  verified local copy only, no install\n  \
                   --download-release MANIFEST SIGNATURE TRUSTED_KEY [policy options] --output NEW_FILE  signed HTTPS artifact to verified staging; no install\n  \
                   --draft-release --release VERSION --channel CHANNEL --sequence N --valid-hours N --asset TARGET KIND FILE [--asset ...]  unsigned JSON only; single-artifact flags also supported\n  \
                   -h, --help      show this help"
            );
            true
        }
        "--runtime" => {
            match bootstrap::info() {
                Some(info) => println!(
                    "{}",
                    serde_json::to_string_pretty(&info).expect("serialize runtime")
                ),
                None => {
                    eprintln!("dsh-ocean: DeepSeek Harness runtime not detected");
                    std::process::exit(1);
                }
            }
            true
        }
        "--print-unit" => {
            let Some(runtime) = bootstrap::detect() else {
                eprintln!("dsh-ocean: DeepSeek Harness runtime not detected");
                std::process::exit(1);
            };
            print!("{}", systemd::render_unit(&runtime, &config::server_url()));
            true
        }
        "--doctor" => {
            let json = args.iter().any(|argument| argument == "--json");
            let strict = args.iter().any(|argument| argument == "--strict");
            if args.len() > 1
                && args[1..]
                    .iter()
                    .any(|argument| argument != "--json" && argument != "--strict")
            {
                eprintln!("dsh-ocean: --doctor accepts only --json and --strict");
                std::process::exit(2);
            }
            let report = dsh_ocean::doctor::report();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report.to_json()).expect("serialize report")
                );
            } else {
                print!("{}", report.text());
            }
            if strict && report.has_blocking_problem() {
                eprintln!(
                    "dsh-ocean: --doctor --strict found a blocking problem (WebKitGTK ABI {:?}, server HTTP {:?})",
                    report.webkit_abi.status, report.server_status
                );
                std::process::exit(1);
            }
            true
        }
        _ => {
            eprintln!("dsh-ocean: unknown option; use --help");
            std::process::exit(2);
        }
    }
}
