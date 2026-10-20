//! Real GTK/WebKit signals with an ephemeral context and injected chooser.
//! CI can map a fixture window on its virtual display; local default stays
//! hidden. No real DSH server, user profile, or interactive save action.
#![cfg(target_os = "linux")]
use std::cell::Cell;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dsh_ocean::{
    download_engine,
    downloads::{History, Status},
};
use gtk::{glib, prelude::*};
use webkit2gtk::{DownloadExt, WebContext, WebContextExt, WebView, WebViewExt};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-webkit-download-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wait_for(history: &download_engine::SharedHistory, count: usize, status: Status) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        // Some WebKit idle sources stay ready continuously; never drain an
        // unbounded queue before checking the test deadline.
        for _ in 0..32 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        let history = history.lock().unwrap();
        if history.entries().len() == count
            && history.entries().last().is_some_and(|e| e.status == status)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native download did not reach {status:?}; actual states: {:?}",
            history
                .entries()
                .iter()
                .map(|e| e.status)
                .collect::<Vec<_>>()
        );
        drop(history);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires a GTK display; CI runs explicitly under Xvfb"]
fn cancelled_and_failed_downloads_do_not_poison_later_real_downloads() {
    eprintln!("native-test: initialize GTK");
    gtk::init().expect("GTK display required (use xvfb-run on CI)");
    eprintln!("native-test: GTK initialized");
    let root = std::env::temp_dir().join(format!(
        "dsh-ocean-webkit-download-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let f = Fixture(root);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = stop.clone();
    let server = std::thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|s| s == b"\r\n\r\n") {
                let Ok(n) = stream.read(&mut buf) else {
                    break;
                };
                if n == 0 || request.len() > 16384 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            if request.starts_with(b"GET /session ") {
                let body = "<html><body>fixture session</body></html>";
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: ocean_fixture=synthetic-cookie-never-store; HttpOnly; Path=/; SameSite=Lax\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            } else if request.starts_with(b"GET /blob-page ") {
                let body = "<!doctype html><html><body><script>const a=document.createElement('a');a.href=URL.createObjectURL(new Blob(['blob fixture\\n'],{type:'text/plain'}));a.download='blob.txt';document.body.appendChild(a);a.click();</script></body></html>";
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            } else if !String::from_utf8_lossy(&request)
                .to_ascii_lowercase()
                .contains("cookie: ocean_fixture=synthetic-cookie-never-store")
            {
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            } else if request.starts_with(b"GET /slow ") {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=slow.bin\r\nContent-Length: 589824\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(&[b'x'; 65536]);
                for _ in 0..128 {
                    if stream.write_all(&[b'x'; 4096]).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            } else {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=report.txt\r\nContent-Length: 14\r\nConnection: close\r\n\r\nfixture bytes\n");
            }
        }
    });
    eprintln!("native-test: create ephemeral context");
    let context = WebContext::new_ephemeral();
    let view = WebView::with_context(&context);
    let fixture_window = if std::env::var_os("DSH_OCEAN_TEST_MAP_WEBVIEW").is_some() {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Ocean download test fixture");
        window.set_default_size(640, 480);
        window.add(&view);
        window.show_all();
        Some(window)
    } else {
        None
    };
    context.connect_download_started(|_, download| {
        eprintln!("native-test: download started");
        download.connect_decide_destination(|_, _| {
            eprintln!("native-test: destination signal");
            false
        });
        download.connect_finished(|_| eprintln!("native-test: finished signal"));
    });
    let history_path = f.0.join("history.json");
    let history = Arc::new(Mutex::new(History::open(&history_path).unwrap()));
    let chooser_count = Rc::new(Cell::new(0));
    let calls = chooser_count.clone();
    let target_root = f.0.clone();
    fs::write(f.0.join("existing.txt"), "do not overwrite").unwrap();
    let chooser = Rc::new(move |_name: &str, source: &str| {
        assert!(!source.contains("synthetic-secret"));
        calls.set(calls.get() + 1);
        match calls.get() {
            1 => None,
            2 => Some(target_root.join("saved.txt")),
            3 => Some(target_root.join("existing.txt")),
            4 => Some(target_root.join("after failure.txt")),
            5 => Some(target_root.join("blob export.txt")),
            6 => Some(target_root.join("user cancelled.bin")),
            _ => Some(target_root.join("after active cancel.txt")),
        }
    });
    let origin = base.clone();
    let terminal_statuses = Arc::new(Mutex::new(Vec::new()));
    let observed_terminal = terminal_statuses.clone();
    let cancellation = download_engine::attach(
        &view,
        Rc::new(move || Some(origin.clone())),
        chooser,
        history.clone(),
        Arc::new(move |entry| {
            observed_terminal
                .lock()
                .unwrap()
                .push((entry.id, entry.status));
        }),
    );
    let loaded = Rc::new(Cell::new(false));
    let loaded_for_signal = loaded.clone();
    view.connect_load_changed(move |_, event| {
        if event == webkit2gtk::LoadEvent::Finished {
            loaded_for_signal.set(true);
        }
    });
    view.load_uri(base.join("session").unwrap().as_str());
    let deadline = Instant::now() + Duration::from_secs(20);
    while !loaded.get() {
        for _ in 0..32 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        assert!(
            Instant::now() < deadline,
            "cookie fixture page did not load"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let uri = base.join("report?token=synthetic-secret").unwrap();
    let mut handles = Vec::new();
    for (index, status) in [
        Status::Cancelled,
        Status::Saved,
        Status::Failed,
        Status::Saved,
    ]
    .into_iter()
    .enumerate()
    {
        eprintln!("native-test: request {} expecting {status:?}", index + 1);
        handles.push(view.download_uri(uri.as_str()).expect("native download"));
        wait_for(&history, index + 1, status);
    }
    eprintln!("native-test: export a blob from a real same-origin document");
    view.load_uri(base.join("blob-page").unwrap().as_str());
    wait_for(&history, 5, Status::Saved);
    assert_eq!(
        fs::read_to_string(f.0.join("blob export.txt")).unwrap(),
        "blob fixture\n"
    );
    eprintln!("native-test: cancel a real authenticated midstream transfer by exact owned ID");
    let received = Rc::new(Cell::new(false));
    let observed = received.clone();
    let slow = view
        .download_uri(base.join("slow").unwrap().as_str())
        .unwrap();
    slow.connect_received_data(move |_, size| {
        if size > 0 {
            observed.set(true);
        }
    });
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        for _ in 0..32 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        let ready = history
            .lock()
            .unwrap()
            .entries()
            .last()
            .is_some_and(|entry| entry.status == Status::Receiving && entry.destination.is_some());
        if received.get() && ready {
            break;
        }
        assert!(Instant::now() < end, "slow transfer did not receive bytes");
        std::thread::sleep(Duration::from_millis(5));
    }
    {
        let records = history.lock().unwrap();
        let entry = records.entries().last().unwrap();
        assert!(entry.received_bytes > 0 && entry.received_bytes < 589824);
        assert_eq!(entry.reported_total, Some(589824));
    }
    let cancelled_id = history.lock().unwrap().entries().last().unwrap().id.clone();
    assert!(cancellation.request(vec![cancelled_id.clone()]));
    assert!(!cancellation.request(vec![cancelled_id.clone()]));
    wait_for(&history, 6, Status::Cancelled);
    assert!(!f.0.join("user cancelled.bin").exists());
    assert!(!fs::read_dir(&f.0).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".dsh-ocean-download-")));
    // A queued stale ID cannot cancel an operation started afterwards.
    assert!(cancellation.request(vec![cancelled_id]));
    handles.push(view.download_uri(uri.as_str()).unwrap());
    wait_for(&history, 7, Status::Saved);
    assert_eq!(
        fs::read_to_string(f.0.join("after active cancel.txt")).unwrap(),
        "fixture bytes\n"
    );
    let end = Instant::now() + Duration::from_secs(5);
    while terminal_statuses.lock().unwrap().len() < 7 {
        for _ in 0..32 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    // Publication callbacks are idle-dispatched and may reorder relative to a
    // later synchronous cancellation; correlate exact IDs, not event order.
    {
        let events = terminal_statuses.lock().unwrap();
        let records = history.lock().unwrap();
        for entry in records.entries() {
            let states = events
                .iter()
                .filter(|(id, _)| id == &entry.id)
                .map(|(_, status)| *status)
                .collect::<Vec<_>>();
            assert_eq!(states, vec![entry.status]);
        }
    }
    let saved_id = history.lock().unwrap().entries().last().unwrap().id.clone();
    assert!(cancellation.request(vec![saved_id]));
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
    assert_eq!(terminal_statuses.lock().unwrap().len(), 7);
    assert_eq!(
        history.lock().unwrap().entries().last().unwrap().status,
        Status::Saved
    );
    assert_eq!(chooser_count.get(), 7);
    assert_eq!(
        fs::read_to_string(f.0.join("saved.txt")).unwrap(),
        "fixture bytes\n"
    );
    assert_eq!(
        fs::read_to_string(f.0.join("after failure.txt")).unwrap(),
        "fixture bytes\n"
    );
    assert_eq!(
        fs::read_to_string(f.0.join("existing.txt")).unwrap(),
        "do not overwrite"
    );
    {
        let records = history.lock().unwrap();
        let entry = records.entries().last().unwrap();
        assert_eq!(entry.status, Status::Saved);
        assert_eq!(entry.received_bytes, 14);
        assert_eq!(entry.reported_total, Some(14));
    }
    let raw = fs::read_to_string(history_path).unwrap();
    assert!(!raw.contains("receivedBytes") && !raw.contains("reportedTotal"));
    assert!(!raw.contains("synthetic-secret") && !raw.contains("synthetic-cookie-never-store"));
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    drop(handles);
    if let Some(window) = fixture_window {
        window.close();
    }
    drop(view);
    drop(context);
}
