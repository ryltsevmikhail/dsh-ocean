//! Read-only storage usage window.
//!
//! Shows how much space DSH sessions and shared stores occupy. Counts and bytes
//! only: no file names, no contents, and no cleanup action — deleting session
//! data is a separate, explicit operator decision that this window cannot make.
use gtk::prelude::*;
use tauri::{AppHandle, Manager};

use crate::session_inventory::{Summary, MAX_DEPTH, MAX_ENTRIES};

/// Count-only plain rendering in the startup language snapshot.
pub fn text(summary: &Summary) -> String {
    text_in(summary, crate::i18n::active_language())
}
pub fn text_in(summary: &Summary, language: crate::i18n::Language) -> String {
    let mut output = crate::i18n::render(
        language,
        "storage.counts",
        &[
            ("session_files", &summary.session_files.to_string()),
            ("session_bytes", &summary.session_bytes.to_string()),
            ("storage_files", &summary.storage_files.to_string()),
            ("storage_bytes", &summary.storage_bytes.to_string()),
            ("skipped", &summary.skipped.to_string()),
        ],
    )
    .expect("validated storage count placeholders");
    output.push_str("\n\n");
    if summary.truncated {
        output.push_str(
            &crate::i18n::render(
                language,
                "storage.truncated",
                &[
                    ("entries", &MAX_ENTRIES.to_string()),
                    ("depth", &MAX_DEPTH.to_string()),
                ],
            )
            .expect("validated storage bound placeholders"),
        );
        output.push_str("\n\n");
    }
    if summary.error.is_some() {
        output.push_str(
            crate::i18n::message(language, "storage.incomplete").expect("embedded storage message"),
        );
        output.push_str("\n\n");
    }
    output.push_str(
        crate::i18n::message(language, "storage.notice").expect("embedded storage notice"),
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
        Some(crate::i18n::text("storage.title")),
        parent,
        gtk::DialogFlags::MODAL,
        &[(crate::i18n::text("common.close"), gtk::ResponseType::Close)],
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
    fn storage_messages_use_selected_language_without_exposing_backend_paths() {
        for (language, _) in crate::i18n::LANGUAGES {
            let rendered = text_in(
                &summary(4, 2048, true, Some("/home/private/<script>")),
                language,
            );
            assert!(
                rendered.contains("2048") && rendered.contains("100") && rendered.contains("3")
            );
            assert!(
                !rendered.contains("/home/private")
                    && !rendered.contains("<script>")
                    && !rendered.contains("{session_files}")
            );
            assert!(
                rendered.contains(crate::i18n::message(language, "storage.incomplete").unwrap())
            );
            if language != crate::i18n::Language::En {
                assert_ne!(
                    rendered,
                    text_in(
                        &summary(4, 2048, true, Some("private")),
                        crate::i18n::Language::En
                    )
                );
            }
        }
        assert!(
            text_in(&summary(4, 2048, false, None), crate::i18n::Language::Ru)
                .contains("Файлы сеансов")
        );
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
