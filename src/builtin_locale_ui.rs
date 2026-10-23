//! Native-only scoped approval; not exposed as a webview command.
use crate::{
    builtin_locale_consent::{self, Scope},
    config, i18n,
    state::RuntimePermit,
};
use anyhow::{Context, Result};
use gtk::prelude::*;
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};
use tauri::{AppHandle, Manager};

pub fn build(
    parent: Option<&gtk::Window>,
    language: i18n::Language,
    profile: &Path,
) -> (gtk::Dialog, gtk::Label) {
    let tr = |key| i18n::message(language, key).expect("validated native permission message");
    let dialog = gtk::Dialog::new();
    dialog.set_title(tr("builtin.title"));
    dialog.set_modal(true);
    dialog.set_default_size(560, -1);
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }
    dialog.add_button(tr("builtin.decline"), gtk::ResponseType::Reject);
    dialog.add_button(tr("builtin.allow"), gtk::ResponseType::Accept);
    dialog.set_default_response(gtk::ResponseType::Reject);
    let text = i18n::render(
        language,
        "builtin.body",
        &[("profile", &profile.display().to_string())],
    )
    .expect("validated native permission message");
    let body = gtk::Label::new(Some(&text));
    body.set_use_markup(false);
    body.set_line_wrap(true);
    body.set_selectable(true);
    body.set_max_width_chars(72);
    body.set_xalign(0.0);
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    body.set_margin_start(16);
    body.set_margin_end(16);
    dialog.content_area().pack_start(&body, true, true, 0);
    (dialog, body)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Allow,
    Refuse,
    Dismissed,
    Stale,
}

/// Wire the actual native dialog once; timers and late responses cannot approve
/// a superseded operation. Kept independent of Tauri for genuine GTK tests.
pub fn watch_choice(
    dialog: &gtk::Dialog,
    current: Rc<dyn Fn() -> bool>,
    finish: impl FnOnce(Choice) + 'static,
) {
    let pending = Rc::new(RefCell::new(Some(finish)));
    let weak = dialog.downgrade();
    let watch = current.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(dialog) = weak.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        if !watch() {
            dialog.response(gtk::ResponseType::Reject);
            return gtk::glib::ControlFlow::Break;
        }
        gtk::glib::ControlFlow::Continue
    });
    dialog.connect_response(move |dialog, response| {
        let Some(finish) = pending.borrow_mut().take() else {
            return;
        };
        let choice = if !current() {
            Choice::Stale
        } else {
            match response {
                gtk::ResponseType::Accept => Choice::Allow,
                gtk::ResponseType::Reject => Choice::Refuse,
                _ => Choice::Dismissed,
            }
        };
        finish(choice);
        dialog.close();
    });
}

pub fn preference_path() -> std::path::PathBuf {
    config::config_dir().join("builtin-locale-consent.json")
}

/// Caller must prove owned cold-start/profile scope and no pending recovery
/// BEFORE this request. This API does not grant unit/process ownership. A stale
/// operation or closing the dialog cannot record approval. Human choice has no
/// short dispatch deadline; lifecycle invalidation closes the outstanding UI.
pub async fn request(app: &AppHandle, scope: &Scope, permit: &RuntimePermit) -> Result<bool> {
    permit.ensure_open()?;
    let path = preference_path();
    if let Some(allowed) = builtin_locale_consent::load(&path, scope)? {
        permit.ensure_open()?;
        return Ok(allowed);
    }
    let stamp = permit.stamp();
    let scope = scope.clone();
    let handle = app.clone();
    let watch = permit.stamp();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        if !stamp.is_current() {
            let _ = tx.send(Err("runtime approval request was superseded".to_owned()));
            return;
        }
        let parent = handle
            .get_webview_window("setup")
            .or_else(|| handle.get_webview_window("main"))
            .and_then(|window| window.gtk_window().ok());
        let (dialog, _) = build(
            parent.as_ref().map(|window| window.upcast_ref()),
            i18n::active_language(),
            scope.profile(),
        );
        watch_choice(
            &dialog,
            Rc::new(move || watch.is_current()),
            move |choice| {
                let result = if !stamp.is_current() || choice == Choice::Stale {
                    Err("runtime approval request was superseded".to_owned())
                } else {
                    match choice {
                        Choice::Allow | Choice::Refuse => {
                            let allowed = choice == Choice::Allow;
                            builtin_locale_consent::record(&path, &scope, allowed)
                                .map(|_| allowed)
                                .map_err(|error| format!("{error:#}"))
                        }
                        Choice::Dismissed => Ok(false),
                        Choice::Stale => unreachable!(),
                    }
                };
                let _ = tx.send(result);
            },
        );
        dialog.show_all();
    })
    .context("schedule native locale approval")?;
    let allowed = rx
        .await
        .context("native locale approval was dropped")?
        .map_err(anyhow::Error::msg)?;
    permit.ensure_open()?;
    Ok(allowed)
}
