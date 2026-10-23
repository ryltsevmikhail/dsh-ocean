use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::state::AppState;
use crate::{auth, bootstrap, config, systemd, ui, util};

fn profile_summary() -> Value {
    profile_summary_at(&config::web_profile_dir())
}

fn profile_summary_at(profile: &Path) -> Value {
    let inventory = crate::plugins_ui::inventory(profile).ok();
    let dependencies = inventory.as_ref().map(|inventory| {
        inventory
            .dependencies
            .iter()
            .map(|(name, version)| (name.clone(), Value::String(version.clone())))
            .collect::<serde_json::Map<String, Value>>()
    });
    json!({
        "path": profile.display().to_string(),
        "exists": profile.join("package.json").exists(),
        "readable": inventory.is_some(),
        "dependencies": dependencies,
        "bundles": inventory.as_ref().map(|inventory| &inventory.bundles),
    })
}

/// App-independent, content-free facts shared with `--doctor`.
///
/// Contains counters and detected versions only: no profile contents, file names,
/// tokens or paths inside the DSH home.
pub fn environment() -> Value {
    let runtime_state = crate::doctor::runtime_summary(&crate::doctor::prefix());
    let storage = crate::session_inventory::inventory(&config::dsh_home());
    json!({
        "runtimeState": {
            "selected": runtime_state.selected,
            "previous": runtime_state.previous,
            "generations": runtime_state.generations,
            "unreferenced": runtime_state.unreferenced,
            "staleScratch": runtime_state.stale_scratch,
            "error": runtime_state.error,
        },
        "backups": crate::doctor::backups_text(&config::profile_backup_dir()),
        "storage": storage,
        "webkitAbi": crate::webkit_abi::probe(),
    })
}

pub fn collect(app: &AppHandle) -> Value {
    let state = app.state::<AppState>();
    let server = state.server();
    let runtime = bootstrap::detect();
    let profile = profile_summary();
    let journal = systemd::journal_tail(80);

    json!({
        "kind": "dsh-ocean-diagnostics",
        "generatedAt": chrono::Local::now().to_rfc3339(),
        "app": {
            "name": "DSH Ocean",
            "version": env!("CARGO_PKG_VERSION"),
            "identifier": "dev.dsh.ocean",
            "tauri": tauri::VERSION,
        },
        "system": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "session": std::env::var("XDG_SESSION_TYPE").ok(),
            "desktop": std::env::var("XDG_CURRENT_DESKTOP").ok(),
            "display": std::env::var("WAYLAND_DISPLAY").ok().or_else(|| std::env::var("DISPLAY").ok()),
        },
        "runtime": runtime.as_ref().map(|runtime| runtime.info()),
        "server": server.as_ref().map(|server| json!({
            "baseUrl": config::server_origin_text(&server.base_url),
            "mode": format!("{:?}", server.mode).to_ascii_lowercase(),
            "unit": server.unit,
            "tokenPresent": !server.token.is_empty(),
        })),
        "config": {
            "path": config::config_path().display().to_string(),
            "dshHome": config::dsh_home().display().to_string(),
            "dataDir": config::data_dir().display().to_string(),
            "stateDir": config::state_dir().display().to_string(),
        },
        "profile": profile,
        "environment": environment(),
        "service": {
            "unit": config::UNIT_NAME,
            "unitPath": systemd::unit_path().display().to_string(),
            "active": systemd::is_active(),
            "status": auth::redact(&systemd::status_text()),
        },
        "journal": {
            "unit": auth::redact(&journal),
        },
    })
}

pub fn write(app: &AppHandle) -> Result<PathBuf> {
    let directory = config::diagnostics_dir();
    let value = collect(app);
    let mut line = serde_json::to_string(&value)?;
    line.push('\n');
    write_bundle(&directory, line.as_bytes())
}

fn write_bundle(directory: &Path, bytes: &[u8]) -> Result<PathBuf> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .with_context(|| format!("create {}", directory.display()))?;
    let path = directory.join(format!(
        "native-ui-{}-{}.jsonl",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        crate::downloads::unique_id()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write {}", path.display()))?;
    file.sync_all()?;
    Ok(path)
}

static SAVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct SaveGuard;
impl SaveGuard {
    fn acquire() -> Option<Self> {
        SAVING
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .ok()
            .map(|_| Self)
    }
}
impl Drop for SaveGuard {
    fn drop(&mut self) {
        SAVING.store(false, std::sync::atomic::Ordering::Release);
    }
}

pub fn save_and_show(app: &AppHandle) {
    let Some(guard) = SaveGuard::acquire() else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let result = write(&app);
        let for_main = app.clone();
        let _ = app.run_on_main_thread(move || show_result(&for_main, result));
    });
}

fn saved_text_in(path: &Path, language: crate::i18n::Language) -> String {
    crate::i18n::render(
        language,
        "diagnostics.saved",
        &[("path", &path.display().to_string())],
    )
    .expect("validated diagnostic receipt path")
}

fn show_result(app: &AppHandle, result: Result<PathBuf>) {
    match result {
        Ok(path) => ui::message(
            app,
            crate::i18n::text("diagnostics.saved_title"),
            &saved_text_in(&path, crate::i18n::active_language()),
            gtk::MessageType::Info,
        ),
        Err(error) => ui::message(
            app,
            crate::i18n::text("diagnostics.failed_title"),
            &format!("{error:#}"),
            gtk::MessageType::Error,
        ),
    }
}

pub fn open_folder() {
    util::open_path(&config::diagnostics_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_receipt_uses_selected_locale_and_keeps_path_literal() {
        for (language, _) in crate::i18n::LANGUAGES {
            let text = saved_text_in(Path::new("/private/{version}<b>"), language);
            assert!(
                text.contains("/private/{version}<b>")
                    && text.contains("WebKitGTK")
                    && text.contains("ABI")
            );
            assert!(!text.contains("{path}"));
            if language == crate::i18n::Language::Ru {
                assert!(text.to_lowercase().contains("перед"));
            }
        }
    }
    #[test]
    fn diagnostic_worker_gate_is_exclusive_and_released_on_drop() {
        let guard = SaveGuard::acquire().expect("first worker");
        assert!(SaveGuard::acquire().is_none());
        drop(guard);
        assert!(SaveGuard::acquire().is_some());
    }

    #[test]
    fn diagnostic_bundles_are_private_and_do_not_overwrite_each_other() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-diagnostics-test-{}",
            crate::downloads::unique_id()
        ));
        let directory = root.join("diagnostics");
        let first = write_bundle(&directory, b"first\n").unwrap();
        let second = write_bundle(&directory, b"second\n").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"first\n");
        assert_eq!(fs::read(&second).unwrap(), b"second\n");
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in [first, second] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::write(root.join("package.json"), r#"{
            "dependencies": {"safe": "^1.2.3", "private": "https://user:secret@example.test/repo", "object": {"token": "secret"}},
            "dsh": {"profile": {"bundles": ["core", "token=secret"]}}
        }"#).unwrap();
        let summary = profile_summary_at(&root);
        assert_eq!(summary["dependencies"], json!({"safe": "^1.2.3"}));
        assert_eq!(summary["bundles"], json!(["core"]));
        assert!(!summary.to_string().contains("secret"));
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-diagnostics-test-"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn environment_section_is_count_only_and_version_only() {
        let value = environment();
        for key in ["runtimeState", "backups", "storage", "webkitAbi"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        let storage = value["storage"].as_object().expect("storage object");
        for key in [
            "sessionFiles",
            "sessionBytes",
            "storageFiles",
            "storageBytes",
            "skipped",
            "truncated",
        ] {
            assert!(storage.contains_key(key), "missing storage.{key}");
        }
        // Counters are numbers, never strings or file names.
        assert!(value["storage"]["sessionFiles"].is_number());
        assert!(value["storage"]["skipped"].is_number());
        assert!(value["backups"].is_string());
        let rendered = value.to_string();
        assert!(!rendered.contains("package.json"));
        assert!(!rendered.contains("token"));
        assert!(!rendered.contains("credentials"));
    }
}
