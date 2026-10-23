//! GTK Save dialog separated from Tauri so its actual nested-loop behavior can
//! be exercised on an isolated display. No environment/test auto-accept hook.
use gtk::prelude::*;
use std::path::{Path, PathBuf};

pub fn build(
    parent: Option<&gtk::Window>,
    name: &str,
    source: &str,
    folder: Option<&Path>,
) -> gtk::FileChooserDialog {
    build_for(crate::i18n::active_language(), parent, name, source, folder)
}
pub fn build_for(
    language: crate::i18n::Language,
    parent: Option<&gtk::Window>,
    name: &str,
    source: &str,
    folder: Option<&Path>,
) -> gtk::FileChooserDialog {
    let tr = |key| crate::i18n::message(language, key).expect("validated download message");
    let origin = url::Url::parse(source)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| tr("downloads.origin").into());
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some(
            &crate::i18n::render(language, "downloads.save_title", &[("origin", &origin)])
                .expect("validated download message"),
        ),
        parent,
        gtk::FileChooserAction::Save,
        &[
            (tr("common.cancel"), gtk::ResponseType::Cancel),
            (tr("downloads.save_new"), gtk::ResponseType::Accept),
        ],
    );
    dialog.set_modal(true);
    dialog.set_default_response(gtk::ResponseType::Accept);
    dialog.set_local_only(true);
    dialog.set_current_name(&crate::downloads::suggested_name(name));
    dialog.set_do_overwrite_confirmation(false);
    let hint = gtk::Label::new(Some(tr("downloads.no_overwrite")));
    hint.set_use_markup(false);
    hint.show();
    dialog.set_extra_widget(&hint);
    if let Some(folder) = folder.filter(|path| path.is_dir()) {
        dialog.set_current_folder(folder);
    }
    dialog
}
pub fn run(dialog: &gtk::FileChooserDialog) -> Option<PathBuf> {
    let path = loop {
        if dialog.run() != gtk::ResponseType::Accept {
            break None;
        }
        let Some(path) = dialog.filename() else {
            continue;
        };
        let warning = match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break Some(path),
            Ok(_) => crate::i18n::text("downloads.exists"),
            Err(_) => crate::i18n::text("downloads.cannot_check"),
        };
        if let Some(label) = dialog
            .extra_widget()
            .and_then(|widget| widget.downcast::<gtk::Label>().ok())
        {
            label.set_text(warning);
        }
        // Keep the same response/chooser alive instead of forcing a refetch.
        // Transfer::prepare and NOREPLACE still recheck races after selection.
    };
    dialog.close();
    path
}
