//! Jupiter Lend Earn REST API (deposit / withdraw tx + read endpoints).
//! See https://developers.jup.ag/docs/lend/earn/api

use serde::Deserialize;
use serde_json::Value;
use solana_sdk::signature::Keypair;

use crate::config::AppConfig;
use crate::services::solana_rpc::SolanaRpc;
use crate::services::utility_bill::{
    decode_transaction_ix_b64, sign_versioned_transaction_with_keypairs,
};

#[derive(Debug, Clone, Deserialize)]
pub struct EarnTokenInfo {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
    #[serde(rename = "assetAddress")]
    pub asset_address: Option<String>,
    #[serde(rename = "supplyRate")]
    pub supply_rate: Option<Value>,
    #[serde(rename = "rewardsRate")]
    pub rewards_rate: Option<Value>,
    #[serde(rename = "totalRate")]
    pub total_rate: Option<Value>,
}

impl EarnTokenInfo {
    pub fn apy_raw_string(&self) -> Option<String> {
        Self::coerce_rate_value(&self.total_rate)
            .or_else(|| Self::coerce_rate_value(&self.supply_rate))
    }

    fn coerce_rate_value(v: &Option<Value>) -> Option<String> {
        match v {
            None => None,
            Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }
}

pub fn normalize_jupiter_apy_display(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('%');
    let Ok(mut v) = s.parse::<f64>() else {
        return raw.trim().to_string();
    };
    if v > 0.0 && v < 1.0 {
        v *= 100.0;
    } else if (100.0..100_000.0).contains(&v) {
        v /= 100.0;
    }
    format!("{:.2}", v)
}

#[derive(Debug, Clone, Deserialize)]
pub struct EarnPositionRow {
    pub address: String,
    #[serde(rename = "assetAddress")]
    pub asset_address: Option<String>,
    pub shares: String,
    #[serde(rename = "underlyingAssets")]
    pub underlying_assets: String,
    #[serde(rename = "underlyingBalance")]
    pub underlying_balance: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EarnEarningRow {
    pub address: String,
    #[serde(rename = "ownerAddress")]
    pub owner_address: String,
    pub earnings: Value,
    pub slot: u64,
}

#[derive(Debug, Deserialize)]
struct EarnTxResponse {
    transaction: String,
}

fn lend_base(config: &AppConfig) -> String {
    config
        .jupiter_lend_base_url
        .trim()
        .trim_end_matches('/')
        .to_string()
}

pub fn earn_fee_raw(base: u64, bps: u32) -> u64 {
    if base == 0 || bps == 0 {
        return 0;
    }
    let num = (base as u128) * (bps as u128);
    num.div_ceil(10_000) as u64
}

pub async fn fetch_earn_tokens(config: &AppConfig) -> Result<Vec<EarnTokenInfo>, String> {
    let base = lend_base(config);
    let url = format!("{base}/earn/tokens");
    let resp = config
        .http
        .get(&url)
        .header("x-api-key", config.jupiter_api_key.as_str())
        .send()
        .await
        .map_err(|e| format!("Jupiter Earn tokens HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Jupiter Earn tokens body: {e}"))?;
    if !status.is_success() {
        let snip: String = body.chars().take(300).collect();
        return Err(format!("Jupiter Earn tokens {status}: {snip}"));
    }
    parse_token_array(&body)
}

fn parse_token_array(body: &str) -> Result<Vec<EarnTokenInfo>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        format!(
            "Jupiter Earn tokens JSON: {e}; body: {}",
            &body[..body.len().min(200)]
        )
    })?;
    let arr = if let Some(a) = v.as_array() {
        a.clone()
    } else if let Some(a) = v.get("tokens").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        a.clone()
    } else {
        return Err("Jupiter Earn tokens: expected array or tokens[]".into());
    };
    let mut out = Vec::new();
    for item in arr {
        match serde_json::from_value::<EarnTokenInfo>(item) {
            Ok(t) => out.push(t),
            Err(e) => log::warn!("[JupiterEarn] skip token row: {e}"),
        }
    }
    Ok(out)
}

pub async fn fetch_earn_positions(
    config: &AppConfig,
    users: &[String],
) -> Result<Vec<EarnPositionRow>, String> {
    if users.is_empty() {
        return Ok(vec![]);
    }
    let base = lend_base(config);
    let url = format!("{base}/earn/positions");
    let joined = users.join(",");
    let resp = config
        .http
        .get(&url)
        .query(&[("users", joined.as_str())])
        .header("x-api-key", config.jupiter_api_key.as_str())
        .send()
        .await
        .map_err(|e| format!("Jupiter Earn positions HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Jupiter Earn positions body: {e}"))?;
    if !status.is_success() {
        let snip: String = body.chars().take(300).collect();
        return Err(format!("Jupiter Earn positions {status}: {snip}"));
    }
    let v: Value =
        serde_json::from_str(&body).map_err(|e| format!("Jupiter Earn positions JSON: {e}"))?;
    let arr = if let Some(a) = v.as_array() {
        a.clone()
    } else if let Some(a) = v.get("positions").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        a.clone()
    } else {
        return Err("Jupiter Earn positions: expected array".into());
    };
    let mut out = Vec::new();
    for item in arr {
        if let Ok(p) = serde_json::from_value::<EarnPositionRow>(item) {
            out.push(p);
        }
    }
    Ok(out)
}

pub async fn fetch_earn_earnings(
    config: &AppConfig,
    user: &str,
    position_addrs: &[String],
) -> Result<Vec<EarnEarningRow>, String> {
    if position_addrs.is_empty() {
        return Ok(vec![]);
    }
    let base = lend_base(config);
    let url = format!("{base}/earn/earnings");
    let positions = position_addrs.join(",");
    let resp = config
        .http
        .get(&url)
        .query(&[("user", user), ("positions", positions.as_str())])
        .header("x-api-key", config.jupiter_api_key.as_str())
        .send()
        .await
        .map_err(|e| format!("Jupiter Earn earnings HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Jupiter Earn earnings body: {e}"))?;
    if !status.is_success() {
        let snip: String = body.chars().take(300).collect();
        return Err(format!("Jupiter Earn earnings {status}: {snip}"));
    }
    let v: Value =
        serde_json::from_str(&body).map_err(|e| format!("Jupiter Earn earnings JSON: {e}"))?;
    let arr = if let Some(a) = v.as_array() {
        a.clone()
    } else if let Some(a) = v.get("earnings").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        a.clone()
    } else {
        return Err("Jupiter Earn earnings: expected array".into());
    };
    let mut out = Vec::new();
    for item in arr {
        if let Ok(e) = serde_json::from_value::<EarnEarningRow>(item) {
            out.push(e);
        }
    }
    Ok(out)
}

async fn post_earn_transaction(
    config: &AppConfig,
    path: &str,
    asset_mint: &str,
    amount_raw: &str,
    signer: &str,
) -> Result<String, String> {
    let base = lend_base(config);
    let url = format!("{base}/earn/{path}");
    let body = serde_json::json!({
        "asset": asset_mint,
        "amount": amount_raw,
        "signer": signer,
    });
    let resp = config
        .http
        .post(&url)
        .header("x-api-key", config.jupiter_api_key.as_str())
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Jupiter Earn {path} HTTP: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Jupiter Earn {path} body: {e}"))?;
    if !status.is_success() {
        let snip: String = text.chars().take(400).collect();
        return Err(format!("Jupiter Earn {path} {status}: {snip}"));
    }
    let parsed: EarnTxResponse = serde_json::from_str(&text).map_err(|e| {
        format!(
            "Jupiter Earn {path} JSON: {e}; {}",
            &text[..text.len().min(200)]
        )
    })?;
    Ok(parsed.transaction)
}

pub async fn earn_deposit_transaction_b64(
    config: &AppConfig,
    asset_mint: &str,
    amount_raw: u64,
    signer: &str,
) -> Result<String, String> {
    post_earn_transaction(
        config,
        "deposit",
        asset_mint,
        &amount_raw.to_string(),
        signer,
    )
    .await
}

pub async fn earn_withdraw_transaction_b64(
    config: &AppConfig,
    asset_mint: &str,
    amount_raw: u64,
    signer: &str,
) -> Result<String, String> {
    post_earn_transaction(
        config,
        "withdraw",
        asset_mint,
        &amount_raw.to_string(),
        signer,
    )
    .await
}

pub async fn sign_send_earn_versioned_tx(
    rpc: &SolanaRpc,
    config: &AppConfig,
    user: &Keypair,
    tx_b64: &str,
) -> Result<String, String> {
    let vtx = decode_transaction_ix_b64(tx_b64)?;
    let signed = sign_versioned_transaction_with_keypairs(vtx, &[user, &*config.fee_payer])?;
    rpc.send_versioned_transaction(&signed).await
}
