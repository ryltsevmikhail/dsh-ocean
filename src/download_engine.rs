//! Direct WebKit signals keep failure state per download (not per context).
//! No network refetch: authenticated responses/blobs stay in WebKit's session.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::{glib, prelude::*};
use url::Url;
use webkit2gtk::WebView;
use webkit2gtk::{DownloadExt, URIRequestExt, URIResponseExt, WebContextExt, WebViewExt};

use crate::downloads::{self, Entry, History, Status, Transfer, MAX_BYTES};
pub type SharedHistory = Arc<Mutex<History>>;
pub type Chooser = Rc<dyn Fn(&str, &str) -> Option<PathBuf>>;
pub type ServerOrigin = Rc<dyn Fn() -> Option<Url>>;
pub type Terminal = Arc<dyn Fn(Entry) + Send + Sync>;

/// Native-only, bounded/coalesced requests for exact session-owned IDs. No URL,
/// destination, remote IPC, refetch or future-download cancellation authority.
#[derive(Clone, Default)]
pub struct Cancellation {
    sender: Option<glib::Sender<Vec<String>>>,
    pending: Arc<std::sync::atomic::AtomicBool>,
}
impl Cancellation {
    pub fn request(&self, ids: Vec<String>) -> bool {
        use std::sync::atomic::Ordering;
        if ids.is_empty() || ids.len() > 4 || ids.iter().any(|id| id.len() > 128) {
            return false;
        }
        let Some(sender) = &self.sender else {
            return false;
        };
        if self.pending.swap(true, Ordering::AcqRel) {
            return false;
        }
        if sender.send(ids).is_err() {
            self.pending.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

#[cfg(test)]
#[test]
fn cancellation_requests_are_bounded_coalesced_and_disconnected_safe() {
    assert!(!Cancellation::default().request(vec!["id".into()]));
    #[allow(deprecated)]
    let (sender, receiver) = glib::MainContext::channel::<Vec<String>>(glib::Priority::DEFAULT);
    let control = Cancellation {
        sender: Some(sender),
        pending: Arc::default(),
    };
    assert!(!control.request(vec![]));
    assert!(!control.request(vec!["x".repeat(129)]));
    assert!(!control.request(vec!["id".into(); 5]));
    assert!(control.request(vec!["owned-id".into()]));
    assert!(
        !control.request(vec!["owned-id".into()]),
        "at most one queued request"
    );
    control
        .pending
        .store(false, std::sync::atomic::Ordering::Release);
    drop(receiver);
    assert!(!control.request(vec!["owned-id".into()]));
    assert!(!control.pending.load(std::sync::atomic::Ordering::Acquire));
}
fn store(history: &SharedHistory, entry: Entry) {
    // This runs in WebKit signal handlers on the GTK main thread. Keep the lock only
    // for the in-memory change and do the fsync outside it, so slow storage cannot
    // freeze the UI or block the history window's timer.
    let (snapshot, persist) = {
        let mut guard = history.lock().expect("downloads mutex");
        guard.put(entry);
        (guard.entries_snapshot(), guard.has_path())
    };
    if persist {
        if let Err(error) = persist_entries(history, snapshot) {
            eprintln!("dsh-ocean: download history could not be persisted: {error:#}");
        }
    }
}

fn persist_entries(
    history: &SharedHistory,
    entries: Vec<crate::downloads::Entry>,
) -> anyhow::Result<()> {
    // Read the path under a short lock, then write with no lock held.
    let path = {
        let guard = history.lock().expect("downloads mutex");
        guard.path_snapshot()
    };
    match path {
        Some(path) => crate::downloads::History::persist_entries_at(&path, entries),
        None => Ok(()),
    }
}
struct Context {
    entry: RefCell<Entry>,
    transfer: RefCell<Option<Transfer>>,
    failed: Cell<bool>,
    bytes: Cell<u64>,
    reason: RefCell<Option<String>>,
    last_progress: Cell<Option<Instant>>,
    finished: Cell<bool>,
    cancel_called: Cell<bool>,
}
fn begin_cancel(called: &Cell<bool>, finished: bool) -> bool {
    !finished && !called.replace(true)
}
fn cancel_native(download: &webkit2gtk::Download, state: &Context) {
    // WebKit may invalidate its native cancellation state on terminal delivery.
    // Nested GTK loops can deliver that while decide-destination is on stack.
    // Mark before entering C so synchronous failed/finished callbacks cannot
    // re-enter cancel, and never call it again when the chooser returns.
    if begin_cancel(&state.cancel_called, state.finished.get()) {
        download.cancel();
    }
}
#[cfg(test)]
#[test]
fn native_cancel_gate_is_one_shot_before_reentrancy_and_closed_after_terminal() {
    let called = Cell::new(false);
    assert!(begin_cancel(&called, false));
    assert!(called.get());
    assert!(!begin_cancel(&called, false));
    assert!(!begin_cancel(&called, true));
    let terminal = Cell::new(false);
    assert!(!begin_cancel(&terminal, true));
    assert!(!terminal.get());
}
fn progress_due(last: &Cell<Option<Instant>>, now: Instant) -> bool {
    if last.get().is_some_and(|previous| {
        now.saturating_duration_since(previous) < Duration::from_millis(250)
    }) {
        return false;
    }
    last.set(Some(now));
    true
}
#[cfg(test)]
#[test]
fn progress_is_throttled_without_timers_or_disk_writes() {
    let last = Cell::new(None);
    let start = Instant::now();
    assert!(progress_due(&last, start));
    assert!(!progress_due(&last, start + Duration::from_millis(249)));
    assert!(progress_due(&last, start + Duration::from_millis(250)));
    assert!(!progress_due(&last, start));
}
fn origin(download: &webkit2gtk::Download, server: &ServerOrigin) -> Option<String> {
    let base = server()?;
    let request = download.request()?.uri()?;
    let url = Url::parse(&request).ok()?;
    let origin = downloads::source_origin(&url, &base).ok()?;
    if let Some(response) = download.response().and_then(|r| r.uri()) {
        downloads::source_origin(&Url::parse(&response).ok()?, &base).ok()?;
    }
    Some(origin)
}

pub fn attach(
    view: &WebView,
    server: ServerOrigin,
    chooser: Chooser,
    history: SharedHistory,
    terminal: Terminal,
) -> Cancellation {
    let Some(web_context) = view.context() else {
        return Cancellation::default();
    };
    let weak = view.downgrade();
    let prompting = Rc::new(Cell::new(false));
    let active = Rc::new(RefCell::new(HashMap::<
        String,
        (webkit2gtk::Download, Rc<Context>),
    >::new()));
    // GTK3/WebKit use glib 0.18; its main-context channel keeps native objects
    // exclusively on the GTK thread. The atomic gate bounds the queue to one.
    #[allow(deprecated)]
    let (sender, receiver) = glib::MainContext::channel::<Vec<String>>(glib::Priority::DEFAULT);
    let cancellation = Cancellation {
        sender: Some(sender),
        pending: Arc::default(),
    };
    let pending = cancellation.pending.clone();
    let owned = Rc::downgrade(&active);
    let alive = view.downgrade();
    receiver.attach(None, move |ids| {
        pending.store(false, std::sync::atomic::Ordering::Release);
        let (Some(owned), Some(_view)) = (owned.upgrade(), alive.upgrade()) else {
            return glib::ControlFlow::Break;
        };
        // Drop the map borrow before cancel(), whose terminal signals may run
        // synchronously and remove entries. Never affect later/unknown IDs.
        let downloads = {
            let active = owned.borrow();
            ids.iter()
                .filter_map(|id| active.get(id).cloned())
                .collect::<Vec<_>>()
        };
        for (download, state) in downloads {
            if !state.finished.get() && state.entry.borrow().status == Status::Receiving {
                {
                    let mut entry = state.entry.borrow_mut();
                    entry.status = Status::Cancelled;
                    entry.message = Some("cancelled by user; destination left untouched".into());
                }
                cancel_native(&download, &state);
            }
        }
        glib::ControlFlow::Continue
    });
    web_context.connect_download_started(move |_, download| {
        let Some(expected) = weak.upgrade() else { download.cancel(); return; };
        if download.web_view().as_ref() != Some(&expected) { download.cancel(); return; }
        let source = origin(download, &server);
        let blocked = source.is_none() || history.lock().expect("downloads mutex").receiving() >= 4 || prompting.get();
        let mut entry = Entry::new(source, "download".into(), if blocked { Status::Blocked } else { Status::Receiving });
        if blocked { entry.message = Some("download blocked: origin, active limit or save dialog already open".into()); }
        store(&history, entry.clone());
        // A JS/blob request has no native caller holding its Download object.
        // Own accepted operations until the terminal signal, bounded to four.
        let active_for_finish = Rc::downgrade(&active);
        let state = Rc::new(Context { entry: RefCell::new(entry), transfer: RefCell::new(None), failed: Cell::new(false), bytes: Cell::new(0), reason: RefCell::new(None), last_progress: Cell::new(None), finished: Cell::new(false), cancel_called: Cell::new(false) });
        if !blocked { active.borrow_mut().insert(state.entry.borrow().id.clone(), (download.clone(), state.clone())); }
        let state_for_decide = state.clone();
        let chooser = chooser.clone(); let history_for_decide = history.clone(); let prompting = prompting.clone(); let server = server.clone();
        download.connect_decide_destination(move |download, suggested| {
            let _keep_alive = download.clone();
            let state = &state_for_decide;
            let receiving = !state.finished.get() && state.entry.borrow().status == Status::Receiving;
            if !receiving { cancel_native(download,state); return true; }
            if origin(download, &server).is_none() {
                state.entry.borrow_mut().status = Status::Blocked; cancel_native(download,state); return true;
            }
            if download.response().is_some_and(|r| r.status_code() >= 400) {
                *state.reason.borrow_mut() = Some("server refused the download".into()); state.failed.set(true); cancel_native(download,state); return true;
            }
            if download.response().is_some_and(|r| r.content_length() > MAX_BYTES) {
                *state.reason.borrow_mut() = Some("download exceeds the 1 GiB limit".into()); state.failed.set(true); cancel_native(download,state); return true;
            }
            state.entry.borrow_mut().reported_total = download.response().map(|response| response.content_length()).filter(|size| *size > 0 && *size <= MAX_BYTES);
            let name = downloads::suggested_name(suggested);
            let source = state.entry.borrow().source_origin.clone().unwrap_or_default();
            state.entry.borrow_mut().name = name.clone();
            prompting.set(true);
            let selected = chooser(&name, &source);
            prompting.set(false);
            // The network can fail while GTK runs a nested save-dialog loop.
            if state.finished.get() || state.failed.get() { return true; }
            if state.entry.borrow().status != Status::Receiving { cancel_native(download,state); return true; }
            let Some(selected) = selected else { state.entry.borrow_mut().status = Status::Cancelled; cancel_native(download,state); return true; };
            match Transfer::prepare(&selected) {
                Ok(transfer) => {
                    let Ok(uri) = Url::from_file_path(&transfer.payload) else { state.failed.set(true); cancel_native(download,state); return true; };
                    if let Some(name) = transfer.destination.file_name().and_then(|s| s.to_str()) { state.entry.borrow_mut().name = name.to_owned(); }
                    state.entry.borrow_mut().destination = Some(transfer.destination.clone());
                    state.entry.borrow_mut().recovery = Some(transfer.payload.clone());
                    store(&history_for_decide, state.entry.borrow().clone());
                    *state.transfer.borrow_mut() = Some(transfer);
                    download.set_allow_overwrite(false);
                    download.set_destination(uri.as_str());
                }
                Err(_) => {
                    *state.reason.borrow_mut() = Some("cannot stage download: choose a new filename in a writable directory; existing files are never overwritten".into());
                    state.failed.set(true); cancel_native(download,state);
                }
            }
            true
        });
        let state_for_data = state.clone(); let history_for_progress = history.clone();
        download.connect_received_data(move |download, size| {
            if state_for_data.finished.get() || state_for_data.entry.borrow().status != Status::Receiving { return; }
            let total = state_for_data.bytes.get().saturating_add(size); state_for_data.bytes.set(total);
            state_for_data.entry.borrow_mut().received_bytes = total.min(MAX_BYTES + 1);
            if progress_due(&state_for_data.last_progress, Instant::now()) {
                // Volatile metadata only: no per-chunk persist/notification.
                history_for_progress.lock().expect("downloads mutex").update_progress(&state_for_data.entry.borrow().id, total);
            }
            let payload_too_large = state_for_data.transfer.borrow().as_ref()
                .and_then(|transfer| std::fs::symlink_metadata(&transfer.payload).ok())
                .is_some_and(|metadata| metadata.len() > MAX_BYTES);
            if total > MAX_BYTES || payload_too_large {
                *state_for_data.reason.borrow_mut() = Some("download exceeds the 1 GiB limit".into());
                state_for_data.failed.set(true); cancel_native(download,&state_for_data);
            }
        });
        let state_for_error = state.clone();
        download.connect_failed(move |_, _| { state_for_error.failed.set(true); }); // Never log an error containing the raw URI.
        let history = history.clone(); let terminal = terminal.clone();
        download.connect_finished(move |download| {
            let _keep_alive = download.clone();
            if state.finished.replace(true) { return; }
            if let Some(active) = active_for_finish.upgrade() { active.borrow_mut().remove(&state.entry.borrow().id); }
            let mut entry = state.entry.borrow().clone();
            let transfer = state.transfer.borrow_mut().take();
            if entry.status == Status::Cancelled || entry.status == Status::Blocked {
                entry.recovery = None; drop(transfer); store(&history, entry.clone()); terminal(entry); return;
            }
            if state.failed.get() || transfer.is_none() {
                entry.status = Status::Failed;
                entry.message = Some(state.reason.borrow().clone().unwrap_or_else(|| "network download failed; destination left untouched".into()));
                entry.recovery = None; drop(transfer); store(&history, entry.clone()); terminal(entry); return;
            }
            let mut transfer = transfer.unwrap();
            let history = history.clone(); let terminal = terminal.clone();
            std::thread::spawn(move || {
                match transfer.publish() {
                    Ok(path) => { entry.status = Status::Saved; entry.destination = Some(path); entry.recovery = None; }
                    Err(_) => { entry.status = Status::Failed; entry.recovery = transfer.recovery_path(); entry.message = Some(if entry.recovery.is_some() { "publication failed; destination was not overwritten; private staging copy retained, verify before reuse" } else { "publication failed; destination was not overwritten; no recovery copy is available" }.into()); }
                }
                drop(transfer); store(&history, entry.clone());
                glib::idle_add_once(move || terminal(entry));
            });
        });
    });
    cancellation
}
