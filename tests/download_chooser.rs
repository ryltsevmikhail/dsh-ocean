//! Actual GTK widgets/nested dialog loop. Run explicitly on an isolated
//! Broadway/Xvfb display, never as an unsolicited dialog on the user's desktop.
use dsh_ocean::{download_chooser, downloads::Transfer};
use gtk::{glib, prelude::*};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-chooser-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn scripted(
    dialog: &gtk::FileChooserDialog,
    folder: &Path,
    name: Option<&str>,
    retry: Option<&str>,
) -> Option<PathBuf> {
    let popup = dialog.clone();
    let folder = folder.to_owned();
    let name = name.map(str::to_owned);
    let accepting = name.is_some();
    let configured = Rc::new(Cell::new(false));
    let configured_for_tick = configured.clone();
    let retry = retry.map(str::to_owned);
    let needs_retry = retry.is_some();
    let retried = Rc::new(Cell::new(false));
    let retried_for_tick = retried.clone();
    let driver = glib::timeout_add_local(Duration::from_millis(50), move || {
        if popup.is_mapped() && popup.current_folder().as_ref() == Some(&folder) {
            if let Some(name) = &name {
                if !configured_for_tick.get() {
                    popup.set_current_name(name);
                    configured_for_tick.set(true);
                }
            }
            if let Some(retry) = &retry {
                let warning = popup
                    .extra_widget()
                    .and_then(|widget| widget.downcast::<gtk::Label>().ok())
                    .is_some_and(|label| label.text().contains("already exists"));
                if warning && !retried_for_tick.get() {
                    popup.set_current_name(retry);
                    retried_for_tick.set(true);
                }
            }
            let response = if name.is_some() {
                gtk::ResponseType::Accept
            } else {
                gtk::ResponseType::Cancel
            };
            if let Some(button) = popup
                .widget_for_response(response)
                .and_then(|widget| widget.downcast::<gtk::Button>().ok())
            {
                if button.is_sensitive() {
                    button.emit_clicked();
                }
            }
        }
        glib::ControlFlow::Continue
    });
    let popup = dialog.clone();
    let timed_out = Rc::new(Cell::new(false));
    let flag = timed_out.clone();
    let watchdog = glib::timeout_add_local(Duration::from_secs(8), move || {
        flag.set(true);
        popup.response(gtk::ResponseType::Cancel);
        glib::ControlFlow::Continue
    });
    let selected = download_chooser::run(dialog);
    driver.remove();
    watchdog.remove();
    assert!(
        !timed_out.get(),
        "GTK chooser failed to respond through its actual buttons"
    );
    if accepting {
        assert!(configured.get());
    }
    if needs_retry {
        assert!(
            retried.get(),
            "existing filename must remain in the chooser for retry"
        );
    }
    selected
}
#[test]
#[ignore = "requires explicitly enabled isolated GTK dialog test display"]
fn actual_save_dialog_cancel_new_path_and_existing_target_are_safe() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let root = std::env::temp_dir().join(format!(
        "dsh-ocean-chooser-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let f = Fixture(fs::canonicalize(root).unwrap());
    let source = "http://user:synthetic-private@127.0.0.1:49001/path?token=synthetic-private-query";
    let dialog = download_chooser::build(None, "../unsafe\nname", source, Some(&f.0));
    assert_eq!(dialog.action(), gtk::FileChooserAction::Save);
    assert!(dialog.is_modal() && dialog.is_local_only());
    assert!(!dialog.does_overwrite_confirmation());
    let title = dialog.title().unwrap();
    assert_eq!(title.as_str(), "Save download from http://127.0.0.1:49001");
    assert!(!title.contains("synthetic-private"));
    let suggested = dialog.current_name().unwrap();
    assert!(!suggested.contains('/') && !suggested.chars().any(char::is_control));
    assert!(scripted(&dialog, &f.0, None, None).is_none());
    assert!(
        fs::read_dir(&f.0).unwrap().next().is_none(),
        "cancel never creates a payload"
    );

    let parent = gtk::Window::new(gtk::WindowType::Toplevel);
    parent.set_title("Ocean chooser fixture");
    parent.show_all();
    let dialog = download_chooser::build(
        Some(&parent),
        "default.txt",
        "http://127.0.0.1:49001/",
        Some(&f.0),
    );
    assert_eq!(dialog.transient_for(), Some(parent.clone()));
    let selected =
        scripted(&dialog, &f.0, Some("отчёт new.txt"), None).expect("GTK selected new file");
    assert_eq!(selected, f.0.join("отчёт new.txt"));
    assert!(!selected.exists(), "chooser does not write the destination");
    let mut transfer = Transfer::prepare(&selected).unwrap();
    fs::write(&transfer.payload, "chooser fixture bytes").unwrap();
    transfer.publish().unwrap();
    assert_eq!(
        fs::read_to_string(&selected).unwrap(),
        "chooser fixture bytes"
    );

    let existing = f.0.join("existing.txt");
    fs::write(&existing, "preserve existing bytes").unwrap();
    let dialog =
        download_chooser::build(None, "default.txt", "http://127.0.0.1:49001/", Some(&f.0));
    let selected = scripted(&dialog, &f.0, Some("existing.txt"), Some("replacement.txt"))
        .expect("same chooser accepts a new name after refusal");
    assert_eq!(selected, f.0.join("replacement.txt"));
    assert!(!selected.exists());
    let mut transfer = Transfer::prepare(&selected).unwrap();
    fs::write(&transfer.payload, "retry fixture bytes").unwrap();
    transfer.publish().unwrap();
    assert_eq!(fs::read_to_string(selected).unwrap(), "retry fixture bytes");
    assert_eq!(
        fs::read_to_string(existing).unwrap(),
        "preserve existing bytes"
    );
    parent.close();
}
