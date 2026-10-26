//! Behavioural contract of the shipped setup page against the Tauri IPC surface.
//!
//! A stub `__TAURI_INTERNALS__.invoke` is installed at document-start, so the real
//! page script runs unchanged while the test records which commands it calls and
//! with what arguments. No Tauri runtime, no DSH server, no network.
//! One GTK test per binary.
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
  window.__pending = [];
  window.__state = {
    phase: "idle",
    canInstall: true,
    installing: false,
    cancelRequested: false,
    logs: [],
    configPath: "/tmp/config.json",
    diagnosticsPath: "/tmp/diag",
    runtime: null,
    server: null,
  };
  window.__TAURI_INTERNALS__ = {
    invoke(command, args) {
      window.__calls.push({ command, args: args || {} });
      if (command === "get_setup_state") {
        return Promise.resolve(Object.assign({}, window.__state));
      }
      if (command === "start_bootstrap") {
        // Report the in-flight installer, like the real backend does, and stay
        // pending so the cancel control stays reachable.
        window.__state.installing = true;
        return new Promise((resolve) => window.__pending.push(resolve));
      }
      if (command === "cancel_bootstrap") {
        window.__state.cancelRequested = true;
        return Promise.resolve(true);
      }
      return Promise.resolve(null);
    },
  };
"#;

fn page() -> (String, PathBuf) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/index.html");
    let html = std::fs::read_to_string(&path).expect("the setup page is readable");
    (html, path)
}

fn pump(done: &Rc<Cell<bool>>, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done.get() {
        pump_once();
        std::thread::sleep(Duration::from_millis(5));
        assert!(Instant::now() < deadline, "{what} never completed");
    }
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
        pump_once();
        if let Some(value) = result.borrow().clone() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "script never completed: {script}"
        );
    }
}

#[test]
#[ignore = "requires explicitly enabled isolated GTK/WebKit display"]
fn setup_page_calls_the_documented_commands_and_keeps_cancel_reachable() {
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
    pump(&loaded, "the setup page");

    // The page asks for its state on load; the auto-start path prepares the
    // service shortly after, so wait for both commands to appear.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut commands = String::new();
    while Instant::now() < deadline {
        commands = evaluate(
            &view,
            "JSON.stringify(window.__calls.map((call) => call.command))",
        );
        if commands.contains("get_setup_state") && commands.contains("prepare_service") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(commands.contains("get_setup_state"), "{commands}");
    assert!(commands.contains("prepare_service"), "{commands}");
    // Nothing is installed or cancelled before the user asks for it.
    assert!(!commands.contains("start_bootstrap"), "{commands}");
    assert!(!commands.contains("cancel_bootstrap"), "{commands}");

    // Install starts the bootstrap and leaves the cancel control usable.
    let after_install = evaluate(
        &view,
        "document.getElementById('install').click(); 'clicked'",
    );
    assert_eq!(after_install, "clicked");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        commands = evaluate(
            &view,
            "JSON.stringify(window.__calls.map((call) => call.command))",
        );
        if commands.contains("start_bootstrap") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(commands.contains("start_bootstrap"), "{commands}");
    // With the request still in flight the page must expose cancellation: the
    // install button is disabled and the cancel control is enabled and visible.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut installing = String::new();
    while Instant::now() < deadline {
        installing = evaluate(
            &view,
            "JSON.stringify({pending: window.__pending.length === 1, installDisabled: document.getElementById('install').disabled, cancelHidden: document.getElementById('cancel-install').classList.contains('hidden'), cancelDisabled: document.getElementById('cancel-install').disabled})",
        );
        if installing.contains("\"cancelHidden\":false") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(installing.contains("\"pending\":true"), "{installing}");
    assert!(
        installing.contains("\"installDisabled\":true"),
        "{installing}"
    );
    assert!(
        installing.contains("\"cancelHidden\":false"),
        "{installing}"
    );
    assert!(
        installing.contains("\"cancelDisabled\":false"),
        "{installing}"
    );

    // Cancelling must call the documented command exactly once.
    evaluate(
        &view,
        "document.getElementById('cancel-install').click(); 'cancelled'",
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        commands = evaluate(
            &view,
            "JSON.stringify(window.__calls.map((call) => call.command))",
        );
        if commands.contains("cancel_bootstrap") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(commands.contains("cancel_bootstrap"), "{commands}");

    // Complete the installer, then exercise the shipped recovery control in
    // real WebKit. The stub is only the IPC boundary, not rendering behavior.
    evaluate(&view, "window.__state.installing=false; window.__pending.shift()(null); window.__state.phase='failed'; window.__state.recoveryRequired=true; window.__state.updating=false; 'pending'");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let controls = evaluate(&view, "JSON.stringify({recover: !!document.getElementById('recover') && !document.getElementById('recover').classList.contains('hidden') && !document.getElementById('recover').disabled, retry: document.getElementById('retry').classList.contains('hidden'), install: document.getElementById('install').classList.contains('hidden')})");
        if controls == r#"{"recover":true,"retry":true,"install":true}"# {
            break;
        }
        assert!(Instant::now() < deadline, "pending controls: {controls}");
        pump_once();
    }
    evaluate(
        &view,
        "window.__calls=[]; document.getElementById('recover').click(); 'recover'",
    );
    let calls = evaluate(&view, "JSON.stringify(window.__calls.map(c=>c.command))");
    assert!(calls.contains("recover_runtime"), "{calls}");
    assert!(
        !calls.contains("prepare_service") && !calls.contains("start_bootstrap"),
        "{calls}"
    );

    // The auxiliary actions use their documented command names too.
    evaluate(&view, "document.getElementById('diagnostics').click(); document.getElementById('folder').click(); document.getElementById('quit').click(); 'aux'");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        commands = evaluate(
            &view,
            "JSON.stringify(window.__calls.map((call) => call.command))",
        );
        if commands.contains("save_diagnostics")
            && commands.contains("open_diagnostics_folder")
            && commands.contains("quit_app")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for expected in ["save_diagnostics", "open_diagnostics_folder", "quit_app"] {
        assert!(
            commands.contains(expected),
            "missing {expected}: {commands}"
        );
    }
    // A new document has no page-local installer promise. Native ownership
    // survives the real WebKit reload through the mocked IPC boundary.
    manager.add_script(&UserScript::new(
        "window.__state.installing=true; window.__state.updating=true;",
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        &[],
        &[],
    ));
    loaded.set(false);
    view.reload();
    pump(&loaded, "reloaded setup page");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ready = evaluate(&view, "String(!document.getElementById('cancel-install').disabled && !document.getElementById('cancel-install').classList.contains('hidden'))");
        if ready == "true" {
            break;
        }
        assert!(Instant::now() < deadline, "cancel unavailable after reload");
        pump_once();
    }
    evaluate(
        &view,
        "document.getElementById('cancel-install').click(); 'cancel'",
    );
    let calls = evaluate(&view, "JSON.stringify(window.__calls.map(c=>c.command))");
    assert!(calls.contains("cancel_bootstrap"), "{calls}");
    assert!(!calls.contains("start_bootstrap"), "{calls}");
    assert!(!calls.contains("prepare_service"), "{calls}");
}
