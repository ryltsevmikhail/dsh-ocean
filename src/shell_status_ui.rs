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
            message: "Loading local updater state…".into(),
        }
    }
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
        trust.set_text(&match &model.settings {
            Some(settings)=>{
                let source_text=|product,title:&str|match settings.source(product){
                    Ok(source)=>match source.trust(){Ok(key)=>format!("{title}: {}\nPublic-key fingerprint: {}",key.id(),key.fingerprint()),Err(_)=>format!("{title}: invalid local trust")},
                    Err(_)=>format!("{title}: not configured"),
                };
                format!("{}\n{}\nNo key is fetched from a feed. Provision public trust with --configure-updates and an independently confirmed fingerprint; current endpoint details are available with --update-settings.",source_text(Product::Shell,"Shell signed source"),source_text(Product::Runtime,"Core signed source"))
            },
            None=>"Updater trust is not configured or local configuration could not be read. Ask the owner/distribution to provision independently authenticated public trust. Advanced: dsh-ocean --configure-updates CONFIG --confirm-shell-key SHA256 (runtime root is separate). No key is discovered from a feed.".into(),
        });
    }
    if let Some(status) = widgets.status.upgrade() {
        let destination = match destination {
            Some(destination) => format!(
                "Destination: {}\n{}\nSelected on disk: {}\n{}\nManaged launcher: {}",
                if destination.enrolled {
                    "enrolled"
                } else {
                    "not enrolled"
                },
                destination.prefix.display(),
                current
                    .map(|current| format!("{} ({})", current.release, kind_name(current.kind)))
                    .unwrap_or_else(|| "none".into()),
                destination.installation_error.as_deref().unwrap_or(
                    if destination.pending_recovery {
                        "Interrupted transaction: explicit recovery required"
                    } else {
                        "Original installation and running process are unchanged"
                    }
                ),
                destination.launcher.display()
            ),
            None => "Destination: unavailable / loading".into(),
        };
        let candidate = model
            .candidate
            .as_ref()
            .map(|candidate| {
                let verified=candidate.verified();let manifest=verified.manifest();let artifact=verified.artifact();
                format!("\nAuthenticated candidate: {} — {} / sequence {}\n{} · {:?} · {} bytes\nExpires: {}\nArtifact SHA-256: {}",manifest.release,channel_name(manifest.channel),manifest.sequence,kind_name(artifact.kind),artifact.target,artifact.size,manifest.expires_at,artifact.sha256)
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
                    let progress = control.snapshot();
                    format!(
                        "{:?}: {}{}{}",
                        progress.phase,
                        progress.bytes,
                        progress
                            .total
                            .map(|total| format!(" / {total} bytes"))
                            .unwrap_or_default(),
                        if progress.cancel_requested {
                            " — cancellation requested; waiting for owned cleanup"
                        } else if progress.phase == Phase::Committing {
                            " — finishing local publication/transaction; cannot cancel"
                        } else {
                            " (counts are not signature/installation success)"
                        }
                    )
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
                result: Err(anyhow::anyhow!(
                    "native worker terminated; refresh local status and inspect pending recovery"
                )),
                cancelled: false,
            },
        };
        let close = {
            let mut model = model.borrow_mut();
            model.active = None;
            if completion.cancelled {
                model.message =
                    "Cancelled; no new operation will be started from this result.".into();
                model.candidate = None;
                discard_cache(model.cached.take());
            } else {
                match completion.result {
                    Err(error) => {
                        model.message =
                            crate::auth::redact(&format!("Update action failed: {error:#}"));
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
                        model.message =
                            "Local state refreshed. No network request was made.".into();
                    }
                    Ok(Outcome::Channel(settings)) => {
                        model.settings = Some(settings);
                        model.candidate = None;
                        discard_cache(model.cached.take());
                        model.message="Channel saved. Check again; previous candidates were invalidated. Core channel here is for signed runtime updates, not the old npm selector.".into();
                    }
                    Ok(Outcome::Enrolled(destination)) => {
                        model.destination = Some(destination);
                        model.message="Explicit managed destination enabled. This does not replace or convert the original package installation.".into();
                    }
                    Ok(Outcome::Checked(candidate)) => {
                        model.candidate = Some(candidate);
                        discard_cache(model.cached.take());
                        model.message="Signature, policy and remembered floor checked. Artifact has NOT been downloaded or applied.".into();
                    }
                    Ok(Outcome::Downloaded(cached)) => {
                        model.cached = Some(cached);
                        model.message="Exact signed artifact downloaded/hashed into private non-executable staging. Nothing is active yet.".into();
                    }
                    Ok(Outcome::Changed(receipt, destination)) => {
                        model.destination = Some(destination);
                        model.candidate = None;
                        discard_cache(model.cached.take());
                        model.message=format!("Local transaction completed; selection changed on disk only. Running client/core/profile are unchanged. {}\nQuit this client using its Quit action, then open DSH Ocean (managed) from the application menu. Closing to tray is not quitting.",receipt.get("launcherError").and_then(|value|value.as_str()).map(|error|format!("Launcher needs operator attention: {error}")).unwrap_or_default());
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
        gtk::ButtonsType::YesNo,
        text,
    );
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
        Some("DSH Ocean signed shell updates"),
        parent,
        gtk::DialogFlags::MODAL,
        &[("Close", gtk::ResponseType::Close)],
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
    kind.append(Some("tar.gz"), "tar.gz — complete package");
    kind.append(Some("appimage"), "AppImage");
    kind.set_active_id(Some("tar.gz"));
    channels.attach(&gtk::Label::new(Some("Shell channel")), 0, 0, 1, 1);
    channels.attach(&shell, 1, 0, 1, 1);
    channels.attach(&gtk::Label::new(Some("Signed core channel")), 2, 0, 1, 1);
    channels.attach(&runtime, 3, 0, 1, 1);
    channels.attach(&gtk::Label::new(Some("Managed shell format")), 0, 1, 1, 1);
    channels.attach(&kind, 1, 1, 3, 1);
    content.pack_start(&channels, false, false, 0);
    let trust = text_label("");
    let status = text_label("Loading local state…");
    status.set_widget_name("shell-update-status");
    let progress = text_label("");
    content.pack_start(&trust, false, false, 0);
    content.pack_start(&status, false, false, 0);
    content.pack_start(&progress, false, false, 0);
    let definitions = [
        ("Refresh local status", "shell-update-refresh"),
        ("Check signed feed", "shell-update-check"),
        ("Download verified artifact", "shell-update-download"),
        ("Apply to managed destination…", "shell-update-apply"),
        ("Enable separate managed install…", "shell-update-enroll"),
        ("Rollback retained version…", "shell-update-rollback"),
        ("Recover interrupted transaction…", "shell-update-recover"),
        ("Cancel operation", "shell-update-cancel"),
    ];
    let actions = gtk::Grid::new();
    actions.set_column_spacing(8);
    actions.set_row_spacing(8);
    let buttons: Vec<_> = definitions
        .iter()
        .enumerate()
        .map(|(index, (text, name))| {
            let button = gtk::Button::with_label(text);
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
        button.connect_clicked(move|_|{
            let Some(dialog)=widgets.dialog.upgrade()else{return;};
            if index==CANCEL{if let Some(control)=&model.borrow().active{control.cancel();}return;}
            if model.borrow().active.is_some()||registry.busy(){return;}
            let run={let model=model.clone();let widgets=widgets.clone();let registry=registry.clone();move||{
                let task:Box<dyn FnOnce(Arc<Control>)->Result<Outcome>+Send>=match index {
                    REFRESH=>Box::new(|control|{control.check()?;let settings=update_settings::load()?;let destination=managed_shell::status()?;control.check()?;Ok(Outcome::Inspected(settings,destination))}),
                    CHECK=>{let kind=model.borrow().kind;Box::new(move|control|{let settings=update_settings::load()?;Ok(Outcome::Checked(Arc::new(update_feed::check_shell_controlled(&settings,kind,control)?)))})}
                    DOWNLOAD=>{let Some(candidate)=model.borrow().candidate.clone()else{return;};Box::new(move|control|Ok(Outcome::Downloaded(Arc::new(managed_shell::download(&candidate,control)?))))}
                    APPLY=>{let data={let model=model.borrow();model.candidate.clone().zip(model.cached.clone())};let Some((candidate,cached))=data else{return;};Box::new(move|control|{let receipt=managed_shell::apply(&candidate,&cached,&control)?;Ok(Outcome::Changed(receipt,managed_shell::status()?))})}
                    ENROLL=>Box::new(|control|{control.begin_commit()?;Ok(Outcome::Enrolled(managed_shell::enroll()?))}),
                    ROLLBACK=>Box::new(|control|{let receipt=managed_shell::rollback(&control)?;Ok(Outcome::Changed(receipt,managed_shell::status()?))}),
                    RECOVER=>Box::new(|control|{let receipt=managed_shell::recover(&control)?;Ok(Outcome::Changed(receipt,managed_shell::status()?))}),
                    _=>return,
                };
                start(registry,model,widgets,task);
            }};
            if matches!(index,APPLY|ENROLL|ROLLBACK|RECOVER){confirm(&dialog,if index==ENROLL{"Enable a separate owned user destination? This does not replace your package/current executable or grant authority from an APPIMAGE/path hint. Unknown existing contents are refused."}else{"Run this explicit local transaction? It cannot be cancelled once applying/rolling back/recovering. The running client and core/profile are unchanged; quit and reopen the managed launcher afterward."},run);}else{run();}
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
            state.message = "Format preference changed; check the signed feed again.".into();
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
