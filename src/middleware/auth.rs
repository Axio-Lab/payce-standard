use actix_web::HttpRequest;

use crate::config::AppConfig;

pub fn validate_callback(req: &HttpRequest, config: &AppConfig) -> bool {
    if !config.is_production() {
        return true;
    }

    if let Some(key) = req.headers().get("x-at-api-key") {
        if key.to_str().unwrap_or("") == config.at_api_key {
            return true;
        }
    }

    let client_ip = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| {
            req.peer_addr()
                .map(|a| a.ip().to_string())
                .unwrap_or_default()
        });

    config
        .at_callback_allowed_ips
        .iter()
        .any(|ip| ip == &client_ip)
}
