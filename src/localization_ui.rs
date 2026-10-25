//! Native Ocean preference dialog. Never installs a plugin or touches DSH settings.
use crate::{
    config, i18n,
    localization::{self, Preference},
    ui,
};
use gtk::prelude::*;
use tauri::{AppHandle, Manager};

pub fn build(
    parent: Option<&gtk::Window>,
    language: i18n::Language,
    selected: Preference,
) -> (gtk::Dialog, gtk::ComboBoxText) {
    let tr = |key| i18n::message(language, key).expect("validated native message");
    let dialog = gtk::Dialog::new();
    dialog.set_title(tr("language.title"));
    dialog.set_modal(true);
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }
    dialog.add_button(tr("common.cancel"), gtk::ResponseType::Cancel);
    dialog.add_button(tr("common.ok"), gtk::ResponseType::Ok);
    dialog.set_default_response(gtk::ResponseType::Ok);
    let content = dialog.content_area();
    content.set_spacing(12);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    let combo = gtk::ComboBoxText::new();
    combo.append(Some("system"), tr("language.system"));
    for (locale, name) in i18n::LANGUAGES {
        combo.append(Some(locale.code()), name);
    }
    combo.set_active_id(Some(selected.code()));
    let notice = gtk::Label::new(Some(tr("language.restart_notice")));
    notice.set_line_wrap(true);
    notice.set_use_markup(false);
    content.pack_start(&combo, false, false, 0);
    content.pack_start(&notice, false, false, 0);
    let dsh_notice = gtk::Label::new(Some(tr("language.dsh_notice")));
    dsh_notice.set_line_wrap(true);
    dsh_notice.set_use_markup(false);
    dsh_notice.set_max_width_chars(64);
    content.pack_start(&dsh_notice, false, false, 0);
    (dialog, combo)
}

pub fn show(app: &AppHandle) {
    let path = config::config_dir().join("localization.json");
    let language = i18n::active_language();
    let selected = match localization::load(&path) {
        Ok(value) => value,
        Err(error) => {
            let body = i18n::render(
                language,
                "language.read_failed",
                &[("detail", &format!("{error:#}"))],
            )
            .expect("validated native message");
            ui::message(
                app,
                i18n::text("language.title"),
                &body,
                gtk::MessageType::Error,
            );
            return;
        }
    };
    let parent = app
        .get_webview_window("setup")
        .or_else(|| app.get_webview_window("main"))
        .and_then(|window| window.gtk_window().ok());
    let (dialog, combo) = build(parent.as_ref().map(|w| w.upcast_ref()), language, selected);
    let app = app.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Ok {
            if let Some(value) = combo.active_id().as_deref().and_then(Preference::parse) {
                match localization::save(&path, value) {
                    Ok(()) => ui::message(
                        &app,
                        i18n::text("language.title"),
                        i18n::text("language.restart_notice"),
                        gtk::MessageType::Info,
                    ),
                    Err(error) => {
                        let body = i18n::render(
                            language,
                            "language.save_failed",
                            &[("detail", &format!("{error:#}"))],
                        )
                        .expect("validated native message");
                        ui::message(
                            &app,
                            i18n::text("language.title"),
                            &body,
                            gtk::MessageType::Error,
                        );
                    }
                }
            }
        }
        dialog.close();
    });
    dialog.show_all();
}
