use std::process::Command;

use semver::Version;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::bootstrap::{self, Runtime};
use crate::state::AppState;
use crate::{auth, config, runtime_update, service, systemd, ui};

#[derive(Clone, Debug)]
struct UpdateInfo {
    current: String,
    latest: String,
    next: String,
    channel: String,
    target: String,
    update_available: bool,
}

fn clean_version(value: &str) -> Result<Version, String> {
    Version::parse(value.strip_prefix('v').unwrap_or(value))
        .map_err(|_| format!("invalid DSH version: {value:?}"))
}

fn dist_tags(runtime: &Runtime) -> Result<Value, String> {
    let npm = bootstrap::npm_path(runtime);
    let output = Command::new(&npm)
        .args([
            "view",
            "@deepseek-ai/dsh",
            "dist-tags",
            "--json",
            "--fetch-timeout=10000",
            "--fetch-retries=1",
        ])
        .env("PATH", bootstrap::path_env(Some(&runtime.node)))
        .output()
        .map_err(|error| format!("could not run {}: {error}", npm.display()))?;
    if !output.status.success() {
        return Err(auth::redact(String::from_utf8_lossy(&output.stderr).trim()));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("invalid npm response: {error}"))
}

fn from_tags(current: &str, channel: &str, tags: &Value) -> Result<UpdateInfo, String> {
    let tag = match channel {
        "latest" | "stable" => "latest",
        "next" => "next",
        other => return Err(format!("unsupported update channel: {other:?}")),
    };
    let target = tags
        .get(tag)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("npm response has no {tag:?} version"))?;
    let target_version = clean_version(target)?;
    let current_version = clean_version(current)?;
    Ok(UpdateInfo {
        current: current.to_owned(),
        latest: tags
            .get("latest")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        next: tags
            .get("next")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        channel: channel.to_owned(),
        target: target_version.to_string(),
        update_available: target_version > current_version,
    })
}

fn query(runtime: &Runtime) -> Result<UpdateInfo, String> {
    let channel = config::load()
        .update_channel
        .unwrap_or_else(|| "latest".into());
    from_tags(&runtime.version, &channel, &dist_tags(runtime)?)
}

pub fn check_and_show(app: &AppHandle) {
    let app = app.clone();
    // Node discovery and npm registry calls must not block GTK's event loop.
    tauri::async_runtime::spawn_blocking(move || {
        let result = bootstrap::detect()
            .ok_or_else(|| {
                "DeepSeek Harness was not found; install it in the setup window first.".to_owned()
            })
            .and_then(|runtime| query(&runtime).map(|info| (runtime, info)));
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || match result {
            Ok((runtime, info)) => present_update(&app_for_main, runtime, info),
            Err(error) => ui::message(
                &app_for_main,
                "DSH update check failed",
                &auth::redact(&error),
                gtk::MessageType::Error,
            ),
        });
    });
}

fn managed_update_base(app: &AppHandle) -> Option<url::Url> {
    let server = app.state::<AppState>().server()?;
    if server.mode == service::ServerMode::Child || !systemd::is_active() {
        return None;
    }
    (server.base_url.scheme() == "http" && server.base_url.host_str() == Some("127.0.0.1"))
        .then_some(server.base_url)
}

fn present_update(app: &AppHandle, runtime: Runtime, info: UpdateInfo) {
    let mut body = format!(
        "Installed:  {}\nChannel:    {}\nlatest:     {}\nnext:       {}",
        info.current, info.channel, info.latest, info.next
    );
    if !info.update_available {
        body.push_str("\n\nYou are on the latest DSH for this channel.");
        ui::message(app, "DSH updates", &body, gtk::MessageType::Info);
        return;
    }
    if !runtime.is_generation_managed() {
        body.push_str("\n\nThis is an external installation. Ocean does not overwrite external npm prefixes. Use the original package manager; safe migration to private generations is not implemented yet.");
        ui::message(
            app,
            "DSH update available",
            &body,
            gtk::MessageType::Warning,
        );
        return;
    }
    if managed_update_base(app).is_none() {
        body.push_str("\n\nAutomatic generation activation currently requires a running local dsh-web.service. Managed-child and remote server updates are not supported yet; no files were changed.");
        ui::message(
            app,
            "DSH update available",
            &body,
            gtk::MessageType::Warning,
        );
        return;
    }
    body.push_str(&format!(
        "\n\nUpdate to {} now?\nThe local server and its WebSocket clients will restart.",
        info.target
    ));
    body.push_str("\nA separate user-manager worker validates the candidate and restores the previous runtime/unit on failure. Closing this window does not cancel the transaction.\nIMPORTANT: rollback does not undo profile/data migrations. Make a profile backup before proceeding.\nIntegrity currently relies on npm/Node checksums; a signed Ocean update feed is not implemented yet.");
    let app_for_update = app.clone();
    ui::confirm(app, "DSH update available", &body, move || {
        tauri::async_runtime::spawn(async move {
            perform_update(app_for_update, runtime, info.target).await;
        });
    });
}

async fn perform_generation_update(app: AppHandle, runtime: Runtime, target: String) {
    let Some(base) = managed_update_base(&app) else {
        show_message(
            &app,
            "DSH update refused",
            "A running local dsh-web.service is required.",
            gtk::MessageType::Warning,
        );
        return;
    };
    let mut task = match runtime_update::prepare(&runtime, &target, &base) {
        Ok(task) => task,
        Err(error) => {
            show_message(
                &app,
                "Could not prepare DSH update",
                &format!("{error:#}"),
                gtk::MessageType::Error,
            );
            return;
        }
    };
    app.state::<AppState>().log(format!(
        "Starting background transaction {}; log: {}",
        task.unit,
        task.log.display()
    ));
    let result = task.command.output().await;
    let tail = runtime_update::log_tail(&task.log);
    for line in tail.lines() {
        app.state::<AppState>().log(line);
    }
    match result {
        Ok(output) if output.status.success() => {
            let refresh = service::refresh_after_update(&app).await;
            let extra = refresh
                .err()
                .map(|e| {
                    format!("\nThe server passed health checks, but window refresh failed: {e:#}")
                })
                .unwrap_or_default();
            show_message(&app, "DSH updated", &format!("DeepSeek Harness {target} is serving from the verified generation.{extra}\nLog: {}", task.log.display()), gtk::MessageType::Info);
        }
        other => {
            // A failed activation may have restarted the restored old unit.
            // Reauthenticate only; never trigger another restart here.
            if systemd::is_active() {
                let _ = service::refresh_after_update(&app).await;
            }
            let detail = match other {
                Ok(output) => format!(
                    "Update worker exited with {}.\n{}",
                    output.status,
                    auth::redact(String::from_utf8_lossy(&output.stderr).trim())
                ),
                Err(error) => format!("Could not confirm background update status: {error}"),
            };
            show_message(
                &app,
                "DSH update not committed",
                &format!(
                    "{detail}\n{tail}\nLog: {}\nWorker: {}",
                    task.log.display(),
                    task.unit
                ),
                gtk::MessageType::Error,
            );
        }
    }
}

async fn perform_update(app: AppHandle, runtime: Runtime, target: String) {
    let state = app.state::<AppState>();
    let Some(_guard) = state.begin_update() else {
        show_message(
            &app,
            "Update already in progress",
            "Wait for the current transaction to finish.",
            gtk::MessageType::Warning,
        );
        return;
    };
    if !runtime.is_generation_managed() {
        show_message(&app, "External installation", "Ocean does not overwrite external npm prefixes. Update with the original package manager; safe migration to private generations is not implemented yet.", gtk::MessageType::Warning);
        return;
    }
    perform_generation_update(app.clone(), runtime, target).await;
}

fn show_message(app: &AppHandle, title: &'static str, body: &str, kind: gtk::MessageType) {
    let app_for_main = app.clone();
    let body = auth::redact(body);
    let _ = app.run_on_main_thread(move || ui::message(&app_for_main, title, &body, kind));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compares_release_candidates() {
        assert!(clean_version("0.2.1-rc.2").unwrap() > clean_version("0.2.0-rc.2").unwrap());
        assert_eq!(
            clean_version("v0.2.0-rc.2").unwrap(),
            Version::parse("0.2.0-rc.2").unwrap()
        );
    }
    #[test]
    fn validates_channel_and_version_without_silent_fallback() {
        let tags = serde_json::json!({"latest": "0.2.0-rc.2", "next": "0.2.1-rc.2"});
        assert!(
            from_tags("0.2.0-rc.2", "next", &tags)
                .unwrap()
                .update_available
        );
        assert!(
            !from_tags("0.2.0-rc.2", "stable", &tags)
                .unwrap()
                .update_available
        );
        assert!(from_tags("broken", "next", &tags).is_err());
        assert!(from_tags("0.2.0-rc.2", "custom", &tags).is_err());
        assert!(from_tags(
            "0.2.0-rc.2",
            "next",
            &serde_json::json!({"latest": "0.2.0-rc.2"})
        )
        .is_err());
        assert!(from_tags(
            "0.2.0-rc.2",
            "next",
            &serde_json::json!({"next": "package@malicious"})
        )
        .is_err());
    }
}
