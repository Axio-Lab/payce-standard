use crate::config::AppConfig;
use crate::services::pin::{is_valid_pin, set_user_pin};
use crate::services::solana_rpc::SolanaRpc;
use crate::services::wallet::create_wallet;
use crate::utils::phone::mask_phone;

use super::pin_gate::log_ussd_unexpected_shape;

pub async fn handle_registration(
    pool: &deadpool_postgres::Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    phone: &str,
    inputs: &[String],
) -> String {
    if inputs.is_empty() {
        return "CON Welcome to Payce!\nYou are not registered.\nEnter 1 to create an account:"
            .into();
    }
    if inputs.len() == 1 && inputs[0] == "1" {
        return "CON Create a 4-digit PIN:".into();
    }
    if inputs.len() == 2 {
        if !is_valid_pin(&inputs[1]) {
            return "END Invalid PIN. Please use exactly 4 digits.".into();
        }
        return "CON Confirm your PIN:".into();
    }
    if inputs.len() == 3 {
        if inputs[1] != inputs[2] {
            return "END PINs do not match. Please dial again to retry.".into();
        }
        let client = match pool.get().await {
            Ok(c) => c,
            Err(_) => return "END Database error. Please try again.".into(),
        };
        let row = client
            .query_one(
                "INSERT INTO users (phone_number, status) VALUES ($1, 'PENDING') RETURNING id::text",
                &[&phone],
            )
            .await
            .unwrap();
        let id: String = row.get(0);

        if let Err(e) = create_wallet(pool, config, rpc, &id, phone).await {
            log::error!(
                "[Registration] Wallet creation failed for {}: {e}",
                mask_phone(phone)
            );
            return "END Could not create wallet right now. Please try again.".into();
        }
        if let Err(e) = set_user_pin(pool, &id, &inputs[1]).await {
            log::error!("[Registration] PIN setup failed for {phone}: {e}");
            return "END Could not save PIN right now. Please try again.".into();
        }

        return "END Welcome to Payce! Your account is ready.\nDial again to start sending and receiving money.".into();
    }
    log_ussd_unexpected_shape("registration", inputs);
    "END Something went wrong. Please dial again.".into()
}

pub async fn handle_pin_setup(
    pool: &deadpool_postgres::Pool,
    _config: &AppConfig,
    user_id: &str,
    inputs: &[String],
) -> String {
    if inputs.is_empty() {
        return "CON Welcome back! Please set a 4-digit PIN:".into();
    }
    if inputs.len() == 1 {
        if !is_valid_pin(&inputs[0]) {
            return "END Invalid PIN. Please use exactly 4 digits.".into();
        }
        return "CON Confirm your PIN:".into();
    }
    if inputs.len() == 2 {
        if inputs[0] != inputs[1] {
            return "END PINs do not match. Dial again to retry.".into();
        }
        if let Err(e) = set_user_pin(pool, user_id, &inputs[0]).await {
            log::error!("[PIN Setup] Failed for user {user_id}: {e}");
            return "END Could not save PIN right now. Please try again.".into();
        }
        return "END PIN set! Dial again to access your account.".into();
    }
    log_ussd_unexpected_shape("pin_setup", inputs);
    "END Something went wrong. Please dial again.".into()
}
