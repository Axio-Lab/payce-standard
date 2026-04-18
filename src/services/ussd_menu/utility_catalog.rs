use redis::AsyncCommands;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::services::airbills::InternetPlanItem;

pub const UTILITY_CATALOG_TTL_SECS: u64 = 900;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BetCatalog {
    pub customer_id: String,
    pub plans: Vec<InternetPlanItem>,
    #[serde(default)]
    pub selected_prod_id: Option<String>,
    #[serde(default)]
    pub menu_sub_len: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CableCatalog {
    pub smart_card: String,
    pub plans: Vec<InternetPlanItem>,
    #[serde(default)]
    pub selected_prod_id: Option<String>,
    #[serde(default)]
    pub menu_sub_len: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElectCatalog {
    pub meter_no: String,
    pub elect_id: String,
    pub plans: Vec<InternetPlanItem>,
    #[serde(default)]
    pub plan_sub_offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElectDiscoPick {
    pub discos: Vec<InternetPlanItem>,
    #[serde(default)]
    pub selected_elect_id: Option<String>,
    #[serde(default)]
    pub menu_sub_len: Option<usize>,
}

pub fn bet_catalog_key(user_id: &str) -> String {
    format!("ussd:vend:bet:{user_id}")
}

pub fn elect_disco_pick_key(user_id: &str) -> String {
    format!("ussd:vend:elect_pick:{user_id}")
}

pub fn elect_active_catalog_key(user_id: &str) -> String {
    format!("ussd:vend:elect_active:{user_id}")
}

pub fn cable_active_catalog_key(user_id: &str) -> String {
    format!("ussd:vend:cable_active:{user_id}")
}

pub async fn redis_store_json<T: Serialize>(
    redis: &redis::Client,
    key: &str,
    value: &T,
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let json = serde_json::to_string(value).map_err(|e| format!("json: {e}"))?;
    conn.set_ex::<_, _, ()>(key, &json, UTILITY_CATALOG_TTL_SECS)
        .await
        .map_err(|e| format!("redis set: {e}"))?;
    Ok(())
}

pub async fn redis_load_json<T: DeserializeOwned>(redis: &redis::Client, key: &str) -> Option<T> {
    let mut conn = redis.get_multiplexed_async_connection().await.ok()?;
    let json: Option<String> = conn.get(key).await.ok()?;
    let s = json.as_ref()?;
    serde_json::from_str(s).ok()
}

pub async fn redis_delete_key(redis: &redis::Client, key: &str) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let _: () = conn.del(key).await.map_err(|e| format!("redis del: {e}"))?;
    Ok(())
}
