use gtk::prelude::*;
use tauri::{AppHandle, Manager};

pub fn build_message(
    parent: Option<&gtk::Window>,
    language: crate::i18n::Language,
    title: &str,
    body: &str,
    kind: gtk::MessageType,
) -> gtk::MessageDialog {
    let dialog = gtk::MessageDialog::new(
        parent,
        gtk::DialogFlags::MODAL,
        kind,
        gtk::ButtonsType::None,
        body,
    );
    dialog.set_title(title);
    dialog.set_use_markup(false);
    dialog.add_button(
        crate::i18n::message(language, "common.ok").unwrap(),
        gtk::ResponseType::Ok,
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    dialog
}

pub fn build_confirmation(
    parent: Option<&gtk::Window>,
    language: crate::i18n::Language,
    title: &str,
    body: &str,
) -> gtk::MessageDialog {
    let dialog = gtk::MessageDialog::new(
        parent,
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Question,
        gtk::ButtonsType::None,
        body,
    );
    dialog.set_title(title);
    dialog.set_use_markup(false);
    dialog.add_button(
        crate::i18n::message(language, "common.no").unwrap(),
        gtk::ResponseType::No,
    );
    dialog.add_button(
        crate::i18n::message(language, "common.yes").unwrap(),
        gtk::ResponseType::Yes,
    );
    dialog.set_default_response(gtk::ResponseType::No);
    dialog
}

fn parent(app: &AppHandle) -> Option<gtk::ApplicationWindow> {
    app.get_webview_window("main")
        .or_else(|| app.get_webview_window("setup"))
        .and_then(|window| window.gtk_window().ok())
}

pub fn message(app: &AppHandle, title: &str, body: &str, kind: gtk::MessageType) {
    let window = parent(app);
    let dialog = build_message(
        window.as_ref().map(|w| w.upcast_ref()),
        crate::i18n::active_language(),
        title,
        body,
        kind,
    );
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.show_all();
}

pub fn confirm<F>(app: &AppHandle, title: &str, body: &str, on_yes: F)
where
    F: FnOnce() + 'static,
{
    let window = parent(app);
    let dialog = build_confirmation(
        window.as_ref().map(|w| w.upcast_ref()),
        crate::i18n::active_language(),
        title,
        body,
    );
    let on_yes = std::cell::Cell::new(Some(on_yes));
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Yes {
            if let Some(callback) = on_yes.take() {
                callback();
            }
        }
        dialog.close();
    });
    dialog.show_all();
}
