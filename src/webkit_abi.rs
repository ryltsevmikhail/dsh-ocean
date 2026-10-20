//! Runtime GTK/WebKitGTK ABI probe for diagnostics.
//!
//! The client is built against WebKitGTK 4.1 with the `v2_40` feature, but the
//! shared libraries it loads are whatever the host provides at run time. This
//! module reports the loaded major/minor versions without creating a window or
//! touching any user data, so a support log can explain a startup failure.
//!
//! Probing uses `dlopen`/`dlsym` only; it never initialises GTK or WebKit.
use std::ffi::CString;

use libc::{c_int, c_void, RTLD_LOCAL, RTLD_NOW};
use serde::Serialize;

/// Minimum WebKitGTK minor version this client is built for (`v2_40`).
pub const REQUIRED_MINOR: u32 = 40;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Versions {
    pub gtk: Option<String>,
    pub webkit: Option<String>,
    /// `ok`, `too old`, `missing` or `unknown`.
    pub status: String,
}

impl Versions {
    /// Fixed, count-free line for `--doctor`.
    pub fn text(&self) -> String {
        let gtk = self.gtk.as_deref().unwrap_or("unavailable");
        let webkit = self.webkit.as_deref().unwrap_or("unavailable");
        format!("gtk={gtk} webkit2gtk-4.1={webkit} ({})", self.status)
    }
}

/// Reads `(major, minor, micro)` from `lib` using the given symbol prefix.
fn triple(lib: &str, prefix: &str, names: [&str; 3]) -> Option<(u32, u32, u32)> {
    let library = CString::new(lib).ok()?;
    // SAFETY: dlopen/dlsym are called with valid C strings; the handle is closed
    // before returning and the symbols are only read as plain u32 returns.
    unsafe {
        let handle = libc::dlopen(library.as_ptr(), RTLD_NOW | RTLD_LOCAL);
        if handle.is_null() {
            return None;
        }
        let mut values = [0u32; 3];
        for (index, suffix) in names.iter().enumerate() {
            let symbol_name = CString::new(format!("{prefix}{suffix}")).ok()?;
            let symbol = libc::dlsym(handle, symbol_name.as_ptr());
            if symbol.is_null() {
                libc::dlclose(handle);
                return None;
            }
            // SAFETY: these GTK/WebKit getters take no arguments and return an
            // `unsigned int`; calling the resolved address is the documented ABI.
            let function: extern "C" fn() -> c_int =
                std::mem::transmute::<*mut c_void, extern "C" fn() -> c_int>(symbol);
            let value = function();
            if value < 0 {
                libc::dlclose(handle);
                return None;
            }
            values[index] = value as u32;
        }
        libc::dlclose(handle);
        Some((values[0], values[1], values[2]))
    }
}

fn format_triple(value: (u32, u32, u32)) -> String {
    format!("{}.{}.{}", value.0, value.1, value.2)
}

/// Version of GTK 3 actually loadable on this host.
pub fn gtk_version() -> Option<String> {
    triple(
        "libgtk-3.so.0",
        "gtk_get_",
        ["major_version", "minor_version", "micro_version"],
    )
    .map(format_triple)
}

/// Version of WebKitGTK 4.1 actually loadable on this host.
pub fn webkit_version() -> Option<(u32, u32, u32)> {
    triple(
        "libwebkit2gtk-4.1.so.0",
        "webkit_get_",
        ["major_version", "minor_version", "micro_version"],
    )
}

/// Classifies a WebKit version against the built feature level.
pub fn classify(webkit: Option<(u32, u32, u32)>) -> &'static str {
    match webkit {
        None => "missing",
        Some((2, minor, _)) if minor >= REQUIRED_MINOR => "ok",
        Some((2, _, _)) => "too old",
        Some(_) => "unknown",
    }
}

/// Full probe used by `--doctor`.
pub fn probe() -> Versions {
    let webkit = webkit_version();
    Versions {
        gtk: gtk_version(),
        webkit: webkit.map(format_triple),
        status: classify(webkit).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_built_feature_level() {
        assert_eq!(classify(None), "missing");
        assert_eq!(classify(Some((2, REQUIRED_MINOR, 0))), "ok");
        assert_eq!(classify(Some((2, REQUIRED_MINOR + 5, 1))), "ok");
        assert_eq!(classify(Some((2, REQUIRED_MINOR - 1, 9))), "too old");
        assert_eq!(classify(Some((1, 40, 0))), "unknown");
        assert_eq!(classify(Some((3, 0, 0))), "unknown");
    }

    #[test]
    fn text_is_fixed_and_never_panics_without_libraries() {
        let empty = Versions::default();
        let text = empty.text();
        assert!(text.contains("gtk=unavailable"));
        assert!(text.contains("webkit2gtk-4.1=unavailable"));
        let full = Versions {
            gtk: Some("3.24.52".into()),
            webkit: Some("2.48.0".into()),
            status: "ok".into(),
        };
        assert_eq!(full.text(), "gtk=3.24.52 webkit2gtk-4.1=2.48.0 (ok)");
    }

    #[test]
    fn probe_reports_this_host_without_aborting() {
        // Either the shared libraries are present (desktop/CI) or the probe
        // degrades to "missing"; both must produce a usable line.
        let versions = probe();
        let text = versions.text();
        assert!(text.contains("webkit2gtk-4.1="));
        assert!(["ok", "too old", "missing", "unknown"].contains(&versions.status.as_str()));
        if versions.webkit.is_some() {
            assert_eq!(versions.status, "ok", "{text}");
            assert!(versions.gtk.is_some(), "{text}");
        }
    }
}
