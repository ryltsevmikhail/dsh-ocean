//! The shipped pages must keep matching the JSON the backend sends them.
//!
//! The pages read named fields out of `get_setup_state` and `get_remote_state`.
//! Renaming a Rust field quietly breaks only the browser side, so this test
//! extracts every `state.<field>` the pages use and checks it against the real
//! serialized payload. No GTK, no server, no network.
use std::collections::BTreeSet;

use dsh_ocean::bootstrap::RuntimeInfo;
use dsh_ocean::remote::RemoteState;
use dsh_ocean::state::{ServerInfo, SetupState};

fn page_fields(name: &str, prefix: &str) -> BTreeSet<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join(name);
    let html = std::fs::read_to_string(&path).expect("the shipped page is readable");
    let needle = format!("{prefix}.");
    let mut fields = BTreeSet::new();
    let mut rest = html.as_str();
    while let Some(index) = rest.find(&needle) {
        rest = &rest[index + needle.len()..];
        let field: String = rest
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect();
        let next = rest[field.len()..].chars().next();
        // Only property reads, not calls such as `state.foo(` or deeper paths.
        if !field.is_empty() && next != Some('(') {
            fields.insert(field);
        }
    }
    fields
}

fn keys(value: &serde_json::Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("payload is an object")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn setup_page_fields_exist_in_the_setup_payload() {
    let payload = SetupState {
        phase: "idle".into(),
        installing: false,
        cancel_requested: false,
        message: None,
        logs: vec!["line".into()],
        server: Some(ServerInfo {
            base_url: "http://127.0.0.1:3080/".into(),
            mode: "attached".into(),
            unit: None,
        }),
        runtime: Some(RuntimeInfo {
            dsh: "/tmp/dsh".into(),
            node: "/tmp/node".into(),
            version: "0.0.0".into(),
            target_version: "0.0.0".into(),
            install_root: None,
            ready: true,
        }),
        can_install: true,
        recovery_required: false,
        updating: false,
        config_path: "/tmp/config.json".into(),
        diagnostics_path: "/tmp/diagnostics".into(),
    };
    let value = serde_json::to_value(&payload).expect("setup payload serializes");
    let available = keys(&value);

    let used = page_fields("index.html", "state");
    assert!(!used.is_empty(), "no state fields found in index.html");
    let missing: Vec<_> = used.difference(&available).cloned().collect();
    assert!(
        missing.is_empty(),
        "setup page reads {missing:?}, which the payload does not send: {available:?}"
    );

    // Nested reads the page performs must exist on their objects too.
    for nested in ["runtime.version", "runtime.node"] {
        let (parent, child) = nested.split_once('.').expect("dotted path");
        let used = page_fields("index.html", &format!("state.{parent}"));
        if used.contains(child) {
            let object = value[parent].as_object().expect("nested object");
            assert!(
                object.contains_key(child),
                "state.{nested} is read by the page but not sent"
            );
        }
    }
    for (parent, child) in [("server", "baseUrl"), ("server", "mode")] {
        let used = page_fields("index.html", &format!("state.{parent}"));
        if used.contains(child) {
            let object = value[parent].as_object().expect("nested object");
            assert!(
                object.contains_key(child),
                "state.{parent}.{child} is read by the page but not sent"
            );
        }
    }
}

#[test]
fn pairing_page_fields_exist_in_the_remote_payload() {
    let payload = RemoteState {
        base_url: "http://127.0.0.1:3080/".into(),
        loopback: true,
        pairing_url: None,
        qr_svg: None,
        note: "Loopback only.".into(),
    };
    let value = serde_json::to_value(&payload).expect("remote payload serializes");
    let available = keys(&value);
    let used = page_fields("remote.html", "state");
    assert!(!used.is_empty(), "no state fields found in remote.html");
    let missing: Vec<_> = used.difference(&available).cloned().collect();
    assert!(
        missing.is_empty(),
        "pairing page reads {missing:?}, which the payload does not send: {available:?}"
    );
    // Sanity: the fields the pairing test drives are really in the payload.
    for expected in ["qrSvg", "note", "pairingUrl"] {
        assert!(available.contains(expected), "missing {expected}");
    }
}

#[test]
fn the_extractor_ignores_calls_and_finds_real_fields() {
    let fields = page_fields("index.html", "state");
    assert!(fields.contains("phase"), "{fields:?}");
    assert!(fields.contains("canInstall"), "{fields:?}");
    assert!(
        !fields.contains("map"),
        "a method call was mistaken for a field"
    );
}

/// Command names invoked by the shipped pages.
fn page_commands() -> BTreeSet<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut commands = BTreeSet::new();
    for name in ["index.html", "remote.html"] {
        let html = std::fs::read_to_string(root.join("ui").join(name)).expect("page is readable");
        let mut rest = html.as_str();
        while let Some(index) = rest.find("invoke(\"") {
            rest = &rest[index + "invoke(\"".len()..];
            let command: String = rest
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect();
            if !command.is_empty() {
                commands.insert(command);
            }
        }
    }
    commands
}

/// Commands registered with the Tauri invoke handler.
fn registered_commands() -> BTreeSet<String> {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .expect("main.rs is readable");
    let start = source
        .find("generate_handler![")
        .expect("the invoke handler list exists");
    let end = source[start..]
        .find(']')
        .map(|offset| start + offset)
        .expect("the invoke handler list is closed");
    let mut commands = BTreeSet::new();
    for entry in source[start + "generate_handler![".len()..end].split(',') {
        let entry = entry.trim();
        if let Some((_, name)) = entry.rsplit_once("::") {
            if !name.is_empty() {
                commands.insert(name.to_owned());
            }
        }
    }
    commands
}

#[test]
fn every_command_a_page_calls_is_registered() {
    let called = page_commands();
    let registered = registered_commands();
    assert!(!called.is_empty(), "no invoke() calls found in the pages");
    assert!(
        !registered.is_empty(),
        "no commands found in the handler list"
    );
    let missing: Vec<_> = called.difference(&registered).cloned().collect();
    assert!(
        missing.is_empty(),
        "pages call {missing:?}, which the invoke handler does not register: {registered:?}"
    );
    // The pages must not reach for a command that belongs to the tray only.
    assert!(
        !called.contains("restart_service"),
        "a page calls restart_service, which should stay a tray action"
    );
}

#[test]
fn every_mutating_command_is_bound_to_an_owning_window() {
    // The main window loads the harness origin, so page scripts there share the IPC
    // bridge. Each command that can change state must name the window allowed to
    // call it, or a third-party bundle could drive it.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/commands.rs"))
        .expect("read commands.rs");
    let remote_state =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/remote.rs"))
            .expect("read remote.rs");
    let guarded = [
        ("get_setup_state", &source, "\"setup\""),
        ("prepare_service", &source, "\"setup\""),
        ("recover_runtime", &source, "\"setup\""),
        ("start_bootstrap", &source, "\"setup\""),
        ("cancel_bootstrap", &source, "\"setup\""),
        ("save_diagnostics", &source, "\"setup\""),
        ("open_diagnostics_folder", &source, "\"setup\""),
        ("quit_app", &source, "\"setup\""),
        ("restart_service", &source, "\"setup\""),
        ("start_pairing_proxy", &source, "\"remote\""),
        ("stop_pairing_proxy", &source, "\"remote\""),
        ("pairing_proxy_status", &source, "\"remote\""),
        ("close_pairing", &source, "\"remote\""),
        ("get_remote_state", &remote_state, "\"remote\""),
    ];
    for (name, text, label) in guarded {
        let start = text
            .find(&format!("fn {name}"))
            .unwrap_or_else(|| panic!("{name} is missing"));
        let body = &text[start..(start + 1400).min(text.len())];
        assert!(
            body.contains("require_window") || body.contains(label),
            "{name} must be restricted to its owning window"
        );
        assert!(
            body.contains(label) || name == "get_remote_state",
            "{name} must name the allowed window label"
        );
    }
}

#[test]
fn the_setup_log_rerenders_when_its_text_changes_past_the_cap() {
    // The ring buffer is capped, so a length comparison would freeze the panel at
    // the cap and hide every later line, including installer failures.
    let page = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ui/index.html"))
        .expect("read index.html");
    assert!(
        page.contains("log !== lastLogText"),
        "the log panel must compare its rendered text, not the line count"
    );
    assert!(
        !page.contains("(state.logs || []).length !== lastLogCount"),
        "comparing the capped length freezes the log panel"
    );
}
