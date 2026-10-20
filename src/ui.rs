use gtk::prelude::*;
use tauri::{AppHandle, Manager};

fn parent(app: &AppHandle) -> Option<gtk::ApplicationWindow> {
    app.get_webview_window("main")
        .or_else(|| app.get_webview_window("setup"))
        .and_then(|window| window.gtk_window().ok())
}

pub fn message(app: &AppHandle, title: &str, body: &str, kind: gtk::MessageType) {
    let window = parent(app);
    let dialog = gtk::MessageDialog::new(
        window.as_ref(),
        gtk::DialogFlags::MODAL,
        kind,
        gtk::ButtonsType::Ok,
        body,
    );
    dialog.set_title(title);
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.show_all();
}

pub fn confirm<F>(app: &AppHandle, title: &str, body: &str, on_yes: F)
where
    F: FnOnce() + 'static,
{
    let window = parent(app);
    let dialog = gtk::MessageDialog::new(
        window.as_ref(),
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Question,
        gtk::ButtonsType::YesNo,
        body,
    );
    dialog.set_title(title);
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
