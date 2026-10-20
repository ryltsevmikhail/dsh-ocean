use url::Url;

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    External,
    Reject,
}
fn local(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|ip| ip.is_loopback() || ip.is_unspecified())
        }
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.');
            host == "localhost" || host.ends_with(".localhost")
        }
        None => false,
    }
}
/// A different loopback port is a different application. Cookie host scoping
/// alone is insufficient: never navigate the authenticated webview there.
pub fn decide(url: &Url, server: &Url) -> Decision {
    if !url.username().is_empty() || url.password().is_some() {
        return Decision::Reject;
    }
    match url.scheme() {
        "http" | "https" if url.origin() == server.origin() => Decision::Allow,
        "http" | "https" if url.host_str() == server.host_str() || local(url) => Decision::Reject,
        "http" | "https" | "mailto" | "tel" => Decision::External,
        "blob" if url.origin() == server.origin() => Decision::Allow,
        "about" if url.as_str() == "about:blank" => Decision::Allow,
        _ => Decision::Reject,
    }
}
