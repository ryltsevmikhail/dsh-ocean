//! Behavioural contract of the shipped pairing page.
//!
//! A stub `__TAURI_INTERNALS__.invoke` is installed at document-start, so the real
//! page script runs unchanged. The stub also drives the interaction with two timers
//! (report an exposed URL, then click Close) and records a transcript, so the test
//! needs a single evaluation instead of polling WebKit in a loop.
#![cfg(target_os = "linux")]

#[path = "common/page_server.rs"]
mod page_server;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use webkit2gtk::{
    UserContentInjectedFrames, UserContentManager, UserContentManagerExt, UserScript,
    UserScriptInjectionTime, WebContext, WebView, WebViewExt,
};

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

const STUB: &str = r#"
  window.__calls = [];
  window.__state = {
    qrSvg: "",
    note: "Loopback only: no phone can reach this server until you expose it.",
    pairingUrl: "",
  };
  window.__TAURI_INTERNALS__ = {
    invoke(command, args) {
      window.__calls.push({ command: command, args: args || {} });
      if (command === "get_remote_state") {
        return Promise.resolve(Object.assign({}, window.__state));
      }
      if (command === "start_pairing_proxy") {
        window.__proxy = {
          listen: "0.0.0.0:3091",
          target: "127.0.0.1:3080",
          ttlSeconds: args.ttlSeconds,
          remainingSeconds: args.ttlSeconds,
          activeConnections: 0,
          accepted: 0,
          revoked: false,
        };
        return Promise.resolve(window.__proxy);
      }
      if (command === "pairing_proxy_status") {
        return Promise.resolve(window.__proxy);
      }
      if (command === "stop_pairing_proxy") {
        window.__proxy = null;
        return Promise.resolve(true);
      }
      return Promise.resolve(true);
    },
  };
  window.__snapshot = null;
  window.__proxy = null;
  window.__tauriTimers = [];
  window.__tauriTimers.push(setTimeout(function () {
    // The backend now reports an exposed pairing URL; the page must render it on
    // the next Refresh.
    window.__state = {
      qrSvg: "<svg xmlns='http://www.w3.org/2000/svg'></svg>",
      note: "Reachable from the local network.",
      pairingUrl: "http://192.168.1.50:3080/",
    };
    document.getElementById("refresh").click();
  }, 400));
  window.__tauriTimers.push(setTimeout(function () {
    document.getElementById("proxy-5").click();
  }, 700));
  window.__tauriTimers.push(setTimeout(function () {
    window.__snapshot_proxy = {
      status: document.getElementById("proxy-status").textContent,
      stopDisabled: document.getElementById("proxy-stop").disabled,
      startedTtl: window.__proxy ? window.__proxy.ttlSeconds : null,
    };
    document.getElementById("proxy-stop").click();
  }, 1200));
  window.__tauriTimers.push(setTimeout(function () {
    window.__snapshot = {
      commands: window.__calls.map(function (call) { return call.command; }),
      urlHidden: document.getElementById("url-box").classList.contains("hidden"),
      url: document.getElementById("url").textContent,
      qrLength: document.getElementById("qr").innerHTML.length,
      note: document.getElementById("note").textContent,
    };
    document.getElementById("close").click();
  }, 1600));
"#;

fn page() -> (String, PathBuf) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/remote.html");
    let html = std::fs::read_to_string(&path).expect("the pairing page is readable");
    (html, path)
}

fn pump() {
    pump_once();
    pump_once();
}

fn evaluate(view: &WebView, script: &str) -> String {
    let result = Rc::new(RefCell::new(None));
    {
        let result = result.clone();
        view.evaluate_javascript(
            script,
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            move |outcome| {
                *result.borrow_mut() = Some(match outcome {
                    Ok(value) => value.to_string(),
                    Err(error) => format!("error: {error}"),
                });
            },
        );
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        pump();
        if let Some(value) = result.borrow().clone() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "script never completed: {script}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires explicitly enabled isolated GTK/WebKit display"]
fn pairing_page_reports_state_and_uses_documented_commands() {
    if std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref() != Ok("1") {
        return;
    }
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();

    let (html, _path) = page();
    let context = WebContext::new_ephemeral();
    let manager = UserContentManager::new();
    manager.add_script(&UserScript::new(
        STUB,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        &[],
        &[],
    ));
    let view = WebView::builder()
        .web_context(&context)
        .user_content_manager(&manager)
        .build();
    // A realized view is required for reliable script execution on CI displays.
    let fixture_window = if std::env::var_os("DSH_OCEAN_TEST_MAP_WEBVIEW").is_some() {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Ocean page fixture");
        window.set_default_size(640, 480);
        window.add(&view);
        window.show_all();
        Some(window)
    } else {
        None
    };
    let loaded = Rc::new(Cell::new(false));
    {
        let loaded = loaded.clone();
        view.connect_load_changed(move |_, event| {
            if matches!(event, webkit2gtk::LoadEvent::Finished) {
                loaded.set(true);
            }
        });
    }
    let _fixture_window = fixture_window;
    let port = page_server::serve(html.clone());
    view.load_uri(&format!("http://127.0.0.1:{port}/page.html"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !loaded.get() {
        pump();
        assert!(Instant::now() < deadline, "the pairing page never loaded");
        std::thread::sleep(Duration::from_millis(10));
    }

    // The stub's timers run on the page's own event loop: give them time while
    // continuing to pump the GTK main context.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = evaluate(&view, "JSON.stringify(window.__snapshot)");
        if snapshot != "null" && !snapshot.starts_with("error") {
            // The snapshot is taken before Close; give the click a moment, then
            // read the final command list.
            std::thread::sleep(Duration::from_millis(200));
            pump();
            let calls = evaluate(
                &view,
                "JSON.stringify(window.__calls.map(function (call) { return call.command; }))",
            );
            assert!(snapshot.contains("get_remote_state"), "{snapshot}");
            assert!(snapshot.contains("\"urlHidden\":false"), "{snapshot}");
            assert!(snapshot.contains("192.168.1.50"), "{snapshot}");
            assert!(!snapshot.contains("\"qrLength\":0"), "{snapshot}");
            assert!(
                snapshot.contains("Reachable from the local network"),
                "{snapshot}"
            );
            assert!(calls.contains("close_pairing"), "{calls}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the pairing page never produced a snapshot: {snapshot}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
