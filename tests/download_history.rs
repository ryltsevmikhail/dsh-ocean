use dsh_ocean::{
    download_history,
    downloads::{Entry, History, Status},
};
use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-history-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-history-test-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn entry(name: &str, status: Status) -> Entry {
    Entry::new(
        Some(
            "http://user:synthetic-private@127.0.0.1:49001/path?token=synthetic-private-query"
                .into(),
        ),
        name.into(),
        status,
    )
}
#[test]
fn renders_newest_first_with_dates_and_without_source_url_secrets() {
    let older = entry("older", Status::Saved);
    let newer = entry("newer", Status::Receiving);
    let text = download_history::render(&[older.clone(), newer.clone()]);
    assert!(text.find("newer").unwrap() < text.find("older").unwrap());
    assert!(text.contains(&newer.created_at) && text.contains(&older.created_at));
    assert!(text.contains("http://127.0.0.1:49001"));
    assert!(!text.contains("synthetic-private") && !text.contains("/path"));
    assert!(download_history::render(&[]).contains("No downloads recorded"));
}
#[test]
fn renders_control_characters_safely_and_does_not_interpret_markup() {
    let mut row = entry("<b>name</b>\0\n", Status::Failed);
    row.message = Some("error\0\u{1b}".into());
    let text = download_history::render(&[row]);
    assert!(text.contains("<b>name</b>"));
    assert!(!text.contains('\0') && !text.contains('\u{1b}'));
    assert!(text.contains("\\n"));
}
#[test]
fn active_selection_uses_ids_not_duplicate_names_and_omits_terminal_rows() {
    let first = entry("same-name", Status::Receiving);
    let second = entry("same-name", Status::Receiving);
    let saved = entry("saved", Status::Saved);
    let choices = download_history::active_choices(&[first.clone(), saved, second.clone()]);
    assert_eq!(
        choices.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        vec![second.id, first.id]
    );
    assert!(choices
        .iter()
        .all(|(_, label)| !label.contains("synthetic-private")));
}
#[test]
fn active_selection_labels_are_escaped_bounded_and_limited_to_four() {
    let mut entries = (0..8)
        .map(|_| entry("x", Status::Receiving))
        .collect::<Vec<_>>();
    entries.last_mut().unwrap().name = format!("<b>name</b>\n\u{202e}{}", "x".repeat(4000));
    entries.last_mut().unwrap().created_at = "t".repeat(1000);
    let choices = download_history::active_choices(&entries);
    assert_eq!(choices.len(), 4);
    assert_eq!(choices[0].0, entries[7].id);
    assert!(choices[0].1.contains("<b>name</b>\\n"));
    assert!(!choices[0].1.contains('\n') && !choices[0].1.contains('\u{202e}'));
    assert!(choices[0].1.contains("\\u{202e}"));
    assert!(choices[0].1.chars().count() < 350);
}
#[test]
fn aggregate_progress_never_invents_totals_and_stays_bounded() {
    let mut first = entry("first", Status::Receiving);
    first.received_bytes = 1_024;
    first.reported_total = Some(4_096);
    let mut second = entry("second", Status::Receiving);
    second.received_bytes = 512;
    second.reported_total = Some(1_024);
    let finished = entry("finished", Status::Saved);
    let aggregate = download_history::aggregate(&[first.clone(), second.clone(), finished]);
    assert_eq!(aggregate.received, 1_536);
    assert_eq!(aggregate.total, Some(5_120));
    assert_eq!(download_history::progress_fraction(aggregate), Some(0.3));
    let label = download_history::progress_label(aggregate);
    assert!(label.contains("1536 of 5120 bytes"), "{label}");
    assert!(label.contains("30%"), "{label}");

    // One missing or implausible total means the aggregate total is unknown.
    let mut unknown = second.clone();
    unknown.reported_total = None;
    let aggregate = download_history::aggregate(&[first.clone(), unknown]);
    assert_eq!(aggregate.received, 1_536);
    assert_eq!(aggregate.total, None);
    assert_eq!(download_history::progress_fraction(aggregate), None);
    assert!(download_history::progress_label(aggregate).contains("total unknown"));

    let mut liar = first.clone();
    liar.reported_total = Some(10);
    let aggregate = download_history::aggregate(&[liar]);
    assert_eq!(aggregate.total, None, "total below received is implausible");

    // Received bytes are clamped and finished rows never contribute.
    let mut huge = first.clone();
    huge.received_bytes = u64::MAX;
    let aggregate = download_history::aggregate(&[huge]);
    assert_eq!(aggregate.received, dsh_ocean::downloads::MAX_BYTES + 1);
    assert!(
        download_history::progress_fraction(download_history::aggregate(&[entry(
            "done",
            Status::Saved
        )]))
        .is_none()
    );
}

fn tick() {
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
}
fn wait(test: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !test() {
        tick();
        assert!(Instant::now() < end, "history window did not refresh");
        std::thread::sleep(Duration::from_millis(2));
    }
    tick();
}
fn click(dialog: &gtk::Dialog, response: gtk::ResponseType) {
    dialog
        .widget_for_response(response)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    tick();
}
#[test]
#[ignore = "requires explicitly enabled isolated GTK dialog display"]
fn live_status_clear_and_persistence_failure_keep_files_and_window_safe() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let f = Fixture::new();
    let path = f.0.join("history.json");
    let saved = f.0.join("saved.txt");
    let recovery = f.0.join("recovery.txt");
    fs::write(&saved, "saved bytes").unwrap();
    fs::write(&recovery, "recovery bytes").unwrap();
    let mut receiving = entry("active-row", Status::Receiving);
    receiving.destination = Some(saved.clone());
    receiving.reported_total = Some(8192);
    let mut finished = entry("finished-row", Status::Failed);
    finished.recovery = Some(recovery.clone());
    let mut other = entry("other-row", Status::Receiving);
    let records = Arc::new(Mutex::new(History::open(&path).unwrap()));
    {
        let mut history = records.lock().unwrap();
        history.put(receiving.clone());
        history.put(finished);
        history.put(other.clone());
        history.persist().unwrap();
    }
    let requested = Rc::new(RefCell::new(Vec::<Vec<String>>::new()));
    let observed = requested.clone();
    let cancel = Rc::new(move |ids| {
        observed.borrow_mut().push(ids);
        true
    });
    let window = download_history::build_with_cancel(None, records.clone(), Some(cancel));
    window.dialog.show_all();
    assert!(window.cancel.is_sensitive());
    assert!(window.selection.active_id().is_none());
    assert!(!window.cancel_selected.is_sensitive());
    assert!(window.selection.set_active_id(Some(&receiving.id)));
    other.name = "renamed-other".into();
    records.lock().unwrap().put(other.clone());
    wait(|| window.text.text().contains("renamed-other"));
    assert_eq!(
        window.selection.active_id().as_deref(),
        Some(receiving.id.as_str())
    );
    assert!(window.selection.set_active_id(Some(&other.id)));
    assert!(window.cancel_selected.is_sensitive());
    click(&window.dialog, gtk::ResponseType::Other(3));
    assert_eq!(requested.borrow().as_slice(), &[vec![other.id.clone()]]);
    assert_eq!(
        records
            .lock()
            .unwrap()
            .entries()
            .iter()
            .find(|entry| entry.id == receiving.id)
            .unwrap()
            .status,
        Status::Receiving
    );
    requested.borrow_mut().clear();
    click(&window.dialog, gtk::ResponseType::Other(2));
    assert_eq!(
        requested.borrow().as_slice(),
        &[vec![receiving.id.clone(), other.id.clone()]]
    );
    assert!(window.notice.text().contains("Cancellation requested"));
    requested.borrow_mut().clear();
    other.status = Status::Cancelled;
    records.lock().unwrap().put(other);
    // The widget still holds the old selected ID; response handling must
    // recheck current state rather than sending it before the next timer tick.
    click(&window.dialog, gtk::ResponseType::Other(3));
    assert!(requested.borrow().is_empty());
    assert!(window.notice.text().contains("No selected/current"));
    assert!(window.selection.active_id().is_none());
    assert!(!window.cancel_selected.is_sensitive());
    assert!(window.dialog.is_visible());
    assert_eq!(fs::read_to_string(&saved).unwrap(), "saved bytes");
    assert!(window.text.is_selectable() && !window.text.uses_markup());
    assert!(window.text.text().contains("Receiving — active-row"));
    let before = fs::read(&path).unwrap();
    assert!(records.lock().unwrap().update_progress(&receiving.id, 4096));
    wait(|| window.text.text().contains("Received: 4096 bytes"));
    assert!(window
        .text
        .text()
        .contains("server reports 8192 bytes total"));
    assert!(window.text.text().contains("not saved until Saved"));
    assert_eq!(fs::read(&path).unwrap(), before);
    click(&window.dialog, gtk::ResponseType::Other(1));
    assert!(window.dialog.is_visible());
    assert_eq!(records.lock().unwrap().entries().len(), 1);
    assert!(!window.text.text().contains("finished-row"));
    assert!(window.notice.text().contains("not removed"));
    receiving.status = Status::Saved;
    receiving.received_bytes = 4096;
    {
        let mut history = records.lock().unwrap();
        history.put(receiving);
        history.persist().unwrap();
    }
    wait(|| window.text.text().contains("Saved — active-row"));
    wait(|| !window.cancel.is_sensitive());
    assert!(!window.cancel_selected.is_sensitive() && !window.selection.is_sensitive());
    click(&window.dialog, gtk::ResponseType::Other(1));
    assert!(window.dialog.is_visible());
    assert!(window.text.text().contains("No downloads recorded"));
    assert!(History::open(&path).unwrap().entries().is_empty());
    assert_eq!(fs::read_to_string(&saved).unwrap(), "saved bytes");
    assert_eq!(fs::read_to_string(&recovery).unwrap(), "recovery bytes");
    click(&window.dialog, gtk::ResponseType::Close);
    wait(|| !window.dialog.is_visible());

    let blocked_path = f.0.join("blocked/history.json");
    let history = History::open(&blocked_path).unwrap();
    fs::create_dir_all(&blocked_path).unwrap();
    let records = Arc::new(Mutex::new(history));
    records
        .lock()
        .unwrap()
        .put(entry("cannot-persist", Status::Saved));
    let window = download_history::build(None, records);
    window.dialog.show_all();
    assert!(
        !window.cancel.is_sensitive()
            && !window.cancel_selected.is_sensitive()
            && !window.selection.is_sensitive()
    );
    click(&window.dialog, gtk::ResponseType::Other(1));
    assert!(window.notice.text().contains("memory only"));
    assert!(window.notice.text().contains("restart"));
    assert!(window.dialog.is_visible());
    assert!(blocked_path.is_dir());
    click(&window.dialog, gtk::ResponseType::Close);
    wait(|| !window.dialog.is_visible());
    // Let refresh sources observe the closed dialogs and release weak owners.
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        tick();
        std::thread::sleep(Duration::from_millis(2));
    }
}
