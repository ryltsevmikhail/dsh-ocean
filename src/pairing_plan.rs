//! Read-only pairing/exposure plan.
//!
//! The client never changes the bind address, installs firewall rules or
//! restarts a service by itself. This module only explains what would have to
//! change, on which LAN address, and what the risk is — so a person can make the
//! decision with the exact command in front of them.
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

use serde::Serialize;
use url::Url;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Configured server origin.
    pub base_url: String,
    pub host: String,
    pub loopback: bool,
    /// LAN address a phone could use, when one can be discovered.
    pub lan_address: Option<String>,
    /// Exact service override a human would have to apply themselves.
    pub suggested_host_override: Option<String>,
    /// Suggested `serverUrl` value for the client config.
    pub suggested_client_url: Option<String>,
    pub exposure: String,
    pub warnings: Vec<String>,
    pub changed_anything: bool,
}

fn is_loopback(host: &str) -> bool {
    match host {
        "localhost" => true,
        other => other
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false),
    }
}

/// Discovers the outbound interface address without sending application traffic.
///
/// A UDP `connect` performs no handshake and transmits no payload; it only asks
/// the kernel which local address it would use. With no route to the probe
/// address the function returns `None` instead of guessing.
pub fn detect_lan_address() -> Option<IpAddr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // Documentation range (RFC 5737): never a real service.
    probe.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let address = probe.local_addr().ok()?.ip();
    if address.is_loopback() || address.is_unspecified() {
        return None;
    }
    Some(address)
}

pub fn plan(base: &Url, port: u16, lan: Option<IpAddr>) -> Plan {
    let host = base.host_str().unwrap_or("").to_owned();
    let loopback = is_loopback(&host);
    let lan_text = lan.map(|address| address.to_string());
    let (exposure, suggested_host_override, suggested_client_url) = if loopback {
        let trusted = lan_text.clone().map(|address| format!("{address}:{port}"));
        (
            "loopback-only: currently unreachable from another device".to_owned(),
            trusted
                .as_ref()
                .map(|trusted| format!("--host 0.0.0.0 --trusted-host {trusted}")),
            trusted.map(|trusted| format!("http://{trusted}/")),
        )
    } else {
        (
            "reachable from other devices on the network".to_owned(),
            None,
            None,
        )
    };
    let mut warnings = vec![
        "Anyone who can reach this server and obtain the token gets full control of the harness."
            .to_owned(),
        "Expose it only on a trusted network, and stop exposing it when you are done."
            .to_owned(),
        "This plan is read-only: no bind address, firewall rule, config value or service was changed."
            .to_owned(),
    ];
    if !loopback {
        warnings.push(
            "To revoke access again, restart the service bound to 127.0.0.1 and remove the LAN serverUrl."
                .to_owned(),
        );
    }
    if lan_text.is_none() {
        warnings.push(
            "No LAN address could be detected; choose the address of the interface you intend to use yourself."
                .to_owned(),
        );
    }
    Plan {
        base_url: base.to_string(),
        host,
        loopback,
        lan_address: lan_text,
        suggested_host_override,
        suggested_client_url,
        exposure,
        warnings,
        changed_anything: false,
    }
}

pub fn text(plan: &Plan) -> String {
    use std::fmt::Write as _;
    let mut output = String::new();
    let _ = writeln!(output, "Server: {}", plan.base_url);
    let _ = writeln!(output, "Exposure: {}", plan.exposure);
    if let Some(address) = &plan.lan_address {
        let _ = writeln!(output, "LAN address: {address}");
    }
    if let Some(override_flag) = &plan.suggested_host_override {
        let _ = writeln!(output, "Service override: {override_flag}");
    }
    if let Some(url) = &plan.suggested_client_url {
        let _ = writeln!(output, "Client serverUrl: {url}");
    }
    output.push_str("\nWarnings:\n");
    for warning in &plan.warnings {
        let _ = writeln!(output, "- {warning}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    #[test]
    fn loopback_server_gets_a_concrete_override_and_client_url() {
        let plan = plan(
            &url("http://127.0.0.1:3080/"),
            3080,
            Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))),
        );
        assert!(plan.loopback);
        assert_eq!(plan.lan_address.as_deref(), Some("192.168.1.20"));
        assert_eq!(
            plan.suggested_host_override.as_deref(),
            Some("--host 0.0.0.0 --trusted-host 192.168.1.20:3080")
        );
        assert_eq!(
            plan.suggested_client_url.as_deref(),
            Some("http://192.168.1.20:3080/")
        );
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("full control")));
        assert!(!plan.changed_anything);
        let rendered = text(&plan);
        assert!(rendered.contains("--host 0.0.0.0"));
        assert!(!rendered.contains("token="));
    }

    #[test]
    fn unknown_lan_address_never_invents_a_command() {
        let plan = plan(&url("http://127.0.0.1:3080/"), 3080, None);
        assert!(plan.loopback);
        assert!(plan.suggested_host_override.is_none());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("could be detected")));
    }

    #[test]
    fn exposed_server_reports_revocation_instead_of_a_switch_command() {
        let plan = plan(
            &url("http://192.168.1.5:3080/"),
            3080,
            Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))),
        );
        assert!(!plan.loopback);
        assert!(plan.suggested_host_override.is_none());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("revoke")));
        assert!(text(&plan).contains("reachable from other devices"));
    }

    #[test]
    fn ipv6_and_localhost_are_recognized_as_loopback() {
        for host in [
            "http://[::1]:3080/",
            "http://localhost:3080/",
            "http://127.0.0.2:3080/",
        ] {
            assert!(plan(&url(host), 3080, None).loopback, "{host}");
        }
        assert!(!plan(&url("http://10.0.0.7:3080/"), 3080, None).loopback);
    }
}
