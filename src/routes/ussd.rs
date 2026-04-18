use actix_web::{web, HttpRequest, HttpResponse};
use serde::Deserialize;

use crate::config::AppConfig;
use crate::middleware::auth::validate_callback;
use crate::middleware::rate_limit::{is_ip_rate_limited, is_phone_rate_limited};
use crate::services::solana_rpc::SolanaRpc;
use crate::services::ussd_menu::handle_ussd_request;

const MAX_USSD_TEXT_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UssdCallback {
    pub session_id: Option<String>,
    pub phone_number: Option<String>,
    pub text: Option<String>,
    #[serde(rename = "serviceCode")]
    pub _service_code: Option<String>,
}

fn redact_text(text: &str) -> String {
    text.split('*')
        .map(|part| {
            if part.len() == 4 && part.chars().all(|c| c.is_ascii_digit()) {
                "****".to_string()
            } else if part.len() > 30 {
                format!("{}...REDACTED", &part[..6])
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("*")
}

pub async fn ussd_callback(
    req: HttpRequest,
    form: web::Form<UssdCallback>,
    pool: web::Data<deadpool_postgres::Pool>,
    redis: web::Data<redis::Client>,
    rpc: web::Data<SolanaRpc>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    if !validate_callback(&req, &config) {
        return HttpResponse::Forbidden().body("Forbidden");
    }

    let session_id = form.session_id.clone().unwrap_or_default();
    let phone_number = form.phone_number.clone().unwrap_or_default();
    let text = form.text.clone().unwrap_or_default();

    if text.len() > MAX_USSD_TEXT_BYTES {
        log::warn!(
            "[USSD] Rejected oversize text ({} bytes) session={}",
            text.len(),
            session_id
        );
        return HttpResponse::BadRequest()
            .content_type("text/plain")
            .body("END Request too long.");
    }

    log::debug!(
        "ussd session={} phone={} text=\"{}\"",
        session_id,
        phone_number,
        redact_text(&text)
    );

    if phone_number.is_empty() {
        return HttpResponse::BadRequest()
            .content_type("text/plain")
            .body("END Missing phoneNumber.");
    }

    let client_ip = req
        .peer_addr()
        .map(|a| a.ip().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "__unknown_ip".into());

    if is_phone_rate_limited(&redis, &phone_number, &config).await
        || is_ip_rate_limited(&redis, &client_ip, &config).await
    {
        return HttpResponse::Ok()
            .content_type("text/plain")
            .body("END Too many requests. Please wait a moment.");
    }

    let response = handle_ussd_request(&pool, &redis, &rpc, &config, &phone_number, &text).await;

    HttpResponse::Ok().content_type("text/plain").body(response)
}
