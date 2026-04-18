//! Jupiter Swap API v2 — order → sign → execute.
//! See https://dev.jup.ag/docs/swap/v2/order-and-execute

use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::config::AppConfig;
use crate::services::utility_bill::{
    decode_transaction_ix_b64, sign_versioned_transaction_with_keypairs,
};

#[derive(Debug, Deserialize)]
pub struct JupiterOrderResponse {
    pub transaction: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "outAmount")]
    pub out_amount: Option<String>,
    pub router: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct JupiterExecuteResponse {
    pub status: String,
    pub signature: Option<String>,
    pub error: Option<String>,
}

pub async fn jupiter_order(
    http: &Client,
    config: &AppConfig,
    input_mint: &str,
    output_mint: &str,
    amount_raw: u64,
    taker: &str,
) -> Result<JupiterOrderResponse, String> {
    let api_key = config.jupiter_api_key.as_str();
    let referral = config.jupiter_referral_account.as_str();
    let payer_pk = config.fee_payer.pubkey().to_string();

    let base = config.jupiter_swap_base_url.trim_end_matches('/');
    let url = format!("{base}/order");
    let resp = http
        .get(&url)
        .header("x-api-key", api_key)
        .query(&[
            ("inputMint", input_mint),
            ("outputMint", output_mint),
            ("amount", &amount_raw.to_string()),
            ("taker", taker),
            ("referralAccount", referral),
            ("referralFee", &config.jupiter_referral_fee_bps.to_string()),
            ("payer", payer_pk.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Jupiter order HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Jupiter order body: {e}"))?;
    if !status.is_success() {
        let snippet = body.chars().take(400).collect::<String>();
        let tail = if body.len() > 400 { "…" } else { "" };
        return Err(format!("Jupiter order {status}: {snippet}{tail}"));
    }
    serde_json::from_str(&body).map_err(|e| {
        let preview: String = body.chars().take(200).collect();
        format!("Jupiter order JSON: {e}; body: {preview}")
    })
}

pub async fn jupiter_execute(
    http: &Client,
    config: &AppConfig,
    signed_transaction_b64: &str,
    request_id: &str,
) -> Result<JupiterExecuteResponse, String> {
    let api_key = config.jupiter_api_key.as_str();

    let base = config.jupiter_swap_base_url.trim_end_matches('/');
    let url = format!("{base}/execute");
    let resp = http
        .post(&url)
        .header("x-api-key", api_key)
        .json(&serde_json::json!({
            "signedTransaction": signed_transaction_b64,
            "requestId": request_id,
        }))
        .send()
        .await
        .map_err(|e| format!("Jupiter execute HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Jupiter execute body: {e}"))?;
    if !status.is_success() {
        let snippet = body.chars().take(400).collect::<String>();
        let tail = if body.len() > 400 { "…" } else { "" };
        return Err(format!("Jupiter execute {status}: {snippet}{tail}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("Jupiter execute JSON: {e}"))
}

pub fn sign_jupiter_order_transaction(
    order_tx_b64: &str,
    user: &Keypair,
    fee_payer: &Keypair,
) -> Result<String, String> {
    let mut vtx = decode_transaction_ix_b64(order_tx_b64)?;
    vtx = sign_versioned_transaction_with_keypairs(vtx, &[user, fee_payer])?;
    let raw = bincode::serialize(&vtx).map_err(|e| format!("serialize tx: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(raw))
}

pub async fn run_swap(
    http: &Client,
    config: &AppConfig,
    user_kp: &Keypair,
    input_mint: &str,
    output_mint: &str,
    amount_raw: u64,
) -> Result<String, String> {
    let taker = user_kp.pubkey().to_string();
    let order = jupiter_order(http, config, input_mint, output_mint, amount_raw, &taker).await?;
    let signed_b64 =
        sign_jupiter_order_transaction(&order.transaction, user_kp, &*config.fee_payer)?;
    let exec = jupiter_execute(http, config, &signed_b64, &order.request_id).await?;
    if exec.status.eq_ignore_ascii_case("Success") {
        exec.signature
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Jupiter: success but no signature".to_string())
    } else {
        Err(exec
            .error
            .unwrap_or_else(|| format!("Jupiter execute status: {}", exec.status)))
    }
}
