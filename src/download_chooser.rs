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
    let origin = url::Url::parse(source)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| "the current DSH server".into());
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some(&format!("Save download from {origin}")),
        parent,
        gtk::FileChooserAction::Save,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Save new file", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_modal(true);
    dialog.set_default_response(gtk::ResponseType::Accept);
    dialog.set_local_only(true);
    dialog.set_current_name(&crate::downloads::suggested_name(name));
    dialog.set_do_overwrite_confirmation(false);
    let hint = gtk::Label::new(Some(
        "Existing files are never overwritten. Choose a new filename.",
    ));
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
            Ok(_) => "This filename already exists. Choose a new filename.",
            Err(_) => "Cannot check this filename. Choose a new location.",
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
