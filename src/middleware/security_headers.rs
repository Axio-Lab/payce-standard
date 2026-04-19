use actix_web::middleware::DefaultHeaders;

use crate::config::AppConfig;

pub fn secure_default_headers(config: &AppConfig) -> DefaultHeaders {
    let mut headers = DefaultHeaders::new()
        .add(("X-Content-Type-Options", "nosniff"))
        .add(("X-Frame-Options", "DENY"))
        .add(("Referrer-Policy", "strict-origin-when-cross-origin"))
        .add(("Cross-Origin-Opener-Policy", "same-origin"))
        .add(("Cross-Origin-Resource-Policy", "same-site"))
        .add(("Permissions-Policy", "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()"));

    if config.is_production() {
        headers = headers.add((
            "Strict-Transport-Security",
            "max-age=63072000; includeSubDomains; preload",
        ));
    }

    headers
}
