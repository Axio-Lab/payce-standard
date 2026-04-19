use deadpool_postgres::Pool;
use serde_json::Value;
use uuid::Uuid;

pub const EVT_P2P_SEND: &str = "P2P_SEND";
pub const EVT_EXTERNAL_SEND: &str = "EXTERNAL_SEND";
pub const EVT_MERCHANT_PAYMENT: &str = "MERCHANT_PAYMENT";
pub const EVT_SWAP: &str = "SWAP";
pub const EVT_EARN_DEPOSIT: &str = "EARN_DEPOSIT";
pub const EVT_EARN_WITHDRAW: &str = "EARN_WITHDRAW";
pub const EVT_UTILITY_BILL: &str = "UTILITY_BILL";
pub const EVT_PAJ_ONRAMP: &str = "PAJ_ONRAMP";
pub const EVT_PAJ_OFFRAMP: &str = "PAJ_OFFRAMP";

pub const ST_PENDING: &str = "PENDING";
pub const ST_CONFIRMED: &str = "CONFIRMED";
pub const ST_FAILED: &str = "FAILED";

#[allow(clippy::too_many_arguments)]
async fn insert_ledger_row(
    pool: &Pool,
    user_id: &str,
    event_type: &str,
    status: &str,
    tx_signature: Option<&str>,
    amount_raw: Option<i64>,
    denom_mint: Option<&str>,
    amount_ngn: Option<f64>,
    exchange_rate: Option<f64>,
    counterparty_user_id: Option<&str>,
    ref_id: Option<&str>,
    metadata: Value,
) {
    let Ok(uid) = Uuid::parse_str(user_id) else {
        log::warn!("[user_activity] skip: invalid user_id {user_id}");
        return;
    };
    let cp = counterparty_user_id.and_then(|s| Uuid::parse_str(s).ok());
    let Ok(client) = pool.get().await else {
        log::warn!("[user_activity] skip: pool get failed");
        return;
    };

    let sql = if ref_id.map(|s| !s.trim().is_empty()).unwrap_or(false) {
        "INSERT INTO user_activity (
            user_id, event_type, status, tx_signature, amount_raw, denom_mint,
            amount_ngn, exchange_rate, counterparty_user_id, ref_id, metadata
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        ON CONFLICT (ref_id, event_type) WHERE ref_id IS NOT NULL AND btrim(ref_id) <> ''
        DO UPDATE SET
            status = CASE WHEN user_activity.status IN ('CONFIRMED','FAILED')
                          THEN user_activity.status ELSE EXCLUDED.status END,
            tx_signature = COALESCE(user_activity.tx_signature, EXCLUDED.tx_signature),
            amount_raw = COALESCE(user_activity.amount_raw, EXCLUDED.amount_raw),
            denom_mint = COALESCE(user_activity.denom_mint, EXCLUDED.denom_mint),
            amount_ngn = COALESCE(user_activity.amount_ngn, EXCLUDED.amount_ngn),
            exchange_rate = COALESCE(user_activity.exchange_rate, EXCLUDED.exchange_rate),
            counterparty_user_id = COALESCE(user_activity.counterparty_user_id, EXCLUDED.counterparty_user_id),
            metadata = user_activity.metadata || EXCLUDED.metadata,
            updated_at = NOW()"
    } else {
        "INSERT INTO user_activity (
            user_id, event_type, status, tx_signature, amount_raw, denom_mint,
            amount_ngn, exchange_rate, counterparty_user_id, ref_id, metadata
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"
    };

    if let Err(e) = client
        .execute(
            sql,
            &[
                &uid,
                &event_type,
                &status,
                &tx_signature,
                &amount_raw,
                &denom_mint,
                &amount_ngn,
                &exchange_rate,
                &cp,
                &ref_id,
                &metadata,
            ],
        )
        .await
    {
        log::warn!("[user_activity] insert failed: {e}");
    }
}

#[allow(clippy::too_many_arguments)]
pub fn log_user_activity(
    pool: &Pool,
    user_id: &str,
    event_type: &str,
    status: &str,
    tx_signature: Option<&str>,
    amount_raw: Option<i64>,
    denom_mint: Option<&str>,
    amount_ngn: Option<f64>,
    exchange_rate: Option<f64>,
    counterparty_user_id: Option<&str>,
    ref_id: Option<&str>,
    metadata: Value,
) {
    let pool = pool.clone();
    let user_id = user_id.to_string();
    let event_type = event_type.to_string();
    let status = status.to_string();
    let tx_signature = tx_signature.map(String::from);
    let denom_mint = denom_mint.map(String::from);
    let counterparty_user_id = counterparty_user_id.map(String::from);
    let ref_id = ref_id.map(String::from);
    tokio::spawn(async move {
        insert_ledger_row(
            &pool,
            &user_id,
            &event_type,
            &status,
            tx_signature.as_deref(),
            amount_raw,
            denom_mint.as_deref(),
            amount_ngn,
            exchange_rate,
            counterparty_user_id.as_deref(),
            ref_id.as_deref(),
            metadata,
        )
        .await;
    });
}

async fn patch_ledger_by_ref(
    pool: &Pool,
    ref_id: &str,
    event_type: &str,
    status: &str,
    tx_signature: Option<&str>,
    metadata_patch: Value,
) {
    let Ok(client) = pool.get().await else {
        log::warn!("[user_activity] patch skip: pool get failed");
        return;
    };

    let updated = client
        .execute(
            "UPDATE user_activity SET \
                status = CASE WHEN status IN ('CONFIRMED','FAILED') THEN status ELSE $3 END, \
                tx_signature = COALESCE($4, tx_signature), \
                metadata = metadata || $5::jsonb, \
                updated_at = NOW() \
             WHERE ref_id = $1 AND event_type = $2",
            &[
                &ref_id,
                &event_type,
                &status,
                &tx_signature,
                &metadata_patch,
            ],
        )
        .await;

    match updated {
        Ok(0) => {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if let Err(e) = client
                .execute(
                    "UPDATE user_activity SET \
                        status = CASE WHEN status IN ('CONFIRMED','FAILED') THEN status ELSE $3 END, \
                        tx_signature = COALESCE($4, tx_signature), \
                        metadata = metadata || $5::jsonb, \
                        updated_at = NOW() \
                     WHERE ref_id = $1 AND event_type = $2",
                    &[
                        &ref_id,
                        &event_type,
                        &status,
                        &tx_signature,
                        &metadata_patch,
                    ],
                )
                .await
            {
                log::warn!("[user_activity] patch retry failed: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("[user_activity] patch failed: {e}"),
    }
}

pub fn patch_user_activity_by_ref(
    pool: &Pool,
    ref_id: &str,
    event_type: &str,
    status: &str,
    tx_signature: Option<&str>,
    metadata_patch: Value,
) {
    let pool = pool.clone();
    let ref_id = ref_id.to_string();
    let event_type = event_type.to_string();
    let status = status.to_string();
    let tx_signature = tx_signature.map(String::from);
    tokio::spawn(async move {
        patch_ledger_by_ref(
            &pool,
            &ref_id,
            &event_type,
            &status,
            tx_signature.as_deref(),
            metadata_patch,
        )
        .await;
    });
}
