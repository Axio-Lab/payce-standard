use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct PajBank {
    pub id: String,
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub country: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PajBankConfirmResponse {
    #[serde(rename = "accountName")]
    pub account_name: String,
    #[serde(rename = "accountNumber")]
    pub account_number: String,
    pub bank: PajBankConfirmBank,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PajBankConfirmBank {
    pub id: String,
    pub name: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PajSavedBankAccount {
    pub id: String,
    pub account_name: String,
    pub account_number: String,
    #[serde(rename = "bank")]
    pub bank_institution_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct SaveBankBody<'a> {
    #[serde(rename = "bankId")]
    bank_id: &'a str,
    #[serde(rename = "accountNumber")]
    account_number: &'a str,
}

fn paj_base(config: &AppConfig) -> Option<String> {
    let b = config.paj_base_url.trim().trim_end_matches('/').to_string();
    if b.is_empty() {
        None
    } else {
        Some(b)
    }
}

pub fn paj_is_configured(config: &AppConfig) -> bool {
    !config.paj_api_key.trim().is_empty() && paj_base(config).is_some()
}

pub async fn list_banks(
    http: &Client,
    config: &AppConfig,
    session_token: &str,
) -> Result<Vec<PajBank>, String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/bank");
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {session_token}"))
        .send()
        .await
        .map_err(|e| format!("PAJ list banks HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("PAJ list banks body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ list banks {status}: {}", snippet(&body)));
    }
    serde_json::from_str(&body).map_err(|e| format!("PAJ list banks JSON: {e}; {}", snippet(&body)))
}

pub async fn confirm_bank_account(
    http: &Client,
    config: &AppConfig,
    session_token: &str,
    bank_id: &str,
    account_number: &str,
) -> Result<PajBankConfirmResponse, String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/bank-account/confirm");
    let resp = http
        .get(&url)
        .query(&[("bankId", bank_id), ("accountNumber", account_number)])
        .header("Authorization", format!("Bearer {session_token}"))
        .send()
        .await
        .map_err(|e| format!("PAJ confirm bank HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("PAJ confirm bank body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ confirm bank {status}: {}", snippet(&body)));
    }
    serde_json::from_str(&body)
        .map_err(|e| format!("PAJ confirm bank JSON: {e}; {}", snippet(&body)))
}

pub async fn save_bank_account(
    http: &Client,
    config: &AppConfig,
    session_token: &str,
    bank_id: &str,
    account_number: &str,
) -> Result<PajSavedBankAccount, String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/bank-account");
    let body_json = SaveBankBody {
        bank_id,
        account_number,
    };
    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {session_token}"))
        .header("Content-Type", "application/json")
        .json(&body_json)
        .send()
        .await
        .map_err(|e| format!("PAJ save bank HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("PAJ save bank body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ save bank {status}: {}", snippet(&body)));
    }
    serde_json::from_str(&body).map_err(|e| format!("PAJ save bank JSON: {e}; {}", snippet(&body)))
}

pub async fn list_saved_bank_accounts(
    http: &Client,
    config: &AppConfig,
    session_token: &str,
) -> Result<Vec<PajSavedBankAccount>, String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/bank-account");
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {session_token}"))
        .send()
        .await
        .map_err(|e| format!("PAJ list saved banks HTTP: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("PAJ list saved banks body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ list saved banks {status}: {}", snippet(&body)));
    }
    serde_json::from_str(&body)
        .map_err(|e| format!("PAJ list saved banks JSON: {e}; {}", snippet(&body)))
}

pub async fn initiate_session(
    http: &Client,
    config: &AppConfig,
    email: Option<&str>,
    phone_digits: Option<&str>,
) -> Result<(), String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/initiate");
    let key = config.paj_api_key.trim();
    if key.is_empty() {
        return Err("PAJ is not configured".into());
    }
    let body = match (email, phone_digits) {
        (Some(em), _) if !em.trim().is_empty() => serde_json::json!({ "email": em.trim() }),
        (_, Some(ph)) if !ph.trim().is_empty() => serde_json::json!({ "phone": ph.trim() }),
        _ => return Err("Need email or phone for PAJ initiate".into()),
    };
    let resp = http
        .post(&url)
        .header("x-api-key", key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("PAJ initiate HTTP: {e}"))?;
    let status = resp.status();
    let body_txt = resp
        .text()
        .await
        .map_err(|e| format!("PAJ initiate body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ initiate {status}: {}", snippet(&body_txt)));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PajVerifyResponse {
    pub token: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub recipient: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub is_active: Option<bool>,
}

#[derive(Debug, Serialize)]
struct VerifyDeviceBody<'a> {
    uuid: &'a str,
    device: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    os: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct VerifyEmailBody<'a> {
    email: &'a str,
    otp: &'a str,
    device: VerifyDeviceBody<'a>,
}

#[derive(Debug, Serialize)]
struct VerifyPhoneBody<'a> {
    phone: &'a str,
    otp: &'a str,
    device: VerifyDeviceBody<'a>,
}

pub async fn verify_session(
    http: &Client,
    config: &AppConfig,
    email: Option<&str>,
    phone_digits: Option<&str>,
    otp: &str,
    device_uuid: &str,
) -> Result<PajVerifyResponse, String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/verify");
    let key = config.paj_api_key.trim();
    if key.is_empty() {
        return Err("PAJ is not configured".into());
    }
    let resp = match (email, phone_digits) {
        (Some(em), _) if !em.trim().is_empty() => {
            let body = VerifyEmailBody {
                email: em.trim(),
                otp: otp.trim(),
                device: VerifyDeviceBody {
                    uuid: device_uuid,
                    device: "USSD",
                    os: Some("Payce"),
                },
            };
            http.post(&url)
                .header("x-api-key", key)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
        }
        (_, Some(ph)) if !ph.trim().is_empty() => {
            let body = VerifyPhoneBody {
                phone: ph.trim(),
                otp: otp.trim(),
                device: VerifyDeviceBody {
                    uuid: device_uuid,
                    device: "USSD",
                    os: Some("Payce"),
                },
            };
            http.post(&url)
                .header("x-api-key", key)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
        }
        _ => return Err("Need email or phone for PAJ verify".into()),
    };
    let resp = resp.map_err(|e| format!("PAJ verify HTTP: {e}"))?;
    let status = resp.status();
    let body_txt = resp
        .text()
        .await
        .map_err(|e| format!("PAJ verify body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ verify {status}: {}", snippet(&body_txt)));
    }
    serde_json::from_str(&body_txt)
        .map_err(|e| format!("PAJ verify JSON: {e}; {}", snippet(&body_txt)))
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateOfframpOrderBody {
    pub bank: String,
    #[serde(rename = "accountNumber")]
    pub account_number: String,
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(rename = "fiatAmount", skip_serializing_if = "Option::is_none")]
    pub fiat_amount: Option<f64>,
    pub mint: String,
    pub chain: String,
    #[serde(rename = "webhookURL")]
    pub webhook_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "businessUSDCFee", skip_serializing_if = "Option::is_none")]
    pub business_usdc_fee: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateOnrampOrderBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(rename = "fiatAmount", skip_serializing_if = "Option::is_none")]
    pub fiat_amount: Option<f64>,
    pub currency: String,
    pub recipient: String,
    pub mint: String,
    pub chain: String,
    #[serde(rename = "webhookURL")]
    pub webhook_url: String,
    #[serde(rename = "businessUSDCFee", skip_serializing_if = "Option::is_none")]
    pub business_usdc_fee: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PajRampOrderResponse {
    pub id: String,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub mint: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub fiat_amount: Option<f64>,
    #[serde(default)]
    pub rate: Option<f64>,
    #[serde(default)]
    pub fee: Option<f64>,
    #[serde(default)]
    pub bank: Option<String>,
    #[serde(default)]
    pub account_number: Option<String>,
    #[serde(default)]
    pub account_name: Option<String>,
    #[serde(default)]
    pub recipient: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
}

pub async fn create_offramp_order(
    http: &Client,
    config: &AppConfig,
    session_token: &str,
    body: CreateOfframpOrderBody,
) -> Result<PajRampOrderResponse, String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/offramp");
    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {session_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("PAJ offramp HTTP: {e}"))?;
    let status = resp.status();
    let body_txt = resp
        .text()
        .await
        .map_err(|e| format!("PAJ offramp body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ offramp {status}: {}", snippet(&body_txt)));
    }
    serde_json::from_str(&body_txt)
        .map_err(|e| format!("PAJ offramp JSON: {e}; {}", snippet(&body_txt)))
}

pub async fn create_onramp_order(
    http: &Client,
    config: &AppConfig,
    session_token: &str,
    body: CreateOnrampOrderBody,
) -> Result<PajRampOrderResponse, String> {
    let base = paj_base(config).ok_or_else(|| "PAJ is not configured".to_string())?;
    let url = format!("{base}/pub/onramp");
    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {session_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("PAJ onramp HTTP: {e}"))?;
    let status = resp.status();
    let body_txt = resp
        .text()
        .await
        .map_err(|e| format!("PAJ onramp body: {e}"))?;
    if !status.is_success() {
        return Err(format!("PAJ onramp {status}: {}", snippet(&body_txt)));
    }
    serde_json::from_str(&body_txt)
        .map_err(|e| format!("PAJ onramp JSON: {e}; {}", snippet(&body_txt)))
}

fn snippet(s: &str) -> String {
    let t: String = s.chars().take(200).collect();
    if s.len() > 200 {
        format!("{t}…")
    } else {
        t
    }
}
