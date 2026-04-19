//! Airbills API: airtime - electricity - data - betting - cable TV - internet.
//! See https://business.airbills.org/docs

use std::collections::HashSet;

use crate::config::AppConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

const DATA_OBJECT_PRODUCT_ARRAY_KEYS: &[&str] = &[
    "bettingCompany",
    "betting_company",
    "cableCompany",
    "cable_company",
    "providers",
    "plans",
    "products",
    "bundles",
    "items",
    "list",
    "cables",
    "cable",
    "rows",
    "catalog",
];

fn data_object_first_product_array(obj: &Map<String, Value>) -> Option<&Vec<Value>> {
    for &key in DATA_OBJECT_PRODUCT_ARRAY_KEYS {
        if let Some(Value::Array(arr)) = obj.get(key) {
            return Some(arr);
        }
    }
    None
}

const VENDOR_PREFIX: &str = "/api/vendor/gateway";

#[derive(Debug, Clone)]
pub struct AirbillsError {
    pub message: String,
    pub status: Option<String>,
}

impl std::fmt::Display for AirbillsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref s) = self.status {
            write!(f, "[{s}] {}", self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for AirbillsError {}

fn vendor_url(config: &AppConfig, path: &str) -> String {
    format!("{}{}{}", config.airbills_base_url, VENDOR_PREFIX, path)
}

fn normalize_status(v: &Value) -> Option<String> {
    v.get("status").and_then(|s| {
        s.as_str()
            .map(String::from)
            .or_else(|| s.as_i64().map(|n| format!("{n:02}")))
            .or_else(|| s.as_u64().map(|n| format!("{n:02}")))
    })
}

fn is_success_status(st: &str) -> bool {
    st == "00" || st == "0"
}

fn vendor_bearer_header(config: &AppConfig) -> String {
    format!("Bearer {}", config.airbills_api_key)
}

fn log_airbills_rejection(path: &str, http_status: reqwest::StatusCode, body: &Value) {
    let snippet = body.to_string();
    let snippet = if snippet.len() > 600 {
        format!("{}…", &snippet[..600])
    } else {
        snippet
    };
    log::warn!(
        "[Airbills] {path} HTTP {http_status} body={snippet}",
        path = path,
        http_status = http_status,
        snippet = snippet
    );
}

async fn vendor_get(config: &AppConfig, path: &str) -> Result<Value, AirbillsError> {
    vendor_get_query(config, path, &[]).await
}

async fn vendor_get_query(
    config: &AppConfig,
    path: &str,
    query: &[(&str, &str)],
) -> Result<Value, AirbillsError> {
    let url = vendor_url(config, path);
    let mut req = config
        .http
        .get(&url)
        .header("secretkey", &config.airbills_api_key)
        .header("Authorization", vendor_bearer_header(config))
        .header("Accept", "application/json");
    for (k, v) in query {
        req = req.query(&[(k, v)]);
    }
    let resp = req.send().await.map_err(|e| AirbillsError {
        message: format!("Airbills request failed: {e}"),
        status: None,
    })?;
    let status = resp.status();
    let body: Value = resp.json().await.map_err(|e| AirbillsError {
        message: format!("Airbills invalid JSON (HTTP {status}): {e}"),
        status: None,
    })?;
    if !status.is_success() {
        log_airbills_rejection(path, status, &body);
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Airbills error")
            .to_string();
        return Err(AirbillsError {
            message: format!("Airbills HTTP {status}: {msg}"),
            status: normalize_status(&body),
        });
    }
    if let Some(st) = normalize_status(&body) {
        if !is_success_status(&st) {
            log_airbills_rejection(path, status, &body);
            let msg = body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Request rejected")
                .to_string();
            return Err(AirbillsError {
                message: msg,
                status: Some(st),
            });
        }
    }
    Ok(body)
}

async fn vendor_post_json(
    config: &AppConfig,
    path: &str,
    body: &Value,
    allow_statuses: &[&str],
) -> Result<Value, AirbillsError> {
    let url = vendor_url(config, path);
    let resp = config
        .http
        .post(&url)
        .header("secretkey", &config.airbills_api_key)
        .header("Authorization", vendor_bearer_header(config))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| AirbillsError {
            message: format!("Airbills request failed: {e}"),
            status: None,
        })?;
    let status = resp.status();
    let parsed: Value = resp.json().await.map_err(|e| AirbillsError {
        message: format!("Airbills invalid JSON (HTTP {status}): {e}"),
        status: None,
    })?;
    if !status.is_success() {
        log_airbills_rejection(path, status, &parsed);
        let msg = parsed
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Airbills error")
            .to_string();
        return Err(AirbillsError {
            message: format!("Airbills HTTP {status}: {msg}"),
            status: normalize_status(&parsed),
        });
    }
    if let Some(st) = normalize_status(&parsed) {
        let ok = is_success_status(&st) || allow_statuses.iter().any(|a| a == &st);
        if !ok {
            log_airbills_rejection(path, status, &parsed);
            let msg = parsed
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Request rejected")
                .to_string();
            return Err(AirbillsError {
                message: msg,
                status: Some(st),
            });
        }
    }
    Ok(parsed)
}

pub async fn businesses_me(config: &AppConfig) -> Result<Value, AirbillsError> {
    let url = format!("{}/api/businesses/user/me", config.airbills_base_url);
    let resp = config
        .http
        .get(&url)
        .header(
            "Authorization",
            format!("Bearer {}", config.airbills_api_key),
        )
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| AirbillsError {
            message: format!("Airbills businesses/me failed: {e}"),
            status: None,
        })?;
    let status = resp.status();
    let body: Value = resp.json().await.map_err(|e| AirbillsError {
        message: format!("Airbills businesses/me bad JSON (HTTP {status}): {e}"),
        status: None,
    })?;
    if !status.is_success() {
        return Err(AirbillsError {
            message: format!("Airbills businesses/me HTTP {status}"),
            status: None,
        });
    }
    Ok(body)
}

pub async fn network_checker(config: &AppConfig, phone: &str) -> Result<String, AirbillsError> {
    let digits = phone.trim_start_matches('+').to_string();
    let v = vendor_get_query(config, "/network-checker", &[("phone", digits.as_str())]).await?;
    v.get("data")
        .and_then(|d| d.get("network"))
        .and_then(|n| n.as_str())
        .map(String::from)
        .ok_or_else(|| AirbillsError {
            message: "Missing network in Airbills response".into(),
            status: None,
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternetPlanItem {
    pub prod_id: String,
    pub label: String,
    #[serde(default)]
    pub amount_ngn: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<String>,
}

fn json_str_from_field(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn prod_id_from_object(obj: &Map<String, Value>) -> Option<String> {
    for key in [
        "prodId",
        "prod_id",
        "productId",
        "product_id",
        "productCode",
        "product_code",
        "code",
        "id",
        "planId",
        "plan_id",
    ] {
        if let Some(v) = obj.get(key) {
            if let Some(s) = json_str_from_field(v) {
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn label_from_object(obj: &Map<String, Value>) -> String {
    for key in [
        "prodName",
        "name",
        "title",
        "planName",
        "plan",
        "product_name",
        "description",
        "bundle",
    ] {
        if let Some(Value::String(s)) = obj.get(key) {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    prod_id_from_object(obj).unwrap_or_else(|| "Plan".into())
}

fn amount_from_object(obj: &Map<String, Value>) -> Option<f64> {
    for key in [
        "prodAmount",
        "amount",
        "price",
        "amountNgn",
        "amount_ngn",
        "cost",
        "planAmount",
    ] {
        if let Some(v) = obj.get(key) {
            let n = if let Some(n) = v.as_f64() {
                n
            } else if let Some(s) = v.as_str() {
                s.trim().parse().ok()?
            } else if let Some(n) = v.as_i64() {
                n as f64
            } else if let Some(n) = v.as_u64() {
                n as f64
            } else {
                continue;
            };
            if n > 0.0 {
                return Some(n);
            }
        }
    }
    None
}

fn collect_data_plan_network_arrays(
    plan_root: &Map<String, Value>,
    out: &mut Vec<InternetPlanItem>,
) {
    for (_network_key, val) in plan_root {
        if let Value::Array(arr) = val {
            for item in arr {
                push_plan_from_value(out, item, None);
            }
        }
    }
}

fn push_plan_from_value(out: &mut Vec<InternetPlanItem>, item: &Value, batch_hint: Option<&str>) {
    let Some(obj) = item.as_object() else {
        return;
    };
    let Some(prod_id) = prod_id_from_object(obj) else {
        return;
    };
    let label = label_from_object(obj);
    let amount_ngn = amount_from_object(obj);
    out.push(InternetPlanItem {
        prod_id,
        label,
        amount_ngn,
        batch: batch_hint.map(|s| s.to_string()),
    });
}

fn cable_tv_nested_total_rows(obj: &Map<String, Value>) -> Option<usize> {
    for wrap_key in ["CableTv", "cableTv", "cable_tv"] {
        if let Some(Value::Object(inner)) = obj.get(wrap_key) {
            let n: usize = inner
                .values()
                .filter_map(|v| v.as_array())
                .map(|a| a.len())
                .sum();
            return Some(n);
        }
    }
    None
}

fn parse_cable_tv_nested_lists(data_obj: &Map<String, Value>) -> Vec<InternetPlanItem> {
    let mut out = Vec::new();
    for wrap_key in ["CableTv", "cableTv", "cable_tv"] {
        let Some(Value::Object(inner)) = data_obj.get(wrap_key) else {
            continue;
        };
        for (_provider_bucket, val) in inner {
            if let Value::Array(arr) = val {
                for item in arr {
                    push_plan_from_value(&mut out, item, None);
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    out
}

pub fn parse_cable_tv_list(body: &Value) -> Vec<InternetPlanItem> {
    let Some(data) = body.get("data") else {
        return Vec::new();
    };
    if let Some(obj) = data.as_object() {
        let nested = parse_cable_tv_nested_lists(obj);
        if !nested.is_empty() {
            return nested;
        }
    }
    parse_internet_plan_list(body)
}

pub fn parse_internet_plan_list(body: &Value) -> Vec<InternetPlanItem> {
    let mut out = Vec::new();
    let Some(data) = body.get("data") else {
        return out;
    };
    if let Some(arr) = data.as_array() {
        for item in arr {
            push_plan_from_value(&mut out, item, None);
        }
        return out;
    }
    if let Some(obj) = data.as_object() {
        if let Some(arr) = data_object_first_product_array(obj) {
            for item in arr {
                push_plan_from_value(&mut out, item, None);
            }
            return out;
        }
        if let Some(Value::Object(plan_root)) = obj.get("dataPlan") {
            collect_data_plan_network_arrays(plan_root, &mut out);
        }
    }
    out
}

fn elect_row_matches_disco(item: &Value, elect_upper: &str) -> bool {
    let Some(obj) = item.as_object() else {
        return false;
    };
    for fld in [
        "electId",
        "elect_id",
        "discoId",
        "disco",
        "discoCode",
        "providerCode",
        "provider",
        "code",
    ] {
        if let Some(s) = obj.get(fld).and_then(json_str_from_field) {
            let t = s.trim();
            if !t.is_empty()
                && (t.eq_ignore_ascii_case(elect_upper)
                    || t.to_uppercase().contains(elect_upper)
                    || elect_upper.contains(&t.to_uppercase()))
            {
                return true;
            }
        }
    }
    false
}

fn elect_bucket_key_matches(key: &str, elect_upper: &str) -> bool {
    let k = key.trim();
    !k.eq_ignore_ascii_case("batch")
        && !k.eq_ignore_ascii_case("status")
        && (k.eq_ignore_ascii_case(elect_upper)
            || k.to_uppercase().contains(elect_upper)
            || elect_upper.contains(&k.to_uppercase()))
}

pub fn parse_elect_plans_for_disco(
    body: &Value,
    elect_id: &str,
    batch: &str,
) -> Vec<InternetPlanItem> {
    let elect = elect_id.trim().to_uppercase();
    let mut out = Vec::new();
    let Some(data) = body.get("data") else {
        return out;
    };
    if let Some(arr) = data.as_array() {
        for item in arr {
            if elect_row_matches_disco(item, &elect) {
                push_plan_from_value(&mut out, item, Some(batch));
            }
        }
        return out;
    }
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            if let Value::Array(arr) = val {
                if elect_bucket_key_matches(key, &elect) {
                    for item in arr {
                        push_plan_from_value(&mut out, item, Some(batch));
                    }
                    if !out.is_empty() {
                        return out;
                    }
                }
            }
        }
        for key in [
            "plans", "products", "bundles", "data", "items", "list", "rows", "catalog",
        ] {
            if let Some(Value::Array(arr)) = obj.get(key) {
                for item in arr {
                    if elect_row_matches_disco(item, &elect) {
                        push_plan_from_value(&mut out, item, Some(batch));
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
        }
        for (_key, val) in obj {
            if let Value::Array(arr) = val {
                for item in arr {
                    if elect_row_matches_disco(item, &elect) {
                        push_plan_from_value(&mut out, item, Some(batch));
                    }
                }
            }
        }
    }
    out
}

pub fn parse_vendor_product_list(body: &Value) -> Vec<InternetPlanItem> {
    parse_internet_plan_list(body)
}

pub fn parse_elect_disco_directory(body: &Value) -> Vec<InternetPlanItem> {
    let mut out: Vec<InternetPlanItem> = Vec::new();
    let Some(data) = body.get("data") else {
        return out;
    };

    if let Some(obj) = data.as_object() {
        const SKIP: &[&str] = &[
            "batch",
            "status",
            "discount",
            "dataPlan",
            "plans",
            "products",
            "bundles",
            "items",
            "list",
            "rows",
            "catalog",
            "data",
            "message",
            "bettingCompany",
            "betting_company",
        ];
        let skip: HashSet<&str> = SKIP.iter().copied().collect();
        for (key, val) in obj {
            if skip.contains(key.as_str()) {
                continue;
            }
            if let Value::Array(arr) = val {
                if arr.is_empty() {
                    continue;
                }
                let code = key.trim().to_uppercase();
                if code.is_empty() {
                    continue;
                }
                out.push(InternetPlanItem {
                    prod_id: code,
                    label: String::new(),
                    amount_ngn: None,
                    batch: None,
                });
            }
        }
    } else if let Some(arr) = data.as_array() {
        let mut seen = HashSet::new();
        for item in arr {
            let Some(o) = item.as_object() else {
                continue;
            };
            for fld in [
                "electId",
                "elect_id",
                "discoCode",
                "disco",
                "providerCode",
                "code",
            ] {
                if let Some(s) = o.get(fld).and_then(json_str_from_field) {
                    let code = s.trim().to_uppercase();
                    if !code.is_empty() && seen.insert(code.clone()) {
                        out.push(InternetPlanItem {
                            prod_id: code,
                            label: String::new(),
                            amount_ngn: None,
                            batch: None,
                        });
                    }
                    break;
                }
            }
        }
    }

    out.sort_by(|a, b| a.prod_id.cmp(&b.prod_id));
    out.dedup_by(|a, b| a.prod_id == b.prod_id);
    out
}

fn network_bucket_matches_json_key(detected: &str, json_key: &str) -> bool {
    let d = detected.trim().to_lowercase();
    let k = json_key.trim().to_lowercase();
    if d.is_empty() || k.is_empty() {
        return false;
    }
    if k == d {
        return true;
    }
    let d_first = d.split_whitespace().next().unwrap_or(d.as_str());
    let k_first = k.split_whitespace().next().unwrap_or(k.as_str());
    if k_first == d_first {
        return true;
    }
    if k.contains(&d) || d.contains(&k) {
        return true;
    }
    if (d_first.contains("9mobile") || d_first.contains("etisalat"))
        && (k_first.contains("9mobile") || k_first.contains("etisalat"))
    {
        return true;
    }
    false
}

pub fn internet_list_batch_from_body(body: &Value) -> String {
    body.get("data")
        .and_then(|d| d.get("batch"))
        .and_then(|b| b.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "01".to_string())
}

pub fn parse_internet_plan_list_for_network(body: &Value, network: &str) -> Vec<InternetPlanItem> {
    let mut out = Vec::new();
    let Some(data) = body.get("data").and_then(|d| d.as_object()) else {
        return parse_internet_plan_list(body);
    };
    let Some(Value::Object(plan_root)) = data.get("dataPlan") else {
        return parse_internet_plan_list(body);
    };
    for (key, val) in plan_root {
        if network_bucket_matches_json_key(network, key) {
            if let Value::Array(arr) = val {
                for item in arr {
                    push_plan_from_value(&mut out, item, None);
                }
            }
            return out;
        }
    }
    out
}

pub fn list_response_data_len(body: &Value) -> usize {
    let Some(data) = body.get("data") else {
        return 0;
    };
    if let Some(a) = data.as_array() {
        return a.len();
    }
    if let Some(obj) = data.as_object() {
        if let Some(n) = cable_tv_nested_total_rows(obj) {
            return n;
        }
        if let Some(a) = data_object_first_product_array(obj) {
            return a.len();
        }
        if let Some(Value::Object(plan_root)) = obj.get("dataPlan") {
            return plan_root
                .values()
                .filter_map(|v| v.as_array())
                .map(|a| a.len())
                .sum();
        }
    }
    0
}

pub async fn list_internet(config: &AppConfig) -> Result<Value, AirbillsError> {
    vendor_get(config, "/list/internet").await
}

pub async fn list_elect(config: &AppConfig) -> Result<Value, AirbillsError> {
    vendor_get(config, "/list/elect").await
}

pub async fn list_bet(config: &AppConfig) -> Result<Value, AirbillsError> {
    vendor_get(config, "/list/bet").await
}

pub async fn list_cable(config: &AppConfig) -> Result<Value, AirbillsError> {
    vendor_get(config, "/list/cable").await
}

pub async fn validate_elect(
    config: &AppConfig,
    batch: &str,
    meter_no: &str,
    elect_id: &str,
) -> Result<Value, AirbillsError> {
    let body = json!({
        "meterNo": meter_no,
        "electId": elect_id,
    });
    vendor_post_json(config, &format!("/validate/elect/{batch}"), &body, &[]).await
}

pub async fn list_tokens(config: &AppConfig) -> Result<Value, AirbillsError> {
    vendor_get(config, "/tokens").await
}

#[derive(Debug, Serialize)]
pub struct TransactRequest {
    pub product_code: String,
    pub pay_with: String,
    pub data: Value,
}

#[derive(Debug, Deserialize)]
pub struct TransactDefaultData {
    pub id: String,
    #[serde(rename = "transactionIx")]
    pub transaction_ix: String,
    #[serde(rename = "amountInToken")]
    pub amount_in_token: Option<f64>,
}

pub async fn transact(
    config: &AppConfig,
    body: &TransactRequest,
) -> Result<TransactDefaultData, AirbillsError> {
    let v = json!({
        "productCode": body.product_code,
        "payWith": body.pay_with,
        "data": body.data,
    });
    let parsed = vendor_post_json(config, "/transact", &v, &[]).await?;
    let data = parsed.get("data").cloned().ok_or_else(|| AirbillsError {
        message: "Airbills transact: missing data".into(),
        status: None,
    })?;
    serde_json::from_value(data).map_err(|e| AirbillsError {
        message: format!("Airbills transact: invalid data shape: {e}"),
        status: None,
    })
}

pub async fn transact_process(
    config: &AppConfig,
    product_code: &str,
    id: &str,
) -> Result<Value, AirbillsError> {
    let body = json!({
        "productCode": product_code,
        "id": id,
    });
    vendor_post_json(config, "/transact/process", &body, &["06"]).await
}
