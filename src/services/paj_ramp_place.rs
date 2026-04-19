use deadpool_postgres::Pool;
use reqwest::Client;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::paj_notify::notify_paj_offramp_deposit_tx;
use crate::services::paj_orders::{insert_paj_order, PajOrderDirection};
use crate::services::paj_ramp::{
    create_offramp_order, create_onramp_order, paj_is_configured, CreateOfframpOrderBody,
    CreateOnrampOrderBody, PajRampOrderResponse,
};
use crate::services::solana_rpc::SolanaRpc;
use crate::services::transfer::settle_paj_offramp_user_deposit;
pub fn ramp_orders_supported(config: &AppConfig) -> bool {
    paj_is_configured(config) && !config.payce_public_base_url.trim().is_empty()
}

pub async fn place_paj_offramp_order(
    pool: &Pool,
    rpc: &SolanaRpc,
    http: &Client,
    config: &AppConfig,
    user_id: Uuid,
    session_token: &str,
    bank_id: String,
    account_number: String,
    currency: String,
    mint: String,
    chain: Option<String>,
    amount: Option<f64>,
    fiat_amount: Option<f64>,
    fee: Option<f64>,
    description: Option<String>,
) -> Result<(Uuid, PajRampOrderResponse, Option<String>), String> {
    let has_token = amount.is_some();
    let has_fiat = fiat_amount.is_some();
    if has_token == has_fiat {
        return Err("Provide exactly one of amount (token) or fiatAmount".into());
    }
    let order_id = Uuid::new_v4();
    let webhook_url = config
        .paj_webhook_order_url(&order_id)
        .ok_or_else(|| "PAYCE_PUBLIC_BASE_URL is not set; cannot build webhook URL".to_string())?;
    let chain_str = chain.unwrap_or_else(|| "SOLANA".into());
    let currency_up = currency.trim().to_uppercase();
    let body = CreateOfframpOrderBody {
        bank: bank_id.trim().to_string(),
        account_number: account_number.trim().to_string(),
        currency: currency_up.clone(),
        amount,
        fiat_amount,
        mint: mint.trim().to_string(),
        chain: chain_str.clone(),
        webhook_url,
        description,
        business_usdc_fee: Some(fee.unwrap_or(config.paj_ramp_business_usdc_fee)),
    };
    let req_json = serde_json::to_value(&body).map_err(|e| e.to_string())?;
    let paj_resp = create_offramp_order(http, config, session_token, body).await?;
    let resp_json = serde_json::to_value(&paj_resp).map_err(|e| e.to_string())?;

    let deposit_addr = paj_resp
        .address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "PAJ did not return deposit address.".to_string())?;
    let token_amt = paj_resp
        .amount
        .filter(|a| *a > 0.0 && a.is_finite())
        .ok_or_else(|| "PAJ did not return token amount.".to_string())?;
    let quote_fee = paj_resp.fee.unwrap_or(0.0).max(0.0);
    let mint_for_settle = paj_resp
        .mint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| mint.trim().to_string());

    insert_paj_order(
        pool,
        order_id,
        user_id,
        PajOrderDirection::Offramp,
        Some(paj_resp.id.as_str()),
        paj_resp.mint.as_deref(),
        Some(chain_str.as_str()),
        Some(currency_up.as_str()),
        &req_json,
        &resp_json,
    )
    .await?;

    let settle_sig = settle_paj_offramp_user_deposit(
        pool,
        rpc,
        config,
        user_id,
        deposit_addr,
        &mint_for_settle,
        token_amt,
        quote_fee,
    )
    .await
    .map_err(|e| {
        log::error!(
            "[PAJ offramp] on-chain settlement failed order_id={order_id} paj_id={} user={user_id}: {e}",
            paj_resp.id
        );
        format!("Order recorded but token transfer failed: {e}")
    })?;

    let pool_n = pool.clone();
    let config_n = config.clone();
    let sig_for_notify = settle_sig.clone();
    tokio::spawn(async move {
        notify_paj_offramp_deposit_tx(&pool_n, &config_n, user_id, order_id, &sig_for_notify).await;
    });

    Ok((order_id, paj_resp, Some(settle_sig)))
}

pub async fn place_paj_onramp_order(
    pool: &Pool,
    _rpc: &SolanaRpc,
    http: &Client,
    config: &AppConfig,
    user_id: Uuid,
    session_token: &str,
    recipient: String,
    currency: String,
    mint: String,
    chain: Option<String>,
    amount: Option<f64>,
    fiat_amount: Option<f64>,
    fee: Option<f64>,
) -> Result<(Uuid, PajRampOrderResponse, Option<String>), String> {
    let has_token = amount.is_some();
    let has_fiat = fiat_amount.is_some();
    if has_token == has_fiat {
        return Err("Provide exactly one of amount or fiatAmount".into());
    }
    let order_id = Uuid::new_v4();
    let webhook_url = config
        .paj_webhook_order_url(&order_id)
        .ok_or_else(|| "PAYCE_PUBLIC_BASE_URL is not set; cannot build webhook URL".to_string())?;
    let chain_str = chain.unwrap_or_else(|| "SOLANA".into());
    let currency_s = currency.trim().to_string();
    let body = CreateOnrampOrderBody {
        amount,
        fiat_amount,
        currency: currency_s.clone(),
        recipient: recipient.trim().to_string(),
        mint: mint.trim().to_string(),
        chain: chain_str.clone(),
        webhook_url,
        business_usdc_fee: Some(fee.unwrap_or(config.paj_ramp_business_usdc_fee)),
    };
    let req_json = serde_json::to_value(&body).map_err(|e| e.to_string())?;
    let paj_resp = create_onramp_order(http, config, session_token, body).await?;
    let resp_json = serde_json::to_value(&paj_resp).map_err(|e| e.to_string())?;
    insert_paj_order(
        pool,
        order_id,
        user_id,
        PajOrderDirection::Onramp,
        Some(paj_resp.id.as_str()),
        paj_resp.mint.as_deref(),
        Some(chain_str.as_str()),
        paj_resp.currency.as_deref().or(Some(currency_s.as_str())),
        &req_json,
        &resp_json,
    )
    .await?;
    Ok((order_id, paj_resp, None))
}
