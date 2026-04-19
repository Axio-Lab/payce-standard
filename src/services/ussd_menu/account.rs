use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::merchant::get_merchant_by_user_id;
use crate::services::solana_rpc::SolanaRpc;
use crate::services::wallet::{export_private_key, import_private_key};
use crate::utils::phone::normalize_nigerian_phone;

use super::paj_bank;
use super::paj_ramp_menu;
use super::paj_verify;
use super::pin_gate::verify_pin_or_fail;
use super::text::wrap_for_ussd;

pub async fn handle_my_account(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    phone: &str,
    inputs: &[String],
) -> String {
    let uid = Uuid::parse_str(user_id).unwrap();
    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("[USSD] my account pool get: {e}");
            return "END Service temporarily unavailable. Please try again shortly.".into();
        }
    };
    let row = client
        .query_opt(
            "SELECT solana_pubkey, secondary_solana_pubkey, kyc_tier::text FROM users WHERE id = $1",
            &[&uid],
        )
        .await
        .unwrap_or(None);

    let (pubkey, secondary_pk, kyc_tier): (Option<String>, Option<String>, String) = match row {
        Some(r) => (r.get(0), r.get(1), r.get(2)),
        None => return "END Account not found.".into(),
    };

    if inputs.len() == 1 {
        return "CON My Account\n1. View full wallet address\n2. Buy token with Naira\n3. Sell token for Naira\n4. Export private key\n5. Import private key\n6. Profile summary\n7. Withdrawal banks\n8. PAJ verification"
            .into();
    }

    match inputs[1].as_str() {
        "7" => {
            let http = config.http.clone();
            return paj_bank::handle_withdrawal_banks_branch(
                pool, redis, &http, config, user_id, inputs,
            )
            .await;
        }
        "8" => {
            let http = config.http.clone();
            return paj_verify::handle_paj_verification_branch(
                pool, redis, &http, config, user_id, phone, inputs,
            )
            .await;
        }
        "2" => {
            let http = config.http.clone();
            return paj_ramp_menu::handle_paj_onramp_ussd(
                pool, redis, &http, rpc, config, user_id, inputs,
            )
            .await;
        }
        "3" => {
            let http = config.http.clone();
            return paj_ramp_menu::handle_paj_offramp_ussd(
                pool, redis, &http, rpc, config, user_id, inputs,
            )
            .await;
        }
        "1" => match pubkey {
            Some(pk) => format!("END Full wallet address:\n{}", wrap_for_ussd(&pk, 22)),
            None => "END Wallet not set up.".into(),
        },
        "4" => {
            if config.is_production() {
                return "END Private key export is disabled over USSD for security.".into();
            }
            if inputs.len() == 2 {
                return "CON Enter PIN to export private key:".into();
            }
            if inputs.len() == 3 {
                if let Some(err) =
                    verify_pin_or_fail(pool, redis, config, user_id, &inputs[2]).await
                {
                    return err;
                }
                match export_private_key(pool, user_id, &config.wallet_encryption_key).await {
                    Ok(Some(key)) => format!(
                        "END Private key (keep secret):\n{}",
                        wrap_for_ussd(&key, 22)
                    ),
                    _ => "END No private key found.".into(),
                }
            } else {
                "END Something went wrong.".into()
            }
        }
        "5" => {
            if inputs.len() == 2 {
                return "CON Enter your private key (base58):".into();
            }
            if inputs.len() == 3 {
                return "CON Enter PIN to confirm import:".into();
            }
            if inputs.len() == 4 {
                if let Some(err) =
                    verify_pin_or_fail(pool, redis, config, user_id, &inputs[3]).await
                {
                    return err;
                }
                match import_private_key(pool, config, rpc, user_id, &inputs[2]).await {
                    Ok(pk) => format!(
                        "END Private key imported.\nNew wallet address:\n{}",
                        wrap_for_ussd(&pk, 22)
                    ),
                    Err(_) => "END Invalid private key format.".into(),
                }
            } else {
                "END Something went wrong.".into()
            }
        }
        "6" => {
            let wallet_display = match &pubkey {
                Some(pk) => wrap_for_ussd(pk, 22),
                None => "Not set up".into(),
            };
            let mut info = format!(
                "END My Account\nPhone: {}\nWallet:\n{}\nKYC: {}",
                normalize_nigerian_phone(phone),
                wallet_display,
                kyc_tier
            );
            if let Some(ref spk) = secondary_pk {
                info.push_str(&format!("\nSecondary:\n{}", wrap_for_ussd(spk, 22)));
            }
            if let Some((name, code)) = get_merchant_by_user_id(pool, user_id).await {
                info.push_str(&format!("\nMerchant: {name} ({code})"));
            }
            info
        }
        _ => "END Invalid option.".into(),
    }
}
