use std::net::IpAddr;

use actix_web::HttpRequest;
use subtle::ConstantTimeEq;

use crate::config::AppConfig;

pub fn validate_callback(req: &HttpRequest, config: &AppConfig) -> bool {
    if !config.is_production() {
        return true;
    }

    if let Some(key) = req.headers().get("x-at-api-key") {
        if let Ok(value) = key.to_str() {
            if ct_eq_str(value.trim(), config.at_api_key.trim()) {
                return true;
            }
        }
    }

    let Some(peer_ip) = req.peer_addr().map(|a| a.ip()) else {
        return false;
    };

    let client_ip = resolve_client_ip(
        peer_ip,
        req.headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok()),
        &config.trusted_proxy_ips,
    );

    config
        .at_callback_allowed_ips
        .iter()
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .any(|allowed| allowed == client_ip)
}

pub fn ct_eq_str(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub fn resolve_client_ip(
    peer_ip: IpAddr,
    xff_header: Option<&str>,
    trusted_proxy_ips: &[IpAddr],
) -> IpAddr {
    if trusted_proxy_ips.iter().any(|ip| ip == &peer_ip) {
        xff_header
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
            .unwrap_or(peer_ip)
    } else {
        peer_ip
    }
}
