use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signer::Signer,
    transaction::Transaction as SolTx,
};
use spl_associated_token_account::instruction::create_associated_token_account;
use std::str::FromStr;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::exchange_rate::{
    format_balance_multi_stable_sol, format_ngn, format_stable_qty_trimmed, format_usdc,
    get_usd_to_ngn_rate, ngn_to_usd, usd_to_ngn,
};
use crate::services::merchant::get_merchant_by_code;
use crate::services::sms::*;
use crate::services::solana_rpc::{derive_associated_token_address, SolanaRpc};
use crate::services::user_activity::{
    log_user_activity, EVT_EXTERNAL_SEND, EVT_MERCHANT_PAYMENT, EVT_P2P_SEND, ST_CONFIRMED,
    ST_FAILED,
};
use crate::services::wallet::{
    get_keypair_for_user, get_native_sol_balance, get_spl_token_balance,
};
use crate::utils::phone::mask_phone;

pub struct TransferResult {
    pub success: bool,
    pub tx_signature: Option<String>,
    pub error: Option<String>,
}

async fn stable_balances_for_summary(
    rpc: &SolanaRpc,
    config: &AppConfig,
    pubkey_str: &str,
) -> Vec<(String, f64)> {
    let mut v = Vec::new();
    for s in &config.stable_coins {
        let b = get_spl_token_balance(rpc, pubkey_str, &s.mint).await;
        v.push((s.code.clone(), b));
    }
    v
}

impl TransferResult {
    fn fail(msg: &str) -> Self {
        Self {
            success: false,
            tx_signature: None,
            error: Some(msg.to_string()),
        }
    }
    fn ok(sig: String) -> Self {
        Self {
            success: true,
            tx_signature: Some(sig),
            error: None,
        }
    }
}

fn spl_token_id() -> Pubkey {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
}

fn build_spl_transfer_ix(
    source_ata: &Pubkey,
    mint: &Pubkey,
    dest_ata: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![12u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);

    Instruction {
        program_id: spl_token_id(),
        accounts: vec![
            AccountMeta::new(*source_ata, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(*dest_ata, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

pub(crate) async fn check_daily_limit(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
    amount_ngn: f64,
    kyc_tier: &str,
    config: &AppConfig,
) -> Option<String> {
    let limit = if kyc_tier == "TIER_2" {
        config.tier2_daily_ngn
    } else {
        config.tier1_daily_ngn
    };
    let today_start = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let today_start =
        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(today_start, chrono::Utc);

    let uid = Uuid::parse_str(user_id).ok()?;
    let client = pool.get().await.ok()?;
    let row = client
        .query_opt(
            "SELECT COALESCE((
                SELECT SUM(amount_ngn) FROM transactions \
                 WHERE sender_id = $1 AND status = 'CONFIRMED' AND created_at >= $2), 0) \
             + COALESCE((
                SELECT SUM(amount_ngn) FROM utility_bill_orders \
                 WHERE user_id = $1 AND created_at >= $2 \
                   AND status IN ('PENDING','ONCHAIN_SUBMITTED','PROCESSING','COMPLETED')), 0) \
             + COALESCE((
                SELECT SUM(amount_ngn) FROM user_activity \
                 WHERE user_id = $1 AND status = 'CONFIRMED' AND created_at >= $2 \
                   AND event_type IN ('P2P_SEND', 'EXTERNAL_SEND')), 0)",
            &[&uid, &today_start],
        )
        .await
        .ok()?;

    let spent = row.and_then(|r| r.get::<_, Option<f64>>(0)).unwrap_or(0.0);
    let remaining = limit - spent;
    if amount_ngn > remaining {
        Some(format!(
            "Daily limit exceeded. You can send {} more today.",
            format_ngn(remaining.max(0.0))
        ))
    } else {
        None
    }
}

const SPL_STABLE_DECIMALS: u8 = 6;

async fn execute_gasless_transfer(
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_keypair: &solana_sdk::signature::Keypair,
    recipient_pubkey_str: &str,
    amount_smallest: u64,
    mint: &Pubkey,
) -> Result<String, String> {
    let user_pk = user_keypair.pubkey();
    let recipient_pk = Pubkey::from_str(recipient_pubkey_str).map_err(|e| e.to_string())?;
    let fee_payer_pk = config.fee_payer.pubkey();

    let user_ata = derive_associated_token_address(&user_pk, mint);
    let recipient_ata = derive_associated_token_address(&recipient_pk, mint);
    let fee_payer_ata = derive_associated_token_address(&fee_payer_pk, mint);

    let mut ixs: Vec<Instruction> = Vec::new();
    if !rpc.account_exists(&user_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &user_pk,
            mint,
            &spl_token::id(),
        ));
    }
    if !rpc.account_exists(&recipient_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &recipient_pk,
            mint,
            &spl_token::id(),
        ));
    }
    if !rpc.account_exists(&fee_payer_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &fee_payer_pk,
            mint,
            &spl_token::id(),
        ));
    }

    ixs.push(build_spl_transfer_ix(
        &user_ata,
        mint,
        &recipient_ata,
        &user_pk,
        amount_smallest,
        SPL_STABLE_DECIMALS,
    ));

    ixs.push(build_spl_transfer_ix(
        &user_ata,
        mint,
        &fee_payer_ata,
        &user_pk,
        config.gas_fee_usdc,
        SPL_STABLE_DECIMALS,
    ));

    let blockhash = rpc
        .get_latest_blockhash()
        .await
        .map_err(|_| "Network error. Please try again.".to_string())?;

    let tx = SolTx::new_signed_with_payer(
        &ixs,
        Some(&fee_payer_pk),
        &[&*config.fee_payer, user_keypair],
        blockhash,
    );

    rpc.send_transaction(&tx).await
}

pub async fn transfer_user_spl_to_owner_with_gas(
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_keypair: &solana_sdk::signature::Keypair,
    mint: &Pubkey,
    recipient_owner: &Pubkey,
    amount_smallest: u64,
    mint_decimals: u8,
) -> Result<String, String> {
    let user_pk = user_keypair.pubkey();
    let fee_payer_pk = config.fee_payer.pubkey();

    let user_ata = derive_associated_token_address(&user_pk, mint);
    let recipient_ata = derive_associated_token_address(recipient_owner, mint);
    let fee_payer_ata = derive_associated_token_address(&fee_payer_pk, mint);

    let mut ixs: Vec<Instruction> = Vec::new();
    if !rpc.account_exists(&user_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &user_pk,
            mint,
            &spl_token::id(),
        ));
    }
    if !rpc.account_exists(&recipient_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            recipient_owner,
            mint,
            &spl_token::id(),
        ));
    }
    if !rpc.account_exists(&fee_payer_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &fee_payer_pk,
            mint,
            &spl_token::id(),
        ));
    }

    ixs.push(build_spl_transfer_ix(
        &user_ata,
        mint,
        &recipient_ata,
        &user_pk,
        amount_smallest,
        mint_decimals,
    ));

    ixs.push(build_spl_transfer_ix(
        &user_ata,
        mint,
        &fee_payer_ata,
        &user_pk,
        config.gas_fee_usdc,
        mint_decimals,
    ));

    let blockhash = rpc
        .get_latest_blockhash()
        .await
        .map_err(|_| "Network error. Please try again.".to_string())?;

    let tx = SolTx::new_signed_with_payer(
        &ixs,
        Some(&fee_payer_pk),
        &[&*config.fee_payer, user_keypair],
        blockhash,
    );

    rpc.send_transaction(&tx).await
}

pub async fn transfer_p2p(
    pool: &deadpool_postgres::Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    sender_phone: &str,
    recipient_phone: &str,
    amount_ngn: f64,
    stable_idx: usize,
) -> TransferResult {
    let Some(stable) = config.stable_coins.get(stable_idx) else {
        return TransferResult::fail("Invalid token choice.");
    };
    let mint = &stable.mint;
    let stable_code = stable.code.as_str();
    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => return TransferResult::fail(&format!("Database error: {e}")),
    };

    let sender_row = client
        .query_opt(
            "SELECT id::text, solana_pubkey, encrypted_keypair, kyc_tier::text FROM users WHERE phone_number = $1",
            &[&sender_phone],
        )
        .await
        .unwrap_or(None);

    let recipient_row = client
        .query_opt(
            "SELECT id::text, solana_pubkey FROM users WHERE phone_number = $1",
            &[&recipient_phone],
        )
        .await
        .unwrap_or(None);

    let (sender_id, sender_pubkey, sender_encrypted, sender_tier) = match sender_row {
        Some(row) => {
            let id: String = row.get(0);
            let pk: Option<String> = row.get(1);
            let ek: Option<String> = row.get(2);
            let tier: String = row.get(3);
            match (pk, ek) {
                (Some(pk), Some(ek)) => (id, pk, ek, tier),
                _ => return TransferResult::fail("Sender wallet not set up"),
            }
        }
        None => return TransferResult::fail("Sender wallet not set up"),
    };
    let (recipient_id, recipient_pubkey) = match recipient_row {
        Some(row) => {
            let id: String = row.get(0);
            let pk: Option<String> = row.get(1);
            match pk {
                Some(pk) => (id, pk),
                None => return TransferResult::fail("Recipient not registered on Payce"),
            }
        }
        None => return TransferResult::fail("Recipient not registered on Payce"),
    };

    if let Some(err) = check_daily_limit(pool, &sender_id, amount_ngn, &sender_tier, config).await {
        return TransferResult::fail(&err);
    }

    let rate = get_usd_to_ngn_rate(config).await;
    let amount_usdc = ngn_to_usd(amount_ngn, rate);
    let amount_smallest = (amount_usdc * 1_000_000.0).round() as u64;

    let total_needed_usdc = (amount_smallest + config.gas_fee_usdc) as f64 / 1_000_000.0;
    let balance = get_spl_token_balance(rpc, &sender_pubkey, mint).await;
    if balance < total_needed_usdc {
        return TransferResult::fail(&format!(
            "Insufficient {} balance. You have ≈{} {} (≈{}).",
            stable_code,
            format_stable_qty_trimmed(balance),
            stable_code,
            format_ngn(usd_to_ngn(balance, rate)),
        ));
    }

    let keypair = match get_keypair_for_user(&sender_encrypted, &config.wallet_encryption_key) {
        Ok(kp) => kp,
        Err(e) => return TransferResult::fail(&format!("Wallet error: {e}")),
    };

    let sig = match execute_gasless_transfer(
        rpc,
        config,
        &keypair,
        &recipient_pubkey,
        amount_smallest,
        mint,
    )
    .await
    {
        Ok(sig) => sig,
        Err(e) => {
            log::error!("[Transfer] P2P failed: {e}");
            return TransferResult::fail("Transfer failed. Please try again.");
        }
    };

    let stable_rows = stable_balances_for_summary(rpc, config, &sender_pubkey).await;
    let new_sol = get_native_sol_balance(rpc, &sender_pubkey).await;
    let balance_line = format_balance_multi_stable_sol(rate, &stable_rows, new_sol);
    let config_c = config.clone();
    let sp = sender_phone.to_string();
    let rp = recipient_phone.to_string();
    let sig_c = sig.clone();
    let code = stable_code.to_string();
    tokio::spawn(async move {
        send_sms(
            &config_c,
            &sp,
            &build_transfer_sent_sms(
                &config_c,
                &format_ngn(amount_ngn),
                &format_usdc(amount_usdc),
                &code,
                &mask_phone(&rp),
                &sig_c,
                &balance_line,
            ),
        )
        .await;
        send_sms(
            &config_c,
            &rp,
            &build_transfer_received_sms(
                &config_c,
                &format_ngn(amount_ngn),
                &format_usdc(amount_usdc),
                &code,
                &mask_phone(&sp),
                &sig_c,
            ),
        )
        .await;
    });
    log_user_activity(
        pool,
        &sender_id,
        EVT_P2P_SEND,
        ST_CONFIRMED,
        Some(&sig),
        i64::try_from(amount_smallest).ok(),
        Some(&mint.to_string()),
        Some(amount_ngn),
        Some(rate),
        Some(&recipient_id),
        None,
        serde_json::json!({
            "stable_code": stable_code,
            "recipient_phone": mask_phone(recipient_phone),
        }),
    );
    TransferResult::ok(sig)
}

pub async fn transfer_to_address(
    pool: &deadpool_postgres::Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    sender_phone: &str,
    recipient_address: &str,
    amount_ngn: f64,
    label: Option<&str>,
    stable_idx: usize,
) -> TransferResult {
    let Some(stable) = config.stable_coins.get(stable_idx) else {
        return TransferResult::fail("Invalid token choice.");
    };
    let mint = &stable.mint;
    let stable_code = stable.code.as_str();
    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => return TransferResult::fail(&format!("Database error: {e}")),
    };

    let sender_row = client
        .query_opt(
            "SELECT id::text, solana_pubkey, encrypted_keypair, kyc_tier::text FROM users WHERE phone_number = $1",
            &[&sender_phone],
        )
        .await
        .unwrap_or(None);

    let (sender_id, sender_pubkey, sender_encrypted, sender_tier) = match sender_row {
        Some(row) => {
            let id: String = row.get(0);
            let pk: Option<String> = row.get(1);
            let ek: Option<String> = row.get(2);
            let tier: String = row.get(3);
            match (pk, ek) {
                (Some(pk), Some(ek)) => (id, pk, ek, tier),
                _ => return TransferResult::fail("Sender wallet not set up"),
            }
        }
        None => return TransferResult::fail("Sender wallet not set up"),
    };

    if Pubkey::from_str(recipient_address).is_err() {
        return TransferResult::fail("Invalid wallet address");
    }

    if let Some(err) = check_daily_limit(pool, &sender_id, amount_ngn, &sender_tier, config).await {
        return TransferResult::fail(&err);
    }

    let rate = get_usd_to_ngn_rate(config).await;
    let amount_usdc = ngn_to_usd(amount_ngn, rate);
    let amount_smallest = (amount_usdc * 1_000_000.0).round() as u64;

    let total_needed_usdc = (amount_smallest + config.gas_fee_usdc) as f64 / 1_000_000.0;
    let balance = get_spl_token_balance(rpc, &sender_pubkey, mint).await;
    if balance < total_needed_usdc {
        return TransferResult::fail(&format!(
            "Insufficient {} balance. You have ≈{} {} (≈{}).",
            stable_code,
            format_stable_qty_trimmed(balance),
            stable_code,
            format_ngn(usd_to_ngn(balance, rate)),
        ));
    }

    let keypair = match get_keypair_for_user(&sender_encrypted, &config.wallet_encryption_key) {
        Ok(kp) => kp,
        Err(e) => return TransferResult::fail(&format!("Wallet error: {e}")),
    };

    let sig = match execute_gasless_transfer(
        rpc,
        config,
        &keypair,
        recipient_address,
        amount_smallest,
        mint,
    )
    .await
    {
        Ok(sig) => sig,
        Err(e) => {
            log::error!("[Transfer] Address transfer failed: {e}");
            return TransferResult::fail("Transfer failed. Please try again.");
        }
    };

    let default_label = format!(
        "{}...{}",
        &recipient_address[..6],
        &recipient_address[recipient_address.len() - 4..]
    );
    let display = label.unwrap_or(&default_label);
    let stable_rows = stable_balances_for_summary(rpc, config, &sender_pubkey).await;
    let new_sol = get_native_sol_balance(rpc, &sender_pubkey).await;
    let balance_line = format_balance_multi_stable_sol(rate, &stable_rows, new_sol);
    let config_c = config.clone();
    let sp = sender_phone.to_string();
    let sig_c = sig.clone();
    let dl = display.to_string();
    let code = stable_code.to_string();
    tokio::spawn(async move {
        send_sms(
            &config_c,
            &sp,
            &build_transfer_sent_sms(
                &config_c,
                &format_ngn(amount_ngn),
                &format_usdc(amount_usdc),
                &code,
                &dl,
                &sig_c,
                &balance_line,
            ),
        )
        .await;
    });
    log_user_activity(
        pool,
        &sender_id,
        EVT_EXTERNAL_SEND,
        ST_CONFIRMED,
        Some(&sig),
        i64::try_from(amount_smallest).ok(),
        Some(&mint.to_string()),
        Some(amount_ngn),
        Some(rate),
        None,
        None,
        serde_json::json!({
            "stable_code": stable_code,
            "recipient_address": recipient_address,
            "label": label,
        }),
    );
    TransferResult::ok(sig)
}

pub async fn pay_merchant(
    pool: &deadpool_postgres::Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    customer_phone: &str,
    merchant_code: &str,
    amount_ngn: f64,
    stable_idx: usize,
) -> TransferResult {
    let Some(stable) = config.stable_coins.get(stable_idx) else {
        return TransferResult::fail("Invalid token choice.");
    };
    let mint = &stable.mint;
    let stable_code = stable.code.as_str();
    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => return TransferResult::fail(&format!("Database error: {e}")),
    };

    let customer_row = client
        .query_opt(
            "SELECT id::text, solana_pubkey, encrypted_keypair, kyc_tier::text FROM users WHERE phone_number = $1",
            &[&customer_phone],
        )
        .await
        .unwrap_or(None);
    let (customer_id, customer_pubkey, customer_encrypted, customer_tier) = match customer_row {
        Some(row) => {
            let id: String = row.get(0);
            let pk: Option<String> = row.get(1);
            let ek: Option<String> = row.get(2);
            let tier: String = row.get(3);
            match (pk, ek) {
                (Some(pk), Some(ek)) => (id, pk, ek, tier),
                _ => return TransferResult::fail("Your wallet is not set up"),
            }
        }
        None => return TransferResult::fail("Your wallet is not set up"),
    };

    let merchant = match get_merchant_by_code(pool, merchant_code).await {
        Some(m) if m.status == "ACTIVE" => m,
        _ => return TransferResult::fail("Merchant not found"),
    };

    let merchant_row = client
        .query_opt(
            "SELECT m.id::text, u.id::text, u.phone_number, u.solana_pubkey \
             FROM merchants m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.merchant_code = $1",
            &[&merchant_code],
        )
        .await
        .unwrap_or(None);
    let (merchant_id, merchant_user_id, merchant_phone, merchant_pubkey) = match merchant_row {
        Some(row) => {
            let mid: String = row.get(0);
            let uid: String = row.get(1);
            let phone: String = row.get(2);
            let pk: Option<String> = row.get(3);
            match pk {
                Some(pk) => (mid, uid, phone, pk),
                None => return TransferResult::fail("Merchant wallet not set up"),
            }
        }
        None => return TransferResult::fail("Merchant not found"),
    };

    if let Some(err) =
        check_daily_limit(pool, &customer_id, amount_ngn, &customer_tier, config).await
    {
        return TransferResult::fail(&err);
    }

    let rate = get_usd_to_ngn_rate(config).await;
    let amount_usd = ngn_to_usd(amount_ngn, rate);
    let amount_smallest = (amount_usd * 1_000_000.0).round() as u64;
    let total_needed = (amount_smallest + config.gas_fee_usdc) as f64 / 1_000_000.0;
    let balance = get_spl_token_balance(rpc, &customer_pubkey, mint).await;
    if balance < total_needed {
        return TransferResult::fail(&format!(
            "Insufficient {} balance. You have ≈{} {} (≈{}).",
            stable_code,
            format_stable_qty_trimmed(balance),
            stable_code,
            format_ngn(usd_to_ngn(balance, rate)),
        ));
    }

    let keypair = match get_keypair_for_user(&customer_encrypted, &config.wallet_encryption_key) {
        Ok(kp) => kp,
        Err(e) => return TransferResult::fail(&format!("Wallet error: {e}")),
    };

    let tx_id = Uuid::new_v4();
    let sender_uuid = match Uuid::parse_str(&customer_id) {
        Ok(v) => v,
        Err(_) => return TransferResult::fail("Invalid customer record"),
    };
    let recipient_uuid = match Uuid::parse_str(&merchant_user_id) {
        Ok(v) => v,
        Err(_) => return TransferResult::fail("Invalid merchant record"),
    };
    let merchant_uuid = match Uuid::parse_str(&merchant_id) {
        Ok(v) => v,
        Err(_) => return TransferResult::fail("Invalid merchant record"),
    };
    let _ = client
        .execute(
            "INSERT INTO transactions \
             (id, sender_id, recipient_id, merchant_id, amount_usdc, amount_ngn, exchange_rate, type, status) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'MERCHANT_PAYMENT', 'PENDING')",
            &[
                &tx_id,
                &sender_uuid,
                &recipient_uuid,
                &merchant_uuid,
                &(amount_smallest as i64),
                &amount_ngn,
                &rate,
            ],
        )
        .await;

    let sig = match execute_gasless_transfer(
        rpc,
        config,
        &keypair,
        &merchant_pubkey,
        amount_smallest,
        mint,
    )
    .await
    {
        Ok(sig) => sig,
        Err(e) => {
            let _ = client
                .execute(
                    "UPDATE transactions SET status = 'FAILED', error_message = $2 WHERE id = $1",
                    &[&tx_id, &e],
                )
                .await;
            log_user_activity(
                pool,
                &customer_id,
                EVT_MERCHANT_PAYMENT,
                ST_FAILED,
                None,
                i64::try_from(amount_smallest).ok(),
                Some(&mint.to_string()),
                Some(amount_ngn),
                Some(rate),
                Some(&merchant_user_id),
                Some(&tx_id.to_string()),
                serde_json::json!({
                    "merchant_code": merchant_code,
                    "error": e,
                }),
            );
            log::error!("[Transfer] Merchant payment failed: {e}");
            return TransferResult::fail("Payment failed. Please try again.");
        }
    };

    let _ = client
        .execute(
            "UPDATE transactions SET status = 'CONFIRMED', tx_signature = $2 WHERE id = $1",
            &[&tx_id, &sig],
        )
        .await;
    log_user_activity(
        pool,
        &customer_id,
        EVT_MERCHANT_PAYMENT,
        ST_CONFIRMED,
        Some(&sig),
        i64::try_from(amount_smallest).ok(),
        Some(&mint.to_string()),
        Some(amount_ngn),
        Some(rate),
        Some(&merchant_user_id),
        Some(&tx_id.to_string()),
        serde_json::json!({
            "merchant_code": merchant_code,
            "merchant_id": merchant_id,
        }),
    );

    let stable_rows = stable_balances_for_summary(rpc, config, &customer_pubkey).await;
    let new_sol = get_native_sol_balance(rpc, &customer_pubkey).await;
    let balance_line = format_balance_multi_stable_sol(rate, &stable_rows, new_sol);
    let config_c = config.clone();
    let customer_phone = customer_phone.to_string();
    let merchant_phone = merchant_phone.to_string();
    let merchant_name = merchant.business_name.clone();
    let sig_c = sig.clone();
    let code = stable_code.to_string();
    tokio::spawn(async move {
        send_sms(
            &config_c,
            &customer_phone,
            &build_merchant_payment_sms(
                &config_c,
                &format_ngn(amount_ngn),
                &format_usdc(amount_usd),
                &code,
                &merchant_name,
                &sig_c,
                &balance_line,
            ),
        )
        .await;
        send_sms(
            &config_c,
            &merchant_phone,
            &build_merchant_receipt_sms(
                &config_c,
                &format_ngn(amount_ngn),
                &format_usdc(amount_usd),
                &code,
                &mask_phone(&customer_phone),
                &sig_c,
            ),
        )
        .await;
    });

    TransferResult::ok(sig)
}

pub fn paj_offramp_spl_decimals(config: &AppConfig, mint: &Pubkey) -> u8 {
    if *mint == config.sol_mint {
        return 9;
    }
    if config.stable_coins.iter().any(|s| s.mint == *mint) {
        return 6;
    }
    6
}

async fn execute_offramp_spl_deposit_with_gas(
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_keypair: &solana_sdk::signature::Keypair,
    recipient_owner: &Pubkey,
    mint: &Pubkey,
    amount_raw: u64,
    transfer_decimals: u8,
) -> Result<String, String> {
    let user_pk = user_keypair.pubkey();
    let fee_payer_pk = config.fee_payer.pubkey();
    let user_ata = derive_associated_token_address(&user_pk, mint);
    let recipient_ata = derive_associated_token_address(recipient_owner, mint);
    let usdc_mint = &config.usdc_mint;
    let user_ata_usdc = derive_associated_token_address(&user_pk, usdc_mint);
    let fee_payer_ata_usdc = derive_associated_token_address(&fee_payer_pk, usdc_mint);

    let mut ixs: Vec<Instruction> = Vec::new();
    if !rpc.account_exists(&user_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &user_pk,
            mint,
            &spl_token::id(),
        ));
    }
    if !rpc.account_exists(&recipient_ata).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            recipient_owner,
            mint,
            &spl_token::id(),
        ));
    }

    ixs.push(build_spl_transfer_ix(
        &user_ata,
        mint,
        &recipient_ata,
        &user_pk,
        amount_raw,
        transfer_decimals,
    ));

    if !rpc.account_exists(&user_ata_usdc).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &user_pk,
            usdc_mint,
            &spl_token::id(),
        ));
    }
    if !rpc.account_exists(&fee_payer_ata_usdc).await {
        ixs.push(create_associated_token_account(
            &fee_payer_pk,
            &fee_payer_pk,
            usdc_mint,
            &spl_token::id(),
        ));
    }
    ixs.push(build_spl_transfer_ix(
        &user_ata_usdc,
        usdc_mint,
        &fee_payer_ata_usdc,
        &user_pk,
        config.gas_fee_usdc,
        SPL_STABLE_DECIMALS,
    ));

    let blockhash = rpc
        .get_latest_blockhash()
        .await
        .map_err(|_| "Network error. Please try again.".to_string())?;

    let tx = SolTx::new_signed_with_payer(
        &ixs,
        Some(&fee_payer_pk),
        &[&*config.fee_payer, user_keypair],
        blockhash,
    );

    rpc.send_transaction(&tx).await
}

pub async fn settle_paj_offramp_user_deposit(
    pool: &deadpool_postgres::Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: Uuid,
    deposit_wallet: &str,
    mint_str: &str,
    token_amount: f64,
    quote_fee: f64,
) -> Result<String, String> {
    let total_ui = token_amount + quote_fee;
    if total_ui <= 0.0 || !total_ui.is_finite() {
        return Err("Invalid offramp amount for settlement.".into());
    }
    let mint = Pubkey::from_str(mint_str.trim()).map_err(|e| e.to_string())?;
    let recipient_owner = Pubkey::from_str(deposit_wallet.trim()).map_err(|e| e.to_string())?;
    let decimals = paj_offramp_spl_decimals(config, &mint);
    let scale = 10f64.powi(decimals as i32);
    let raw = (total_ui * scale).round() as u64;
    if raw == 0 {
        return Err("Rounded deposit amount is zero.".into());
    }

    let client = pool.get().await.map_err(|e| e.to_string())?;
    let row = client
        .query_opt(
            "SELECT solana_pubkey, encrypted_keypair FROM users WHERE id = $1",
            &[&user_id],
        )
        .await
        .map_err(|e| e.to_string())?;
    let (pk, enc): (Option<String>, Option<String>) = match row {
        Some(r) => (r.get(0), r.get(1)),
        None => return Err("User not found.".into()),
    };
    let (pubkey_str, enc_s) = match (pk, enc) {
        (Some(pk), Some(ek)) if !pk.trim().is_empty() => (pk, ek),
        _ => return Err("Wallet not set up.".into()),
    };
    let keypair = get_keypair_for_user(&enc_s, &config.wallet_encryption_key)?;

    let usdc_mint = &config.usdc_mint;
    let gas_ui = config.gas_fee_usdc as f64 / 1_000_000.0;
    let bal_mint = get_spl_token_balance(rpc, &pubkey_str, &mint).await;
    let bal_usdc = get_spl_token_balance(rpc, &pubkey_str, usdc_mint).await;

    if mint == *usdc_mint {
        if bal_usdc < total_ui + gas_ui {
            return Err(format!(
                "Insufficient USDC: need ≈{} for deposit + fee + network (have ≈{}).",
                format_stable_qty_trimmed(total_ui + gas_ui),
                format_stable_qty_trimmed(bal_usdc)
            ));
        }
    } else if bal_mint < total_ui || bal_usdc < gas_ui {
        return Err(format!(
            "Insufficient balance: need ≈{} of sell mint and ≈{} USDC for network fee.",
            format_stable_qty_trimmed(total_ui),
            format_stable_qty_trimmed(gas_ui)
        ));
    }

    execute_offramp_spl_deposit_with_gas(
        rpc,
        config,
        &keypair,
        &recipient_owner,
        &mint,
        raw,
        decimals,
    )
    .await
}
