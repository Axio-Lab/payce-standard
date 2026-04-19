use base64::Engine;
use deadpool_postgres::Pool;
use serde_json::{json, Value};
use solana_sdk::{
    signature::{Keypair, Signature, Signer},
    transaction::{Transaction, VersionedTransaction},
};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::airbills::{self, AirbillsError, TransactRequest};
use crate::services::exchange_rate::{get_usd_to_ngn_rate, ngn_to_usd};
use crate::services::solana_rpc::SolanaRpc;
use crate::services::transfer::check_daily_limit;
use crate::services::user_activity::{
    log_user_activity, patch_user_activity_by_ref, EVT_UTILITY_BILL, ST_CONFIRMED, ST_FAILED,
    ST_PENDING,
};
use crate::services::wallet::{get_keypair_for_user, get_spl_token_balance};
use crate::utils::phone::{normalize_nigerian_phone, phone_local_nigeria_11_digits};

fn airbills_ngn_whole(amount_ngn: f64) -> i64 {
    amount_ngn.round() as i64
}

pub fn network_name_to_id(network: &str) -> &'static str {
    match network.trim().to_uppercase().as_str() {
        "MTN" => "01",
        "GLO" => "02",
        "9MOBILE" | "9 MOBILE" | "ETISALAT" => "03",
        "AIRTEL" => "04",
        _ => "01",
    }
}

pub fn sign_versioned_transaction_with_keypairs(
    mut vtx: VersionedTransaction,
    keypairs: &[&Keypair],
) -> Result<VersionedTransaction, String> {
    let msg_bytes = vtx.message.serialize();
    let num = vtx.message.header().num_required_signatures as usize;
    let keys = vtx.message.static_account_keys();
    if keys.len() < num {
        return Err("Invalid versioned transaction: not enough account keys".into());
    }
    if vtx.signatures.len() < num {
        vtx.signatures.resize(num, Signature::default());
    }
    for (i, key) in keys.iter().enumerate().take(num) {
        if vtx.signatures[i] != Signature::default() {
            continue;
        }
        let Some(kp) = keypairs.iter().find(|k| k.pubkey() == *key) else {
            continue;
        };
        vtx.signatures[i] = kp.sign_message(&msg_bytes);
    }
    for (i, key) in keys.iter().enumerate().take(num) {
        if vtx.signatures[i] == Signature::default() {
            return Err(format!(
                "Missing signature for signer position {} ({})",
                i, key
            ));
        }
    }
    Ok(vtx)
}

pub fn decode_transaction_ix_b64(b64: &str) -> Result<VersionedTransaction, String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("transactionIx base64: {e}"))?;
    if let Ok(v) = bincode::deserialize::<VersionedTransaction>(&raw) {
        return Ok(v);
    }
    let legacy: Transaction =
        bincode::deserialize(&raw).map_err(|e| format!("transactionIx decode: {e}"))?;
    Ok(VersionedTransaction::from(legacy))
}

pub struct UtilityPurchaseOk {
    pub chain_signature: String,
    pub airbills_id: String,
}

pub struct UtilityPurchaseErr {
    pub user_message: String,
}

async fn insert_order_pending(
    pool: &Pool,
    user_uuid: Uuid,
    airbills_id: &str,
    product_code: &str,
    amount_ngn: f64,
    amount_usdc: Option<f64>,
    rate: f64,
    metadata: Value,
) -> Result<Uuid, String> {
    let client = pool.get().await.map_err(|e| e.to_string())?;
    let row = client
        .query_one(
            "INSERT INTO utility_bill_orders (user_id, airbills_id, product_code, status, amount_ngn, amount_usdc, exchange_rate, metadata) \
             VALUES ($1, $2, $3, 'PENDING', $4, $5, $6, $7) RETURNING id",
            &[
                &user_uuid,
                &airbills_id,
                &product_code,
                &amount_ngn,
                &amount_usdc,
                &rate,
                &metadata,
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    let id: Uuid = row.get(0);
    log_user_activity(
        pool,
        &user_uuid.to_string(),
        EVT_UTILITY_BILL,
        ST_PENDING,
        None,
        None,
        None,
        Some(amount_ngn),
        Some(rate),
        None,
        Some(airbills_id),
        json!({
            "product_code": product_code,
            "airbills_id": airbills_id,
            "order_uuid": id,
        }),
    );
    Ok(id)
}

async fn update_order_status(
    pool: &Pool,
    airbills_id: &str,
    status: &str,
    chain_sig: Option<&str>,
    err: Option<&str>,
) -> Result<(), String> {
    let client = pool.get().await.map_err(|e| e.to_string())?;
    client
        .execute(
            "UPDATE utility_bill_orders SET status = $1::utility_bill_status, chain_tx_signature = COALESCE($2, chain_tx_signature), \
             error_message = $3, updated_at = NOW() WHERE airbills_id = $4",
            &[&status, &chain_sig, &err, &airbills_id],
        )
        .await
        .map_err(|e| e.to_string())?;
    let ua_status = match status {
        "COMPLETED" => ST_CONFIRMED,
        "FAILED" => ST_FAILED,
        _ => ST_PENDING,
    };
    let patch = json!({
        "utility_status": status,
        "error_message": err,
    });
    patch_user_activity_by_ref(
        pool,
        airbills_id,
        EVT_UTILITY_BILL,
        ua_status,
        chain_sig,
        patch,
    );
    Ok(())
}

async fn sign_send_process(
    pool: &Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_kp: &Keypair,
    product_code: &str,
    airbills_id: &str,
    transaction_ix_b64: &str,
) -> Result<String, String> {
    let vtx = decode_transaction_ix_b64(transaction_ix_b64)?;
    let signers: Vec<&Keypair> = vec![user_kp, &*config.fee_payer];
    let signed = sign_versioned_transaction_with_keypairs(vtx, &signers)?;
    let sig = match rpc.send_versioned_transaction(&signed).await {
        Ok(s) => s,
        Err(e) => {
            update_order_status(pool, airbills_id, "FAILED", None, Some(&e)).await?;
            return Err(e);
        }
    };
    update_order_status(pool, airbills_id, "ONCHAIN_SUBMITTED", Some(&sig), None).await?;
    if let Err(e) = airbills::transact_process(config, product_code, airbills_id).await {
        let msg = e.to_string();
        update_order_status(pool, airbills_id, "FAILED", Some(&sig), Some(&msg)).await?;
        return Err(msg);
    }
    update_order_status(pool, airbills_id, "COMPLETED", Some(&sig), None).await?;
    Ok(sig)
}

pub async fn purchase_airtime(
    pool: &Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    encrypted_keypair: &str,
    beneficiary_phone: &str,
    amount_ngn: f64,
) -> Result<UtilityPurchaseOk, UtilityPurchaseErr> {
    if !(50.0..=50_000.0).contains(&amount_ngn) {
        return Err(UtilityPurchaseErr {
            user_message: "Airtime amount must be between 50 and 50,000 NGN.".into(),
        });
    }
    let phone = normalize_nigerian_phone(beneficiary_phone);
    let network = airbills::network_checker(config, &phone)
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Could not detect network: {e}"),
        })?;
    let network_id = network_name_to_id(&network).to_string();
    let rate = get_usd_to_ngn_rate(config).await;
    let usdc = ngn_to_usd(amount_ngn, rate);
    let uid = Uuid::parse_str(user_id).map_err(|_| UtilityPurchaseErr {
        user_message: "Invalid account.".into(),
    })?;
    let client = pool.get().await.map_err(|e| UtilityPurchaseErr {
        user_message: format!("Database error: {e}"),
    })?;
    let row = client
        .query_opt(
            "SELECT solana_pubkey, kyc_tier::text FROM users WHERE id = $1",
            &[&uid],
        )
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Database error: {e}"),
        })?;
    let (pubkey_opt, kyc_tier): (Option<String>, Option<String>) =
        row.map(|r| (r.get(0), r.get(1))).unwrap_or((None, None));
    let pubkey: String = pubkey_opt.ok_or_else(|| UtilityPurchaseErr {
        user_message: "Wallet not set up.".into(),
    })?;
    let tier = kyc_tier.unwrap_or_else(|| "TIER_1".into());
    if let Some(msg) = check_daily_limit(pool, user_id, amount_ngn, &tier, config).await {
        return Err(UtilityPurchaseErr { user_message: msg });
    }
    let keypair =
        get_keypair_for_user(encrypted_keypair, &config.wallet_encryption_key).map_err(|e| {
            UtilityPurchaseErr {
                user_message: format!("Wallet error: {e}"),
            }
        })?;
    let balance = get_spl_token_balance(rpc, &pubkey, &config.usdc_mint).await;
    let min_usdc = usdc + config.gas_fee_usdc as f64 / 1_000_000.0;
    if balance < min_usdc {
        return Err(UtilityPurchaseErr {
            user_message: "Insufficient USDC balance for this purchase.".into(),
        });
    }
    let phone_air = phone_local_nigeria_11_digits(&phone);
    log::info!(
        "[Airbills] transact airtime network_id={network_id} amount_ngn={} phone_prefix={}",
        airbills_ngn_whole(amount_ngn),
        phone_air.chars().take(4).collect::<String>()
    );
    let data = json!({
        "pubKey": pubkey,
        "token": "USDC",
        "amount": airbills_ngn_whole(amount_ngn),
        "phoneNumber": phone_air,
        "networkId": network_id,
    });
    let req = TransactRequest {
        product_code: "100".into(),
        pay_with: "default".into(),
        data,
    };
    let t = airbills::transact(config, &req)
        .await
        .map_err(|e: AirbillsError| UtilityPurchaseErr {
            user_message: format!("Airbills: {}", e.message),
        })?;
    insert_order_pending(
        pool,
        uid,
        &t.id,
        "100",
        amount_ngn,
        t.amount_in_token,
        rate,
        json!({"type":"airtime","phone": phone, "network": network}),
    )
    .await
    .map_err(|e| UtilityPurchaseErr { user_message: e })?;
    let sig = sign_send_process(pool, rpc, config, &keypair, "100", &t.id, &t.transaction_ix)
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Payment failed: {e}"),
        })?;
    Ok(UtilityPurchaseOk {
        chain_signature: sig,
        airbills_id: t.id,
    })
}

pub async fn purchase_data(
    pool: &Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    encrypted_keypair: &str,
    beneficiary_phone: &str,
    batch: &str,
    prod_id: &str,
    network_id: &str,
    amount_ngn: f64,
) -> Result<UtilityPurchaseOk, UtilityPurchaseErr> {
    let phone = normalize_nigerian_phone(beneficiary_phone);
    let rate = get_usd_to_ngn_rate(config).await;
    let usdc = ngn_to_usd(amount_ngn, rate);
    let uid = Uuid::parse_str(user_id).map_err(|_| UtilityPurchaseErr {
        user_message: "Invalid account.".into(),
    })?;
    let client = pool.get().await.map_err(|e| UtilityPurchaseErr {
        user_message: format!("Database error: {e}"),
    })?;
    let row = client
        .query_opt(
            "SELECT solana_pubkey, kyc_tier::text FROM users WHERE id = $1",
            &[&uid],
        )
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Database error: {e}"),
        })?;
    let (pubkey_opt, kyc_tier): (Option<String>, Option<String>) =
        row.map(|r| (r.get(0), r.get(1))).unwrap_or((None, None));
    let pubkey: String = pubkey_opt.ok_or_else(|| UtilityPurchaseErr {
        user_message: "Wallet not set up.".into(),
    })?;
    let tier = kyc_tier.unwrap_or_else(|| "TIER_1".into());
    if let Some(msg) = check_daily_limit(pool, user_id, amount_ngn, &tier, config).await {
        return Err(UtilityPurchaseErr { user_message: msg });
    }
    let keypair =
        get_keypair_for_user(encrypted_keypair, &config.wallet_encryption_key).map_err(|e| {
            UtilityPurchaseErr {
                user_message: format!("Wallet error: {e}"),
            }
        })?;
    let balance = get_spl_token_balance(rpc, &pubkey, &config.usdc_mint).await;
    let min_usdc = usdc + config.gas_fee_usdc as f64 / 1_000_000.0;
    if balance < min_usdc {
        return Err(UtilityPurchaseErr {
            user_message: "Insufficient USDC balance for this purchase.".into(),
        });
    }
    let phone_air = phone_local_nigeria_11_digits(&phone);
    let data = json!({
        "pubKey": pubkey,
        "token": "USDC",
        "amount": airbills_ngn_whole(amount_ngn),
        "phoneNumber": phone_air,
        "networkId": network_id,
        "prodId": prod_id,
        "batch": batch,
    });
    let req = TransactRequest {
        product_code: "102".into(),
        pay_with: "default".into(),
        data,
    };
    let t = airbills::transact(config, &req)
        .await
        .map_err(|e: AirbillsError| UtilityPurchaseErr {
            user_message: format!("Airbills: {}", e.message),
        })?;
    insert_order_pending(
        pool,
        uid,
        &t.id,
        "102",
        amount_ngn,
        t.amount_in_token,
        rate,
        json!({"type":"data","phone": phone, "prodId": prod_id, "batch": batch}),
    )
    .await
    .map_err(|e| UtilityPurchaseErr { user_message: e })?;
    let sig = sign_send_process(pool, rpc, config, &keypair, "102", &t.id, &t.transaction_ix)
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Payment failed: {e}"),
        })?;
    Ok(UtilityPurchaseOk {
        chain_signature: sig,
        airbills_id: t.id,
    })
}

pub async fn purchase_electricity(
    pool: &Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    encrypted_keypair: &str,
    meter_no: &str,
    elect_id: &str,
    batch: &str,
    prod_id: &str,
    amount_ngn: f64,
) -> Result<UtilityPurchaseOk, UtilityPurchaseErr> {
    if amount_ngn < 2000.0 {
        return Err(UtilityPurchaseErr {
            user_message: "Electricity amount must be at least 2,000 NGN.".into(),
        });
    }
    let rate = get_usd_to_ngn_rate(config).await;
    let usdc = ngn_to_usd(amount_ngn, rate);
    let uid = Uuid::parse_str(user_id).map_err(|_| UtilityPurchaseErr {
        user_message: "Invalid account.".into(),
    })?;
    let client = pool.get().await.map_err(|e| UtilityPurchaseErr {
        user_message: format!("Database error: {e}"),
    })?;
    let row = client
        .query_opt(
            "SELECT solana_pubkey, kyc_tier::text FROM users WHERE id = $1",
            &[&uid],
        )
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Database error: {e}"),
        })?;
    let (pubkey_opt, kyc_tier): (Option<String>, Option<String>) =
        row.map(|r| (r.get(0), r.get(1))).unwrap_or((None, None));
    let pubkey: String = pubkey_opt.ok_or_else(|| UtilityPurchaseErr {
        user_message: "Wallet not set up.".into(),
    })?;
    let tier = kyc_tier.unwrap_or_else(|| "TIER_1".into());
    if let Some(msg) = check_daily_limit(pool, user_id, amount_ngn, &tier, config).await {
        return Err(UtilityPurchaseErr { user_message: msg });
    }
    let keypair =
        get_keypair_for_user(encrypted_keypair, &config.wallet_encryption_key).map_err(|e| {
            UtilityPurchaseErr {
                user_message: format!("Wallet error: {e}"),
            }
        })?;
    let balance = get_spl_token_balance(rpc, &pubkey, &config.usdc_mint).await;
    let min_usdc = usdc + config.gas_fee_usdc as f64 / 1_000_000.0;
    if balance < min_usdc {
        return Err(UtilityPurchaseErr {
            user_message: "Insufficient USDC balance for this purchase.".into(),
        });
    }
    let data = json!({
        "pubKey": pubkey,
        "token": "USDC",
        "amount": airbills_ngn_whole(amount_ngn),
        "meterNo": meter_no,
        "electId": elect_id,
        "prodId": prod_id,
        "batch": batch,
    });
    let req = TransactRequest {
        product_code: "101".into(),
        pay_with: "default".into(),
        data,
    };
    let t = airbills::transact(config, &req)
        .await
        .map_err(|e: AirbillsError| UtilityPurchaseErr {
            user_message: format!("Airbills: {}", e.message),
        })?;
    insert_order_pending(
        pool,
        uid,
        &t.id,
        "101",
        amount_ngn,
        t.amount_in_token,
        rate,
        json!({"type":"electricity","meter": meter_no, "electId": elect_id, "batch": batch}),
    )
    .await
    .map_err(|e| UtilityPurchaseErr { user_message: e })?;
    let sig = sign_send_process(pool, rpc, config, &keypair, "101", &t.id, &t.transaction_ix)
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Payment failed: {e}"),
        })?;
    Ok(UtilityPurchaseOk {
        chain_signature: sig,
        airbills_id: t.id,
    })
}

pub async fn purchase_betting(
    pool: &Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    encrypted_keypair: &str,
    customer_id: &str,
    prod_id: &str,
    amount_ngn: f64,
) -> Result<UtilityPurchaseOk, UtilityPurchaseErr> {
    if !(1_000.0..=100_000.0).contains(&amount_ngn) {
        return Err(UtilityPurchaseErr {
            user_message: "Betting amount must be between 1,000 and 100,000 NGN.".into(),
        });
    }
    let rate = get_usd_to_ngn_rate(config).await;
    let usdc = ngn_to_usd(amount_ngn, rate);
    let uid = Uuid::parse_str(user_id).map_err(|_| UtilityPurchaseErr {
        user_message: "Invalid account.".into(),
    })?;
    let client = pool.get().await.map_err(|e| UtilityPurchaseErr {
        user_message: format!("Database error: {e}"),
    })?;
    let row = client
        .query_opt(
            "SELECT solana_pubkey, kyc_tier::text FROM users WHERE id = $1",
            &[&uid],
        )
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Database error: {e}"),
        })?;
    let (pubkey_opt, kyc_tier): (Option<String>, Option<String>) =
        row.map(|r| (r.get(0), r.get(1))).unwrap_or((None, None));
    let pubkey: String = pubkey_opt.ok_or_else(|| UtilityPurchaseErr {
        user_message: "Wallet not set up.".into(),
    })?;
    let tier = kyc_tier.unwrap_or_else(|| "TIER_1".into());
    if let Some(msg) = check_daily_limit(pool, user_id, amount_ngn, &tier, config).await {
        return Err(UtilityPurchaseErr { user_message: msg });
    }
    let keypair =
        get_keypair_for_user(encrypted_keypair, &config.wallet_encryption_key).map_err(|e| {
            UtilityPurchaseErr {
                user_message: format!("Wallet error: {e}"),
            }
        })?;
    let balance = get_spl_token_balance(rpc, &pubkey, &config.usdc_mint).await;
    let min_usdc = usdc + config.gas_fee_usdc as f64 / 1_000_000.0;
    if balance < min_usdc {
        return Err(UtilityPurchaseErr {
            user_message: "Insufficient USDC balance for this purchase.".into(),
        });
    }
    let data = json!({
        "pubKey": pubkey,
        "token": "USDC",
        "amount": airbills_ngn_whole(amount_ngn),
        "customerId": customer_id.trim(),
        "prodId": prod_id.trim(),
    });
    let req = TransactRequest {
        product_code: "103".into(),
        pay_with: "default".into(),
        data,
    };
    let t = airbills::transact(config, &req)
        .await
        .map_err(|e: AirbillsError| UtilityPurchaseErr {
            user_message: format!("Airbills: {}", e.message),
        })?;
    insert_order_pending(
        pool,
        uid,
        &t.id,
        "103",
        amount_ngn,
        t.amount_in_token,
        rate,
        json!({"type":"betting","customerId": customer_id.trim(), "prodId": prod_id.trim()}),
    )
    .await
    .map_err(|e| UtilityPurchaseErr { user_message: e })?;
    let sig = sign_send_process(pool, rpc, config, &keypair, "103", &t.id, &t.transaction_ix)
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Payment failed: {e}"),
        })?;
    Ok(UtilityPurchaseOk {
        chain_signature: sig,
        airbills_id: t.id,
    })
}

pub async fn purchase_cable_tv(
    pool: &Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    encrypted_keypair: &str,
    smart_card_no: &str,
    contact_phone: &str,
    prod_id: &str,
    amount_ngn: f64,
) -> Result<UtilityPurchaseOk, UtilityPurchaseErr> {
    if !(100.0..=500_000.0).contains(&amount_ngn) {
        return Err(UtilityPurchaseErr {
            user_message: "Cable amount must be between 100 and 500,000 NGN.".into(),
        });
    }
    let phone = normalize_nigerian_phone(contact_phone);
    let phone_air = phone_local_nigeria_11_digits(&phone);
    if phone_air.len() != 11 {
        return Err(UtilityPurchaseErr {
            user_message: "Invalid phone for cable subscription.".into(),
        });
    }
    let rate = get_usd_to_ngn_rate(config).await;
    let usdc = ngn_to_usd(amount_ngn, rate);
    let uid = Uuid::parse_str(user_id).map_err(|_| UtilityPurchaseErr {
        user_message: "Invalid account.".into(),
    })?;
    let client = pool.get().await.map_err(|e| UtilityPurchaseErr {
        user_message: format!("Database error: {e}"),
    })?;
    let row = client
        .query_opt(
            "SELECT solana_pubkey, kyc_tier::text FROM users WHERE id = $1",
            &[&uid],
        )
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Database error: {e}"),
        })?;
    let (pubkey_opt, kyc_tier): (Option<String>, Option<String>) =
        row.map(|r| (r.get(0), r.get(1))).unwrap_or((None, None));
    let pubkey: String = pubkey_opt.ok_or_else(|| UtilityPurchaseErr {
        user_message: "Wallet not set up.".into(),
    })?;
    let tier = kyc_tier.unwrap_or_else(|| "TIER_1".into());
    if let Some(msg) = check_daily_limit(pool, user_id, amount_ngn, &tier, config).await {
        return Err(UtilityPurchaseErr { user_message: msg });
    }
    let keypair =
        get_keypair_for_user(encrypted_keypair, &config.wallet_encryption_key).map_err(|e| {
            UtilityPurchaseErr {
                user_message: format!("Wallet error: {e}"),
            }
        })?;
    let balance = get_spl_token_balance(rpc, &pubkey, &config.usdc_mint).await;
    let min_usdc = usdc + config.gas_fee_usdc as f64 / 1_000_000.0;
    if balance < min_usdc {
        return Err(UtilityPurchaseErr {
            user_message: "Insufficient USDC balance for this purchase.".into(),
        });
    }
    let data = json!({
        "pubKey": pubkey,
        "token": "USDC",
        "amount": airbills_ngn_whole(amount_ngn),
        "smartCardNo": smart_card_no.trim(),
        "phoneNumber": phone_air,
        "prodId": prod_id.trim(),
    });
    let req = TransactRequest {
        product_code: "104".into(),
        pay_with: "default".into(),
        data,
    };
    let t = airbills::transact(config, &req)
        .await
        .map_err(|e: AirbillsError| UtilityPurchaseErr {
            user_message: format!("Airbills: {}", e.message),
        })?;
    insert_order_pending(
        pool,
        uid,
        &t.id,
        "104",
        amount_ngn,
        t.amount_in_token,
        rate,
        json!({"type":"cable","smartCard": smart_card_no.trim(), "prodId": prod_id.trim(), "phone": phone}),
    )
    .await
    .map_err(|e| UtilityPurchaseErr { user_message: e })?;
    let sig = sign_send_process(pool, rpc, config, &keypair, "104", &t.id, &t.transaction_ix)
        .await
        .map_err(|e| UtilityPurchaseErr {
            user_message: format!("Payment failed: {e}"),
        })?;
    Ok(UtilityPurchaseOk {
        chain_signature: sig,
        airbills_id: t.id,
    })
}
