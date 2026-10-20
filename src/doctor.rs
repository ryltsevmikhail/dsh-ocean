//! Read-only runtime/backup summary for `--doctor`.
//!
//! Nothing here deletes, moves, downloads or starts anything. Counting is
//! bounded and never follows a symlink, so a hostile or broken prefix can only
//! produce "unavailable" output instead of side effects.
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::profile_backup;

/// Counters for one private runtime prefix. Only names that the installer
/// itself generates are considered; everything else is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSummary {
    pub selected: Option<String>,
    pub previous: Option<String>,
    pub generations: usize,
    pub unreferenced: usize,
    pub stale_scratch: usize,
    pub error: Option<String>,
}

fn generation_name(name: &str) -> bool {
    name.strip_prefix("gen.").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

impl RuntimeSummary {
    /// Fixed, content-free text used by `--doctor`.
    pub fn text(&self) -> String {
        if let Some(error) = &self.error {
            return format!("unavailable ({error})");
        }
        format!(
            "selected={} previous={} known={} unreferenced={} staleScratch={}",
            self.selected.as_deref().unwrap_or("none"),
            self.previous.as_deref().unwrap_or("none"),
            self.generations,
            self.unreferenced,
            self.stale_scratch
        )
    }
}

/// Reads the selector and `.previous` of the selected generation, then counts
/// direct `generations/gen.*` children and `.download.*`/`.pointer.*` scratch.
pub fn runtime_summary(prefix: &Path) -> RuntimeSummary {
    let mut summary = RuntimeSummary::default();
    let selector = prefix.join("current");
    match fs::symlink_metadata(&selector) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::read_link(&selector) {
            Ok(target) => {
                let name = target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                if target
                    .parent()
                    .and_then(Path::file_name)
                    .map(|parent| parent == "generations")
                    == Some(true)
                    && name.as_deref().is_some_and(generation_name)
                {
                    summary.selected = name;
                } else {
                    summary.error = Some("invalid selector".to_owned());
                    return summary;
                }
            }
            Err(error) => {
                summary.error = Some(format!("selector unreadable: {error}"));
                return summary;
            }
        },
        Ok(_) => {
            summary.error = Some("selector is not a symlink".to_owned());
            return summary;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            summary.error = Some(format!("selector unreadable: {error}"));
            return summary;
        }
    }
    let generations = prefix.join("generations");
    if let Some(selected) = &summary.selected {
        if let Ok(raw) =
            crate::util::read_regular_text(&generations.join(selected).join(".previous"), 1024)
        {
            let previous = raw.trim();
            if generation_name(previous) {
                summary.previous = Some(previous.to_owned());
            }
        }
    }
    let entries = match fs::read_dir(&generations) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return summary,
        Err(error) => {
            summary.error = Some(format!("generations unreadable: {error}"));
            return summary;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !generation_name(&name) || !meta.is_dir() {
            continue;
        }
        summary.generations += 1;
        let referenced = summary.selected.as_deref() == Some(name.as_str())
            || summary.previous.as_deref() == Some(name.as_str());
        if !referenced {
            summary.unreferenced += 1;
        }
    }
    if let Ok(entries) = fs::read_dir(prefix) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir()
                && (name.starts_with(".download.") || name.starts_with(".pointer."))
                && name.len() > ".download.".len()
            {
                summary.stale_scratch += 1;
            }
        }
    }
    summary
}

/// Fixed text for the snapshot inventory; never lists names or paths.
pub fn backups_text(backups: &Path) -> String {
    match profile_backup::snapshots(backups) {
        Ok(snapshots) => format!(
            "count={} bytes={}",
            snapshots.len(),
            snapshots.iter().map(|item| item.bytes).sum::<u64>()
        ),
        Err(_) => "unavailable (snapshot inventory could not be read)".to_owned(),
    }
}

/// One runtime generation directory with its own size accounting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationInfo {
    /// Installed generation directory name (`gen.*`), never a file path.
    pub name: String,
    /// True when `current` or its recorded `.previous` references it.
    pub referenced: bool,
    pub files: usize,
    pub bytes: u64,
    pub truncated: bool,
}

const MAX_GENERATION_ENTRIES: usize = 200_000;

fn directory_size(root: &Path, files: &mut usize, bytes: &mut u64) -> bool {
    let mut truncated = false;
    let mut budget = MAX_GENERATION_ENTRIES;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth > 32 {
            truncated = true;
            continue;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                truncated = true;
                continue;
            }
        };
        for entry in entries.flatten() {
            if budget == 0 {
                return true;
            }
            budget -= 1;
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                truncated = true;
                continue;
            };
            if metadata.file_type().is_dir() {
                stack.push((path, depth + 1));
            } else if metadata.file_type().is_file() {
                *files += 1;
                *bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    truncated
}

/// Lists `generations/gen.*` children with sizes, marking referenced ones.
///
/// Read-only and bounded: symlinks and special files are never followed or counted.
pub fn generations(prefix: &Path) -> Vec<GenerationInfo> {
    let summary = runtime_summary(prefix);
    let directory = prefix.join("generations");
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.strip_prefix("gen.").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        }) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let mut files = 0usize;
        let mut bytes = 0u64;
        let truncated = directory_size(&entry.path(), &mut files, &mut bytes);
        result.push(GenerationInfo {
            referenced: summary.selected.as_deref() == Some(name.as_str())
                || summary.previous.as_deref() == Some(name.as_str()),
            name,
            files,
            bytes,
            truncated,
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    result
}

/// Full diagnostic snapshot for `--doctor`, available as text or JSON.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub client_version: String,
    pub runtime: Option<String>,
    pub server_url: String,
    /// HTTP status of the configured origin, when it answered.
    pub server_status: Option<u16>,
    pub service_active: bool,
    pub unit: String,
    pub config: String,
    pub runtime_state: RuntimeSummary,
    pub backups: String,
    pub storage: crate::session_inventory::Summary,
    pub webkit_abi: crate::webkit_abi::Versions,
    /// Per-generation size accounting (read-only).
    pub runtime_generations: Vec<GenerationInfo>,
    /// Configuration source diagnosis (missing/unparseable/overridden).
    pub config_report: crate::config::Report,
    /// Whether the cached launch token is still accepted, when anything answered.
    pub launch_token: TokenState,
}

/// Result of a read-only token check against the configured origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenState {
    /// No cached token exists, so nothing was checked.
    Absent,
    /// The origin accepted the token (303 index exchange).
    Valid,
    /// The origin answered but refused the token.
    Rejected,
    /// The origin did not answer at all.
    Unknown,
}

impl TokenState {
    pub fn text(self) -> &'static str {
        match self {
            TokenState::Absent => "no cached token",
            TokenState::Valid => "accepted",
            TokenState::Rejected => "rejected",
            TokenState::Unknown => "not verified (origin did not answer)",
        }
    }
}

/// Pure classification so tests never send a request.
pub fn classify_token(token: Option<&str>, exchange: Option<u16>) -> TokenState {
    match (token, exchange) {
        (None, _) => TokenState::Absent,
        (Some(token), Some(303)) if !token.is_empty() => TokenState::Valid,
        (Some(_), Some(_)) => TokenState::Rejected,
        (Some(_), None) => TokenState::Unknown,
    }
}

/// Checks the cached token against the configured origin. Read-only: the exchange
/// only proves the token, and the token itself is never printed.
pub fn launch_token_state() -> TokenState {
    let token = crate::auth::cached_token();
    let answered = token.as_ref().and_then(|token| {
        // A display fallback is not an authority to receive a cached credential.
        let base = crate::config::checked_server_url().ok()?;
        crate::http::status(&crate::auth::token_url(&base, token))
    });
    classify_token(token.as_deref(), answered)
}

impl Report {
    /// True when the parts a user cannot work around are broken: the WebKitGTK
    /// ABI is unusable, or the configured server does not answer at all.
    pub fn has_blocking_problem(&self) -> bool {
        matches!(self.webkit_abi.status.as_str(), "missing" | "too old")
            || self.server_status.is_none()
    }

    pub fn text(&self) -> String {
        use std::fmt::Write as _;
        let mut output = String::new();
        let _ = writeln!(
            output,
            "runtime: {}",
            self.runtime.as_deref().unwrap_or("not installed")
        );
        let _ = writeln!(
            output,
            "server:  {} (HTTP {:?})",
            self.server_url, self.server_status
        );
        let _ = writeln!(
            output,
            "service: {} (active: {})",
            crate::config::UNIT_NAME,
            self.service_active
        );
        let _ = writeln!(output, "unit:    {}", self.unit);
        let _ = writeln!(output, "config:  {}", self.config);
        let _ = writeln!(output, "runtime state: {}", self.runtime_state.text());
        let _ = writeln!(output, "backups:       {}", self.backups);
        let _ = writeln!(output, "storage:       {}", self.storage.text());
        let _ = writeln!(output, "webkit abi:    {}", self.webkit_abi.text());
        let total: u64 = self.runtime_generations.iter().map(|item| item.bytes).sum();
        let unreferenced = self
            .runtime_generations
            .iter()
            .filter(|item| !item.referenced)
            .count();
        let _ = writeln!(
            output,
            "generation bytes: {total} across {} generation(s), {unreferenced} unreferenced",
            self.runtime_generations.len()
        );
        let _ = writeln!(output, "launch token:  {}", self.launch_token.text());
        let _ = writeln!(
            output,
            "config source: {:?} -> {}",
            self.config_report.url_source, self.config_report.effective_url
        );
        for issue in &self.config_report.issues {
            let _ = writeln!(output, "config issue:  {issue}");
        }
        output
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({"error": "report"}))
    }
}

/// Collects the report. Read-only: no download, install, cleanup or restart.
pub fn report() -> Report {
    let base = crate::config::server_url();
    Report {
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
        runtime: crate::bootstrap::info()
            .map(|info| format!("DSH {} ({})", info.version, info.dsh)),
        server_status: crate::http::status(&base),
        server_url: crate::config::server_origin_text(&base),
        service_active: crate::systemd::is_active(),
        unit: crate::systemd::unit_path().display().to_string(),
        config: crate::config::config_path().display().to_string(),
        runtime_state: runtime_summary(&prefix()),
        backups: backups_text(&crate::config::profile_backup_dir()),
        storage: crate::session_inventory::inventory(&crate::config::dsh_home()),
        webkit_abi: crate::webkit_abi::probe(),
        runtime_generations: generations(&prefix()),
        config_report: crate::config::diagnose(),
        launch_token: launch_token_state(),
    }
}

/// Resolves the prefix the app itself uses, for the CLI.
pub fn prefix() -> PathBuf {
    crate::config::data_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-doctor-{}-{}-{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn missing_prefix_reports_nothing_without_error() {
        let root = temp("missing");
        let summary = runtime_summary(&root.join("absent"));
        assert_eq!(summary.selected, None);
        assert_eq!(summary.generations, 0);
        assert_eq!(summary.error, None);
        assert!(summary.text().contains("selected=none"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn counts_referenced_and_unreferenced_generations_without_deleting() {
        let root = temp("counts");
        let generations = root.join("generations");
        for name in ["gen.aaa", "gen.bbb", "gen.ccc", "not-a-generation"] {
            fs::create_dir_all(generations.join(name)).unwrap();
        }
        fs::write(generations.join("gen.bbb/.previous"), "gen.aaa\n").unwrap();
        fs::write(generations.join("gen.ccc/.previous"), "none\n").unwrap();
        // A symlinked generation must be ignored rather than followed or counted.
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, generations.join("gen.ddd")).unwrap();
        fs::create_dir_all(root.join(".download.stale")).unwrap();
        fs::create_dir_all(root.join(".pointer.stale")).unwrap();
        fs::write(root.join(".download.stale/f"), b"x").unwrap();
        symlink("generations/gen.bbb", root.join("current")).unwrap();

        let summary = runtime_summary(&root);
        assert_eq!(summary.error, None);
        assert_eq!(summary.selected.as_deref(), Some("gen.bbb"));
        assert_eq!(summary.previous.as_deref(), Some("gen.aaa"));
        assert_eq!(summary.generations, 3);
        assert_eq!(summary.unreferenced, 1);
        assert_eq!(summary.stale_scratch, 2);
        assert!(summary.text().contains("selected=gen.bbb previous=gen.aaa"));
        // Read-only: every fixture entry still exists.
        for name in ["gen.aaa", "gen.bbb", "gen.ccc"] {
            assert!(generations.join(name).is_dir());
        }
        assert!(root.join(".download.stale").is_dir());
        assert!(outside.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_selector_is_reported_without_guessing() {
        let root = temp("selector");
        fs::create_dir_all(root.join("generations/gen.aaa")).unwrap();
        fs::write(root.join("current"), b"not a link").unwrap();
        let summary = runtime_summary(&root);
        assert!(summary.error.is_some());
        assert!(summary.text().starts_with("unavailable"));
        fs::remove_file(root.join("current")).unwrap();
        symlink("../escape", root.join("current")).unwrap();
        let summary = runtime_summary(&root);
        assert!(summary.error.is_some());
        assert!(summary.selected.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generation_sizes_are_bounded_and_mark_references() {
        let root = temp("generation-sizes");
        let generations = root.join("generations");
        for name in ["gen.aaa", "gen.bbb"] {
            fs::create_dir_all(generations.join(name).join("nested")).unwrap();
            fs::write(generations.join(name).join("nested/payload"), b"0123456789").unwrap();
        }
        fs::write(generations.join("gen.aaa/.previous"), "none\n").unwrap();
        fs::write(generations.join("gen.bbb/.previous"), "gen.aaa\n").unwrap();
        fs::create_dir_all(generations.join("not-a-generation")).unwrap();
        fs::create_dir_all(generations.join("gen.ccc")).unwrap();
        // A symlinked generation is ignored, never followed.
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("big"), vec![0u8; 4096]).unwrap();
        std::os::unix::fs::symlink(&outside, generations.join("gen.ddd")).unwrap();
        std::os::unix::fs::symlink("generations/gen.bbb", root.join("current")).unwrap();

        let listed = super::generations(&root);
        assert_eq!(
            listed
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["gen.aaa", "gen.bbb", "gen.ccc"]
        );
        let aaa = listed.iter().find(|item| item.name == "gen.aaa").unwrap();
        assert!(aaa.referenced, "the rollback target stays referenced");
        assert_eq!(aaa.files, 2, "payload plus .previous");
        // "none\n" (5) + the nested payload (10).
        assert_eq!(aaa.bytes, 5 + 10);
        let bbb = listed.iter().find(|item| item.name == "gen.bbb").unwrap();
        assert!(bbb.referenced, "the selected generation stays referenced");
        assert_eq!(bbb.files, 2);
        let ccc = listed.iter().find(|item| item.name == "gen.ccc").unwrap();
        assert!(!ccc.referenced);
        assert_eq!((ccc.files, ccc.bytes), (0, 0));
        assert!(listed.iter().all(|item| !item.truncated));
        // Sizes come from the real tree and nothing is modified.
        assert_eq!(
            fs::read(generations.join("gen.aaa/nested/payload")).unwrap(),
            b"0123456789"
        );
        assert!(outside.join("big").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backups_text_is_count_only_or_fixed_failure() {
        let root = temp("backups");
        assert_eq!(backups_text(&root.join("absent")), "count=0 bytes=0");
        assert!(backups_text(&root).starts_with("count="));
        // A file where the root should be is reported, not followed.
        let file = root.join("file");
        fs::write(&file, b"x").unwrap();
        let text = backups_text(&file);
        assert!(text.starts_with("unavailable"));
        assert!(!text.contains(root.to_string_lossy().as_ref()));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod report_tests {
    use super::*;

    fn report_with(webkit: &str, server: Option<u16>) -> Report {
        Report {
            client_version: "0.0.0".into(),
            runtime: None,
            server_url: "http://127.0.0.1:9/".into(),
            server_status: server,
            service_active: false,
            unit: "/tmp/unit".into(),
            config: "/tmp/config".into(),
            runtime_state: RuntimeSummary::default(),
            backups: "count=0 bytes=0".into(),
            storage: crate::session_inventory::Summary::default(),
            webkit_abi: crate::webkit_abi::Versions {
                gtk: Some("3.24.52".into()),
                webkit: Some("2.48.0".into()),
                status: webkit.into(),
            },
            runtime_generations: Vec::new(),
            config_report: crate::config::analyze("/x/config.json", None, None),
            launch_token: TokenState::Absent,
        }
    }

    #[test]
    fn strictness_only_fails_for_unusable_webkit_or_an_unreachable_server() {
        assert!(!report_with("ok", Some(401)).has_blocking_problem());
        assert!(report_with("ok", None).has_blocking_problem());
        assert!(report_with("missing", Some(200)).has_blocking_problem());
        assert!(report_with("too old", Some(200)).has_blocking_problem());
        assert!(!report_with("unknown", Some(200)).has_blocking_problem());
    }

    #[test]
    fn token_states_are_classified_without_network_access() {
        assert_eq!(classify_token(None, Some(303)), TokenState::Absent);
        assert_eq!(classify_token(Some(""), Some(303)), TokenState::Rejected);
        assert_eq!(classify_token(Some("t"), Some(303)), TokenState::Valid);
        assert_eq!(classify_token(Some("t"), Some(401)), TokenState::Rejected);
        assert_eq!(classify_token(Some("t"), Some(200)), TokenState::Rejected);
        assert_eq!(classify_token(Some("t"), None), TokenState::Unknown);
        assert!(TokenState::Unknown.text().contains("did not answer"));
    }

    #[test]
    fn text_and_json_carry_the_same_facts() {
        let report = report_with("ok", Some(401));
        let text = report.text();
        assert!(
            text.contains("server:  http://127.0.0.1:9/ (HTTP Some(401))"),
            "{text}"
        );
        assert!(text.contains("webkit abi:"), "{text}");
        let json = report.to_json();
        assert_eq!(json["serverStatus"], 401);
        assert_eq!(json["webkitAbi"]["status"], "ok");
        assert_eq!(json["clientVersion"], "0.0.0");
        assert_eq!(json["runtimeState"]["generations"], 0);
    }
}
