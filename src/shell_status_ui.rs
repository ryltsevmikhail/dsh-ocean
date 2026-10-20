use gtk::prelude::*;
use tauri::{AppHandle, Manager};

pub struct Window {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
}
pub fn build(parent: Option<&gtk::Window>, report: &crate::shell_status::Report) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some("DSH Ocean shell updates"),
        parent,
        gtk::DialogFlags::MODAL,
        &[("Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(680, 420);
    let label = gtk::Label::new(Some(&crate::shell_status::text(report)));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(85);
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_margin_start(18);
    label.set_margin_end(18);
    label.set_margin_top(18);
    label.set_margin_bottom(18);
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&label);
    dialog.content_area().pack_start(&scroll, true, true, 0);
    dialog.connect_response(|dialog, _| dialog.close());
    Window { dialog, label }
}
pub fn show(app: &AppHandle) {
    let parent = app
        .get_webview_window("main")
        .and_then(|window| window.gtk_window().ok());
    let report = crate::shell_status::detect();
    let window = build(parent.as_ref().map(|window| window.upcast_ref()), &report);
    window.dialog.show_all();
}
