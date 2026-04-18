use deadpool_postgres::Pool;
use redis::Client as RedisClient;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::airbills::InternetPlanItem;
use crate::services::paj_ramp::{
    confirm_bank_account, list_banks, list_saved_bank_accounts, paj_is_configured,
    save_bank_account,
};
use crate::services::ussd_menu::data_plans::{
    format_data_plan_menu_body_ex, format_data_plan_menu_ussd_ex,
    parse_data_plan_menu_input_with_page_ex, DataPlanNav, DATA_PLAN_PAGE_SIZE,
};
use crate::services::ussd_menu::pin_gate::verify_pin_or_fail;
use crate::services::ussd_menu::text::truncate_ussd_label;
use crate::services::ussd_menu::utility_catalog::{redis_delete_key, redis_load_json};
use redis::AsyncCommands;

const PAJ_BANK_FLOW_TTL_SECS: u64 = 900;

async fn redis_store_paj_flow(
    redis: &RedisClient,
    key: &str,
    flow: &PajBankFlow,
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let json = serde_json::to_string(flow).map_err(|e| format!("json: {e}"))?;
    conn.set_ex::<_, _, ()>(key, &json, PAJ_BANK_FLOW_TTL_SECS)
        .await
        .map_err(|e| format!("redis set: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PajBankFlow {
    banks: Vec<InternetPlanItem>,
    account_seg: Option<usize>,
    bank_id: Option<String>,
    bank_label: Option<String>,
    #[serde(default)]
    pending_account_number: Option<String>,
    #[serde(default)]
    pending_account_name: Option<String>,
    #[serde(default)]
    pending_bank_code: Option<String>,
    save_choice_seg: Option<usize>,
    pin_seg: Option<usize>,
}

fn flow_key(user_id: &str) -> String {
    format!("ussd:paj_bank_flow:{user_id}")
}

fn is_nuban_10(s: &str) -> bool {
    s.len() == 10 && s.chars().all(|c| c.is_ascii_digit())
}

fn mask_account_tail(s: &str) -> String {
    if s.len() <= 4 {
        return "****".into();
    }
    format!("****{}", &s[s.len().saturating_sub(4)..])
}

fn con_picker_error(msg: &str, banks: &[InternetPlanItem], page: usize) -> String {
    format!(
        "CON {}\n{}",
        msg,
        format_data_plan_menu_body_ex(banks, page, DATA_PLAN_PAGE_SIZE, "Withdrawal bank", false,)
    )
}

fn paj_session_gate_result(
    token: Option<String>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    email_verified_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<String, String> {
    if email_verified_at.is_none() {
        return Err(
            "CON Complete PAJ verification first.\nMy Account > 8 PAJ verification:\nrequest code, then enter OTP.\nThen return here to save a bank."
                .into(),
        );
    }
    let Some(tok) = token.filter(|t| !t.trim().is_empty()) else {
        return Err(
            "CON No PAJ session yet.\nDial 5*8 (PAJ verification), then return to Withdrawal banks."
                .into(),
        );
    };
    if let Some(exp) = expires_at {
        if exp <= chrono::Utc::now() + chrono::Duration::minutes(1) {
            return Err(
                "CON Your PAJ session expired.\nDial 5*8 to request a new code and enter OTP, then try again."
                    .into(),
            );
        }
    }
    Ok(tok)
}

pub async fn handle_withdrawal_banks_branch(
    pool: &Pool,
    redis: &RedisClient,
    http: &HttpClient,
    config: &AppConfig,
    user_id: &str,
    inputs: &[String],
) -> String {
    if !paj_is_configured(config) {
        return "CON Bank save is not available on this server (PAJ not configured).".into();
    }

    let uid = match Uuid::parse_str(user_id) {
        Ok(u) => u,
        Err(_) => return "END Invalid account.".into(),
    };

    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("[PAJ bank] pool: {e}");
            return "END Service temporarily unavailable.".into();
        }
    };

    let row = match client
        .query_opt(
            "SELECT paj_session_token, paj_session_expires_at, email_verified_at FROM users WHERE id = $1",
            &[&uid],
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::error!("[PAJ bank] query user: {e}");
            return "END Database error.".into();
        }
    };
    let (token, expires_at, email_verified_at): (
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = match row {
        Some(r) => (r.get(0), r.get(1), r.get(2)),
        None => return "END Account not found.".into(),
    };

    let session_token = match paj_session_gate_result(token, expires_at, email_verified_at) {
        Ok(t) => t,
        Err(msg) => return msg,
    };

    if inputs.len() == 2 {
        return "CON Withdrawal banks\n1. Add bank\n2. My saved banks".into();
    }

    match inputs[2].as_str() {
        "2" => return list_saved_flow(http, config, &session_token).await,
        "1" => {}
        _ => {
            return "CON Invalid option.\nWithdrawal banks\n1. Add bank\n2. My saved banks".into();
        }
    }

    const PREFIX: usize = 3;
    let rest = if inputs.len() > PREFIX {
        &inputs[PREFIX..]
    } else {
        &[][..]
    };

    if rest.is_empty() {
        return start_add_bank_flow(redis, http, config, user_id, &session_token).await;
    }

    let key = flow_key(user_id);
    let mut flow: PajBankFlow = match redis_load_json(redis, &key).await {
        Some(f) => f,
        None => return start_add_bank_flow(redis, http, config, user_id, &session_token).await,
    };

    if flow.account_seg.is_none() {
        let nav = match parse_data_plan_menu_input_with_page_ex(
            rest,
            flow.banks.len(),
            DATA_PLAN_PAGE_SIZE,
            false,
        ) {
            Ok(n) => n,
            Err((msg, page)) => return con_picker_error(msg, &flow.banks, page),
        };
        match nav {
            DataPlanNav::ShowMenu { page } => {
                let _ = redis_store_paj_flow(redis, &key, &flow).await;
                return format_data_plan_menu_ussd_ex(
                    &flow.banks,
                    page,
                    DATA_PLAN_PAGE_SIZE,
                    "Withdrawal bank",
                    false,
                );
            }
            DataPlanNav::Picked {
                plan_index,
                consumed,
            } => {
                let plan = match flow.banks.get(plan_index) {
                    Some(p) => p,
                    None => {
                        return con_picker_error("That bank is not on this page.", &flow.banks, 0);
                    }
                };
                let bid = plan.prod_id.clone();
                let blabel = plan.label.clone();
                flow.bank_id = Some(bid);
                flow.bank_label = Some(blabel);
                flow.account_seg = Some(PREFIX + consumed);
                if let Err(e) = redis_store_paj_flow(redis, &key, &flow).await {
                    return format!("END Could not save session: {e}");
                }
                return format!(
                    "CON {}\nEnter 10-digit account number:",
                    truncate_ussd_label(flow.bank_label.as_deref().unwrap_or("Bank"), 28)
                );
            }
        }
    }

    let acc_idx = match flow.account_seg {
        Some(i) => i,
        None => {
            let _ = redis_delete_key(redis, &key).await;
            return start_add_bank_flow(redis, http, config, user_id, &session_token).await;
        }
    };

    if flow.pending_account_number.is_none() {
        if inputs.len() <= acc_idx {
            return format!(
                "CON {}\nEnter 10-digit account number:",
                truncate_ussd_label(flow.bank_label.as_deref().unwrap_or("Bank"), 28)
            );
        }
        let acct = inputs[acc_idx].trim();
        if !is_nuban_10(acct) {
            return format!(
                "CON Enter exactly 10 digits for account number.\n{}",
                format_data_plan_menu_body_ex(
                    &flow.banks,
                    0,
                    DATA_PLAN_PAGE_SIZE,
                    "Withdrawal bank",
                    false,
                )
            );
        }
        match confirm_bank_account(
            http,
            config,
            &session_token,
            flow.bank_id.as_deref().unwrap_or(""),
            acct,
        )
        .await
        {
            Ok(c) => {
                flow.pending_account_number = Some(acct.to_string());
                flow.pending_account_name = Some(c.account_name.clone());
                flow.pending_bank_code = Some(c.bank.code.clone());
                flow.save_choice_seg = Some(acc_idx + 1);
                if let Err(e) = redis_store_paj_flow(redis, &key, &flow).await {
                    return format!("END Could not save session: {e}");
                }
                let nm = truncate_ussd_label(&c.account_name, 24);
                return format!("CON Account name: {nm}\n1. Save this bank\n2. Cancel");
            }
            Err(e) => {
                log::warn!("[PAJ bank] confirm failed: {e}");
                return format!(
                    "CON Could not verify account ({e}).\nCheck the number and try again.\n{}",
                    format_data_plan_menu_body_ex(
                        &flow.banks,
                        0,
                        DATA_PLAN_PAGE_SIZE,
                        "Withdrawal bank",
                        false,
                    )
                );
            }
        }
    }

    let choice_idx = match flow.save_choice_seg {
        Some(i) => i,
        None => {
            let _ = redis_delete_key(redis, &key).await;
            return "END Session lost. Start Add bank again.".into();
        }
    };

    if flow.pin_seg.is_none() {
        if inputs.len() <= choice_idx {
            let nm = truncate_ussd_label(flow.pending_account_name.as_deref().unwrap_or(""), 24);
            return format!("CON Account name: {nm}\n1. Save this bank\n2. Cancel");
        }
        match inputs[choice_idx].as_str() {
            "2" => {
                let _ = redis_delete_key(redis, &key).await;
                return "END Bank save cancelled.".into();
            }
            "1" => {
                flow.pin_seg = Some(choice_idx + 1);
                if let Err(e) = redis_store_paj_flow(redis, &key, &flow).await {
                    return format!("END Could not save session: {e}");
                }
                return "CON Enter your PIN to save this bank:".into();
            }
            _ => {
                let nm =
                    truncate_ussd_label(flow.pending_account_name.as_deref().unwrap_or(""), 24);
                return format!("CON Press 1 to save or 2 to cancel.\nAccount name: {nm}");
            }
        }
    }

    let pin_idx = flow.pin_seg.unwrap();
    if inputs.len() <= pin_idx {
        return "CON Enter your PIN to save this bank:".into();
    }
    if inputs.len() > pin_idx + 1 {
        let _ = redis_delete_key(redis, &key).await;
        return "END Too many entries. Start Add bank again from My Account.".into();
    }
    if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &inputs[pin_idx]).await {
        return err;
    }

    let bank_id = flow.bank_id.clone().unwrap_or_default();
    let acct = flow.pending_account_number.clone().unwrap_or_default();
    let acct_name = flow.pending_account_name.clone().unwrap_or_default();
    let bank_label = flow.bank_label.clone().unwrap_or_default();

    match save_bank_account(http, config, &session_token, &bank_id, &acct).await {
        Ok(saved) => {
            let _ = redis_delete_key(redis, &key).await;
            let bank_code = flow.pending_bank_code.as_deref().unwrap_or("");
            if let Err(e) = client
                .execute(
                    "INSERT INTO user_paj_bank_accounts (user_id, paj_saved_account_id, paj_bank_institution_id, bank_code, bank_name, account_number, account_name) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (user_id, paj_saved_account_id) DO NOTHING",
                    &[&uid, &saved.id, &bank_id, &bank_code, &bank_label, &acct, &acct_name],
                )
                .await
            {
                log::warn!("[PAJ bank] mirror insert: {e}");
            }
            let mask = mask_account_tail(&acct);
            format!(
                "END Bank saved.\n{} {}\nRef: {}",
                truncate_ussd_label(&bank_label, 20),
                mask,
                truncate_ussd_label(&saved.id, 12)
            )
        }
        Err(e) => {
            log::error!("[PAJ bank] save: {e}");
            let _ = redis_delete_key(redis, &key).await;
            format!("END Could not save bank: {e}")
        }
    }
}

async fn list_saved_flow(http: &HttpClient, config: &AppConfig, session_token: &str) -> String {
    match list_saved_bank_accounts(http, config, session_token).await {
        Ok(list) if list.is_empty() => {
            "CON You have no saved banks yet.\nDial 5*7*1 to add a bank.\nOr pick:\n1. Add bank\n2. My saved banks"
                .into()
        }
        Ok(list) => {
            let mut lines: Vec<String> = vec!["CON Saved banks:".into()];
            for (i, a) in list.iter().take(8).enumerate() {
                let mask = mask_account_tail(&a.account_number);
                let nm = truncate_ussd_label(&a.account_name, 18);
                lines.push(format!("{}. {} {}", i + 1, nm, mask));
            }
            if list.len() > 8 {
                lines.push("…more in app later".into());
            }
            lines.join("\n")
        }
        Err(e) => {
            log::warn!("[PAJ bank] list saved: {e}");
            format!("END Could not load saved banks: {e}")
        }
    }
}

async fn start_add_bank_flow(
    redis: &RedisClient,
    http: &HttpClient,
    config: &AppConfig,
    user_id: &str,
    session_token: &str,
) -> String {
    let banks_raw = match list_banks(http, config, session_token).await {
        Ok(b) => b,
        Err(e) => {
            log::warn!("[PAJ bank] list banks: {e}");
            return format!("END Could not load bank list: {e}");
        }
    };
    if banks_raw.is_empty() {
        return "END No banks returned from PAJ.".into();
    }
    let banks: Vec<InternetPlanItem> = banks_raw
        .into_iter()
        .map(|b| InternetPlanItem {
            prod_id: b.id,
            label: {
                let n = b.name.trim();
                if n.is_empty() {
                    b.code
                } else {
                    n.to_string()
                }
            },
            amount_ngn: None,
            batch: None,
        })
        .collect();
    let flow = PajBankFlow {
        banks: banks.clone(),
        account_seg: None,
        bank_id: None,
        bank_label: None,
        pending_account_number: None,
        pending_account_name: None,
        pending_bank_code: None,
        save_choice_seg: None,
        pin_seg: None,
    };
    let key = flow_key(user_id);
    if let Err(e) = redis_store_paj_flow(redis, &key, &flow).await {
        return format!("END Could not save session: {e}");
    }
    format_data_plan_menu_ussd_ex(&banks, 0, DATA_PLAN_PAGE_SIZE, "Withdrawal bank", false)
}

#[cfg(test)]
mod tests {
    use super::is_nuban_10;

    #[test]
    fn nuban_accepts_ten_digits() {
        assert!(is_nuban_10("0123456789"));
        assert!(!is_nuban_10("012345678"));
        assert!(!is_nuban_10("01234567890"));
        assert!(!is_nuban_10("01234a6789"));
    }
}
