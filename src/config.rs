use std::ffi::{CStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

pub const APP_DIR: &str = "dsh-ocean";
pub const DEFAULT_SERVER: &str = "http://127.0.0.1:3080/";
pub const UNIT_NAME: &str = "dsh-web.service";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Optional override for the DSH web origin, e.g. a remote SSH tunnel.
    pub server_url: Option<String>,
    /// Hide primary windows on close (default true); false quits the client.
    pub close_to_tray: Option<bool>,
    /// npm dist-tag to offer in update checks: latest or next.
    pub update_channel: Option<String>,
    /// Legacy key retained for diagnosis only; the runtime manifest pins bootstrap.
    pub dsh_version: Option<String>,
}

fn absolute_path(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|path| path.is_absolute())
}

fn absolute_env_path(var: &str) -> Option<PathBuf> {
    absolute_path(std::env::var_os(var))
}

/// Reentrant account lookup: never changes HOME or trusts the working directory.
fn account_home() -> Option<PathBuf> {
    let mut size = 16 * 1024;
    while size <= 1024 * 1024 {
        let mut buffer = vec![0u8; size];
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: record, result and buffer are valid writable storage; the
        // returned string is copied before its backing buffer is dropped.
        let status = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE {
            size *= 2;
            continue;
        }
        if status != 0 || result.is_null() {
            return None;
        }
        // SAFETY: getpwuid_r succeeded and returned a non-null record.
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() {
            return None;
        }
        // SAFETY: pw_dir is a NUL-terminated string owned by buffer on success.
        let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes();
        return absolute_path(Some(OsString::from_vec(bytes.to_vec())));
    }
    None
}

pub fn home_dir() -> PathBuf {
    absolute_env_path("HOME")
        .or_else(account_home)
        .expect("cannot determine an absolute home directory; set HOME to an absolute directory")
}

pub fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    absolute_env_path(var).unwrap_or_else(|| home_dir().join(fallback))
}

/// Shared location for the session socket, lock and offline-restore guard.
pub fn runtime_dir() -> PathBuf {
    absolute_env_path("XDG_RUNTIME_DIR").unwrap_or_else(state_dir)
}

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join(APP_DIR)
}

pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state").join(APP_DIR)
}

pub fn data_dir() -> PathBuf {
    if let Some(directory) = absolute_env_path("DSH_OCEAN_DATA_DIR") {
        return directory;
    }
    xdg_dir("XDG_DATA_HOME", ".local/share").join(APP_DIR)
}

pub fn diagnostics_dir() -> PathBuf {
    state_dir().join("diagnostics")
}

pub fn profile_backup_dir() -> PathBuf {
    state_dir().join("profile-backups")
}

pub fn dsh_home() -> PathBuf {
    absolute_env_path("DSH_HOME").unwrap_or_else(|| home_dir().join(".dsh"))
}

pub fn web_profile_dir() -> PathBuf {
    dsh_home().join("profiles").join("web")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn load() -> Config {
    load_checked().unwrap_or_default()
}

/// Absent configuration permits defaults; existing invalid/rejected files do not.
pub fn load_checked() -> anyhow::Result<Config> {
    let path = config_path();
    match crate::util::read_regular_text(&path, MAX_CONFIG_BYTES) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|_| anyhow::anyhow!("configuration is invalid; use --doctor to inspect it")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(_) => anyhow::bail!("configuration cannot be read safely; use --doctor to inspect it"),
    }
}

pub fn server_url() -> Url {
    checked_server_url()
        .unwrap_or_else(|_| Url::parse(DEFAULT_SERVER).expect("valid default server URL"))
}

/// Side-effecting commands must not act on loopback when an explicit override
/// is invalid. Doctor/display callers may still use the documented fallback.
pub fn checked_server_url() -> anyhow::Result<Url> {
    let candidate = match std::env::var("DSH_OCEAN_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => Some(value),
        None => load_checked()?.server_url,
    };
    match candidate {
        Some(value) => parse_server_origin(&value).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid configured server origin; use --doctor to inspect the configuration"
            )
        }),
        None => Ok(Url::parse(DEFAULT_SERVER).expect("valid default server URL")),
    }
}

/// Accept only a plain HTTP origin, matching the attach/probe transport.
/// Authentication is obtained separately, never from config.
fn parse_server_origin(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    ((url.scheme() == "http")
        && url.host_str().is_some()
        && url.port_or_known_default().is_some_and(|port| port != 0)
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

/// Support-report representation, also safe for a URL supplied outside config.
/// The URL origin excludes credentials, query, fragment and path.
pub fn server_origin_text(url: &Url) -> String {
    format!("{}/", url.origin().ascii_serialization())
}

pub fn server_port(url: &Url) -> u16 {
    url.port_or_known_default().unwrap_or(3080)
}

/// Where the effective server origin came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UrlSource {
    /// `DSH_OCEAN_URL` in the environment.
    Environment,
    /// `serverUrl` in `config.json`.
    ConfigFile,
    /// Built-in loopback default.
    Default,
}

/// Read-only configuration diagnosis for `--doctor`.
///
/// The client silently falls back to the loopback default when the config cannot
/// be read or parsed; this report makes that fallback visible instead of leaving a
/// user wondering why their override is ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub path: String,
    /// The file exists.
    pub exists: bool,
    /// The file could be read and parsed.
    pub valid: bool,
    /// `serverUrl` came from here.
    pub url_source: UrlSource,
    /// Effective origin after all fallbacks.
    pub effective_url: String,
    pub issues: Vec<String>,
}

/// Pure classification so tests never depend on the real home directory.
pub fn analyze(path: &str, text: Option<&str>, environment_override: Option<&str>) -> Report {
    let mut issues = Vec::new();
    let parsed: Option<Config> = match text {
        None => {
            issues.push("config.json is missing; defaults are in use".to_owned());
            None
        }
        Some(text) => match serde_json::from_str::<Config>(text) {
            Ok(config) => Some(config),
            Err(_) => {
                issues.push(
                    "config.json could not be parsed; every setting falls back to its default"
                        .to_owned(),
                );
                None
            }
        },
    };
    let configured = parsed.as_ref().and_then(|config| config.server_url.clone());
    let (url_source, candidate) =
        if let Some(value) = environment_override.filter(|value| !value.trim().is_empty()) {
            if configured.is_some() {
                issues.push("DSH_OCEAN_URL overrides the serverUrl from config.json".to_owned());
            }
            (UrlSource::Environment, Some(value.to_owned()))
        } else if let Some(value) = configured {
            (UrlSource::ConfigFile, Some(value))
        } else {
            (UrlSource::Default, None)
        };
    let effective = match candidate {
        Some(value) => match parse_server_origin(&value) {
            Some(url) => server_origin_text(&url),
            None => {
                issues.push(
                    "the configured server URL must be an HTTP origin without credentials, path, query or fragment and with a nonzero port; HTTPS/TLS is not supported by the connection probe; the default is used".to_owned(),
                );
                DEFAULT_SERVER.to_owned()
            }
        },
        None => DEFAULT_SERVER.to_owned(),
    };
    if parsed
        .as_ref()
        .and_then(|config| config.update_channel.as_deref())
        .is_some_and(|channel| channel != "latest" && channel != "next")
    {
        issues.push(
            "updateChannel is neither latest nor next; the default channel is used".to_owned(),
        );
    }
    if parsed
        .as_ref()
        .is_some_and(|config| config.dsh_version.is_some())
    {
        issues.push("dshVersion is unsupported and ignored; bootstrap uses the pinned runtime manifest version".to_owned());
    }
    Report {
        path: path.to_owned(),
        exists: text.is_some(),
        valid: parsed.is_some(),
        url_source,
        effective_url: effective,
        issues,
    }
}

/// Reads the live configuration and classifies it.
pub fn diagnose() -> Report {
    let path = config_path();
    let text = crate::util::read_regular_text(&path, MAX_CONFIG_BYTES).ok();
    let environment = std::env::var("DSH_OCEAN_URL").ok();
    if text.is_none() && fs::symlink_metadata(&path).is_ok() {
        // A rejected file is not absent, and must not hide the environment URL.
        let mut report = analyze(&path.display().to_string(), None, environment.as_deref());
        report.exists = true;
        report
            .issues
            .retain(|issue| issue != "config.json is missing; defaults are in use");
        report.issues.push("config.json could not be read safely; expected a readable, regular non-symlink UTF-8 file of at most 64 KiB".to_owned());
        return report;
    }
    analyze(
        &path.display().to_string(),
        text.as_deref(),
        environment.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_is_reported_but_uses_the_default() {
        let report = analyze("/x/config.json", None, None);
        assert!(!report.exists && !report.valid);
        assert_eq!(report.url_source, UrlSource::Default);
        assert_eq!(report.effective_url, DEFAULT_SERVER);
        assert!(report.issues.iter().any(|issue| issue.contains("missing")));
    }

    #[test]
    fn unparseable_config_is_reported_instead_of_silently_ignored() {
        let report = analyze("/x/config.json", Some("{not json"), None);
        assert!(report.exists);
        assert!(!report.valid);
        assert_eq!(report.effective_url, DEFAULT_SERVER);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("could not be parsed")));
    }

    #[test]
    fn a_valid_override_is_used_and_reported_as_such() {
        let report = analyze(
            "/x/config.json",
            Some("{\"serverUrl\":\"http://192.168.1.5:3080/\"}"),
            None,
        );
        assert!(report.valid);
        assert_eq!(report.url_source, UrlSource::ConfigFile);
        assert_eq!(report.effective_url, "http://192.168.1.5:3080/");
        assert!(report.issues.is_empty(), "{:?}", report.issues);
    }

    #[test]
    fn environment_wins_and_says_so() {
        let report = analyze(
            "/x/config.json",
            Some("{\"serverUrl\":\"http://192.168.1.5:3080/\"}"),
            Some("http://127.0.0.1:4000/"),
        );
        assert_eq!(report.url_source, UrlSource::Environment);
        assert_eq!(report.effective_url, "http://127.0.0.1:4000/");
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("overrides")));
    }

    #[test]
    fn authenticated_or_non_origin_urls_are_never_accepted_or_reported() {
        for value in [
            "http://127.0.0.1:3080/?token=private-secret",
            "http://private-secret@127.0.0.1:3080/",
            "https://user:private-secret@example.test/",
            "https://example.test/private-secret",
            "https://example.test/#private-secret",
            "http://127.0.0.1:0/",
            "file:///private-secret",
        ] {
            assert!(parse_server_origin(value).is_none(), "accepted {value}");
            let text = serde_json::json!({"serverUrl": value}).to_string();
            for override_value in [None, Some(value)] {
                let report = analyze("/config.json", Some(&text), override_value);
                assert_eq!(report.effective_url, DEFAULT_SERVER);
                assert!(!report.issues.is_empty());
                assert!(!serde_json::to_string(&report)
                    .unwrap()
                    .contains("private-secret"));
            }
        }
    }

    #[test]
    fn origin_display_removes_credentials_and_all_url_suffixes() {
        let url = Url::parse("https://user:private-secret@example.test:444/private-secret?token=private-secret#private-secret").unwrap();
        assert_eq!(server_origin_text(&url), "https://example.test:444/");
        for value in [
            "http://127.0.0.1:3080/",
            "http://example.test/",
            "http://[::1]:4000/",
        ] {
            assert_eq!(
                server_origin_text(&parse_server_origin(value).unwrap()),
                value
            );
        }
    }

    #[test]
    fn blank_environment_urls_do_not_override_the_configured_origin() {
        for value in ["", " ", "\t\n"] {
            let report = analyze(
                "/config.json",
                Some(r#"{"serverUrl":"http://127.0.0.1:4567/"}"#),
                Some(value),
            );
            assert_eq!(report.url_source, UrlSource::ConfigFile);
            assert_eq!(report.effective_url, "http://127.0.0.1:4567/");
            assert!(report.issues.is_empty());
        }
    }

    #[test]
    fn https_is_reported_as_unsupported_instead_of_accepted_but_unreachable() {
        let report = analyze(
            "/config.json",
            Some(r#"{"serverUrl":"https://example.test/"}"#),
            None,
        );
        assert!(parse_server_origin("https://example.test/").is_none());
        assert_eq!(report.effective_url, DEFAULT_SERVER);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("HTTPS/TLS is not supported")));
    }

    #[test]
    fn legacy_bootstrap_version_is_flagged_without_echoing_its_value() {
        let report = analyze(
            "/config.json",
            Some(r#"{"dshVersion":"private-secret"}"#),
            None,
        );
        assert!(report.valid);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("dshVersion is unsupported and ignored")));
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("private-secret"));
    }

    #[test]
    fn unusable_urls_and_channels_are_flagged() {
        for value in ["file:///etc/passwd", "http://[bad"] {
            let report = analyze(
                "/x/config.json",
                Some(&format!("{{\"serverUrl\":{value:?}}}")),
                None,
            );
            assert_eq!(report.effective_url, DEFAULT_SERVER, "{value}");
            assert!(!report.issues.is_empty(), "{value}");
        }
        let report = analyze(
            "/x/config.json",
            Some("{\"updateChannel\":\"nightly\"}"),
            None,
        );
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("updateChannel")));
    }
}
