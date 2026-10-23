//! Native-only live history. Clearing removes metadata, never files/staging.
use crate::{download_engine::SharedHistory, downloads::Entry};
use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
pub type Cancel = Rc<dyn Fn(Vec<String>) -> bool>;

fn text(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for c in value.chars() {
        if c.is_control()
            || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            result.extend(c.escape_default());
        } else {
            result.push(c);
        }
    }
    result
}
pub fn render(entries: &[Entry]) -> String {
    render_in(entries, crate::i18n::active_language())
}
pub fn render_in(entries: &[Entry], language: crate::i18n::Language) -> String {
    if entries.is_empty() {
        return crate::i18n::message(language, "downloads.history.empty")
            .expect("embedded history empty message")
            .into();
    }
    entries
        .iter()
        .rev()
        .map(|entry| {
            let status = match entry.status {
                crate::downloads::Status::Receiving => "downloads.history.receiving",
                crate::downloads::Status::Saved => "downloads.history.saved",
                crate::downloads::Status::Cancelled => "downloads.history.cancelled",
                crate::downloads::Status::Blocked => "downloads.history.blocked",
                crate::downloads::Status::Failed => "downloads.history.failed",
                crate::downloads::Status::Interrupted => "downloads.history.interrupted",
            };
            let mut line = format!(
                "{} — {}\n{}\n{}",
                crate::i18n::message(language, status).expect("embedded history status"),
                text(&entry.name),
                text(&entry.created_at),
                text(entry.source_origin.as_deref().unwrap_or_else(|| {
                    crate::i18n::message(language, "downloads.history.blocked_source")
                        .expect("embedded blocked source message")
                }))
            );
            if entry.status == crate::downloads::Status::Receiving || entry.received_bytes > 0 {
                line.push_str(
                    &crate::i18n::render(
                        language,
                        "downloads.history.received",
                        &[(
                            "bytes",
                            &entry
                                .received_bytes
                                .min(crate::downloads::MAX_BYTES + 1)
                                .to_string(),
                        )],
                    )
                    .expect("validated received count"),
                );
                if let Some(total) = entry
                    .reported_total
                    .filter(|total| *total > 0 && *total <= crate::downloads::MAX_BYTES)
                {
                    line.push_str(
                        &crate::i18n::render(
                            language,
                            "downloads.history.total",
                            &[("total", &total.to_string())],
                        )
                        .expect("validated reported total"),
                    );
                }
                if entry.status == crate::downloads::Status::Receiving {
                    line.push_str(
                        crate::i18n::message(language, "downloads.history.pending_save")
                            .expect("embedded pending-save message"),
                    );
                }
            }
            if let Some(path) = &entry.destination {
                line.push_str(
                    &crate::i18n::render(
                        language,
                        "downloads.history.destination",
                        &[("path", &text(&path.to_string_lossy()))],
                    )
                    .expect("validated destination path message"),
                );
            }
            if let Some(path) = &entry.recovery {
                line.push_str(
                    &crate::i18n::render(
                        language,
                        "downloads.history.staging",
                        &[("path", &text(&path.to_string_lossy()))],
                    )
                    .expect("validated staging path message"),
                );
            }
            if let Some(message) = &entry.message {
                line.push_str(&format!("\n{}", text(message)));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
fn refresh(label: &gtk::Label, history: &SharedHistory) {
    let value = render(history.lock().expect("downloads mutex").entries());
    if label.text().as_str() != value {
        label.set_text(&value);
    }
}
/// Aggregate byte progress across currently receiving transfers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aggregate {
    pub received: u64,
    /// Sum of server-reported totals, only when every active transfer reported a
    /// plausible one; otherwise progress is unknown and must not be invented.
    pub total: Option<u64>,
}

/// Aggregate counter. Never mixes finished rows into live progress.
pub fn aggregate(entries: &[Entry]) -> Aggregate {
    let mut received = 0u64;
    let mut total = 0u64;
    let mut active = 0usize;
    let mut complete = true;
    for entry in entries
        .iter()
        .filter(|entry| entry.status == crate::downloads::Status::Receiving)
    {
        active += 1;
        let reported = entry.received_bytes.min(crate::downloads::MAX_BYTES + 1);
        received = received.saturating_add(reported);
        match entry.reported_total.filter(|total| {
            *total > 0 && *total <= crate::downloads::MAX_BYTES && *total >= reported
        }) {
            Some(value) => total = total.saturating_add(value),
            None => complete = false,
        }
    }
    Aggregate {
        received,
        total: (active > 0 && complete && total > 0).then(|| total.max(received)),
    }
}

/// Human-readable progress; counts only, never a filename or URL.
pub fn progress_label(aggregate: Aggregate) -> String {
    progress_label_in(aggregate, crate::i18n::active_language())
}
pub fn progress_label_in(aggregate: Aggregate, language: crate::i18n::Language) -> String {
    match aggregate.total {
        Some(total) if total > 0 => {
            let percent = (aggregate.received.min(total) * 100 / total).min(100);
            crate::i18n::render(
                language,
                "downloads.history.progress_known",
                &[
                    ("received", &aggregate.received.to_string()),
                    ("total", &total.to_string()),
                    ("percent", &percent.to_string()),
                ],
            )
            .expect("validated progress counts")
        }
        _ => crate::i18n::render(
            language,
            "downloads.history.progress_unknown",
            &[("received", &aggregate.received.to_string())],
        )
        .expect("validated unknown progress count"),
    }
}

/// Fraction for a determinate bar, or `None` when the total is unknown.
pub fn progress_fraction(aggregate: Aggregate) -> Option<f64> {
    match aggregate.total {
        Some(total) if total > 0 => Some((aggregate.received.min(total) as f64) / (total as f64)),
        _ => None,
    }
}

/// Presentation is plain/escaped and bounded; identity is never a filename.
pub fn active_choices(entries: &[Entry]) -> Vec<(String, String)> {
    entries
        .iter()
        .rev()
        .filter(|entry| entry.status == crate::downloads::Status::Receiving)
        .take(4)
        .map(|entry| {
            let escaped = text(&entry.name);
            let name = escaped.chars().take(256).collect::<String>();
            (
                entry.id.clone(),
                format!(
                    "{}{} — {}",
                    name,
                    if name.len() < escaped.len() {
                        "…"
                    } else {
                        ""
                    },
                    text(&entry.created_at).chars().take(80).collect::<String>()
                ),
            )
        })
        .collect()
}
fn refresh_choices(
    combo: &gtk::ComboBoxText,
    history: &SharedHistory,
    previous: &RefCell<Vec<(String, String)>>,
    available: bool,
) {
    let next = {
        let records = history.lock().expect("downloads mutex");
        active_choices(records.entries())
    };
    combo.set_sensitive(available && !next.is_empty());
    if *previous.borrow() == next {
        return;
    }
    let selected = combo.active_id();
    combo.remove_all();
    for (id, label) in &next {
        combo.append(Some(id), label);
    }
    if let Some(id) = selected.filter(|id| next.iter().any(|(key, _)| key == id.as_str())) {
        combo.set_active_id(Some(&id));
    } else {
        combo.set_active(None);
    }
    *previous.borrow_mut() = next;
}
pub struct Window {
    pub dialog: gtk::Dialog,
    pub progress: gtk::ProgressBar,
    pub progress_text: gtk::Label,
    pub text: gtk::Label,
    pub notice: gtk::Label,
    pub cancel: gtk::Button,
    pub cancel_selected: gtk::Button,
    pub selection: gtk::ComboBoxText,
}
pub fn build(parent: Option<&gtk::Window>, history: SharedHistory) -> Window {
    build_with_cancel(parent, history, None)
}
pub fn build_with_cancel(
    parent: Option<&gtk::Window>,
    history: SharedHistory,
    cancel: Option<Cancel>,
) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(crate::i18n::text("downloads.history.title")),
        parent,
        gtk::DialogFlags::MODAL,
        &[
            (
                crate::i18n::text("downloads.history.cancel_selected"),
                gtk::ResponseType::Other(3),
            ),
            (
                crate::i18n::text("downloads.history.cancel_all"),
                gtk::ResponseType::Other(2),
            ),
            (
                crate::i18n::text("downloads.history.clear_finished"),
                gtk::ResponseType::Other(1),
            ),
            (crate::i18n::text("common.close"), gtk::ResponseType::Close),
        ],
    );
    dialog.set_default_size(720, 440);
    let cancel_button = dialog
        .widget_for_response(gtk::ResponseType::Other(2))
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    let selected_button = dialog
        .widget_for_response(gtk::ResponseType::Other(3))
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    selected_button.set_sensitive(false);
    let can_cancel = cancel.is_some();
    let selection = gtk::ComboBoxText::new();
    selection.set_hexpand(true);
    selection.set_tooltip_text(Some(crate::i18n::text("downloads.history.selection_hint")));
    let choices = Rc::new(RefCell::new(Vec::new()));
    refresh_choices(&selection, &history, &choices, can_cancel);
    let weak_selected = selected_button.downgrade();
    selection.connect_changed(move |combo| {
        if let Some(button) = weak_selected.upgrade() {
            button.set_sensitive(can_cancel && combo.active_id().is_some());
        }
    });
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_start(16);
    row.set_margin_end(16);
    row.set_margin_top(12);
    row.set_margin_bottom(8);
    row.pack_start(
        &gtk::Label::new(Some(crate::i18n::text("downloads.history.active_transfer"))),
        false,
        false,
        0,
    );
    row.pack_start(&selection, true, true, 0);
    dialog.content_area().pack_start(&row, false, false, 0);
    cancel_button
        .set_sensitive(can_cancel && history.lock().expect("downloads mutex").receiving() > 0);
    let progress = gtk::ProgressBar::new();
    progress.set_show_text(false);
    progress.set_margin_start(16);
    progress.set_margin_end(16);
    progress.set_margin_bottom(8);
    dialog.content_area().pack_start(&progress, false, false, 0);
    let progress_text = gtk::Label::new(None);
    progress_text.set_use_markup(false);
    progress_text.set_xalign(0.0);
    progress_text.set_margin_start(16);
    progress_text.set_margin_end(16);
    dialog
        .content_area()
        .pack_start(&progress_text, false, false, 0);
    let notice = gtk::Label::new(None);
    notice.set_use_markup(false);
    notice.set_line_wrap(true);
    notice.set_margin_start(16);
    notice.set_margin_end(16);
    dialog.content_area().pack_start(&notice, false, false, 0);
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    let label = gtk::Label::new(None);
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_margin_start(16);
    label.set_margin_end(16);
    label.set_margin_top(16);
    label.set_margin_bottom(16);
    refresh(&label, &history);
    scroll.add(&label);
    dialog.content_area().pack_start(&scroll, true, true, 0);
    let records = history.clone();
    let label_for_clear = label.clone();
    let notice_for_clear = notice.clone();
    let selection_for_response = selection.clone();
    let choices_for_response = choices.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Other(1) {
            let result = {
                let mut history = records.lock().expect("downloads mutex");
                history.clear_finished();
                history.persist()
            };
            notice_for_clear.set_text(crate::i18n::text(if result.is_ok() {
                "downloads.history.clear_ok"
            } else {
                "downloads.history.clear_failed"
            }));
            refresh(&label_for_clear, &records);
            refresh_choices(
                &selection_for_response,
                &records,
                &choices_for_response,
                can_cancel,
            );
        } else if response == gtk::ResponseType::Other(2) || response == gtk::ResponseType::Other(3)
        {
            let selected = selection_for_response.active_id();
            let ids = {
                let history = records.lock().expect("downloads mutex");
                history
                    .entries()
                    .iter()
                    .filter(|entry| {
                        entry.status == crate::downloads::Status::Receiving
                            && (response == gtk::ResponseType::Other(2)
                                || selected.as_ref().is_some_and(|id| id.as_str() == entry.id))
                    })
                    .take(4)
                    .map(|entry| entry.id.clone())
                    .collect::<Vec<_>>()
            };
            refresh_choices(
                &selection_for_response,
                &records,
                &choices_for_response,
                can_cancel,
            );
            notice_for_clear.set_text(crate::i18n::text(if ids.is_empty() {
                "downloads.history.cancel_none"
            } else if cancel.as_ref().is_some_and(|request| request(ids)) {
                "downloads.history.cancel_requested"
            } else {
                "downloads.history.cancel_unavailable"
            }));
        } else {
            dialog.close();
        }
    });
    let weak_cancel = cancel_button.downgrade();
    let weak_dialog = dialog.downgrade();
    let weak_label = label.downgrade();
    let weak_progress = progress.downgrade();
    let weak_progress_text = progress_text.downgrade();
    let weak_selection = selection.downgrade();
    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        let (Some(dialog), Some(label)) = (weak_dialog.upgrade(), weak_label.upgrade()) else {
            return glib::ControlFlow::Break;
        };
        if !dialog.is_visible() {
            return glib::ControlFlow::Break;
        }
        refresh(&label, &history);
        let aggregate = {
            let records = history.lock().expect("downloads mutex");
            aggregate(records.entries())
        };
        if let (Some(bar), Some(text)) = (weak_progress.upgrade(), weak_progress_text.upgrade()) {
            match progress_fraction(aggregate) {
                Some(fraction) => {
                    bar.set_show_text(false);
                    bar.set_fraction(fraction);
                    bar.show();
                }
                None if aggregate.received > 0 || aggregate.total.is_some() => {
                    bar.pulse();
                    bar.show();
                }
                None => bar.hide(),
            }
            text.set_text(&progress_label(aggregate));
            text.set_visible(aggregate.received > 0 || aggregate.total.is_some());
        }
        if let Some(combo) = weak_selection.upgrade() {
            refresh_choices(&combo, &history, &choices, can_cancel);
        }
        if let Some(button) = weak_cancel.upgrade() {
            button.set_sensitive(
                can_cancel && history.lock().expect("downloads mutex").receiving() > 0,
            );
        }
        glib::ControlFlow::Continue
    });
    Window {
        dialog,
        progress,
        progress_text,
        text: label,
        notice,
        cancel: cancel_button,
        cancel_selected: selected_button,
        selection,
    }
}
