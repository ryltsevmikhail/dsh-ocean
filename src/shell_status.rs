//! Shell installation hints, deliberately not package ownership or update
//! authority. No package-manager/network/profile/service probing occurs here.
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Hint {
    AppImageEnvironment,
    FlatpakEnvironment,
    DevelopmentPath,
    SystemPrefix,
    Conflicting,
    Unknown,
}
impl Hint {
    fn label(self) -> &'static str {
        match self {
            Self::AppImageEnvironment => "AppImage environment (unverified)",
            Self::FlatpakEnvironment => "Flatpak environment (unverified)",
            Self::DevelopmentPath => "Development path (heuristic)",
            Self::SystemPrefix => "System prefix (ownership unknown)",
            Self::Conflicting => "Conflicting hints (installation unknown)",
            Self::Unknown => "Unknown installation",
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub shell_version: &'static str,
    pub installation_hint: Hint,
    pub ownership_verified: bool,
    pub automatic_update_available: bool,
    pub production_signed_feed_configured: bool,
    pub evidence: &'static str,
    pub guidance: &'static str,
    pub limits: &'static str,
}
pub fn classify(executable: Option<&Path>, appimage_env: bool, flatpak_env: bool) -> Report {
    let dev = executable.is_some_and(|path| {
        let parts = path
            .components()
            .map(|part| part.as_os_str())
            .collect::<Vec<_>>();
        parts.len() >= 3
            && parts[parts.len() - 3] == "target"
            && (parts[parts.len() - 2] == "debug" || parts[parts.len() - 2] == "release")
            && parts[parts.len() - 1] == "dsh-ocean"
    });
    let system = executable.is_some_and(|path| {
        path.starts_with("/usr/bin")
            || path.starts_with("/usr/local/bin")
            || path.starts_with("/opt")
    });
    let hint =
        if (appimage_env && flatpak_env) || ((dev || system) && (appimage_env || flatpak_env)) {
            Hint::Conflicting
        } else if appimage_env {
            Hint::AppImageEnvironment
        } else if flatpak_env {
            Hint::FlatpakEnvironment
        } else if dev {
            Hint::DevelopmentPath
        } else if system {
            Hint::SystemPrefix
        } else {
            Hint::Unknown
        };
    let (evidence,guidance)=match hint {
        Hint::AppImageEnvironment => ("APPIMAGE environment hint only; image identity was not verified.", "If this shell was installed as an AppImage, use its original distribution method. Verify independently authenticated signed release metadata and final bytes before any manual replacement. Offline verification/staging is not installation."),
        Hint::FlatpakEnvironment => ("FLATPAK_ID environment hint only; this application's package ownership was not verified.", "If installed through Flatpak, use that Flatpak installation's update mechanism. Do not replace files inside a sandbox/container manually."),
        Hint::DevelopmentPath => ("Executable path resembles a local Cargo target build; this is a path heuristic.", "For a development build, update the trusted checkout and rebuild. This screen neither pulls source nor overwrites the running executable."),
        Hint::SystemPrefix => ("Executable is under a conventional system prefix; package ownership was not queried.", "Use the original installer/package manager. For AUR/Arch use your package workflow; for deb/rpm use that package manager. Do not overwrite a package-owned executable with an AppImage/tarball. Confirm ownership independently."),
        Hint::Conflicting => ("Conflicting inherited environment/path hints; install type is not established.", "Determine the actual installation source before updating. Use its original package manager or distribution method; do not infer ownership from environment variables."),
        Hint::Unknown => ("No reliable installation context hint is available.", "Determine the original installation method and use its update workflow. No automatic shell replacement is offered for an unclassified installation."),
    };
    Report {shell_version:env!("CARGO_PKG_VERSION"),installation_hint:hint,ownership_verified:false,automatic_update_available:false,production_signed_feed_configured:false,evidence,guidance,
        limits:"Shell updates are separate from npm/DSH core runtime updates. This is read-only guidance, not a network update check, verified package identity or permission to apply an update. No files, services, keys or processes are changed."}
}
pub fn detect() -> Report {
    let executable = std::env::current_exe().ok();
    let present = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    classify(
        executable.as_deref(),
        present("APPIMAGE"),
        present("FLATPAK_ID"),
    )
}
/// Localized native presentation only; classification and machine-readable fields stay unchanged.
pub fn introduction_text_in(report: &Report, language: crate::i18n::Language) -> String {
    let hint = match report.installation_hint {
        Hint::AppImageEnvironment => "appimage",
        Hint::FlatpakEnvironment => "flatpak",
        Hint::DevelopmentPath => "development",
        Hint::SystemPrefix => "system",
        Hint::Conflicting => "conflicting",
        Hint::Unknown => "unknown",
    };
    let part = |suffix| {
        crate::i18n::message(language, &format!("installation.{hint}.{suffix}"))
            .expect("embedded installation guidance")
    };
    crate::i18n::render(
        language,
        "installation.introduction",
        &[
            ("version", report.shell_version),
            ("hint", part("label")),
            ("evidence", part("evidence")),
            ("guidance", part("guidance")),
        ],
    )
    .expect("validated installation hint presentation")
}

pub fn text(report: &Report) -> String {
    format!("DSH Ocean shell {}\n\nInstallation hint: {}\n{}\n\n{}\n\nAutomatic shell update: unavailable\nProduction signed feed: not configured\n\n{}",report.shell_version,report.installation_hint.label(),report.evidence,report.guidance,report.limits)
}
