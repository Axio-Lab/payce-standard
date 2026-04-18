use crate::config::AppConfig;
use crate::services::exchange_rate::*;
use crate::services::merchant::{get_merchant_by_code, get_merchant_by_user_id, register_merchant};
use crate::services::solana_rpc::SolanaRpc;
use crate::services::transfer::pay_merchant;

use super::pin_gate::{log_ussd_unexpected_shape, verify_pin_or_fail};

pub async fn handle_pay_merchant(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    customer_phone: &str,
    inputs: &[String],
) -> String {
    if inputs.len() == 1 {
        return "CON Enter merchant code:".into();
    }
    if inputs.len() == 2 {
        let m = match get_merchant_by_code(pool, &inputs[1]).await {
            Some(mi) if mi.status == "ACTIVE" => mi,
            _ => return "END Merchant not found.".into(),
        };
        return format!(
            "CON Pay {} ({})\nPick token:\n{}",
            m.business_name,
            m.merchant_code,
            config.ussd_stable_token_menu()
        );
    }
    if inputs.len() == 3 {
        let m = match get_merchant_by_code(pool, &inputs[1]).await {
            Some(mi) if mi.status == "ACTIVE" => mi,
            _ => return "END Merchant not found.".into(),
        };
        if config.stable_choice_index(&inputs[2]).is_none() {
            let hdr = format!("Pay {} ({})", m.business_name, m.merchant_code);
            return config.ussd_stable_pick_invalid_con(Some(hdr));
        }
        return format!(
            "CON Pay {} ({})\nEnter amount in Naira:",
            m.business_name, m.merchant_code
        );
    }
    if inputs.len() == 4 {
        let m = match get_merchant_by_code(pool, &inputs[1]).await {
            Some(mi) if mi.status == "ACTIVE" => mi,
            _ => return "END Merchant not found.".into(),
        };
        let stable_idx = match config.stable_choice_index(&inputs[2]) {
            Some(i) => i,
            None => {
                let hdr = format!("Pay {} ({})", m.business_name, m.merchant_code);
                return config.ussd_stable_pick_invalid_con(Some(hdr));
            }
        };
        let amount: f64 = match inputs[3].parse() {
            Ok(a) if a > 0.0 => a,
            _ => return "END Invalid amount.".into(),
        };
        let rate = get_usd_to_ngn_rate(config).await;
        let usd = amount / rate;
        let code = &config.stable_coins[stable_idx].code;
        return format!(
            "CON Pay {} ( {}) to {}?\nEnter your PIN to confirm:",
            format_ngn(amount),
            format_stable_amount_with_code(usd, code),
            m.business_name,
        );
    }
    if inputs.len() == 5 {
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &inputs[4]).await {
            return err;
        }
        let m = match get_merchant_by_code(pool, &inputs[1]).await {
            Some(mi) if mi.status == "ACTIVE" => mi,
            _ => return "END Merchant not found.".into(),
        };
        let stable_idx = match config.stable_choice_index(&inputs[2]) {
            Some(i) => i,
            None => {
                let hdr = format!("Pay {} ({})", m.business_name, m.merchant_code);
                return config.ussd_stable_pick_invalid_con(Some(hdr));
            }
        };
        let amount: f64 = inputs[3].parse().unwrap_or(0.0);
        let result = pay_merchant(
            pool,
            rpc,
            config,
            customer_phone,
            &inputs[1],
            amount,
            stable_idx,
        )
        .await;
        if !result.success {
            return format!("END {}", result.error.unwrap_or_default());
        }
        let sig = result.tx_signature.unwrap_or_default();
        let short_sig = &sig[..sig.len().min(8)];
        return format!(
            "END Payment successful!\n{} paid.\nRef: {}\nSMS with tx link sent.",
            format_ngn(amount),
            short_sig
        );
    }
    log_ussd_unexpected_shape("pay_merchant", inputs);
    "END Something went wrong. Please dial again.".into()
}

pub async fn handle_merchant_registration(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
    inputs: &[String],
) -> String {
    if let Some((name, code)) = get_merchant_by_user_id(pool, user_id).await {
        return format!("END Already registered.\nBusiness: {name}\nCode: {code}");
    }
    if inputs.len() == 1 {
        return "CON Enter your business name:".into();
    }
    if inputs.len() == 2 {
        if inputs[1].trim().len() < 2 {
            return "END Business name too short.".into();
        }
        return format!(
            "CON Register \"{}\" as a merchant?\n1. Confirm\n2. Cancel",
            inputs[1]
        );
    }
    if inputs.len() == 3 {
        if inputs[2] != "1" {
            return "END Registration cancelled.".into();
        }
        match register_merchant(pool, user_id, &inputs[1], "general").await {
            Ok(code) => format!(
                "END Merchant registered!\nBusiness: {}\nYour code: {code}\nShare with customers.",
                inputs[1]
            ),
            Err(e) => {
                log::error!("[USSD] merchant registration failed: {e}");
                "END Registration could not be completed. Please try again later.".into()
            }
        }
    } else {
        log_ussd_unexpected_shape("merchant_registration", inputs);
        "END Something went wrong.".into()
    }
}
