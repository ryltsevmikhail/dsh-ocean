//! Read-only storage usage window.
//!
//! Shows how much space DSH sessions and shared stores occupy. Counts and bytes
//! only: no file names, no contents, and no cleanup action — deleting session
//! data is a separate, explicit operator decision that this window cannot make.
use gtk::prelude::*;
use tauri::{AppHandle, Manager};

use crate::session_inventory::{Summary, MAX_DEPTH, MAX_ENTRIES};

pub const TITLE: &str = "DSH Ocean storage usage";

/// Fixed, content-free rendering.
pub fn text(summary: &Summary) -> String {
    use std::fmt::Write as _;
    let mut output = String::new();
    let _ = writeln!(
        output,
        "Sessions: {} file(s), {} bytes\nShared stores: {} file(s), {} bytes\nSkipped entries (links/special): {}\n",
        summary.session_files,
        summary.session_bytes,
        summary.storage_files,
        summary.storage_bytes,
        summary.skipped
    );
    if summary.truncated {
        let _ = writeln!(
            output,
            "Note: the scan stopped at its bound ({} entries / {} levels); the numbers \
             are a lower bound.\n",
            MAX_ENTRIES, MAX_DEPTH
        );
    }
    if summary.error.is_some() {
        let _ = writeln!(
            output,
            "One storage directory could not be inspected; the numbers may be incomplete.\n"
        );
    }
    output.push_str(
        "Counts only: no file names or contents were read, and symlinked or special \
         files were skipped rather than followed.\n\n\
         Nothing was deleted or moved here. Cleaning up sessions or shared stores is \
         an explicit offline decision, and DSH keeps its own source of truth.",
    );
    output
}

pub fn inventory() -> Summary {
    crate::session_inventory::inventory(&crate::config::dsh_home())
}

pub struct Window {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
}

pub fn build(parent: Option<&gtk::Window>, body: &str) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(TITLE),
        parent,
        gtk::DialogFlags::MODAL,
        &[("Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(640, 380);
    let label = gtk::Label::new(Some(body));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(80);
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
    let window = build(
        parent.as_ref().map(|window| window.upcast_ref()),
        &text(&inventory()),
    );
    window.dialog.show_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        session_files: usize,
        session_bytes: u64,
        truncated: bool,
        error: Option<&str>,
    ) -> Summary {
        Summary {
            session_files,
            session_bytes,
            storage_files: 2,
            storage_bytes: 100,
            skipped: 3,
            truncated,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn text_is_count_only_and_explicitly_non_destructive() {
        let rendered = text(&summary(4, 2_048, false, None));
        assert!(rendered.contains("Sessions: 4 file(s), 2048 bytes"));
        assert!(rendered.contains("Shared stores: 2 file(s), 100 bytes"));
        assert!(rendered.contains("Skipped entries (links/special): 3"));
        assert!(rendered.contains("no file names or contents"));
        assert!(rendered.contains("Nothing was deleted"));
        // No file path or delete affordance may appear.
        assert!(!rendered.contains("/home") && !rendered.contains("~/.dsh"));
        assert!(!rendered.contains("Delete now"));
    }

    #[test]
    fn truncation_and_errors_are_stated_not_hidden() {
        let rendered = text(&summary(1, 10, true, Some("denied")));
        assert!(rendered.contains("lower bound"));
        assert!(rendered.contains("could not be inspected"));
        // The raw backend message is not forwarded.
        assert!(!rendered.contains("denied"));
    }
}
