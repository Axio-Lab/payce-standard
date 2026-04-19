use deadpool_postgres::Pool;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::user_activity::{
    log_user_activity, patch_user_activity_by_ref, EVT_PAJ_OFFRAMP, EVT_PAJ_ONRAMP, ST_CONFIRMED,
    ST_FAILED, ST_PENDING,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PajOrderDirection {
    Onramp,
    Offramp,
}

impl PajOrderDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Onramp => "onramp",
            Self::Offramp => "offramp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PajOrderStatus {
    Pending,
    Success,
    Failed,
    Unknown,
}

impl PajOrderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        let t = s.trim().to_ascii_lowercase();
        match t.as_str() {
            "success" | "completed" | "paid" | "succeeded" | "done" => Self::Success,
            "failed" | "failure" | "cancelled" | "canceled" | "rejected" => Self::Failed,
            "pending" | "processing" | "init" | "in_progress" | "awaiting" => Self::Pending,
            _ => Self::Unknown,
        }
    }
}

pub async fn insert_paj_order(
    pool: &Pool,
    id: Uuid,
    user_id: Uuid,
    direction: PajOrderDirection,
    paj_order_id: Option<&str>,
    mint: Option<&str>,
    chain: Option<&str>,
    currency: Option<&str>,
    request_json: &Value,
    response_json: &Value,
) -> Result<(), String> {
    let client = pool.get().await.map_err(|e| e.to_string())?;
    let dir = direction.as_str();
    client
        .execute(
            "INSERT INTO paj_orders (id, user_id, direction, status, paj_order_id, mint, chain, currency, request_json, response_json) \
             VALUES ($1, $2, $3, 'pending', $4, $5, $6, $7, $8, $9)",
            &[
                &id,
                &user_id,
                &dir,
                &paj_order_id,
                &mint,
                &chain,
                &currency,
                request_json,
                response_json,
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    let evt = match direction {
        PajOrderDirection::Onramp => EVT_PAJ_ONRAMP,
        PajOrderDirection::Offramp => EVT_PAJ_OFFRAMP,
    };
    log_user_activity(
        pool,
        &user_id.to_string(),
        evt,
        ST_PENDING,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&id.to_string()),
        json!({
            "paj_order_id": paj_order_id,
            "mint": mint,
            "chain": chain,
            "currency": currency,
        }),
    );
    Ok(())
}

pub async fn apply_webhook_atomic(
    pool: &Pool,
    order_id: Uuid,
    status: PajOrderStatus,
    paj_order_id: Option<&str>,
    last_payload: &Value,
) -> Result<Option<(Uuid, String)>, String> {
    let mut client = pool.get().await.map_err(|e| e.to_string())?;
    let tx = client
        .transaction()
        .await
        .map_err(|e| format!("begin tx: {e}"))?;

    let status_s = status.as_str();
    let row = tx
        .query_opt(
            "UPDATE paj_orders SET \
                status = CASE WHEN status IN ('success','failed') THEN status ELSE $2 END, \
                paj_order_id = COALESCE(NULLIF(btrim($3), ''), paj_order_id), \
                last_webhook_payload = $4, \
                updated_at = NOW() \
             WHERE id = $1 \
             RETURNING user_id, direction",
            &[&order_id, &status_s, &paj_order_id, last_payload],
        )
        .await
        .map_err(|e| format!("update paj_orders: {e}"))?;

    let pair = row.map(|r| (r.get::<_, Uuid>(0), r.get::<_, String>(1)));

    if pair.is_some() {
        tx.execute(
            "INSERT INTO paj_order_events (order_id, payload) VALUES ($1, $2)",
            &[&order_id, last_payload],
        )
        .await
        .map_err(|e| format!("insert paj_order_events: {e}"))?;
    }

    tx.commit().await.map_err(|e| format!("commit tx: {e}"))?;

    if let Some((_uid, dir)) = &pair {
        let evt = if dir == "onramp" {
            EVT_PAJ_ONRAMP
        } else {
            EVT_PAJ_OFFRAMP
        };
        let ua_status = match status {
            PajOrderStatus::Success => ST_CONFIRMED,
            PajOrderStatus::Failed => ST_FAILED,
            _ => ST_PENDING,
        };
        let patch = json!({
            "paj_status": status_s,
            "paj_order_id": paj_order_id,
        });
        patch_user_activity_by_ref(pool, &order_id.to_string(), evt, ua_status, None, patch);
    }
    Ok(pair)
}

pub async fn fetch_user_contact(
    pool: &Pool,
    user_id: Uuid,
) -> Result<(Option<String>, String), String> {
    let client = pool.get().await.map_err(|e| e.to_string())?;
    let row = client
        .query_opt(
            "SELECT email, phone_number FROM users WHERE id = $1",
            &[&user_id],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(r) = row else {
        return Ok((None, String::new()));
    };
    let email: Option<String> = r.get(0);
    let phone: String = r.get(1);
    Ok((email, phone))
}
