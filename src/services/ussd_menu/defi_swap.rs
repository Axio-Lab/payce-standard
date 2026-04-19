use deadpool_postgres::Pool;
use redis::Client as RedisClient;
use serde_json::json;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

use crate::config::AppConfig;
use crate::services::exchange_rate::{format_sol, format_stable_qty_trimmed};
use crate::services::jupiter_swap::run_swap;
use crate::services::solana_rpc::SolanaRpc;
use crate::services::user_activity::{log_user_activity, EVT_SWAP, ST_CONFIRMED};
use crate::services::wallet::{
    get_keypair_for_user, get_native_sol_balance, get_spl_token_balance,
};

use super::data_plans::{DataPlanNav, DATA_PLAN_PAGE_SIZE};
use super::pin_gate::{log_ussd_unexpected_shape, verify_pin_or_fail};
use super::text::truncate_ussd_label;

async fn swap_input_balance_human(
    rpc: &SolanaRpc,
    wallet_address: &str,
    route_id: usize,
    in_mint: &Pubkey,
) -> f64 {
    match route_id {
        7..=9 => get_native_sol_balance(rpc, wallet_address).await,
        _ => get_spl_token_balance(rpc, wallet_address, in_mint).await,
    }
}

fn swap_amount_covered(route_id: usize, balance: f64, amount: f64) -> bool {
    let tol = if matches!(route_id, 7..=9) {
        1e-9
    } else {
        1e-6
    };
    balance + tol >= amount
}

fn format_swap_balance_snippet(route_id: usize, qty: f64, asset: &str) -> String {
    if matches!(route_id, 7..=9) {
        format_sol(qty)
    } else {
        let raw = format_stable_qty_trimmed(qty);
        let trimmed = trim_trailing_zeros_after_decimal(&raw);
        format!("{trimmed} {asset}")
    }
}

fn trim_trailing_zeros_after_decimal(s: &str) -> String {
    match s.rsplit_once('.') {
        None => s.to_string(),
        Some((int, frac)) => {
            let frac_trim = frac.trim_end_matches('0');
            if frac_trim.is_empty() {
                int.to_string()
            } else {
                format!("{int}.{frac_trim}")
            }
        }
    }
}

fn end_insufficient_swap_balance(asset: &str, route_id: usize, have: f64, need: f64) -> String {
    let have_line = format_swap_balance_snippet(route_id, have, asset);
    let need_line = format_swap_balance_snippet(route_id, need, asset);
    format!("END Insufficient balance to complete swap.\nYou have ≈{have_line}; need ≈{need_line}.")
}

async fn pubkey_for_user_phone(pool: &Pool, user_phone: &str) -> Result<String, String> {
    let client = pool
        .get()
        .await
        .map_err(|e| format!("Database error: {e}"))?;
    let row = client
        .query_opt(
            "SELECT solana_pubkey FROM users WHERE phone_number = $1",
            &[&user_phone],
        )
        .await
        .map_err(|e| format!("Database error: {e}"))?;
    let pk: Option<String> = row.and_then(|r| r.get(0));
    pk.ok_or_else(|| "Wallet not set up.".to_string())
}

fn human_to_raw(amount: f64, decimals: u8) -> Option<u64> {
    if amount <= 0.0 || !amount.is_finite() {
        return None;
    }
    let scale = 10f64.powi(decimals as i32);
    let v = (amount * scale).round();
    if v <= 0.0 || v > u64::MAX as f64 {
        None
    } else {
        Some(v as u64)
    }
}

const SWAP_ROUTE_COUNT: usize = 12;

const SWAP_ROUTE_LABELS: [&str; SWAP_ROUTE_COUNT] = [
    "USDC→USDT",
    "USDT→USDC",
    "USDC→USDG",
    "USDG→USDC",
    "USDT→USDG",
    "USDG→USDT",
    "SOL→USDC",
    "SOL→USDT",
    "SOL→USDG",
    "USDC→SOL",
    "USDT→SOL",
    "USDG→SOL",
];

fn swap_route_input_asset(route_id: usize) -> Option<&'static str> {
    match route_id {
        1 | 3 | 10 => Some("USDC"),
        2 | 5 | 11 => Some("USDT"),
        4 | 6 | 12 => Some("USDG"),
        7..=9 => Some("SOL"),
        _ => None,
    }
}

fn swap_route_menu_body(page: usize) -> String {
    let page_size = DATA_PLAN_PAGE_SIZE;
    let total_pages = std::cmp::max(1, SWAP_ROUTE_COUNT.div_ceil(page_size));
    let start = page * page_size;
    let mut lines: Vec<String> = vec![
        format!("Swap routes (page {}/{}):", page + 1, total_pages),
        "Pick 1-4 for a route, 5 for next page.".into(),
    ];
    for j in 0..page_size {
        let idx = start + j;
        if idx >= SWAP_ROUTE_COUNT {
            break;
        }
        let label = truncate_ussd_label(SWAP_ROUTE_LABELS[idx], 26);
        lines.push(format!("{}. {}", j + 1, label));
    }
    if (page + 1) * page_size < SWAP_ROUTE_COUNT {
        lines.push(format!("{}. More plans", page_size + 1));
    }
    lines.join("\n")
}

fn format_swap_route_menu_ussd(page: usize) -> String {
    format!("CON {}", swap_route_menu_body(page))
}

fn con_swap_route_menu_error(message: &str, page: usize) -> String {
    format!("CON {message}\n{body}", body = swap_route_menu_body(page))
}

fn parse_swap_route_menu_input(
    rest: &[String],
    route_count: usize,
    page_size: usize,
) -> Result<DataPlanNav, (&'static str, usize)> {
    let mut page: usize = 0;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "5" => {
                if (page + 1) * page_size < route_count {
                    page += 1;
                    i += 1;
                } else {
                    return Err((
                        "There is no next page of routes. Pick 1-4 on this page.",
                        page,
                    ));
                }
            }
            "1" | "2" | "3" | "4" => {
                let slot: usize = match rest[i].parse() {
                    Ok(s) if (1..=page_size).contains(&s) => s,
                    _ => return Err(("Pick a route from 1 to 4 on this page.", page)),
                };
                let idx = page * page_size + (slot - 1);
                if idx >= route_count {
                    return Err(("That route is not on this page.", page));
                }
                i += 1;
                return Ok(DataPlanNav::Picked {
                    plan_index: idx,
                    consumed: i,
                });
            }
            _ => {
                return Err(("Not a valid choice.", page));
            }
        }
    }
    Ok(DataPlanNav::ShowMenu { page })
}

fn swap_route_by_route_id(
    config: &AppConfig,
    route_id: usize,
) -> Option<(&'static str, Pubkey, Pubkey, u8)> {
    if !(1..=SWAP_ROUTE_COUNT).contains(&route_id) {
        return None;
    }
    let sc = &config.stable_coins;
    if sc.len() < 3 {
        return None;
    }
    let usdc = sc[0].mint;
    let usdt = sc[1].mint;
    let usdg = sc[2].mint;
    let sol = config.sol_mint;
    let label = SWAP_ROUTE_LABELS[route_id - 1];
    let (in_mint, out_mint, dec) = match route_id {
        1 => (usdc, usdt, 6u8),
        2 => (usdt, usdc, 6),
        3 => (usdc, usdg, 6),
        4 => (usdg, usdc, 6),
        5 => (usdt, usdg, 6),
        6 => (usdg, usdt, 6),
        7 => (sol, usdc, 9),
        8 => (sol, usdt, 9),
        9 => (sol, usdg, 9),
        10 => (usdc, sol, 6),
        11 => (usdt, sol, 6),
        12 => (usdg, sol, 6),
        _ => return None,
    };
    Some((label, in_mint, out_mint, dec))
}

pub async fn handle_swap_token(
    pool: &Pool,
    redis: &RedisClient,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    user_phone: &str,
    sub: &[String],
) -> String {
    if sub.is_empty() {
        return format_swap_route_menu_ussd(0);
    }

    let nav = match parse_swap_route_menu_input(sub, SWAP_ROUTE_COUNT, DATA_PLAN_PAGE_SIZE) {
        Ok(n) => n,
        Err((msg, page)) => return con_swap_route_menu_error(msg, page),
    };

    match nav {
        DataPlanNav::ShowMenu { page } => format_swap_route_menu_ussd(page),
        DataPlanNav::Picked {
            plan_index,
            consumed,
        } => {
            let route_id = plan_index + 1;
            let Some((label, _, _, _)) = swap_route_by_route_id(config, route_id) else {
                return format_swap_route_menu_ussd(0);
            };
            let asset = swap_route_input_asset(route_id).unwrap_or("token");
            let tail = &sub[consumed..];
            if tail.is_empty() {
                return format!(
                    "CON Route: Swap {label}\nEnter {asset} amount to send (whole units, e.g. 10.5):"
                );
            }
            if tail.len() == 1 {
                let amount: f64 = match tail[0].parse() {
                    Ok(a) if a > 0.0 => a,
                    _ => {
                        return format!(
                            "END Enter a positive number (e.g. 10.5 whole {asset} units)."
                        );
                    }
                };
                let Some((_, in_mint, _, dec)) = swap_route_by_route_id(config, route_id) else {
                    return format_swap_route_menu_ussd(0);
                };
                if human_to_raw(amount, dec).is_none() {
                    return "END Amount is too small or too large for this route.".to_string();
                }
                let pk = match pubkey_for_user_phone(pool, user_phone).await {
                    Ok(p) => p,
                    Err(e) => return format!("END {e}"),
                };
                let bal = swap_input_balance_human(rpc, &pk, route_id, &in_mint).await;
                if !swap_amount_covered(route_id, bal, amount) {
                    return end_insufficient_swap_balance(asset, route_id, bal, amount);
                }
                return format!("CON Swap ~{amount} {label}?\nEnter your PIN to confirm:");
            }
            if tail.len() == 2 {
                if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &tail[1]).await
                {
                    return err;
                }
                let amount: f64 = match tail[0].parse() {
                    Ok(a) if a > 0.0 => a,
                    _ => {
                        return format!(
                            "END Enter a positive number (e.g. 10.5 whole {asset} units)."
                        );
                    }
                };
                let Some((label, in_mint, out_mint, dec)) =
                    swap_route_by_route_id(config, route_id)
                else {
                    return format_swap_route_menu_ussd(0);
                };
                let Some(raw) = human_to_raw(amount, dec) else {
                    return "END Amount is too small or too large for this route.".to_string();
                };

                let client = match pool.get().await {
                    Ok(c) => c,
                    Err(e) => return format!("END Database error: {e}"),
                };
                let row = client
                    .query_opt(
                        "SELECT encrypted_keypair FROM users WHERE phone_number = $1",
                        &[&user_phone],
                    )
                    .await
                    .unwrap_or(None);
                let enc: Option<String> = row.and_then(|r| r.get(0));
                let Some(enc) = enc else {
                    return "END Wallet not set up.".into();
                };
                let kp = match get_keypair_for_user(&enc, &config.wallet_encryption_key) {
                    Ok(k) => k,
                    Err(e) => return format!("END Wallet error: {e}"),
                };

                let pk_str = kp.pubkey().to_string();
                let bal = swap_input_balance_human(rpc, &pk_str, route_id, &in_mint).await;
                if !swap_amount_covered(route_id, bal, amount) {
                    return end_insufficient_swap_balance(
                        swap_route_input_asset(route_id).unwrap_or("token"),
                        route_id,
                        bal,
                        amount,
                    );
                }

                let in_s = in_mint.to_string();
                let out_s = out_mint.to_string();
                match run_swap(&config.http, config, &kp, &in_s, &out_s, raw).await {
                    Ok(sig) => {
                        log_user_activity(
                            pool,
                            user_id,
                            EVT_SWAP,
                            ST_CONFIRMED,
                            Some(&sig),
                            i64::try_from(raw).ok(),
                            Some(&in_s),
                            None,
                            None,
                            None,
                            None,
                            json!({
                                "route_label": label,
                                "route_id": route_id,
                                "in_mint": in_s,
                                "out_mint": out_s,
                                "amount_human": amount,
                            }),
                        );
                        let short = &sig[..sig.len().min(8)];
                        let link = crate::services::sms::tx_explorer_url(config, &sig);
                        format!("END Swap ok ({label}).\nRef: {short}\n{link}")
                    }
                    Err(e) => {
                        log::error!("[defi_swap] Jupiter swap failed: {e}");
                        let msg = if e.len() > 120 {
                            format!("{}…", &e[..118])
                        } else {
                            e
                        };
                        format!("END Swap failed: {msg}")
                    }
                }
            } else {
                log_ussd_unexpected_shape("defi_swap", sub);
                "END Too many entries for this step. Start swap again from DeFi.".into()
            }
        }
    }
}
