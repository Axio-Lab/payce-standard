//! Jupiter Price API v3 — live USD mid for a mint (SOL leg of Lend & Earn USSD copy).

use reqwest::Client;
use serde_json::Value;

use crate::config::AppConfig;

pub async fn fetch_mint_usd_price(http: &Client, api_key: &str, mint: &str) -> Result<f64, String> {
    let url = format!("https://api.jup.ag/price/v3?ids={mint}");
    let mut req = http.get(&url);
    let key = api_key.trim();
    if !key.is_empty() {
        req = req.header("x-api-key", key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Jupiter price HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Jupiter price body: {e}"))?;
    if !status.is_success() {
        let snip: String = body.chars().take(200).collect();
        return Err(format!("Jupiter price {status}: {snip}"));
    }
    parse_usd_price_body(&body, mint)
        .map_err(|e| format!("Jupiter price JSON: {e}; {}", &body[..body.len().min(120)]))
}

pub async fn fetch_sol_usd_price(config: &AppConfig) -> Result<f64, String> {
    let mint = config.sol_mint.to_string();
    fetch_mint_usd_price(&config.http, &config.jupiter_api_key, &mint).await
}

pub fn parse_usd_price_body(body: &str, mint: &str) -> Result<f64, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let row = v
        .get(mint)
        .ok_or_else(|| format!("missing entry for {mint}"))?;
    row.get("usdPrice")
        .and_then(|p| p.as_f64())
        .filter(|p| p.is_finite() && *p > 0.0)
        .ok_or_else(|| format!("no usdPrice for {mint}"))
}
