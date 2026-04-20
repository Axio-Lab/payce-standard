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

    if !config.at_callback_url_key.is_empty() {
        let expected = config.at_callback_url_key.trim();
        if let Some(provided) = req.match_info().get("at_key") {
            if ct_eq_str(provided.trim(), expected) {
                return true;
            }
        }
        let q = req.uri().query().unwrap_or_else(|| req.query_string());
        if !q.is_empty() {
            if let Some(provided) = q.split('&').find_map(|kv| {
                let mut it = kv.splitn(2, '=');
                match (it.next(), it.next()) {
                    (Some("key"), Some(v)) => Some(v),
                    _ => None,
                }
            }) {
                if ct_eq_str(provided.trim(), expected) {
                    return true;
                }
            }
        }
    }

    let Some(client_ip) = client_ip(req, config) else {
        return false;
    };

    config
        .at_callback_allowed_ips
        .iter()
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .any(|allowed| allowed == client_ip)
}

pub fn client_ip(req: &HttpRequest, config: &AppConfig) -> Option<IpAddr> {
    let peer_ip = req.peer_addr().map(|a| a.ip());

    if config.trust_proxy_xff {
        let xff_first = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<IpAddr>().ok());
        xff_first.or(peer_ip)
    } else {
        peer_ip
    }
}

pub fn ct_eq_str(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
