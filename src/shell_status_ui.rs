//! Native-only signed updater. GTK objects stay on the main thread; filesystem,
//! transport and transactions run on one guarded worker. No WebView invoke API.
use crate::release_feed::{Channel, Kind};
use crate::update_control::{Control, Phase};
use crate::update_settings::{self, Product, Settings};
use crate::{managed_shell, native_updates, update_feed};
use anyhow::Result;
use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use tauri::{AppHandle, Manager};

pub struct Window {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
}
struct Model {
    settings: Option<Settings>,
    destination: Option<managed_shell::Status>,
    candidate: Option<Arc<update_feed::ShellCandidate>>,
    cached: Option<Arc<managed_shell::CachedArtifact>>,
    kind: Kind,
    active: Option<Arc<Control>>,
    close_after: bool,
    message: String,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            settings: None,
            destination: None,
            candidate: None,
            cached: None,
            kind: Kind::TarGz,
            active: None,
            close_after: false,
            message: crate::i18n::text("shell_ui.initial").into(),
        }
    }
}
#[cfg(test)]
#[test]
fn selected_locale_names_all_signed_actions_and_keeps_enrollment_authority_explicit() {
    for (language, _) in crate::i18n::LANGUAGES {
        let labels = action_labels_in(language);
        assert_eq!(labels.len(), 8);
        for (index, key) in [
            "refresh", "check", "download", "apply", "enroll", "rollback", "recover", "cancel",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(
                labels[index],
                crate::i18n::message(language, &format!("shell_ui.{key}")).unwrap()
            );
        }
        let enrollment = transaction_confirmation_in(true, language);
        assert!(enrollment.contains("APPIMAGE"));
        assert_eq!(
            transaction_confirmation_in(false, language),
            crate::i18n::message(language, "shell_ui.confirm_transaction").unwrap()
        );
        if language == crate::i18n::Language::Ru {
            assert_ne!(labels[0], "Refresh local status");
        }
    }
}

#[cfg(test)]
#[test]
fn selected_locale_progress_preserves_commit_cancel_and_non_success_qualifications() {
    for (language, _) in crate::i18n::LANGUAGES {
        for (phase, key) in [
            (Phase::Checking, "checking"),
            (Phase::Downloading, "downloading"),
            (Phase::Preparing, "preparing"),
            (Phase::Verifying, "verifying"),
            (Phase::Committing, "committing"),
            (Phase::Finished, "finished"),
        ] {
            let progress = crate::update_control::Progress {
                phase,
                bytes: 2048,
                total: Some(4096),
                cancel_requested: false,
                finished: false,
            };
            let text = progress_text_in(&progress, language);
            assert!(
                text.contains("2048")
                    && text.contains("4096")
                    && text.contains(
                        crate::i18n::message(language, &format!("shell_status.phase.{key}"))
                            .unwrap()
                    )
            );
            let notice = if phase == Phase::Committing {
                "notice_commit"
            } else {
                "notice_counts"
            };
            assert!(text.contains(
                crate::i18n::message(language, &format!("shell_status.{notice}")).unwrap()
            ));
        }
        let cancelled = crate::update_control::Progress {
            cancel_requested: true,
            ..Default::default()
        };
        assert!(progress_text_in(&cancelled, language)
            .contains(crate::i18n::message(language, "shell_status.notice_cancel").unwrap()));
        let receipt = serde_json::json!({"launcherError":"<b>{bytes}</b>"});
        assert!(changed_text_in(&receipt, language).contains("<b>{bytes}</b>"));
    }
}

fn progress_text_in(
    progress: &crate::update_control::Progress,
    language: crate::i18n::Language,
) -> String {
    let phase = match progress.phase {
        Phase::Checking => "checking",
        Phase::Downloading => "downloading",
        Phase::Preparing => "preparing",
        Phase::Verifying => "verifying",
        Phase::Committing => "committing",
        Phase::Finished => "finished",
    };
    let phase = crate::i18n::message(language, &format!("shell_status.phase.{phase}"))
        .expect("embedded update phase");
    let total = progress
        .total
        .map(|total| {
            crate::i18n::render(
                language,
                "shell_status.total",
                &[("total", &total.to_string())],
            )
            .expect("validated progress total")
        })
        .unwrap_or_default();
    let notice = if progress.cancel_requested {
        "shell_status.notice_cancel"
    } else if progress.phase == Phase::Committing {
        "shell_status.notice_commit"
    } else {
        "shell_status.notice_counts"
    };
    crate::i18n::render(
        language,
        "shell_status.progress",
        &[
            ("phase", phase),
            ("bytes", &progress.bytes.to_string()),
            ("total", &total),
            (
                "notice",
                crate::i18n::message(language, notice)
                    .expect("embedded update progress qualification"),
            ),
        ],
    )
    .expect("validated update progress fields")
}
fn changed_text_in(receipt: &serde_json::Value, language: crate::i18n::Language) -> String {
    let launcher = receipt
        .get("launcherError")
        .and_then(|value| value.as_str())
        .map(|error| {
            crate::i18n::render(language, "shell_status.launcher_error", &[("error", error)])
                .expect("validated launcher diagnostic")
        })
        .unwrap_or_default();
    crate::i18n::render(
        language,
        "shell_status.result_changed",
        &[("launcher", &launcher)],
    )
    .expect("validated disk-only transaction receipt")
}

#[cfg(test)]
#[test]
fn selected_locale_trust_and_destination_reports_do_not_infer_owned_or_authenticated_state() {
    let feed = crate::update_settings::Feed {
        manifest_url: "https://example.invalid/manifest".into(),
        signature_url: "https://example.invalid/signature".into(),
        minimum_sequence: 1,
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&[47; 32]);
    let source = crate::update_settings::Source {
        key: crate::release_feed::PublicKeyDocument {
            version: 1,
            key_id: "synthetic-report-key".into(),
            public_key_hex: crate::release_feed::hex(key.verifying_key().as_bytes()),
        },
        stable: feed.clone(),
        next: feed,
    };
    let configured = Settings {
        shell: Some(source.clone()),
        ..Settings::default()
    };
    let fingerprint = source.trust().unwrap().fingerprint();
    let mut invalid = configured.clone();
    invalid.shell.as_mut().unwrap().key.public_key_hex = "invalid".into();
    let status = managed_shell::Status {
        prefix: "/private/{bytes}<b>".into(),
        enrolled: false,
        installation: None,
        installation_error: None,
        pending_recovery: true,
        launcher: "/private/launcher".into(),
    };
    for (language, _) in crate::i18n::LANGUAGES {
        assert_eq!(
            trust_text_in(None, language),
            crate::i18n::message(language, "shell_report.trust_unavailable").unwrap()
        );
        let valid_report = trust_text_in(Some(&configured), language);
        assert!(
            valid_report.contains("synthetic-report-key") && valid_report.contains(&fingerprint)
        );
        let invalid_report = trust_text_in(Some(&invalid), language);
        assert!(
            !invalid_report.contains(&fingerprint)
                && !invalid_report.contains("synthetic-report-key")
        );
        let settings = Settings::default();
        let trust = trust_text_in(Some(&settings), language);
        assert!(
            trust.contains("--configure-updates")
                && trust
                    .contains(crate::i18n::message(language, "shell_report.shell_source").unwrap())
        );
        let destination = destination_text_in(Some(&status), language);
        assert!(
            destination.contains("/private/{bytes}<b>")
                && destination
                    .contains(crate::i18n::message(language, "shell_report.not_enrolled").unwrap())
                && destination
                    .contains(crate::i18n::message(language, "shell_report.recovery").unwrap())
        );
        assert_eq!(
            destination_text_in(None, language),
            crate::i18n::message(language, "shell_report.unavailable").unwrap()
        );
    }
}

fn trust_text_in(settings: Option<&Settings>, language: crate::i18n::Language) -> String {
    let Some(settings) = settings else {
        return crate::i18n::message(language, "shell_report.trust_unavailable")
            .expect("embedded unavailable trust warning")
            .into();
    };
    let source_text = |product, title_key| {
        let title =
            crate::i18n::message(language, title_key).expect("embedded signed source title");
        match settings.source(product) {
            Ok(source) => match source.trust() {
                Ok(key) => crate::i18n::render(
                    language,
                    "shell_report.trust_valid",
                    &[
                        ("title", title),
                        ("id", key.id()),
                        ("fingerprint", &key.fingerprint()),
                    ],
                )
                .expect("validated public trust fields"),
                Err(_) => {
                    crate::i18n::render(language, "shell_report.trust_invalid", &[("title", title)])
                        .expect("validated invalid trust title")
                }
            },
            Err(_) => crate::i18n::render(
                language,
                "shell_report.trust_unconfigured",
                &[("title", title)],
            )
            .expect("validated missing trust title"),
        }
    };
    crate::i18n::render(
        language,
        "shell_report.trust",
        &[
            (
                "shell",
                &source_text(Product::Shell, "shell_report.shell_source"),
            ),
            (
                "core",
                &source_text(Product::Runtime, "shell_report.core_source"),
            ),
        ],
    )
    .expect("validated independently provisioned trust report")
}
fn destination_text_in(
    destination: Option<&managed_shell::Status>,
    language: crate::i18n::Language,
) -> String {
    let Some(destination) = destination else {
        return crate::i18n::message(language, "shell_report.unavailable")
            .expect("embedded unavailable destination message")
            .into();
    };
    let current = destination
        .installation
        .as_ref()
        .and_then(|installation| installation.current.as_ref())
        .map(|current| format!("{} ({})", current.release, kind_name(current.kind)))
        .unwrap_or_else(|| {
            crate::i18n::message(language, "shell_report.none")
                .expect("embedded empty selection message")
                .into()
        });
    let detail = destination
        .installation_error
        .as_deref()
        .unwrap_or_else(|| {
            crate::i18n::message(
                language,
                if destination.pending_recovery {
                    "shell_report.recovery"
                } else {
                    "shell_report.unchanged"
                },
            )
            .expect("embedded local transaction status")
        });
    crate::i18n::render(
        language,
        "shell_report.destination",
        &[
            (
                "enrolled",
                crate::i18n::message(
                    language,
                    if destination.enrolled {
                        "shell_report.enrolled"
                    } else {
                        "shell_report.not_enrolled"
                    },
                )
                .expect("embedded enrollment state"),
            ),
            ("prefix", &destination.prefix.display().to_string()),
            ("current", &current),
            ("detail", detail),
            ("launcher", &destination.launcher.display().to_string()),
        ],
    )
    .expect("validated local destination report")
}

fn discard_cache(cached: Option<Arc<managed_shell::CachedArtifact>>) {
    if let Some(cached) = cached {
        std::thread::spawn(move || drop(cached));
    }
}
impl Model {
    fn invalidate(&mut self) {
        self.candidate = None;
        discard_cache(self.cached.take());
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        discard_cache(self.cached.take());
    }
}
enum Outcome {
    Inspected(Settings, managed_shell::Status),
    Channel(Settings),
    Enrolled(managed_shell::Status),
    Checked(Arc<update_feed::ShellCandidate>),
    Downloaded(Arc<managed_shell::CachedArtifact>),
    Changed(serde_json::Value, managed_shell::Status),
}
struct Completion {
    result: Result<Outcome>,
    cancelled: bool,
}
#[derive(Clone)]
struct Widgets {
    dialog: glib::WeakRef<gtk::Dialog>,
    status: glib::WeakRef<gtk::Label>,
    trust: glib::WeakRef<gtk::Label>,
    progress: glib::WeakRef<gtk::Label>,
    shell: glib::WeakRef<gtk::ComboBoxText>,
    runtime: glib::WeakRef<gtk::ComboBoxText>,
    kind: glib::WeakRef<gtk::ComboBoxText>,
    buttons: Vec<glib::WeakRef<gtk::Button>>,
}
const REFRESH: usize = 0;
const CHECK: usize = 1;
const DOWNLOAD: usize = 2;
const APPLY: usize = 3;
const ENROLL: usize = 4;
const ROLLBACK: usize = 5;
const RECOVER: usize = 6;
const CANCEL: usize = 7;
fn action_labels_in(language: crate::i18n::Language) -> [&'static str; 8] {
    [
        "shell_ui.refresh",
        "shell_ui.check",
        "shell_ui.download",
        "shell_ui.apply",
        "shell_ui.enroll",
        "shell_ui.rollback",
        "shell_ui.recover",
        "shell_ui.cancel",
    ]
    .map(|key| crate::i18n::message(language, key).expect("embedded signed updater action"))
}
fn transaction_confirmation_in(enroll: bool, language: crate::i18n::Language) -> &'static str {
    crate::i18n::message(
        language,
        if enroll {
            "shell_ui.confirm_enroll"
        } else {
            "shell_ui.confirm_transaction"
        },
    )
    .expect("embedded signed updater confirmation")
}
fn channel_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::Next => "next",
    }
}
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::AppImage => "appimage",
        Kind::TarGz => "tar.gz",
    }
}
fn text_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(95);
    label.set_xalign(0.0);
    label
}
fn render(widgets: &Widgets, model: &Model, registry: &native_updates::Registry) {
    let busy = model.active.is_some() || registry.busy();
    let source = model
        .settings
        .as_ref()
        .and_then(|settings| settings.source(Product::Shell).ok());
    let destination = model.destination.as_ref();
    let current = destination
        .and_then(|status| status.installation.as_ref())
        .and_then(|installation| installation.current.as_ref());
    let healthy = destination
        .is_some_and(|status| status.installation_error.is_none() && !status.pending_recovery);
    let readiness = [
        !busy,
        !busy && source.is_some(),
        !busy && model.candidate.is_some(),
        !busy
            && model.cached.is_some()
            && model.candidate.is_some()
            && destination.is_some_and(|status| status.enrolled)
            && healthy,
        !busy && destination.is_some_and(|status| !status.enrolled),
        !busy
            && healthy
            && destination
                .and_then(|status| status.installation.as_ref())
                .is_some_and(|installation| installation.previous.is_some()),
        !busy && destination.is_some_and(|status| status.enrolled && status.pending_recovery),
        model.active.as_ref().is_some_and(|control| {
            let progress = control.snapshot();
            !progress.finished && !progress.cancel_requested && progress.phase != Phase::Committing
        }),
    ];
    for (button, sensitive) in widgets.buttons.iter().zip(readiness) {
        if let Some(button) = button.upgrade() {
            button.set_sensitive(sensitive);
        }
    }
    if let Some(combo) = widgets.shell.upgrade() {
        combo.set_sensitive(!busy && model.settings.is_some());
        if let Some(settings) = &model.settings {
            combo.set_active_id(Some(channel_name(settings.shell_channel)));
        }
    }
    if let Some(combo) = widgets.runtime.upgrade() {
        combo.set_sensitive(!busy && model.settings.is_some());
        if let Some(settings) = &model.settings {
            combo.set_active_id(Some(channel_name(settings.runtime_channel)));
        }
    }
    if let Some(combo) = widgets.kind.upgrade() {
        combo.set_sensitive(!busy && current.is_none());
        combo.set_active_id(Some(kind_name(model.kind)));
    }
    if let Some(trust) = widgets.trust.upgrade() {
        trust.set_text(&trust_text_in(
            model.settings.as_ref(),
            crate::i18n::active_language(),
        ));
    }
    if let Some(status) = widgets.status.upgrade() {
        let destination = destination_text_in(destination, crate::i18n::active_language());
        let candidate = model
            .candidate
            .as_ref()
            .map(|candidate| {
                let verified = candidate.verified();
                let manifest = verified.manifest();
                let artifact = verified.artifact();
                crate::i18n::render(
                    crate::i18n::active_language(),
                    "shell_report.candidate",
                    &[
                        ("release", &manifest.release),
                        ("channel", channel_name(manifest.channel)),
                        ("sequence", &manifest.sequence.to_string()),
                        ("kind", kind_name(artifact.kind)),
                        ("target", &format!("{:?}", artifact.target)),
                        ("bytes", &artifact.size.to_string()),
                        ("expires", &manifest.expires_at),
                        ("sha256", &artifact.sha256),
                    ],
                )
                .expect("validated authenticated candidate report")
            })
            .unwrap_or_default();
        status.set_text(&format!("{destination}{candidate}\n\n{}", model.message));
    }
    if let Some(label) = widgets.progress.upgrade() {
        label.set_text(
            &model
                .active
                .as_ref()
                .map(|control| {
                    progress_text_in(&control.snapshot(), crate::i18n::active_language())
                })
                .unwrap_or_default(),
        );
    }
}
fn start(
    registry: Arc<native_updates::Registry>,
    model: Rc<RefCell<Model>>,
    widgets: Widgets,
    task: impl FnOnce(Arc<Control>) -> Result<Outcome> + Send + 'static,
) {
    let operation = match registry.begin() {
        Ok(operation) => operation,
        Err(error) => {
            model.borrow_mut().message = error.to_string();
            return;
        }
    };
    let control = operation.control.clone();
    model.borrow_mut().active = Some(control.clone());
    render(&widgets, &model.borrow(), &registry);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = task(control.clone());
        let cancelled = !control.complete();
        drop(operation); // release the global gate BEFORE exposing completion
        let _ = tx.send(Completion { result, cancelled });
    });
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        let Some(dialog) = widgets.dialog.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let completion = match rx.try_recv() {
            Ok(completion) => completion,
            Err(mpsc::TryRecvError::Empty) => {
                render(&widgets, &model.borrow(), &registry);
                return glib::ControlFlow::Continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => Completion {
                result: Err(anyhow::anyhow!(crate::i18n::text(
                    "shell_status.worker_failed"
                ))),
                cancelled: false,
            },
        };
        let close = {
            let mut model = model.borrow_mut();
            model.active = None;
            if completion.cancelled {
                model.message = crate::i18n::text("shell_status.result_cancelled").into();
                model.candidate = None;
                discard_cache(model.cached.take());
            } else {
                match completion.result {
                    Err(error) => {
                        model.message = crate::auth::redact(
                            &crate::i18n::render(
                                crate::i18n::active_language(),
                                "shell_status.result_failed",
                                &[("error", &format!("{error:#}"))],
                            )
                            .expect("validated update error context"),
                        );
                        model.candidate = None;
                        discard_cache(model.cached.take());
                    }
                    Ok(Outcome::Inspected(settings, destination)) => {
                        model.invalidate();
                        if let Some(current) = destination
                            .installation
                            .as_ref()
                            .and_then(|installation| installation.current.as_ref())
                        {
                            model.kind = current.kind;
                        }
                        model.settings = Some(settings);
                        model.destination = Some(destination);
                        model.message = crate::i18n::text("shell_status.result_refreshed").into();
                    }
                    Ok(Outcome::Channel(settings)) => {
                        model.settings = Some(settings);
                        model.candidate = None;
                        discard_cache(model.cached.take());
                        model.message = crate::i18n::text("shell_status.result_channel").into();
                    }
                    Ok(Outcome::Enrolled(destination)) => {
                        model.destination = Some(destination);
                        model.message = crate::i18n::text("shell_status.result_enrolled").into();
                    }
                    Ok(Outcome::Checked(candidate)) => {
                        model.candidate = Some(candidate);
                        discard_cache(model.cached.take());
                        model.message = crate::i18n::text("shell_status.result_checked").into();
                    }
                    Ok(Outcome::Downloaded(cached)) => {
                        model.cached = Some(cached);
                        model.message = crate::i18n::text("shell_status.result_downloaded").into();
                    }
                    Ok(Outcome::Changed(receipt, destination)) => {
                        model.destination = Some(destination);
                        model.candidate = None;
                        discard_cache(model.cached.take());
                        model.message = changed_text_in(&receipt, crate::i18n::active_language());
                    }
                }
            }
            model.close_after
        };
        render(&widgets, &model.borrow(), &registry);
        if close {
            dialog.close();
        }
        glib::ControlFlow::Break
    });
}
fn confirm(parent: &gtk::Dialog, text: &str, action: impl FnOnce() + 'static) {
    let confirmation = gtk::MessageDialog::new(
        Some(parent),
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Warning,
        gtk::ButtonsType::None,
        text,
    );
    confirmation.add_buttons(&[
        (crate::i18n::text("common.no"), gtk::ResponseType::No),
        (crate::i18n::text("common.yes"), gtk::ResponseType::Yes),
    ]);
    let action = Rc::new(RefCell::new(Some(action)));
    confirmation.connect_response(move |dialog, response| {
        dialog.close();
        if response == gtk::ResponseType::Yes {
            if let Some(action) = action.borrow_mut().take() {
                action();
            }
        }
    });
    confirmation.show_all();
}

pub fn build(parent: Option<&gtk::Window>, report: &crate::shell_status::Report) -> Window {
    build_with_registry(
        parent,
        report,
        Arc::new(native_updates::Registry::default()),
    )
}
pub fn build_with_registry(
    parent: Option<&gtk::Window>,
    report: &crate::shell_status::Report,
    registry: Arc<native_updates::Registry>,
) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(crate::i18n::text("shell_ui.title")),
        parent,
        gtk::DialogFlags::MODAL,
        &[(crate::i18n::text("common.close"), gtk::ResponseType::Close)],
    );
    dialog.set_default_size(780, 720);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    let label=text_label(&format!("Running shell: {}\nInstallation hint: {:?} — {}\n{}\nInstallation information is not a network update check or package ownership proof. Package users: update the original via AUR/deb/Flatpak as applicable. Managed actions below affect only an explicitly enabled separate user destination, never the original executable.",env!("CARGO_PKG_VERSION"),report.installation_hint,report.evidence,report.guidance));
    content.pack_start(&label, false, false, 0);
    let channels = gtk::Grid::new();
    channels.set_column_spacing(12);
    channels.set_row_spacing(8);
    let shell = gtk::ComboBoxText::new();
    shell.set_widget_name("shell-update-channel");
    let runtime = gtk::ComboBoxText::new();
    runtime.set_widget_name("runtime-update-channel");
    for combo in [&shell, &runtime] {
        combo.append(Some("stable"), "stable");
        combo.append(Some("next"), "next");
        combo.set_active_id(Some("stable"));
    }
    let kind = gtk::ComboBoxText::new();
    kind.set_widget_name("shell-update-kind");
    kind.append(Some("tar.gz"), crate::i18n::text("shell_ui.tar_format"));
    kind.append(Some("appimage"), "AppImage");
    kind.set_active_id(Some("tar.gz"));
    channels.attach(
        &gtk::Label::new(Some(crate::i18n::text("shell_ui.shell_channel"))),
        0,
        0,
        1,
        1,
    );
    channels.attach(&shell, 1, 0, 1, 1);
    channels.attach(
        &gtk::Label::new(Some(crate::i18n::text("shell_ui.core_channel"))),
        2,
        0,
        1,
        1,
    );
    channels.attach(&runtime, 3, 0, 1, 1);
    channels.attach(
        &gtk::Label::new(Some(crate::i18n::text("shell_ui.format"))),
        0,
        1,
        1,
        1,
    );
    channels.attach(&kind, 1, 1, 3, 1);
    content.pack_start(&channels, false, false, 0);
    let trust = text_label("");
    let status = text_label(crate::i18n::text("shell_ui.loading"));
    status.set_widget_name("shell-update-status");
    let progress = text_label("");
    content.pack_start(&trust, false, false, 0);
    content.pack_start(&status, false, false, 0);
    content.pack_start(&progress, false, false, 0);
    let definitions = [
        "shell-update-refresh",
        "shell-update-check",
        "shell-update-download",
        "shell-update-apply",
        "shell-update-enroll",
        "shell-update-rollback",
        "shell-update-recover",
        "shell-update-cancel",
    ];
    let labels = action_labels_in(crate::i18n::active_language());
    let actions = gtk::Grid::new();
    actions.set_column_spacing(8);
    actions.set_row_spacing(8);
    let buttons: Vec<_> = definitions
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let button = gtk::Button::with_label(labels[index]);
            button.set_widget_name(name);
            actions.attach(&button, (index % 2) as i32, (index / 2) as i32, 1, 1);
            button
        })
        .collect();
    content.pack_start(&actions, false, false, 0);
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&content);
    dialog.content_area().pack_start(&scroll, true, true, 0);
    let model = Rc::new(RefCell::new(Model::default()));
    let widgets = Widgets {
        dialog: dialog.downgrade(),
        status: status.downgrade(),
        trust: trust.downgrade(),
        progress: progress.downgrade(),
        shell: shell.downgrade(),
        runtime: runtime.downgrade(),
        kind: kind.downgrade(),
        buttons: buttons.iter().map(|button| button.downgrade()).collect(),
    };
    for (index, button) in buttons.iter().enumerate() {
        let model = model.clone();
        let widgets = widgets.clone();
        let registry = registry.clone();
        button.connect_clicked(move |_| {
            let Some(dialog) = widgets.dialog.upgrade() else {
                return;
            };
            if index == CANCEL {
                if let Some(control) = &model.borrow().active {
                    control.cancel();
                }
                return;
            }
            if model.borrow().active.is_some() || registry.busy() {
                return;
            }
            let run = {
                let model = model.clone();
                let widgets = widgets.clone();
                let registry = registry.clone();
                move || {
                    let task: Box<dyn FnOnce(Arc<Control>) -> Result<Outcome> + Send> = match index
                    {
                        REFRESH => Box::new(|control| {
                            control.check()?;
                            let settings = update_settings::load()?;
                            let destination = managed_shell::status()?;
                            control.check()?;
                            Ok(Outcome::Inspected(settings, destination))
                        }),
                        CHECK => {
                            let kind = model.borrow().kind;
                            Box::new(move |control| {
                                let settings = update_settings::load()?;
                                Ok(Outcome::Checked(Arc::new(
                                    update_feed::check_shell_controlled(&settings, kind, control)?,
                                )))
                            })
                        }
                        DOWNLOAD => {
                            let Some(candidate) = model.borrow().candidate.clone() else {
                                return;
                            };
                            Box::new(move |control| {
                                Ok(Outcome::Downloaded(Arc::new(managed_shell::download(
                                    &candidate, control,
                                )?)))
                            })
                        }
                        APPLY => {
                            let data = {
                                let model = model.borrow();
                                model.candidate.clone().zip(model.cached.clone())
                            };
                            let Some((candidate, cached)) = data else {
                                return;
                            };
                            Box::new(move |control| {
                                let receipt = managed_shell::apply(&candidate, &cached, &control)?;
                                Ok(Outcome::Changed(receipt, managed_shell::status()?))
                            })
                        }
                        ENROLL => Box::new(|control| {
                            control.begin_commit()?;
                            Ok(Outcome::Enrolled(managed_shell::enroll()?))
                        }),
                        ROLLBACK => Box::new(|control| {
                            let receipt = managed_shell::rollback(&control)?;
                            Ok(Outcome::Changed(receipt, managed_shell::status()?))
                        }),
                        RECOVER => Box::new(|control| {
                            let receipt = managed_shell::recover(&control)?;
                            Ok(Outcome::Changed(receipt, managed_shell::status()?))
                        }),
                        _ => return,
                    };
                    start(registry, model, widgets, task);
                }
            };
            if matches!(index, APPLY | ENROLL | ROLLBACK | RECOVER) {
                confirm(
                    &dialog,
                    transaction_confirmation_in(index == ENROLL, crate::i18n::active_language()),
                    run,
                );
            } else {
                run();
            }
        });
    }
    for (combo, product) in [(&shell, Product::Shell), (&runtime, Product::Runtime)] {
        let model = model.clone();
        let widgets = widgets.clone();
        let registry = registry.clone();
        combo.connect_changed(move |combo| {
            let Some(channel) = combo.active_id().and_then(|id| match id.as_str() {
                "stable" => Some(Channel::Stable),
                "next" => Some(Channel::Next),
                _ => None,
            }) else {
                return;
            };
            let unchanged = {
                let model = model.borrow();
                model.active.is_some()
                    || model
                        .settings
                        .as_ref()
                        .is_none_or(|settings| settings.channel(product) == channel)
            };
            if unchanged || registry.busy() {
                return;
            }
            start(
                registry.clone(),
                model.clone(),
                widgets.clone(),
                move |control| {
                    control.begin_commit()?;
                    Ok(Outcome::Channel(update_settings::set_channel(
                        product, channel,
                    )?))
                },
            );
        });
    }
    {
        let model = model.clone();
        let widgets = widgets.clone();
        let registry = registry.clone();
        kind.connect_changed(move |combo| {
            let Some(kind) = combo.active_id().and_then(|id| match id.as_str() {
                "appimage" => Some(Kind::AppImage),
                "tar.gz" => Some(Kind::TarGz),
                _ => None,
            }) else {
                return;
            };
            if model.borrow().kind == kind || model.borrow().active.is_some() || registry.busy() {
                return;
            }
            let mut state = model.borrow_mut();
            state.kind = kind;
            state.invalidate();
            state.message = crate::i18n::text("shell_ui.format_changed").into();
            drop(state);
            render(&widgets, &model.borrow(), &registry);
        });
    }
    {
        let model = model.clone();
        dialog.connect_response(move |dialog, _| {
            let mut model = model.borrow_mut();
            if let Some(control) = &model.active {
                control.cancel();
                model.close_after = true;
            } else {
                dialog.close();
            }
        });
    }
    {
        let model = model.clone();
        dialog.connect_delete_event(move |_, _| {
            let mut model = model.borrow_mut();
            if let Some(control) = &model.active {
                control.cancel();
                model.close_after = true;
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    {
        let model = model.clone();
        dialog.connect_destroy(move |_| {
            if let Some(control) = &model.borrow().active {
                control.cancel();
            }
        });
    }
    {
        let model = model.clone();
        let widgets = widgets.clone();
        let registry = registry.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let Some(dialog) = widgets.dialog.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !dialog.is_visible() {
                return glib::ControlFlow::Break;
            }
            render(&widgets, &model.borrow(), &registry);
            glib::ControlFlow::Continue
        });
    }
    render(&widgets, &model.borrow(), &registry);
    start(registry, model, widgets, |control| {
        control.check()?;
        let settings = update_settings::load()?;
        let destination = managed_shell::status()?;
        control.check()?;
        Ok(Outcome::Inspected(settings, destination))
    });
    Window { dialog, label }
}
pub fn show(app: &AppHandle) {
    let parent = app
        .get_webview_window("main")
        .and_then(|window| window.gtk_window().ok());
    let report = crate::shell_status::detect();
    let window = build_with_registry(
        parent.as_ref().map(|window| window.upcast_ref()),
        &report,
        app.state::<crate::state::AppState>().updates.clone(),
    );
    window.dialog.show_all();
}
