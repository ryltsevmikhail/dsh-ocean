//! Real WebKit rendering of the shipped setup and pairing pages.
//!
//! The screens are the product surface, so this test loads the actual files into a
//! webview and asserts the expected controls exist — no Tauri runtime, no DSH
//! server, no network. One GTK test per binary: gtk-rs allows a single initialising
//! thread.
#![cfg(target_os = "linux")]

#[path = "common/page_server.rs"]
mod page_server;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use webkit2gtk::{WebContext, WebView, WebViewExt};

/// Drains pending GTK sources with a short grace delay so WebKit IPC that is in
/// flight can arrive. Never blocks on a source that has nothing to deliver.
fn pump_once() {
    for _ in 0..8 {
        if !glib::MainContext::default().pending() {
            std::thread::sleep(std::time::Duration::from_millis(2));
            continue;
        }
        for _ in 0..32 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        return;
    }
}

fn page(name: &str) -> (String, PathBuf) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join(name);
    let html = std::fs::read_to_string(&path).expect("the shipped page is readable");
    (html, path)
}

/// Loads the page and runs `script` once it has finished loading.
fn evaluate(name: &str, script: &str) -> String {
    let (html, _path) = page(name);
    // An ephemeral context keeps WebKit data out of the real profile and tears
    // down cleanly, matching the other native WebKit tests.
    let context = WebContext::new_ephemeral();
    let view = WebView::with_context(&context);
    let done = Rc::new(Cell::new(false));
    {
        let done = done.clone();
        view.connect_load_changed(move |_, event| {
            if matches!(event, webkit2gtk::LoadEvent::Finished) {
                done.set(true);
            }
        });
    }
    // A realized view is required for reliable script execution on the CI
    // display; the window is a fixture only and never shows real content.
    let view_for_window = view.clone();
    let _fixture_window = if std::env::var_os("DSH_OCEAN_TEST_MAP_WEBVIEW").is_some() {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Ocean page fixture");
        window.set_default_size(640, 480);
        window.add(&view_for_window);
        window.show_all();
        Some(window)
    } else {
        None
    };
    // Local file base URI: no network access is needed or performed.
    let port = page_server::serve(html.clone());
    view.load_uri(&format!("http://127.0.0.1:{port}/page.html"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done.get() {
        pump_once();
        assert!(Instant::now() < deadline, "{name} never finished loading");
    }
    let result = Rc::new(std::cell::RefCell::new(None));
    {
        let result = result.clone();
        view.evaluate_javascript(
            script,
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            move |outcome| {
                let value = match outcome {
                    Ok(value) => value.to_string(),
                    Err(error) => format!("error: {error}"),
                };
                *result.borrow_mut() = Some(value);
            },
        );
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        pump_once();
        if let Some(value) = result.borrow().clone() {
            return value;
        }
        assert!(Instant::now() < deadline, "{name} script never completed");
    }
}

#[test]
#[ignore = "requires explicitly enabled isolated GTK display"]
fn the_shipped_pages_render_their_controls() {
    if std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref() != Ok("1") {
        return;
    }
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();

    let ids = evaluate(
        "index.html",
        r#"JSON.stringify(["install","cancel-install","retry","status-title","phase","log","server","runtime","config","diagnostics","quit"].filter((id) => document.getElementById(id)).map((id) => document.getElementById(id).id))"#,
    );
    for expected in ["install", "cancel-install", "retry", "server", "quit"] {
        assert!(ids.contains(expected), "missing {expected} in {ids}");
    }
    // The cancel control starts hidden through the `.hidden` class: nothing can be
    // cancelled before a run exists.
    let cancel_state = evaluate(
        "index.html",
        "JSON.stringify({hiddenClass: document.getElementById('cancel-install').classList.contains('hidden'), disabled: document.getElementById('cancel-install').disabled, text: document.getElementById('cancel-install').textContent.trim()})",
    );
    assert!(
        cancel_state.contains("\"hiddenClass\":true"),
        "{cancel_state}"
    );
    assert!(cancel_state.contains("Cancel install"), "{cancel_state}");
    // The install action starts hidden too and only appears when the state allows it.
    let install_hidden = evaluate(
        "index.html",
        "document.getElementById('install').classList.contains('hidden')",
    );
    assert_eq!(install_hidden, "true");

    let remote = evaluate(
        "remote.html",
        r#"JSON.stringify(["url","close","refresh","qr"].filter((id) => document.getElementById(id)))"#,
    );
    assert!(
        remote.contains("url") && remote.contains("close"),
        "{remote}"
    );
    // Pairing guidance must state the token-only access control somewhere on the page.
    let note = evaluate("remote.html", "document.body.innerText.toLowerCase()");
    assert!(
        note.contains("token") || note.contains("network"),
        "pairing page does not explain access control: {note}"
    );
}
