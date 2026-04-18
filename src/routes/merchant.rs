use actix_web::{web, HttpResponse};

use crate::services::merchant::get_merchant_by_code;
use crate::utils::validation::is_valid_merchant_code_param;

pub async fn lookup_merchant(
    path: web::Path<String>,
    pool: web::Data<deadpool_postgres::Pool>,
) -> HttpResponse {
    let code = path.into_inner();
    if !is_valid_merchant_code_param(&code) {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Invalid merchant code format",
        }));
    }
    match get_merchant_by_code(&pool, &code).await {
        Some(m) => HttpResponse::Ok().json(serde_json::json!({
            "merchantCode": m.merchant_code,
            "businessName": m.business_name,
            "category": m.category,
            "status": m.status,
        })),
        None => HttpResponse::NotFound().json(serde_json::json!({ "error": "Merchant not found" })),
    }
}

pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}
