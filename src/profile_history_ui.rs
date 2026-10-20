//! Read-only backup history window.
//!
//! The window is deliberately informational: it lists snapshot metadata (profile,
//! validated time, counts, bytes) and never offers restore, prune, delete or
//! apply. Deletion stays an explicit CLI operation with its own confirmation,
//! because a destructive GUI action in a modal that can be mis-clicked is a much
//! larger safety decision than this screen is allowed to make.
use gtk::prelude::*;
use tauri::{AppHandle, Manager};

use crate::profile_backup;

pub const TITLE: &str = "DSH Ocean backup history";
pub const FAILURE_TEXT: &str = "Could not read the backup folder. Check that it exists and that you own the files, then try again. No files were changed.";

/// Fixed, content-free rendering of the inventory.
pub fn text(summaries: &[profile_backup::SnapshotSummary]) -> String {
    text_with_data(summaries, &[])
}

/// Renders configuration snapshots plus whole-data snapshots (sessions/stores).
pub fn text_with_data(
    summaries: &[profile_backup::SnapshotSummary],
    data: &[crate::data_backup::DataSummary],
) -> String {
    use std::fmt::Write as _;
    let mut output = String::new();
    if summaries.is_empty() && data.is_empty() {
        output.push_str(
            "No configuration backups found.\n\nUse \"Back up web configuration…\" to create one.",
        );
        return output;
    }
    if summaries.is_empty() {
        output.push_str("No configuration backups found.\n\n");
    } else {
        let total_bytes: u64 = summaries.iter().map(|item| item.bytes).sum();
        let _ = writeln!(
            output,
            "{} configuration backup(s), {} configuration bytes in total.\n",
            summaries.len(),
            total_bytes
        );
        for item in summaries {
            let when = match &item.created_at {
                Some(time) => time.as_str(),
                None => "timestamp unavailable",
            };
            let _ = writeln!(
            output,
            "profile:    {}\ntime:       {when}\nfiles:      {}\ndirectories:{}\nbytes:      {}\n",
            item.profile, item.files, item.directories, item.bytes
        );
        }
    }

    if !data.is_empty() {
        let total: u64 = data.iter().map(|item| item.bytes).sum();
        let _ = writeln!(
            output,
            "{} whole-data snapshot(s) (sessions and shared stores), {} bytes in total:\n",
            data.len(),
            total
        );
        for item in data {
            let when = match &item.created_at {
                Some(time) => time.as_str(),
                None => "timestamp unavailable",
            };
            let _ = writeln!(
                output,
                "time:       {when}\nfiles:      {}\nbytes:      {}\n",
                item.files, item.bytes
            );
        }
    }
    output.push_str(
        "Checksums are not signatures: this list does not prove authenticity, \
         a frozen snapshot or that other writers were stopped. Profile files can \
         contain secrets. Listing reads snapshot manifests only.\n\n\
         Restoring and pruning remain explicit offline CLI operations; nothing \
         was changed from this window.",
    );
    output
}

/// Reads the inventory. Errors are reduced to fixed text.
pub fn inventory() -> Result<Vec<profile_backup::SnapshotSummary>, String> {
    profile_backup::snapshots(&crate::config::profile_backup_dir())
        .map_err(|error| format!("{error:#}"))
        .map_err(|_| FAILURE_TEXT.to_owned())
}

/// Reads the whole-data inventory (metadata only).
pub fn data_inventory() -> Result<Vec<crate::data_backup::DataSummary>, String> {
    crate::data_backup::list(&crate::config::profile_backup_dir())
        .map_err(|error| format!("{error:#}"))
        .map_err(|_| FAILURE_TEXT.to_owned())
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
    dialog.set_default_size(680, 460);
    let label = gtk::Label::new(Some(body));
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
    let body = match (inventory(), data_inventory()) {
        (Ok(summaries), Ok(data)) => text_with_data(&summaries, &data),
        _ => FAILURE_TEXT.to_owned(),
    };
    let window = build(parent.as_ref().map(|window| window.upcast_ref()), &body);
    window.dialog.show_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        profile: &str,
        created: Option<&str>,
        files: usize,
        bytes: u64,
    ) -> profile_backup::SnapshotSummary {
        profile_backup::SnapshotSummary::for_test(
            "profile-web-1-1",
            profile,
            created.map(str::to_owned),
            files,
            1,
            bytes,
        )
    }

    #[test]
    fn text_is_content_free_and_explicitly_non_destructive() {
        let rendered = text(&[
            summary("web", Some("2023-11-14T22:13:20+00:00"), 3, 2048),
            summary("tui", None, 1, 10),
        ]);
        assert!(rendered.contains("2 configuration backup(s)"));
        assert!(rendered.contains("2058"));
        assert!(rendered.contains("2023-11-14T22:13:20+00:00"));
        assert!(rendered.contains("timestamp unavailable"));
        assert!(rendered.contains("profile:    tui"));
        assert!(rendered.contains("Checksums are not signatures"));
        assert!(rendered.contains("nothing"));
        // No restore/prune affordance may be advertised as available here.
        assert!(!rendered.contains("Restore now"));
        assert!(!rendered.contains("Delete"));
    }

    #[test]
    fn empty_inventory_explains_the_next_step() {
        let rendered = text(&[]);
        assert!(rendered.contains("No configuration backups found"));
        assert!(rendered.contains("Back up web configuration…"));
        assert!(!rendered.contains("Checksums are not signatures"));
    }

    fn data_summary(
        files: usize,
        bytes: u64,
        created: Option<&str>,
    ) -> crate::data_backup::DataSummary {
        crate::data_backup::DataSummary {
            name: "data-1-2".to_owned(),
            created_at: created.map(str::to_owned),
            files,
            bytes,
        }
    }

    #[test]
    fn whole_data_snapshots_are_listed_separately_and_content_free() {
        let rendered = text_with_data(
            &[summary("web", Some("2023-11-14T22:13:20+00:00"), 3, 2048)],
            &[
                data_summary(7, 4096, Some("2023-11-15T10:00:00+00:00")),
                data_summary(1, 10, None),
            ],
        );
        assert!(rendered.contains("1 configuration backup(s)"), "{rendered}");
        assert!(rendered.contains("2 whole-data snapshot(s)"), "{rendered}");
        assert!(rendered.contains("4106 bytes in total"), "{rendered}");
        assert!(rendered.contains("2023-11-15T10:00:00+00:00"), "{rendered}");
        assert!(rendered.contains("timestamp unavailable"), "{rendered}");
        // Names, paths and contents never appear.
        assert!(!rendered.contains("data-1-2"), "{rendered}");
        assert!(!rendered.contains("sessions/"), "{rendered}");
        assert!(!rendered.contains("Restore now"), "{rendered}");
    }

    #[test]
    fn data_only_inventory_does_not_claim_there_are_no_backups() {
        let rendered = text_with_data(&[], &[data_summary(2, 20, None)]);
        assert!(
            !rendered.contains("No configuration backups found.")
                || rendered.contains("whole-data")
        );
        assert!(
            rendered.contains("No configuration backups found."),
            "{rendered}"
        );
        assert!(rendered.contains("1 whole-data snapshot(s)"), "{rendered}");
    }

    #[test]
    fn failure_text_is_fixed_and_does_not_forward_backend_detail() {
        assert!(FAILURE_TEXT.contains("No files were changed"));
        assert!(!FAILURE_TEXT.contains('/'));
    }
}
