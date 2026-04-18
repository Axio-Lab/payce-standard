mod account;
mod balance;
mod data_plans;
mod defi;
mod favorites;
mod merchant_menu;
mod nav;
mod onboarding;
mod paj_bank;
mod paj_ramp_menu;
mod paj_verify;
mod persist;
mod pin_gate;
mod send_money;
mod text;
mod utility;
mod utility_catalog;

use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::solana_rpc::SolanaRpc;
use crate::utils::phone::normalize_nigerian_phone;

use nav::{apply_back_navigation, finalize_ussd_response, parse_ussd_segments, with_back_option};

pub async fn handle_ussd_request(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    phone_number: &str,
    text: &str,
) -> String {
    let normalized = normalize_nigerian_phone(phone_number);
    let inputs = apply_back_navigation(parse_ussd_segments(text));

    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("[USSD] initial pool get: {e}");
            return "END Service temporarily unavailable. Please try again shortly.".into();
        }
    };

    let user_row = client
        .query_opt(
            "SELECT id::text, pin_hash, solana_pubkey, status::text, locked_until FROM users WHERE phone_number = $1",
            &[&normalized],
        )
        .await
        .unwrap_or(None);

    let (user_id, pin_hash, _solana_pubkey, status, locked_until): (
        String,
        Option<String>,
        Option<String>,
        String,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = match user_row {
        Some(row) => (row.get(0), row.get(1), row.get(2), row.get(3), row.get(4)),
        None => {
            return with_back_option(
                &onboarding::handle_registration(pool, rpc, config, &normalized, &inputs).await,
            )
        }
    };

    if status == "LOCKED" {
        if let Some(until) = locked_until {
            if until > chrono::Utc::now() {
                let mins = ((until - chrono::Utc::now()).num_seconds() + 59) / 60;
                return format!("END Your account is locked. Try again in {mins} minutes.");
            }
        }
        let uid = Uuid::parse_str(&user_id).unwrap();
        let _ = client
            .execute(
                "UPDATE users SET status = 'ACTIVE', locked_until = NULL, failed_pin_attempts = 0 WHERE id = $1",
                &[&uid],
            )
            .await;
    }

    if pin_hash.is_none() {
        return with_back_option(
            &onboarding::handle_pin_setup(pool, config, &user_id, &inputs).await,
        );
    }

    if inputs.is_empty() {
        return with_back_option("CON Welcome to Payce\n1. Send Money\n2. Check Balance\n3. Pay Merchant\n4. Favorites\n5. My Account\n6. Register as Merchant\n7. Pay Utility\n8. DeFi");
    }

    let result = match inputs[0].as_str() {
        "1" => {
            send_money::handle_send_money(pool, redis, rpc, config, &user_id, &normalized, &inputs)
                .await
        }
        "2" => balance::handle_check_balance(pool, rpc, config, &user_id).await,
        "3" => {
            merchant_menu::handle_pay_merchant(
                pool,
                redis,
                rpc,
                config,
                &user_id,
                &normalized,
                &inputs,
            )
            .await
        }
        "4" => favorites::handle_favorites(pool, &user_id, &inputs).await,
        "5" => {
            account::handle_my_account(pool, redis, rpc, config, &user_id, &normalized, &inputs)
                .await
        }
        "6" => merchant_menu::handle_merchant_registration(pool, &user_id, &inputs).await,
        "7" => {
            utility::handle_pay_utility(pool, redis, rpc, config, &user_id, &normalized, &inputs)
                .await
        }
        "8" => defi::handle_defi(pool, redis, rpc, config, &user_id, &normalized, &inputs).await,
        _ => "END Invalid option. Please try again.".to_string(),
    };
    finalize_ussd_response(&result)
}
