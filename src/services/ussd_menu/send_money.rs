use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use crate::config::AppConfig;
use crate::services::exchange_rate::*;
use crate::services::solana_rpc::SolanaRpc;
use crate::services::transfer::*;
use crate::utils::phone::*;

use super::nav::repair_send_money_menu_pick;
use super::persist::get_favorites;
use super::pin_gate::{log_ussd_unexpected_shape, verify_pin_or_fail};

pub async fn handle_send_money(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    sender_phone: &str,
    inputs: &[String],
) -> String {
    let inputs = repair_send_money_menu_pick(inputs);
    if inputs.len() == 1 {
        return "CON Send to:\n1. Phone number\n2. Wallet address\n3. Favorite".into();
    }
    let sub = &inputs[2..];
    match inputs[1].as_str() {
        "1" => handle_send_to_phone(pool, redis, rpc, config, user_id, sender_phone, sub).await,
        "2" => handle_send_to_address(pool, redis, rpc, config, user_id, sender_phone, sub).await,
        "3" => handle_send_to_favorite(pool, redis, rpc, config, user_id, sender_phone, sub).await,
        _ => "CON Invalid option. Choose:\n1. Phone number\n2. Wallet address\n3. Favorite".into(),
    }
}

async fn handle_send_to_phone(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    sender_phone: &str,
    sub: &[String],
) -> String {
    if sub.is_empty() {
        return "CON Enter recipient phone number:".into();
    }
    if sub.len() == 1 {
        if !is_valid_nigerian_phone(&sub[0]) {
            return "END Invalid phone number. Use format: 08012345678".into();
        }
        return format!(
            "CON Pick token (Send Money):\n{}",
            config.ussd_stable_token_menu()
        );
    }
    if sub.len() == 2 {
        if !is_valid_nigerian_phone(&sub[0]) {
            return "END Invalid phone number.".into();
        }
        if config.stable_choice_index(&sub[1]).is_none() {
            return config.ussd_stable_pick_invalid_con(None);
        }
        return "CON Enter amount in Naira:".into();
    }
    if sub.len() == 3 {
        let stable_idx = match config.stable_choice_index(&sub[1]) {
            Some(i) => i,
            None => return config.ussd_stable_pick_invalid_con(None),
        };
        let amount: f64 = match sub[2].parse() {
            Ok(a) if a > 0.0 => a,
            _ => return "END Invalid amount.".into(),
        };
        let recip = normalize_nigerian_phone(&sub[0]);
        let rate = get_usd_to_ngn_rate(config).await;
        let usd = amount / rate;
        let code = &config.stable_coins[stable_idx].code;
        return format!(
            "CON Send {} ( {}) to {}?\nEnter your PIN to confirm:",
            format_ngn(amount),
            format_stable_amount_with_code(usd, code),
            mask_phone(&recip),
        );
    }
    if sub.len() == 4 {
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &sub[3]).await {
            return err;
        }
        let stable_idx = match config.stable_choice_index(&sub[1]) {
            Some(i) => i,
            None => return config.ussd_stable_pick_invalid_con(None),
        };
        let recip = normalize_nigerian_phone(&sub[0]);
        let amount: f64 = sub[2].parse().unwrap_or(0.0);
        let result =
            transfer_p2p(pool, rpc, config, sender_phone, &recip, amount, stable_idx).await;
        if !result.success {
            return format!("END {}", result.error.unwrap_or_default());
        }
        let sig = result.tx_signature.unwrap_or_default();
        let short_sig = &sig[..sig.len().min(8)];
        return format!(
            "END Sent {} to {}.\nRef: {}\nSMS with tx link sent.",
            format_ngn(amount),
            mask_phone(&recip),
            short_sig
        );
    }
    log_ussd_unexpected_shape("send_to_phone", sub);
    "END Something went wrong. Please dial again.".into()
}

async fn handle_send_to_address(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    sender_phone: &str,
    sub: &[String],
) -> String {
    if sub.is_empty() {
        return "CON Enter recipient wallet address:".into();
    }
    if sub.len() == 1 {
        if Pubkey::from_str(&sub[0]).is_err() {
            return "END Invalid Solana wallet address.".into();
        }
        return format!(
            "CON Pick token (Send Money):\n{}",
            config.ussd_stable_token_menu()
        );
    }
    if sub.len() == 2 {
        if Pubkey::from_str(&sub[0]).is_err() {
            return "END Invalid Solana wallet address.".into();
        }
        if config.stable_choice_index(&sub[1]).is_none() {
            return config.ussd_stable_pick_invalid_con(None);
        }
        return "CON Enter amount in Naira:".into();
    }
    if sub.len() == 3 {
        let stable_idx = match config.stable_choice_index(&sub[1]) {
            Some(i) => i,
            None => return config.ussd_stable_pick_invalid_con(None),
        };
        let amount: f64 = match sub[2].parse() {
            Ok(a) if a > 0.0 => a,
            _ => return "END Invalid amount.".into(),
        };
        let rate = get_usd_to_ngn_rate(config).await;
        let usd = amount / rate;
        let code = &config.stable_coins[stable_idx].code;
        let short = format!("{}...{}", &sub[0][..6], &sub[0][sub[0].len() - 4..]);
        return format!(
            "CON Send {} ( {}) to {}?\nEnter your PIN to confirm:",
            format_ngn(amount),
            format_stable_amount_with_code(usd, code),
            short,
        );
    }
    if sub.len() == 4 {
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &sub[3]).await {
            return err;
        }
        let stable_idx = match config.stable_choice_index(&sub[1]) {
            Some(i) => i,
            None => return config.ussd_stable_pick_invalid_con(None),
        };
        let amount: f64 = sub[2].parse().unwrap_or(0.0);
        let result = transfer_to_address(
            pool,
            rpc,
            config,
            sender_phone,
            &sub[0],
            amount,
            None,
            stable_idx,
        )
        .await;
        if !result.success {
            return format!("END {}", result.error.unwrap_or_default());
        }
        let sig = result.tx_signature.unwrap_or_default();
        let short_sig = &sig[..sig.len().min(8)];
        return format!(
            "END Sent {} to wallet address.\nRef: {}\nSMS with tx link sent.",
            format_ngn(amount),
            short_sig
        );
    }
    log_ussd_unexpected_shape("send_to_address", sub);
    "END Something went wrong. Please dial again.".into()
}

async fn handle_send_to_favorite(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    sender_phone: &str,
    sub: &[String],
) -> String {
    let favs = get_favorites(pool, user_id).await;
    if favs.is_empty() {
        return "END You currently have no favorites.".into();
    }

    if sub.is_empty() {
        let opts: String = favs
            .iter()
            .enumerate()
            .take(9)
            .map(|(i, (alias, _))| format!("{}. {alias}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        return format!("CON Choose favorite:\n{opts}\nEnter number:");
    }

    let idx: usize = match sub[0].parse::<usize>() {
        Ok(i) if i >= 1 && i <= favs.len() => i - 1,
        _ => return "END Invalid favorite number.".into(),
    };
    let (alias, address) = &favs[idx];

    if sub.len() == 1 {
        return format!(
            "CON Pick token for {alias}:\n{}",
            config.ussd_stable_token_menu()
        );
    }
    if sub.len() == 2 {
        if config.stable_choice_index(&sub[1]).is_none() {
            return config.ussd_stable_pick_invalid_con(None);
        }
        return format!("CON Enter amount in Naira for {alias}:");
    }
    if sub.len() == 3 {
        let stable_idx = match config.stable_choice_index(&sub[1]) {
            Some(i) => i,
            None => return config.ussd_stable_pick_invalid_con(None),
        };
        let amount: f64 = match sub[2].parse() {
            Ok(a) if a > 0.0 => a,
            _ => return "END Invalid amount.".into(),
        };
        let rate = get_usd_to_ngn_rate(config).await;
        let usd = amount / rate;
        let code = &config.stable_coins[stable_idx].code;
        return format!(
            "CON Send {} ( {}) to {alias}?\nEnter your PIN to confirm:",
            format_ngn(amount),
            format_stable_amount_with_code(usd, code),
        );
    }
    if sub.len() == 4 {
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &sub[3]).await {
            return err;
        }
        let stable_idx = match config.stable_choice_index(&sub[1]) {
            Some(i) => i,
            None => return config.ussd_stable_pick_invalid_con(None),
        };
        let amount: f64 = sub[2].parse().unwrap_or(0.0);
        let result = transfer_to_address(
            pool,
            rpc,
            config,
            sender_phone,
            address,
            amount,
            Some(alias),
            stable_idx,
        )
        .await;
        if !result.success {
            return format!("END {}", result.error.unwrap_or_default());
        }
        let sig = result.tx_signature.unwrap_or_default();
        let short_sig = &sig[..sig.len().min(8)];
        return format!(
            "END Sent {} to {alias}.\nRef: {}\nSMS with tx link sent.",
            format_ngn(amount),
            short_sig
        );
    }
    log_ussd_unexpected_shape("send_to_favorite", sub);
    "END Something went wrong. Please dial again.".into()
}
