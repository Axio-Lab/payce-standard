use actix_web::{web, HttpRequest, HttpResponse};
use deadpool_postgres::Pool;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::paj_notify::notify_paj_order_update;
use crate::services::paj_orders::{
    append_order_event, update_order_after_webhook, PajOrderDirection, PajOrderStatus,
};
use crate::services::paj_ramp::paj_is_configured;
use crate::services::paj_ramp_place::place_paj_offramp_order;
use crate::services::paj_ramp_place::place_paj_onramp_order;
use crate::services::paj_session::{load_usable_paj_session_token, PajSessionError};
use crate::services::solana_rpc::SolanaRpc;

fn check_internal_key(req: &HttpRequest, config: &AppConfig) -> bool {
    let key = config.payce_internal_api_key.trim();
    if key.is_empty() {
        return false;
    }
    req.headers()
        .get("X-Payce-Key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim() == key)
        .unwrap_or(false)
}

fn webhook_query_secret_ok(req: &HttpRequest, config: &AppConfig) -> bool {
    let expected = config.paj_webhook_secret.trim();
    if expected.is_empty() {
        return true;
    }
    req.uri()
        .query()
        .map(|q| {
            for pair in q.split('&') {
                let mut it = pair.splitn(2, '=');
                let k = it.next().unwrap_or("");
                let v = it.next().unwrap_or("");
                if k == "k" && v == expected {
                    return true;
                }
            }
            false
        })
        .unwrap_or(false)
}

fn extract_status_from_webhook(v: &Value) -> PajOrderStatus {
    let try_str = |node: &Value| node.as_str().map(PajOrderStatus::parse);
    let keys = [
        "status",
        "orderStatus",
        "state",
        "transactionStatus",
        "paymentStatus",
    ];
    for k in keys {
        if let Some(s) = v.get(k).and_then(|x| try_str(x)) {
            return s;
        }
        if let Some(s) = v
            .get("data")
            .and_then(|d| d.get(k))
            .and_then(|x| try_str(x))
        {
            return s;
        }
        if let Some(s) = v
            .get("order")
            .and_then(|o| o.get(k))
            .and_then(|x| try_str(x))
        {
            return s;
        }
    }
    PajOrderStatus::Unknown
}

fn extract_paj_order_id_from_webhook(v: &Value) -> Option<String> {
    v.get("id")
        .and_then(|x| x.as_str())
        .map(String::from)
        .or_else(|| v.get("orderId").and_then(|x| x.as_str()).map(String::from))
        .or_else(|| {
            v.get("data")
                .and_then(|d| d.get("id"))
                .and_then(|x| x.as_str())
                .map(String::from)
        })
}

fn direction_from_db(s: &str) -> PajOrderDirection {
    if s.eq_ignore_ascii_case("offramp") {
        PajOrderDirection::Offramp
    } else {
        PajOrderDirection::Onramp
    }
}

fn summary_snippet(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s
    }
}

pub async fn paj_webhook(
    path: web::Path<Uuid>,
    req: HttpRequest,
    body: web::Bytes,
    pool: web::Data<Pool>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let order_id = path.into_inner();
    if !webhook_query_secret_ok(&req, &config) {
        log::warn!("[PAJ webhook] bad secret for order {order_id}");
        return HttpResponse::Forbidden().json(serde_json::json!({ "ok": false }));
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "[PAJ webhook] invalid JSON for {order_id}: {e} body={}",
                truncate(&String::from_utf8_lossy(&body), 400)
            );
            return HttpResponse::BadRequest().json(serde_json::json!({ "ok": false }));
        }
    };
    log::info!(
        "[PAJ webhook] order={} payload={}",
        order_id,
        truncate(&payload.to_string(), 800)
    );

    let status = extract_status_from_webhook(&payload);
    let ext_id = extract_paj_order_id_from_webhook(&payload);

    let st = status.clone();
    let updated =
        match update_order_after_webhook(&pool, order_id, st.clone(), ext_id.as_deref(), &payload)
            .await
        {
            Ok(x) => x,
            Err(e) => {
                log::error!("[PAJ webhook] update: {e}");
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({ "ok": false }));
            }
        };

    if updated.is_none() {
        log::warn!("[PAJ webhook] unknown order_id={order_id}");
        return HttpResponse::NotFound().json(serde_json::json!({ "ok": false }));
    }

    if let Err(e) = append_order_event(&pool, order_id, &payload).await {
        log::error!("[PAJ webhook] append event: {e}");
    }

    if let Some((user_id, dir_s)) = updated {
        let dir = direction_from_db(&dir_s);
        notify_paj_order_update(
            &pool,
            &config,
            user_id,
            order_id,
            dir,
            &st,
            &summary_snippet(&payload),
        )
        .await;
    }

    HttpResponse::Ok().json(serde_json::json!({ "ok": true }))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[derive(Debug, Deserialize)]
pub struct OfframpCreateRequest {
    pub user_id: Uuid,
    pub bank_id: String,
    pub account_number: String,
    pub currency: String,
    pub mint: String,
    #[serde(default)]
    pub chain: Option<String>,
    pub amount: Option<f64>,
    #[serde(rename = "fiatAmount", default)]
    pub fiat_amount: Option<f64>,
    #[serde(default)]
    pub fee: Option<f64>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OnrampCreateRequest {
    pub user_id: Uuid,
    pub recipient: String,
    pub currency: String,
    pub mint: String,
    #[serde(default)]
    pub chain: Option<String>,
    pub amount: Option<f64>,
    #[serde(rename = "fiatAmount", default)]
    pub fiat_amount: Option<f64>,
    #[serde(default)]
    pub fee: Option<f64>,
}

pub async fn paj_offramp(
    req: HttpRequest,
    json: web::Json<OfframpCreateRequest>,
    pool: web::Data<Pool>,
    config: web::Data<AppConfig>,
    rpc: web::Data<SolanaRpc>,
) -> HttpResponse {
    if !check_internal_key(&req, &config) {
        return HttpResponse::Unauthorized().json(serde_json::json!({ "error": "Unauthorized" }));
    }
    if !config.payce_ramp_api_enabled() {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({ "error": "Ramp API not configured" }));
    }
    if !paj_is_configured(&config) {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({ "error": "PAJ not configured" }));
    }
    let j = json.into_inner();
    let has_token = j.amount.is_some();
    let has_fiat = j.fiat_amount.is_some();
    if has_token == has_fiat {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Provide exactly one of amount (token) or fiatAmount"
        }));
    }
    let session = match load_usable_paj_session_token(&pool, &j.user_id).await {
        Ok(t) => t,
        Err(e) => {
            return match e {
                PajSessionError::NotFound => {
                    HttpResponse::NotFound().json(serde_json::json!({ "error": e.to_string() }))
                }
                _ => HttpResponse::Forbidden().json(serde_json::json!({ "error": e.to_string() })),
            };
        }
    };
    let http = Client::new();
    let (order_id, paj_resp, settle_sig) = match place_paj_offramp_order(
        &pool,
        &rpc,
        &http,
        &config,
        j.user_id,
        &session,
        j.bank_id,
        j.account_number,
        j.currency,
        j.mint,
        j.chain,
        j.amount,
        j.fiat_amount,
        j.fee,
        j.description,
    )
    .await
    {
        Ok(x) => x,
        Err(e) => {
            log::warn!("[PAJ offramp API] {e}");
            return HttpResponse::BadGateway().json(serde_json::json!({ "error": e }));
        }
    };
    HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "orderId": order_id,
        "pajOrderId": paj_resp.id,
        "message": "Order created. You will receive an SMS and email shortly as the status updates.",
        "address": paj_resp.address,
        "amount": paj_resp.amount,
        "fiatAmount": paj_resp.fiat_amount,
        "rate": paj_resp.rate,
        "fee": paj_resp.fee,
        "mint": paj_resp.mint,
        "depositTxSignature": settle_sig,
    }))
}

pub async fn paj_onramp(
    req: HttpRequest,
    json: web::Json<OnrampCreateRequest>,
    pool: web::Data<Pool>,
    config: web::Data<AppConfig>,
    rpc: web::Data<SolanaRpc>,
) -> HttpResponse {
    if !check_internal_key(&req, &config) {
        return HttpResponse::Unauthorized().json(serde_json::json!({ "error": "Unauthorized" }));
    }
    if !config.payce_ramp_api_enabled() {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({ "error": "Ramp API not configured" }));
    }
    if !paj_is_configured(&config) {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({ "error": "PAJ not configured" }));
    }
    let j = json.into_inner();
    let has_token = j.amount.is_some();
    let has_fiat = j.fiat_amount.is_some();
    if has_token == has_fiat {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Provide exactly one of amount or fiatAmount"
        }));
    }
    let session = match load_usable_paj_session_token(&pool, &j.user_id).await {
        Ok(t) => t,
        Err(e) => {
            return match e {
                PajSessionError::NotFound => {
                    HttpResponse::NotFound().json(serde_json::json!({ "error": e.to_string() }))
                }
                _ => HttpResponse::Forbidden().json(serde_json::json!({ "error": e.to_string() })),
            };
        }
    };
    let http = Client::new();
    let (order_id, paj_resp, _) = match place_paj_onramp_order(
        &pool,
        &rpc,
        &http,
        &config,
        j.user_id,
        &session,
        j.recipient,
        j.currency,
        j.mint,
        j.chain,
        j.amount,
        j.fiat_amount,
        j.fee,
    )
    .await
    {
        Ok(x) => x,
        Err(e) => {
            log::warn!("[PAJ onramp API] {e}");
            return HttpResponse::BadGateway().json(serde_json::json!({ "error": e }));
        }
    };
    HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "orderId": order_id,
        "pajOrderId": paj_resp.id,
        "message": "Order created. You will receive an SMS and email shortly as the status updates.",
        "accountNumber": paj_resp.account_number,
        "accountName": paj_resp.account_name,
        "bank": paj_resp.bank,
        "amount": paj_resp.amount,
        "fiatAmount": paj_resp.fiat_amount,
        "rate": paj_resp.rate,
        "fee": paj_resp.fee,
        "recipient": paj_resp.recipient,
    }))
}
