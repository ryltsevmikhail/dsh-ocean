use anyhow::{bail, Result};
use qrcodegen::{QrCode, QrCodeEcc};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::auth;
use crate::state::AppState;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteState {
    pub base_url: String,
    pub loopback: bool,
    pub pairing_url: Option<String>,
    pub qr_svg: Option<String>,
    pub note: String,
}

/// LAN authority a phone can use for an active proxy, if one is reachable.
///
/// The proxy must be bound to a non-loopback address and a LAN address must be
/// discoverable; anything else stays loopback-only rather than advertising a URL
/// that cannot work.
pub fn proxy_pairing_url(
    base: &url::Url,
    lan: Option<std::net::IpAddr>,
    proxy: Option<&crate::pairing_proxy::Status>,
) -> Option<url::Url> {
    let proxy = proxy?;
    if proxy.revoked || proxy.remaining_seconds == 0 {
        return None;
    }
    let listen: std::net::SocketAddr = proxy.listen.parse().ok()?;
    if listen.ip().is_loopback() {
        return None;
    }
    let lan = lan.filter(|address| !address.is_loopback() && !address.is_unspecified())?;
    let mut url = base.clone();
    // The phone talks to the LAN address and the proxy's port, not to loopback.
    url.set_host(Some(&lan.to_string())).ok()?;
    url.set_port(Some(listen.port())).ok()?;
    url.set_path("/");
    Some(url)
}

pub fn state(app: &AppHandle) -> Result<RemoteState> {
    let state = app.state::<AppState>();
    let server = state
        .server()
        .ok_or_else(|| anyhow::anyhow!("DSH is not running yet"))?;
    let host = server.base_url.host_str().unwrap_or("");
    let loopback = matches!(host, "127.0.0.1" | "127.0.0.2" | "localhost" | "::1");

    // A short-lived forwarder is the supported way to pair a phone: the server
    // itself keeps listening on loopback.
    let status = state.pairing_status();
    if loopback {
        if let Some(url) = proxy_pairing_url(
            &server.base_url,
            crate::pairing_plan::detect_lan_address(),
            status.as_ref(),
        ) {
            let url = auth::token_url(&url, &server.token);
            let svg = qr_svg(url.as_str())?;
            let remaining = status
                .as_ref()
                .map(|status| status.remaining_seconds)
                .unwrap_or(0);
            return Ok(RemoteState {
                base_url: server.base_url.to_string(),
                loopback,
                pairing_url: Some(url.to_string()),
                qr_svg: Some(svg),
                note: temporary_note_in(remaining, crate::i18n::active_language()),
            });
        }
    }

    if loopback {
        return Ok(RemoteState {
            base_url: server.base_url.to_string(),
            loopback,
            pairing_url: None,
            qr_svg: None,
            note: crate::i18n::text("remote.loopback_note").to_string(),
        });
    }

    let url = auth::token_url(&server.base_url, &server.token);
    let svg = qr_svg(url.as_str())?;
    Ok(RemoteState {
        base_url: server.base_url.to_string(),
        loopback,
        pairing_url: Some(url.to_string()),
        qr_svg: Some(svg),
        note: crate::i18n::text("remote.direct_note").to_string(),
    })
}

#[tauri::command]
pub fn get_remote_state(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<RemoteState, String> {
    // Only the pairing window renders this state; the harness page must not be able
    // to learn the pairing URL and token shape from it.
    if window.label() != "remote" {
        return Err("remote state is only available to the pairing window".to_owned());
    }
    state(&app).map_err(|error| format!("{error:#}"))
}

pub fn open_window(app: &AppHandle) -> Result<()> {
    if let Some(window) = app.get_webview_window("remote") {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))?;
    tauri::WebviewWindowBuilder::new(app, "remote", tauri::WebviewUrl::App("remote.html".into()))
        .title(crate::i18n::text("remote.title"))
        .inner_size(540.0, 680.0)
        .min_inner_size(460.0, 540.0)
        .center()
        .icon(icon)?
        .build()?;
    Ok(())
}

fn temporary_note_in(remaining: u64, language: crate::i18n::Language) -> String {
    crate::i18n::render(
        language,
        "remote.active_note",
        &[("remaining", &remaining.to_string())],
    )
    .expect("validated temporary pairing safety note")
}
fn qr_svg(text: &str) -> Result<String> {
    qr_svg_in(text, crate::i18n::active_language())
}
fn qr_svg_in(text: &str, language: crate::i18n::Language) -> Result<String> {
    let label = gtk::glib::markup_escape_text(
        crate::i18n::message(language, "remote.qr_label").expect("embedded QR label"),
    );
    let code = QrCode::encode_text(text, QrCodeEcc::Medium)?;
    let size = code.size();
    let border = 4i32;
    let total = size + border * 2;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if code.get_module(x, y) {
                path.push_str(&format!("M{} {}h1v1h-1z", x + border, y + border));
            }
        }
    }
    if path.is_empty() {
        bail!("QR encoder produced an empty code");
    }
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total} {total}" shape-rendering="crispEdges" role="img" aria-label="{label}"><rect width="100%" height="100%" fill="#ffffff"/><path d="{path}" fill="#0b0d12"/></svg>"##
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_safety_notes_and_qr_label_follow_selected_language() {
        for (language, _) in crate::i18n::LANGUAGES {
            let note = temporary_note_in(57, language);
            assert!(note.contains("57"));
            for key in ["remote.loopback_note", "remote.direct_note"] {
                let value = crate::i18n::message(language, key).expect("embedded safety note");
                if language != crate::i18n::Language::En {
                    assert_ne!(
                        value,
                        crate::i18n::message(crate::i18n::Language::En, key).unwrap()
                    );
                }
            }
            let svg = qr_svg_in("http://127.0.0.1:9/?token=synthetic", language).unwrap();
            let label = gtk::glib::markup_escape_text(
                crate::i18n::message(language, "remote.qr_label").unwrap(),
            );
            assert!(svg.contains(&format!("aria-label=\"{label}\"")));
            assert!(!svg.contains("token=synthetic"));
        }
    }
    #[test]
    fn qr_svg_contains_a_path_and_viewbox() {
        let svg = qr_svg("http://192.168.1.2:3080/?token=test").expect("qr");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("viewBox=\"0 0 "));
        assert!(svg.contains("<path"));
    }
}

#[cfg(test)]
mod pairing_url_tests {
    use super::*;
    use std::net::IpAddr;

    fn status(listen: &str, remaining: u64, revoked: bool) -> crate::pairing_proxy::Status {
        crate::pairing_proxy::Status {
            listen: listen.to_owned(),
            target: "127.0.0.1:3080".to_owned(),
            ttl_seconds: 300,
            remaining_seconds: remaining,
            active_connections: 0,
            accepted: 0,
            revoked,
        }
    }

    fn lan(text: &str) -> Option<IpAddr> {
        text.parse().ok()
    }

    #[test]
    fn an_active_forwarder_yields_a_lan_url_with_the_proxy_port() {
        let base = url::Url::parse("http://127.0.0.1:3080/").unwrap();
        let url = proxy_pairing_url(
            &base,
            lan("192.168.1.50"),
            Some(&status("0.0.0.0:3091", 120, false)),
        )
        .expect("active forwarder yields a URL");
        assert_eq!(url.as_str(), "http://192.168.1.50:3091/");
        // The token is appended by the caller, never stored in the status.
        let with_token = auth::token_url(&url, "secret-token");
        assert!(with_token.as_str().contains("token=secret-token"));
        assert!(!format!("{:?}", status("0.0.0.0:3091", 120, false)).contains("secret"));
    }

    #[test]
    fn an_inactive_or_loopback_bound_forwarder_never_advertises_a_url() {
        let base = url::Url::parse("http://127.0.0.1:3080/").unwrap();
        assert!(proxy_pairing_url(&base, lan("192.168.1.50"), None).is_none());
        assert!(
            proxy_pairing_url(
                &base,
                lan("192.168.1.50"),
                Some(&status("0.0.0.0:3091", 0, false))
            )
            .is_none(),
            "an expired forwarder must not advertise"
        );
        assert!(proxy_pairing_url(
            &base,
            lan("192.168.1.50"),
            Some(&status("0.0.0.0:3091", 100, true))
        )
        .is_none());
        assert!(
            proxy_pairing_url(
                &base,
                lan("192.168.1.50"),
                Some(&status("127.0.0.1:3091", 100, false))
            )
            .is_none(),
            "a loopback-bound proxy is unreachable from a phone"
        );
    }

    #[test]
    fn without_a_lan_address_nothing_is_advertised() {
        let base = url::Url::parse("http://127.0.0.1:3080/").unwrap();
        assert!(
            proxy_pairing_url(&base, None, Some(&status("0.0.0.0:3091", 100, false))).is_none()
        );
        assert!(proxy_pairing_url(
            &base,
            lan("127.0.0.1"),
            Some(&status("0.0.0.0:3091", 100, false))
        )
        .is_none());
    }
}
