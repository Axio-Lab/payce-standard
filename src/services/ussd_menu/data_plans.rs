use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::services::airbills::InternetPlanItem;
use crate::services::exchange_rate::format_ngn;

use super::text::truncate_ussd_label;

pub const DATA_PLAN_PAGE_SIZE: usize = 4;
pub const DATA_CATALOG_TTL_SECS: u64 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataPlanBucket {
    Daily,
    Weekly,
    Monthly,
    Other,
}

pub fn bucket_public_label(bucket: DataPlanBucket) -> &'static str {
    match bucket {
        DataPlanBucket::Daily => "Daily",
        DataPlanBucket::Weekly => "Weekly",
        DataPlanBucket::Monthly => "Monthly",
        DataPlanBucket::Other => "Other",
    }
}

pub fn classify_data_plan_bucket(plan: &InternetPlanItem) -> DataPlanBucket {
    let t = format!("{} {}", plan.label, plan.prod_id).to_lowercase();
    if t.contains("month")
        || t.contains("30 day")
        || t.contains("30-day")
        || t.contains("30days")
        || t.contains("28 day")
        || t.contains("28-day")
        || t.contains("31 day")
        || t.contains("60 day")
        || t.contains("90 day")
        || t.contains("90-day")
        || t.contains("quarter")
    {
        return DataPlanBucket::Monthly;
    }
    if t.contains("week") || t.contains("7 day") || t.contains("7-day") || t.contains("7days") {
        return DataPlanBucket::Weekly;
    }
    if t.contains("daily")
        || t.contains("24 hour")
        || t.contains("24hour")
        || t.contains("24hr")
        || t.contains("24 hr")
        || t.contains("24h")
        || t.contains("1 day")
        || t.contains("1-day")
        || t.contains("2 day")
        || t.contains("2-day")
        || t.contains("3 day")
        || t.contains("3-day")
    {
        return DataPlanBucket::Daily;
    }
    if t.contains(" day") || t.contains("-day") || t.contains("days") {
        return DataPlanBucket::Daily;
    }
    DataPlanBucket::Other
}

pub fn plans_for_bucket(
    plans: &[InternetPlanItem],
    bucket: DataPlanBucket,
) -> Vec<InternetPlanItem> {
    plans
        .iter()
        .filter(|p| classify_data_plan_bucket(p) == bucket)
        .cloned()
        .collect()
}

pub fn nonempty_bucket_menu(plans: &[InternetPlanItem]) -> Vec<DataPlanBucket> {
    const ORDER: [DataPlanBucket; 4] = [
        DataPlanBucket::Daily,
        DataPlanBucket::Weekly,
        DataPlanBucket::Monthly,
        DataPlanBucket::Other,
    ];
    ORDER
        .into_iter()
        .filter(|b| !plans_for_bucket(plans, *b).is_empty())
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataPlanCatalog {
    pub batch: String,
    pub network_display: String,
    pub plans: Vec<InternetPlanItem>,
    #[serde(default)]
    pub active_bucket: Option<DataPlanBucket>,
}

pub fn data_catalog_redis_key(user_id: &str, phone_local: &str) -> String {
    format!("ussd:data_catalog:{user_id}:{phone_local}")
}

pub async fn redis_store_data_catalog(
    redis: &redis::Client,
    key: &str,
    catalog: &DataPlanCatalog,
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let json = serde_json::to_string(catalog).map_err(|e| format!("json: {e}"))?;
    conn.set_ex::<_, _, ()>(key, &json, DATA_CATALOG_TTL_SECS)
        .await
        .map_err(|e| format!("redis set: {e}"))?;
    Ok(())
}

pub async fn redis_load_data_catalog(redis: &redis::Client, key: &str) -> Option<DataPlanCatalog> {
    let mut conn = redis.get_multiplexed_async_connection().await.ok()?;
    let json: Option<String> = conn.get(key).await.ok()?;
    let s = json.as_ref()?;
    serde_json::from_str(s).ok()
}

pub fn format_data_bucket_menu_ussd(
    network_display: &str,
    buckets: &[DataPlanBucket],
    plans: &[InternetPlanItem],
) -> String {
    let raw = format!("{} data", network_display.trim());
    let title = truncate_ussd_label(&raw, 22);
    let mut lines: Vec<String> = vec![format!("CON {title} — pick type:")];
    for (i, b) in buckets.iter().enumerate() {
        let n = plans_for_bucket(plans, *b).len();
        let label = bucket_public_label(*b);
        lines.push(format!("{}. {} ({})", i + 1, label, n));
    }
    lines.join("\n")
}

pub fn format_data_plan_menu_body_ex(
    plans: &[InternetPlanItem],
    page: usize,
    page_size: usize,
    menu_title: &str,
    show_previous_page: bool,
) -> String {
    let total_pages = std::cmp::max(1, plans.len().div_ceil(page_size));
    let start = page * page_size;
    let raw = if menu_title.trim().is_empty() {
        "Plans"
    } else {
        menu_title.trim()
    };
    let title = truncate_ussd_label(raw, 22);
    let mut lines: Vec<String> = vec![format!("{title} (page {}/{}):", page + 1, total_pages)];
    for j in 0..page_size {
        let idx = start + j;
        if idx >= plans.len() {
            break;
        }
        let p = &plans[idx];
        let amt = p
            .amount_ngn
            .map(|a| format!(" {}", format_ngn(a)))
            .unwrap_or_default();
        let label = truncate_ussd_label(&p.label, 26);
        lines.push(format!("{}. {}{}", j + 1, label, amt));
    }
    if (page + 1) * page_size < plans.len() {
        lines.push(format!("{}. More plans", page_size + 1));
    }
    if page > 0 && show_previous_page {
        lines.push(format!("{}. Previous page", page_size + 2));
    }
    lines.join("\n")
}

pub enum DataPlanNav {
    ShowMenu { page: usize },
    Picked { plan_index: usize, consumed: usize },
}

pub fn format_data_plan_menu_ussd(
    plans: &[InternetPlanItem],
    page: usize,
    page_size: usize,
    menu_title: &str,
) -> String {
    format_data_plan_menu_ussd_ex(plans, page, page_size, menu_title, true)
}

pub fn format_data_plan_menu_ussd_ex(
    plans: &[InternetPlanItem],
    page: usize,
    page_size: usize,
    menu_title: &str,
    show_previous_page: bool,
) -> String {
    format!(
        "CON {}",
        format_data_plan_menu_body_ex(plans, page, page_size, menu_title, show_previous_page)
    )
}

pub fn parse_data_plan_menu_input_with_page_ex(
    rest: &[String],
    plan_count: usize,
    page_size: usize,
    allow_previous_page: bool,
) -> Result<DataPlanNav, (&'static str, usize)> {
    let mut page: usize = 0;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "5" => {
                if (page + 1) * page_size < plan_count {
                    page += 1;
                    i += 1;
                } else {
                    let msg = if allow_previous_page {
                        "No more plans. Use 6 for previous or pick 1-4."
                    } else {
                        "No more plans. Pick 1-4."
                    };
                    return Err((msg, page));
                }
            }
            "6" => {
                if !allow_previous_page {
                    return Err(("Pick 1-4 for a plan or 5 for next page.", page));
                }
                if page > 0 {
                    page -= 1;
                    i += 1;
                } else {
                    return Err(("Already on the first page of plans.", page));
                }
            }
            "1" | "2" | "3" | "4" => {
                let slot: usize = match rest[i].parse() {
                    Ok(s) if (1..=page_size).contains(&s) => s,
                    _ => return Err(("Pick a plan from 1 to 4 on this page.", page)),
                };
                let idx = page * page_size + (slot - 1);
                if idx >= plan_count {
                    return Err(("That option is not available.", page));
                }
                i += 1;
                return Ok(DataPlanNav::Picked {
                    plan_index: idx,
                    consumed: i,
                });
            }
            _ => {
                let msg = if allow_previous_page {
                    "Pick 1-4 for a plan, 5 for next page, 6 for previous."
                } else {
                    "Pick 1-4 for a plan or 5 for next page."
                };
                return Err((msg, page));
            }
        }
    }
    Ok(DataPlanNav::ShowMenu { page })
}

pub fn parse_data_plan_menu_input(
    rest: &[String],
    plan_count: usize,
    page_size: usize,
) -> Result<DataPlanNav, &'static str> {
    parse_data_plan_menu_input_with_page_ex(rest, plan_count, page_size, true)
        .map_err(|(msg, _)| msg)
}
