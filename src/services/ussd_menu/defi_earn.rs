use deadpool_postgres::Pool;
use redis::Client as RedisClient;
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

use crate::config::AppConfig;
use crate::services::exchange_rate::{
    format_ngn, format_sol, format_stable_amount_with_code, format_stable_qty_trimmed,
    get_usd_to_ngn_rate, usd_to_ngn,
};
use crate::services::jupiter_earn::{
    earn_deposit_transaction_b64, earn_fee_raw, earn_withdraw_transaction_b64, fetch_earn_earnings,
    fetch_earn_positions, fetch_earn_tokens, normalize_jupiter_apy_display,
    sign_send_earn_versioned_tx, EarnPositionRow, EarnTokenInfo,
};
use crate::services::sms;
use crate::services::solana_rpc::SolanaRpc;
use crate::services::transfer::transfer_user_spl_to_owner_with_gas;
use crate::services::user_activity::{
    log_user_activity, EVT_EARN_DEPOSIT, EVT_EARN_WITHDRAW, ST_CONFIRMED,
};
use crate::services::wallet::{get_keypair_for_user, get_spl_token_balance};

use super::pin_gate::verify_pin_or_fail;
use super::text::truncate_ussd_label;

const STABLE_DECIMALS: u8 = 6;

#[derive(Clone, Copy)]
enum MintSlot {
    Stable(usize),
    Wsol,
}

impl MintSlot {
    fn mint_pk(self, config: &AppConfig) -> Pubkey {
        match self {
            MintSlot::Stable(i) => config.stable_coins[i].mint,
            MintSlot::Wsol => config.sol_mint,
        }
    }

    fn code(self, config: &AppConfig) -> String {
        match self {
            MintSlot::Stable(i) => config.stable_coins[i].code.clone(),
            MintSlot::Wsol => "SOL".into(),
        }
    }

    fn decimals(self) -> u8 {
        match self {
            MintSlot::Stable(_) => STABLE_DECIMALS,
            MintSlot::Wsol => 9,
        }
    }
}

fn earn_deposit_option_count(config: &AppConfig) -> usize {
    config.stable_coins.len() + 1
}

fn mint_slot_from_deposit_digit(config: &AppConfig, digit: usize) -> Option<MintSlot> {
    let n = config.stable_coins.len();
    if digit >= 1 && digit <= n {
        Some(MintSlot::Stable(digit - 1))
    } else if digit == n + 1 {
        Some(MintSlot::Wsol)
    } else {
        None
    }
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

fn apy_display(tokens: &[EarnTokenInfo], mint: &str) -> String {
    let t = tokens.iter().find(|t| {
        t.address == mint
            || t.asset_address
                .as_deref()
                .map(|a| a == mint)
                .unwrap_or(false)
    });
    match t {
        Some(row) => row
            .apy_raw_string()
            .map(|s| normalize_jupiter_apy_display(&s))
            .unwrap_or_else(|| "?".into()),
        None => "?".into(),
    }
}

fn parse_underlying_units(s: &str, decimals: u8) -> Option<u64> {
    let t = s.trim();
    if let Ok(v) = t.parse::<u64>() {
        return Some(v);
    }
    if let Ok(v) = t.parse::<f64>() {
        if v <= 0.0 || !v.is_finite() {
            return None;
        }
        let scale = 10f64.powi(decimals as i32);
        return Some((v * scale).round() as u64);
    }
    None
}

fn format_underlying_human(wsol_mint: &str, mint_s: &str, raw: u64, decimals: u8) -> String {
    let div = 10f64.powi(decimals as i32);
    let v = raw as f64 / div;
    if mint_s == wsol_mint {
        format_sol(v)
    } else {
        format_stable_qty_trimmed(v)
    }
}

#[derive(Clone, Copy)]
struct EarnDisplayCtx {
    stable_ngn_per_usd: f64,
    sol_ngn_per_usd: f64,
    sol_usd: Option<f64>,
}

async fn load_earn_display_ctx(config: &AppConfig) -> EarnDisplayCtx {
    let (stable_ngn_per_usd, paj, jup_sol) = tokio::join!(
        get_usd_to_ngn_rate(config),
        crate::services::paj_rates::get_paj_ngn_per_usd_cached(config),
        crate::services::jupiter_price::fetch_sol_usd_price(config),
    );
    let sol_ngn_per_usd = paj
        .map(|p| p.off_ramp)
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(stable_ngn_per_usd);
    let sol_usd = match jup_sol {
        Ok(v) if v.is_finite() && v > 0.0 => Some(v),
        Err(e) => {
            log::warn!("[Earn] Jupiter SOL/USD failed: {e}");
            None
        }
        _ => None,
    };
    EarnDisplayCtx {
        stable_ngn_per_usd,
        sol_ngn_per_usd,
        sol_usd,
    }
}

fn raw_to_human_amount(raw: u64, decimals: u8) -> f64 {
    let div = 10f64.powi(decimals as i32);
    raw as f64 / div
}

fn display_stable_ngn_token(ngn_per_usd: f64, qty: f64, code: &str, max_chars: usize) -> String {
    let ngn = usd_to_ngn(qty, ngn_per_usd);
    let s = format!(
        "{} ({})",
        format_ngn(ngn),
        format_stable_amount_with_code(qty, code)
    );
    truncate_ussd_label(&s, max_chars)
}

fn display_sol_ngn_token(
    paj_ngn_per_usd: f64,
    sol_usd: Option<f64>,
    qty: f64,
    max_chars: usize,
) -> String {
    let s = if let Some(usd) = sol_usd.filter(|u| u.is_finite() && *u > 0.0) {
        let ngn = usd_to_ngn(qty * usd, paj_ngn_per_usd);
        format!("{} ({})", format_ngn(ngn), format_sol(qty))
    } else {
        format_sol(qty)
    };
    truncate_ussd_label(&s, max_chars)
}

impl EarnDisplayCtx {
    fn fmt_stable(&self, qty: f64, code: &str, max: usize) -> String {
        display_stable_ngn_token(self.stable_ngn_per_usd, qty, code, max)
    }

    fn fmt_sol(&self, qty: f64, max: usize) -> String {
        display_sol_ngn_token(self.sol_ngn_per_usd, self.sol_usd, qty, max)
    }

    fn label_position_balance(
        &self,
        wsol: &str,
        mint: &str,
        raw: u64,
        dec: u8,
        code: &str,
    ) -> String {
        let qty = raw_to_human_amount(raw, dec);
        if mint == wsol {
            self.fmt_sol(qty, 22)
        } else {
            self.fmt_stable(qty, code, 22)
        }
    }
}

fn json_numberish(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_u64().map(|u| u as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

pub async fn handle_lend_earn(
    pool: &Pool,
    redis: &RedisClient,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    user_phone: &str,
    sub: &[String],
) -> String {
    if sub.is_empty() {
        return "CON Lend & Earn\n1. Deposit\n2. Withdraw\n3. Rewards".into();
    }

    match sub[0].as_str() {
        "1" => handle_deposit(pool, redis, rpc, config, user_id, user_phone, &sub[1..]).await,
        "2" => handle_withdraw(pool, redis, rpc, config, user_id, user_phone, &sub[1..]).await,
        "3" => handle_rewards(pool, rpc, config, user_phone, &sub[1..]).await,
        _ => "CON Pick 1–3:\n1. Deposit\n2. Withdraw\n3. Rewards".into(),
    }
}

async fn handle_rewards(
    pool: &Pool,
    _rpc: &SolanaRpc,
    config: &AppConfig,
    user_phone: &str,
    tail: &[String],
) -> String {
    if !tail.is_empty() {
        return "END Too many steps. Open Rewards from Lend & Earn again.".into();
    }
    let pk = match pubkey_for_user_phone(pool, user_phone).await {
        Ok(p) => p,
        Err(e) => return format!("END {e}"),
    };
    let rows = match fetch_earn_positions(config, std::slice::from_ref(&pk)).await {
        Ok(r) => r,
        Err(e) => return format!("END Could not load positions: {e}"),
    };
    if rows.is_empty() {
        return "END No Jupiter Earn positions found.".into();
    }
    let mut pos_addrs: Vec<String> = rows.iter().map(|p| p.address.clone()).collect();
    pos_addrs.sort();
    pos_addrs.dedup();
    let earn = match fetch_earn_earnings(config, &pk, &pos_addrs).await {
        Ok(e) => e,
        Err(e) => return format!("END Could not load earnings: {e}"),
    };
    let wsol = config.sol_mint.to_string();
    let ctx = load_earn_display_ctx(config).await;
    let mut lines: Vec<String> = vec!["END Earn rewards".into()];
    for p in &rows {
        let mint = p.asset_address.as_deref().unwrap_or("?");
        let dec = if mint == wsol.as_str() {
            9u8
        } else {
            STABLE_DECIMALS
        };
        let sym = if mint == wsol.as_str() {
            "SOL".to_string()
        } else {
            config
                .stable_coins
                .iter()
                .find(|s| s.mint.to_string() == mint)
                .map(|s| s.code.clone())
                .unwrap_or_else(|| "Token".into())
        };
        let is_sol = mint == wsol.as_str();
        let is_known_stable = config
            .stable_coins
            .iter()
            .any(|s| s.mint.to_string() == mint);

        let bal_label = match parse_underlying_units(&p.underlying_balance, dec) {
            Some(u) if is_sol => ctx.label_position_balance(&wsol, mint, u, dec, &sym),
            Some(u) if is_known_stable => ctx.label_position_balance(&wsol, mint, u, dec, &sym),
            Some(u) => truncate_ussd_label(&format_underlying_human(&wsol, mint, u, dec), 18),
            None => truncate_ussd_label(&p.underlying_balance, 18),
        };

        let earn_row = earn
            .iter()
            .find(|e| e.address == p.address || e.owner_address == p.address);
        let rewards_label = match earn_row.and_then(|e| json_numberish(&e.earnings)) {
            Some(amt) if is_sol => ctx.fmt_sol(amt, 20),
            Some(amt) if is_known_stable => ctx.fmt_stable(amt, &sym, 20),
            Some(_) => earn_row
                .map(|e| truncate_ussd_label(&e.earnings.to_string(), 20))
                .unwrap_or_else(|| "-".into()),
            None => earn_row
                .map(|e| truncate_ussd_label(&e.earnings.to_string(), 20))
                .unwrap_or_else(|| "-".into()),
        };

        lines.push(format!(
            "{}: bal ~{} | rewards {}",
            truncate_ussd_label(&sym, 6),
            bal_label,
            rewards_label
        ));
    }
    lines.join("\n")
}

fn deposit_token_menu(config: &AppConfig, tokens: &[EarnTokenInfo]) -> String {
    let mut body = String::from("CON Deposit — pick token:\n");
    for (i, sc) in config.stable_coins.iter().enumerate() {
        let mint_s = sc.mint.to_string();
        let apy = apy_display(tokens, &mint_s);
        body.push_str(&format!(
            "{}. {} ~{}% APY\n",
            i + 1,
            sc.code,
            truncate_ussd_label(&apy, 8)
        ));
    }
    let sol_mint = config.sol_mint.to_string();
    let sol_apy = apy_display(tokens, &sol_mint);
    let n = config.stable_coins.len();
    body.push_str(&format!(
        "{}. SOL ~{}% APY\n",
        n + 1,
        truncate_ussd_label(&sol_apy, 8)
    ));
    body.push_str("Enter number:");
    body
}

async fn handle_deposit(
    pool: &Pool,
    redis: &RedisClient,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    user_phone: &str,
    tail: &[String],
) -> String {
    let fee_recipient = config.fee_payer.pubkey();
    let tokens = match fetch_earn_tokens(config).await {
        Ok(t) => t,
        Err(e) => return format!("END Could not load Earn markets: {e}"),
    };
    let n_opts = earn_deposit_option_count(config);

    if tail.is_empty() {
        return deposit_token_menu(config, &tokens);
    }

    let choice = match tail[0].parse::<usize>() {
        Ok(d) => match mint_slot_from_deposit_digit(config, d) {
            Some(s) => s,
            None => {
                return format!(
                    "{}\nPick 1–{}.",
                    deposit_token_menu(config, &tokens),
                    n_opts
                );
            }
        },
        _ => {
            return format!(
                "{}\nPick 1–{}.",
                deposit_token_menu(config, &tokens),
                n_opts
            );
        }
    };
    let mint_pk = choice.mint_pk(config);
    let mint_s = mint_pk.to_string();
    let dec = choice.decimals();
    let code = choice.code(config);

    if tail.len() == 1 {
        let pk = match pubkey_for_user_phone(pool, user_phone).await {
            Ok(p) => p,
            Err(e) => return format!("END {e}"),
        };
        let bal = get_spl_token_balance(rpc, &pk, &mint_pk).await;
        let bal_line = if matches!(choice, MintSlot::Wsol) {
            let ctx = load_earn_display_ctx(config).await;
            ctx.fmt_sol(bal, 24)
        } else {
            let rate = get_usd_to_ngn_rate(config).await;
            display_stable_ngn_token(rate, bal, &code, 24)
        };
        let apy = apy_display(&tokens, &mint_s);
        return format!(
            "CON {} (~{}% APY)\nBalance ~{}\nEnter amount (whole units):",
            code,
            truncate_ussd_label(&apy, 8),
            bal_line
        );
    }

    let amount: f64 = match tail[1].parse() {
        Ok(a) if a > 0.0 => a,
        _ => return "END Enter a positive amount.".into(),
    };
    let Some(dep_raw) = human_to_raw(amount, dec) else {
        return "END Amount too small or too large.".into();
    };
    let fee_raw = earn_fee_raw(dep_raw, config.payce_earn_fee_bps);
    let pk = match pubkey_for_user_phone(pool, user_phone).await {
        Ok(p) => p,
        Err(e) => return format!("END {e}"),
    };
    let bal = get_spl_token_balance(rpc, &pk, &mint_pk).await;
    let bal_raw = human_to_raw(bal, dec).unwrap_or(0);
    let need = dep_raw
        .saturating_add(fee_raw)
        .saturating_add(config.gas_fee_usdc);
    if bal_raw < need {
        return "END Insufficient balance to complete deposit.".into();
    }

    if tail.len() == 2 {
        let amt_label = if matches!(choice, MintSlot::Wsol) {
            let ctx = load_earn_display_ctx(config).await;
            ctx.fmt_sol(amount, 34)
        } else {
            let rate = get_usd_to_ngn_rate(config).await;
            display_stable_ngn_token(rate, amount, &code, 34)
        };
        return format!("CON Deposit {} (+ fee). Confirm?\nEnter PIN:", amt_label);
    }

    if tail.len() == 3 {
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &tail[2]).await {
            return err;
        }
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

        if fee_raw > 0 {
            if let Err(e) = transfer_user_spl_to_owner_with_gas(
                rpc,
                config,
                &kp,
                &mint_pk,
                &fee_recipient,
                fee_raw,
                dec,
            )
            .await
            {
                log::error!("[Earn] fee transfer failed: {e}");
                return format!("END Fee payment failed: {e}");
            }
        }

        let tx_b64 =
            match earn_deposit_transaction_b64(config, &mint_s, dep_raw, &kp.pubkey().to_string())
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    log::error!("[Earn] deposit tx build failed: {e}");
                    return format!("END Deposit build failed: {e}");
                }
            };

        match sign_send_earn_versioned_tx(rpc, config, &kp, &tx_b64).await {
            Ok(sig) => {
                log_user_activity(
                    pool,
                    user_id,
                    EVT_EARN_DEPOSIT,
                    ST_CONFIRMED,
                    Some(&sig),
                    i64::try_from(dep_raw).ok(),
                    Some(&mint_s),
                    None,
                    None,
                    None,
                    None,
                    json!({
                        "token_code": code,
                        "amount_human": amount,
                    }),
                );
                let short = &sig[..sig.len().min(8)];
                let link = sms::tx_explorer_url(config, &sig);
                let ctx = load_earn_display_ctx(config).await;
                let done_amt = if matches!(choice, MintSlot::Wsol) {
                    ctx.fmt_sol(amount, 40)
                } else {
                    ctx.fmt_stable(amount, &code, 40)
                };
                format!(
                    "END Deposited to Earn ({}).\nRef: {short}\n{link}",
                    done_amt
                )
            }
            Err(e) => {
                log::error!("[Earn] deposit send failed: {e}");
                format!("END Deposit failed: {e}")
            }
        }
    } else {
        "END Too many steps. Start deposit again.".into()
    }
}

fn indexed_withdraw_positions<'a>(
    config: &AppConfig,
    rows: &'a [EarnPositionRow],
) -> Vec<(MintSlot, &'a EarnPositionRow)> {
    let mut v: Vec<(MintSlot, &'a EarnPositionRow)> = Vec::new();
    let wsol = config.sol_mint.to_string();
    for p in rows {
        let mint = match p.asset_address.as_deref() {
            Some(m) => m,
            None => continue,
        };
        let slot = if mint == wsol {
            MintSlot::Wsol
        } else if let Some((idx, _)) = config
            .stable_coins
            .iter()
            .enumerate()
            .find(|(_, s)| s.mint.to_string() == mint)
        {
            MintSlot::Stable(idx)
        } else {
            continue;
        };
        let dec = slot.decimals();
        let bal_u = parse_underlying_units(&p.underlying_balance, dec).unwrap_or(0);
        if bal_u == 0 {
            continue;
        }
        v.push((slot, p));
    }
    v
}

fn withdraw_positions_menu(
    config: &AppConfig,
    rows: &[EarnPositionRow],
    ctx: &EarnDisplayCtx,
) -> String {
    let indexed = indexed_withdraw_positions(config, rows);
    if indexed.is_empty() {
        return "END No withdrawable Earn balance.".into();
    }
    let mut lines: Vec<String> = vec!["CON Withdraw — pick:".into()];
    for (i, (slot, prow)) in indexed.iter().enumerate() {
        let dec = slot.decimals();
        let bal_u = parse_underlying_units(&prow.underlying_balance, dec).unwrap_or(0);
        let qty = raw_to_human_amount(bal_u, dec);
        let amount_label = if matches!(slot, MintSlot::Wsol) {
            ctx.fmt_sol(qty, 20)
        } else {
            ctx.fmt_stable(qty, &slot.code(config), 20)
        };
        lines.push(format!("{}. {}", i + 1, amount_label));
    }
    lines.push("Pick number:".into());
    lines.join("\n")
}

async fn handle_withdraw(
    pool: &Pool,
    redis: &RedisClient,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    user_phone: &str,
    tail: &[String],
) -> String {
    let fee_recipient = config.fee_payer.pubkey();
    let pk = match pubkey_for_user_phone(pool, user_phone).await {
        Ok(p) => p,
        Err(e) => return format!("END {e}"),
    };
    let rows = match fetch_earn_positions(config, std::slice::from_ref(&pk)).await {
        Ok(r) => r,
        Err(e) => return format!("END Could not load positions: {e}"),
    };
    let wsol = config.sol_mint.to_string();
    let ctx = load_earn_display_ctx(config).await;

    if tail.is_empty() {
        return withdraw_positions_menu(config, &rows, &ctx);
    }

    let indexed = indexed_withdraw_positions(config, &rows);
    if indexed.is_empty() {
        return "END No withdrawable Earn balance.".into();
    }

    let choice: usize = match tail[0].parse() {
        Ok(n) if n >= 1 && n <= indexed.len() => n,
        _ => {
            return format!(
                "{}\nPick 1–{}.",
                withdraw_positions_menu(config, &rows, &ctx),
                indexed.len()
            );
        }
    };
    let (slot, prow) = &indexed[choice - 1];
    let mint_pk = slot.mint_pk(config);
    let mint_s = mint_pk.to_string();
    let dec = slot.decimals();
    let code = slot.code(config);
    let max_u = parse_underlying_units(&prow.underlying_balance, dec).unwrap_or(0);

    if tail.len() == 1 {
        let qty_max = raw_to_human_amount(max_u, dec);
        let max_label = if matches!(slot, MintSlot::Wsol) {
            ctx.fmt_sol(qty_max, 26)
        } else {
            ctx.fmt_stable(qty_max, &code, 26)
        };
        return format!("CON Withdraw {} (max {}).\nEnter amount:", code, max_label);
    }

    let amount: f64 = match tail[1].parse() {
        Ok(a) if a > 0.0 => a,
        _ => return "END Enter a positive amount.".into(),
    };
    let Some(w_raw) = human_to_raw(amount, dec) else {
        return "END Amount too small or too large.".into();
    };
    if w_raw > max_u {
        let human = format_underlying_human(&wsol, &mint_s, max_u, dec);
        return format!(
            "END Max withdraw ~{} {}.",
            truncate_ussd_label(&human, 14),
            code
        );
    }

    let fee_raw = earn_fee_raw(w_raw, config.payce_earn_fee_bps);
    let bal = get_spl_token_balance(rpc, &pk, &mint_pk).await;
    let bal_raw = human_to_raw(bal, dec).unwrap_or(0);
    let need_after = fee_raw.saturating_add(config.gas_fee_usdc);
    if bal_raw.saturating_add(w_raw) < need_after {
        return format!(
            "END After this withdraw your {} wallet must cover the {}% fee + gas (~{} smallest units). Top up or withdraw less.",
            code,
            (config.payce_earn_fee_bps as f64) / 100.0,
            need_after
        );
    }

    if tail.len() == 2 {
        let amt_label = if matches!(slot, MintSlot::Wsol) {
            ctx.fmt_sol(amount, 32)
        } else {
            ctx.fmt_stable(amount, &code, 32)
        };
        return format!("CON Withdraw {} (+ fee). Confirm?\nEnter PIN:", amt_label);
    }

    if tail.len() == 3 {
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &tail[2]).await {
            return err;
        }
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

        let tx_b64 =
            match earn_withdraw_transaction_b64(config, &mint_s, w_raw, &kp.pubkey().to_string())
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    log::error!("[Earn] withdraw tx build failed: {e}");
                    return format!("END Withdraw build failed: {e}");
                }
            };

        let sig = match sign_send_earn_versioned_tx(rpc, config, &kp, &tx_b64).await {
            Ok(s) => s,
            Err(e) => {
                log::error!("[Earn] withdraw send failed: {e}");
                return format!("END Withdraw failed: {e}");
            }
        };

        if fee_raw > 0 {
            if let Err(e) = transfer_user_spl_to_owner_with_gas(
                rpc,
                config,
                &kp,
                &mint_pk,
                &fee_recipient,
                fee_raw,
                dec,
            )
            .await
            {
                log::error!("[Earn] post-withdraw fee failed: {e}");
                let short = &sig[..sig.len().min(8)];
                let link = sms::tx_explorer_url(config, &sig);
                return format!(
                    "END Withdraw submitted (ref {short}) but fee transfer failed: {e}\n{link}"
                );
            }
        }

        log_user_activity(
            pool,
            user_id,
            EVT_EARN_WITHDRAW,
            ST_CONFIRMED,
            Some(&sig),
            i64::try_from(w_raw).ok(),
            Some(&mint_s),
            None,
            None,
            None,
            None,
            json!({
                "token_code": code,
                "amount_human": amount,
            }),
        );
        let short = &sig[..sig.len().min(8)];
        let link = sms::tx_explorer_url(config, &sig);
        let done_amt = if matches!(slot, MintSlot::Wsol) {
            ctx.fmt_sol(amount, 40)
        } else {
            ctx.fmt_stable(amount, &code, 40)
        };
        format!("END Withdrew {}.\nRef: {short}\n{link}", done_amt)
    } else {
        "END Too many steps. Start withdraw again.".into()
    }
}
