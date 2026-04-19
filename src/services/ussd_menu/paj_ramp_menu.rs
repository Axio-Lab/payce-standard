use deadpool_postgres::Pool;
use redis::Client as RedisClient;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::paj_ramp::{
    list_banks, list_saved_bank_accounts, paj_is_configured, PajBank, PajSavedBankAccount,
};
use crate::services::paj_ramp_place::{
    place_paj_offramp_order, place_paj_onramp_order, ramp_orders_supported,
};
use crate::services::paj_session::{load_usable_paj_session_token, PajSessionError};
use crate::services::solana_rpc::SolanaRpc;
use crate::services::ussd_menu::pin_gate::verify_pin_or_fail;
use crate::services::ussd_menu::text::{sanitize_ussd_ascii, truncate_ussd_label};
use crate::services::ussd_menu::utility_catalog::{redis_delete_key, redis_load_json};
use redis::AsyncCommands;

const FLOW_TTL_SECS: u64 = 900;

fn offramp_flow_key(user_id: &str) -> String {
    format!("ussd:paj_offramp_flow:{user_id}")
}

fn onramp_flow_key(user_id: &str) -> String {
    format!("ussd:paj_onramp_flow:{user_id}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OfframpFlowState {
    #[serde(default)]
    mint: String,
    #[serde(default)]
    token_label: String,
    accounts: Vec<PajSavedBankAccount>,
    #[serde(default)]
    picked_idx: Option<usize>,
    #[serde(default)]
    pending_amount: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OnrampFlowState {
    mint: String,
    #[serde(default)]
    pending_ngn: Option<f64>,
}

async fn redis_store_offramp(
    redis: &RedisClient,
    key: &str,
    flow: &OfframpFlowState,
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let json = serde_json::to_string(flow).map_err(|e| format!("json: {e}"))?;
    conn.set_ex::<_, _, ()>(key, &json, FLOW_TTL_SECS)
        .await
        .map_err(|e| format!("redis set: {e}"))?;
    Ok(())
}

async fn redis_store_onramp(
    redis: &RedisClient,
    key: &str,
    flow: &OnrampFlowState,
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let json = serde_json::to_string(flow).map_err(|e| format!("json: {e}"))?;
    conn.set_ex::<_, _, ()>(key, &json, FLOW_TTL_SECS)
        .await
        .map_err(|e| format!("redis set: {e}"))?;
    Ok(())
}

fn session_gate_ussd(e: PajSessionError) -> String {
    match e {
        PajSessionError::NotVerified => {
            "CON Complete PAJ verification first.\nMy Account > 8 PAJ verification.\nThen try again."
                .into()
        }
        PajSessionError::NoToken | PajSessionError::Expired => {
            "CON PAJ session missing or expired.\nDial My Account > 8 to verify, then try again."
                .into()
        }
        PajSessionError::NotFound => "END Account not found.".into(),
        PajSessionError::Db(msg) => {
            log::error!("[PAJ ramp USSD] session db: {msg}");
            "END Service error. Try again later.".into()
        }
    }
}

fn mask_acct(s: &str) -> String {
    if s.len() <= 4 {
        "****".into()
    } else {
        format!("****{}", &s[s.len().saturating_sub(4)..])
    }
}

fn token_label_for_digit(digit: &str) -> &'static str {
    match digit.trim() {
        "1" => "USDC",
        "2" => "USDT",
        "3" => "USDG",
        "4" => "SOL",
        _ => "Token",
    }
}

fn mint_for_token_digit(config: &AppConfig, digit: &str) -> Option<String> {
    match digit.trim() {
        "1" => Some(config.usdc_mint.to_string()),
        "2" => config
            .stable_coins
            .iter()
            .find(|c| c.code.eq_ignore_ascii_case("USDT"))
            .map(|c| c.mint.to_string()),
        "3" => config
            .stable_coins
            .iter()
            .find(|c| c.code.eq_ignore_ascii_case("USDG"))
            .map(|c| c.mint.to_string()),
        "4" => Some(config.sol_mint.to_string()),
        _ => None,
    }
}

fn con_token_pick(title: &str, pick_line: &str) -> String {
    format!("CON {title}\n\n{pick_line}\n1.USDC\n2.USDT\n3.USDG\n4.SOL\n0. Back")
}

fn format_paj_place_err_ussd(e: &str) -> String {
    if e.contains("Failed to get token price") {
        "END PAJ could not price this token in NGN.".into()
    } else if e.contains("Can't find bank with id") {
        "END PAJ bank id mismatch. Re-save payout bank:\nMy Account > 7 > Add bank.".into()
    } else if e.contains("bank must be a mongodb id") {
        "END Bank reference is out of date. Re-save payout bank:\nMy Account > 7 > Add bank.".into()
    } else if e.contains("Insufficient USDC") && e.contains("deposit + fee") {
        "END Not enough USDC. Add USDC, then try offramp again.".into()
    } else if e.contains("Insufficient balance") && e.contains("network fee") {
        "END Not enough token or USDC for gas. Add funds, then try again.".into()
    } else if e.contains("Wallet not set up") || e.contains("wallet not set up") {
        "END Wallet not set up. Fund or import wallet first.".into()
    } else if e.contains("Rounded deposit amount is zero") {
        "END Amount too small after fees. Enter a larger amount.".into()
    } else if e.contains("Invalid offramp amount") {
        "END Invalid amount for this sell. Check and try again.".into()
    } else if e.contains("Order recorded but token transfer failed") {
        let tail = e
            .strip_prefix("Order recorded but token transfer failed: ")
            .unwrap_or(e);
        let clean = sanitize_ussd_ascii(tail);
        format!(
            "END Order placed but wallet send failed: {}",
            truncate_ussd_label(&clean, 100)
        )
    } else {
        let clean = sanitize_ussd_ascii(e);
        format!(
            "END Could not finish sell: {}",
            truncate_ussd_label(&clean, 120)
        )
    }
}

fn normalize_bank_match_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn looks_like_mongo_object_id(s: &str) -> bool {
    let t = s.trim();
    t.len() == 24 && t.chars().all(|c| c.is_ascii_hexdigit())
}

fn bank_institution_id_from_pa_list(banks: &[PajBank], label: &str) -> Option<String> {
    let want = normalize_bank_match_key(label);
    if want.is_empty() {
        return None;
    }
    if let Some(b) = banks
        .iter()
        .find(|b| normalize_bank_match_key(&b.name) == want)
    {
        return Some(b.id.clone());
    }
    if let Some(b) = banks.iter().find(|b| {
        let bn = normalize_bank_match_key(&b.name);
        bn.contains(&want) || want.contains(&bn)
    }) {
        return Some(b.id.clone());
    }
    banks
        .iter()
        .find(|b| b.code.trim() == label.trim())
        .map(|b| b.id.clone())
}

async fn resolve_offramp_bank_institution_id(
    pool: &Pool,
    http: &HttpClient,
    config: &AppConfig,
    session_token: &str,
    user_id: Uuid,
    acc: &PajSavedBankAccount,
) -> Option<String> {
    let banks = match list_banks(http, config, session_token).await {
        Ok(b) => Some(b),
        Err(e) => {
            log::warn!("[PAJ offramp USSD] list_banks for institution id: {e}");
            None
        }
    };

    if let Ok(client) = pool.get().await {
        match client
            .query_opt(
                "SELECT paj_bank_institution_id, bank_code, bank_name \
                 FROM user_paj_bank_accounts \
                 WHERE user_id = $1 AND paj_saved_account_id = $2",
                &[&user_id, &acc.id],
            )
            .await
        {
            Ok(Some(row)) => {
                let inst: Option<String> = row.get(0);
                let bank_code: Option<String> = row.get(1);
                let bank_name: Option<String> = row.get(2);

                if let Some(s) = inst
                    .as_ref()
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                {
                    if looks_like_mongo_object_id(&s) {
                        return Some(s);
                    }
                }

                if let Some(ref blist) = banks {
                    if let Some(c) = bank_code
                        .as_ref()
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                    {
                        if let Some(b) = blist.iter().find(|b| b.code.trim() == c) {
                            return Some(b.id.clone());
                        }
                    }
                    if let Some(n) = bank_name
                        .as_ref()
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                    {
                        if let Some(id) = bank_institution_id_from_pa_list(blist, &n) {
                            return Some(id);
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => log::warn!("[PAJ offramp USSD] mirror row lookup: {e}"),
        }
    }

    let paj_label = acc.bank_institution_id.trim();
    if looks_like_mongo_object_id(paj_label) {
        return Some(paj_label.to_string());
    }
    if !paj_label.is_empty() {
        if let Some(ref blist) = banks {
            if let Some(id) = bank_institution_id_from_pa_list(blist, paj_label) {
                return Some(id);
            }
        }
    }

    None
}

pub async fn handle_paj_onramp_ussd(
    pool: &Pool,
    redis: &RedisClient,
    http: &HttpClient,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    inputs: &[String],
) -> String {
    if !paj_is_configured(config) || !ramp_orders_supported(config) {
        return "CON PAJ buy is not available (check PAJ + PAYCE_PUBLIC_BASE_URL).".into();
    }
    let uid = match Uuid::parse_str(user_id) {
        Ok(u) => u,
        Err(_) => return "END Invalid account.".into(),
    };

    let key = onramp_flow_key(user_id);

    match inputs.len() {
        2 => con_token_pick("Buy token (PAJ onramp)", "Pick token to buy:"),
        3 => {
            let mint = match mint_for_token_digit(config, &inputs[2]) {
                Some(m) => m,
                None => return "END Pick 1–4 for token. Dial My Account > 2 to restart.".into(),
            };
            let flow = OnrampFlowState {
                mint,
                pending_ngn: None,
            };
            if let Err(e) = redis_store_onramp(redis, &key, &flow).await {
                return format!("END Session error: {e}");
            }
            "CON Enter NGN amount (bank pay-in):".into()
        }
        4 => {
            let mut flow: OnrampFlowState =
                match redis_load_json::<OnrampFlowState>(redis, &key).await {
                    Some(f) if !f.mint.is_empty() => f,
                    _ => {
                        return "END Session expired.\nDial My Account > 2 to start again.".into();
                    }
                };
            let amt: f64 = match inputs[3].trim().parse() {
                Ok(v) if v > 0.0 => v,
                _ => return "END Enter a valid NGN amount.".into(),
            };
            flow.pending_ngn = Some(amt);
            if let Err(e) = redis_store_onramp(redis, &key, &flow).await {
                return format!("END Session error: {e}");
            }
            "CON Enter PIN to confirm buy:".into()
        }
        5 => {
            let flow: OnrampFlowState = match redis_load_json::<OnrampFlowState>(redis, &key).await
            {
                Some(f) if !f.mint.is_empty() => f,
                _ => {
                    return "END Session expired.\nDial My Account > 2 to start again.".into();
                }
            };
            let amt = match flow.pending_ngn {
                Some(a) if a > 0.0 => a,
                _ => return "END Amount missing. Start again from My Account > 2.".into(),
            };
            if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &inputs[4]).await {
                return err;
            }
            let client = match pool.get().await {
                Ok(c) => c,
                Err(e) => {
                    log::error!("[PAJ onramp USSD] pool: {e}");
                    return "END Service temporarily unavailable.".into();
                }
            };
            let row = match client
                .query_opt(
                    "SELECT solana_pubkey::text FROM users WHERE id = $1",
                    &[&uid],
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    log::error!("[PAJ onramp USSD] query: {e}");
                    return "END Database error.".into();
                }
            };
            let recipient: String = match row.and_then(|r| r.get::<_, Option<String>>(0)) {
                Some(pk) if !pk.trim().is_empty() => pk.trim().to_string(),
                _ => return "END Wallet not set up. Fund or import a wallet first.".into(),
            };
            let session = match load_usable_paj_session_token(pool, &uid).await {
                Ok(t) => t,
                Err(e) => return session_gate_ussd(e),
            };
            let mint = flow.mint.clone();
            let res = place_paj_onramp_order(
                pool,
                rpc,
                http,
                config,
                uid,
                &session,
                recipient,
                "NGN".into(),
                mint,
                None,
                None,
                Some(amt),
                None,
            )
            .await;
            match res {
                Ok((_oid, r, _)) => {
                    let _ = redis_delete_key(redis, &key).await;
                    let bank = r.bank.as_deref().unwrap_or("—");
                    let acct = r
                        .account_number
                        .as_deref()
                        .map(|s| truncate_ussd_label(s, 18))
                        .unwrap_or_else(|| "—".into());
                    format!(
                        "END Order placed. Pay to:\nBank {}\nAcct {}\nYou will receive SMS and email shortly with updates.",
                        truncate_ussd_label(bank, 18),
                        acct
                    )
                }
                Err(e) => {
                    log::warn!("[PAJ onramp USSD] place: {e}");
                    format_paj_place_err_ussd(&e)
                }
            }
        }
        _ => "END Invalid step. Dial My Account > 2 to start again.".into(),
    }
}

pub async fn handle_paj_offramp_ussd(
    pool: &Pool,
    redis: &RedisClient,
    http: &HttpClient,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    inputs: &[String],
) -> String {
    if !paj_is_configured(config) || !ramp_orders_supported(config) {
        return "CON PAJ sell is not available (check PAJ + PAYCE_PUBLIC_BASE_URL).".into();
    }
    let uid = match Uuid::parse_str(user_id) {
        Ok(u) => u,
        Err(_) => return "END Invalid account.".into(),
    };

    let key = offramp_flow_key(user_id);

    match inputs.len() {
        2 => con_token_pick("Sell token (PAJ offramp)", "Pick token to sell:"),
        3 => {
            let mint = match mint_for_token_digit(config, &inputs[2]) {
                Some(m) => m,
                None => return "END Pick 1–4 for token. Dial My Account > 3 to restart.".into(),
            };
            let session = match load_usable_paj_session_token(pool, &uid).await {
                Ok(t) => t,
                Err(e) => return session_gate_ussd(e),
            };
            let accounts = match list_saved_bank_accounts(http, config, &session).await {
                Ok(a) => a,
                Err(e) => {
                    log::warn!("[PAJ offramp USSD] list banks: {e}");
                    return format!("END Could not load banks: {}", truncate_ussd_label(&e, 50));
                }
            };
            log::info!(
                "[PAJ offramp USSD] list_saved_bank_accounts user_id={} count={} (PAJ `bank` = bank_institution_id below)",
                user_id,
                accounts.len()
            );
            for (i, a) in accounts.iter().enumerate() {
                log::info!(
                    "[PAJ offramp USSD]   [{}] saved_account_id={} bank_field={:?} account_name={:?} account_number={}",
                    i,
                    a.id,
                    a.bank_institution_id,
                    a.account_name,
                    mask_acct(&a.account_number),
                );
            }
            if accounts.is_empty() {
                return "END No saved payout bank.\nDial My Account > 7 > 1 to add a bank first."
                    .into();
            }
            let token_label = token_label_for_digit(&inputs[2]).to_string();
            let flow = OfframpFlowState {
                mint,
                token_label: token_label.clone(),
                accounts: accounts.clone(),
                picked_idx: None,
                pending_amount: None,
            };
            if let Err(e) = redis_store_offramp(redis, &key, &flow).await {
                return format!("END Session error: {e}");
            }
            let mut lines: Vec<String> = vec![format!(
                "CON Selling {}\nPick payout bank:",
                truncate_ussd_label(token_label.as_str(), 10)
            )];
            for (i, a) in accounts.iter().take(8).enumerate() {
                let nm = truncate_ussd_label(&a.account_name, 16);
                lines.push(format!(
                    "{}. {} {}",
                    i + 1,
                    nm,
                    mask_acct(&a.account_number)
                ));
            }
            lines.join("\n")
        }
        4 => {
            let mut flow: OfframpFlowState =
                match redis_load_json::<OfframpFlowState>(redis, &key).await {
                    Some(f) if !f.mint.is_empty() && !f.accounts.is_empty() => f,
                    _ => return "END Session expired.\nDial My Account > 3 to start again.".into(),
                };
            let pick: usize = match inputs[3].trim().parse::<usize>() {
                Ok(n) if n >= 1 && n <= flow.accounts.len() => n - 1,
                _ => return "END Pick a valid bank number from the list.".into(),
            };
            if let Some(chosen) = flow.accounts.get(pick) {
                log::info!(
                    "[PAJ offramp USSD] user picked bank user_id={} pick_menu_1based={} saved_account_id={} bank_field={:?} account_name={:?} account_number={}",
                    user_id,
                    pick + 1,
                    chosen.id,
                    chosen.bank_institution_id,
                    chosen.account_name,
                    mask_acct(&chosen.account_number),
                );
            }
            flow.picked_idx = Some(pick);
            if let Err(e) = redis_store_offramp(redis, &key, &flow).await {
                return format!("END Session error: {e}");
            }
            "CON Enter token amount to sell (e.g. 10 or 25.5):".into()
        }
        5 => {
            let mut flow: OfframpFlowState =
                match redis_load_json::<OfframpFlowState>(redis, &key).await {
                    Some(f) if !f.mint.is_empty() => f,
                    _ => return "END Session expired.\nDial My Account > 3 to start again.".into(),
                };
            if flow.picked_idx.is_none() {
                return "END Pick a bank first (start from My Account > 3).".into();
            }
            let amt: f64 = match inputs[4].trim().parse() {
                Ok(v) if v > 0.0 => v,
                _ => return "END Enter a valid token amount.".into(),
            };
            flow.pending_amount = Some(amt);
            if let Err(e) = redis_store_offramp(redis, &key, &flow).await {
                return format!("END Session error: {e}");
            }
            "CON Enter PIN to confirm sell:".into()
        }
        6 => {
            let flow: OfframpFlowState =
                match redis_load_json::<OfframpFlowState>(redis, &key).await {
                    Some(f) if !f.mint.is_empty() => f,
                    _ => return "END Session expired.\nDial My Account > 3 to start again.".into(),
                };
            let pick = match flow.picked_idx {
                Some(i) => i,
                None => return "END Pick a bank first.".into(),
            };
            let amt = match flow.pending_amount {
                Some(a) if a > 0.0 => a,
                _ => return "END Amount missing. Start again from My Account > 3.".into(),
            };
            if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &inputs[5]).await {
                return err;
            }
            let acc = match flow.accounts.get(pick) {
                Some(a) => a.clone(),
                None => return "END Invalid bank selection.".into(),
            };
            let session = match load_usable_paj_session_token(pool, &uid).await {
                Ok(t) => t,
                Err(e) => return session_gate_ussd(e),
            };
            let mint = flow.mint.clone();
            let bank_institution = match resolve_offramp_bank_institution_id(
                pool, http, config, &session, uid, &acc,
            )
            .await
            {
                Some(b) => b,
                None => {
                    log::warn!(
                        "[PAJ offramp USSD] could not resolve institution bank_id for saved_account_id={}",
                        acc.id
                    );
                    return "END No bank account found.".into();
                }
            };
            log::info!(
                "[PAJ offramp USSD] place_order prep user_id={} saved_account_id={} PAJ_list_bank_field={:?} resolved_bank_for_offramp_body={:?} amount={}",
                user_id,
                acc.id,
                acc.bank_institution_id,
                bank_institution,
                amt,
            );
            let res = place_paj_offramp_order(
                pool,
                rpc,
                http,
                config,
                uid,
                &session,
                bank_institution,
                acc.account_number.clone(),
                "NGN".into(),
                mint,
                None,
                Some(amt),
                None,
                None,
                None,
            )
            .await;
            match res {
                Ok((_oid, r, _sig)) => {
                    let _ = redis_delete_key(redis, &key).await;
                    let addr = r
                        .address
                        .as_deref()
                        .map(|s| truncate_ussd_label(s, 28))
                        .unwrap_or_else(|| "—".into());
                    format!(
                        "END Order placed. Tokens sent from your wallet.\nDeposit addr (ref):\n{addr}\nYou will receive SMS and email shortly with updates."
                    )
                }
                Err(e) => {
                    log::warn!("[PAJ offramp USSD] place: {e}");
                    format_paj_place_err_ussd(&e)
                }
            }
        }
        _ => "END Invalid step. Dial My Account > 3 to start again.".into(),
    }
}
