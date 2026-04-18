use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::exchange_rate::*;
use crate::services::solana_rpc::SolanaRpc;
use crate::services::wallet::{get_native_sol_balance, get_spl_token_balance};

pub async fn handle_check_balance(
    pool: &deadpool_postgres::Pool,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
) -> String {
    let uid = Uuid::parse_str(user_id).unwrap();
    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("[USSD] balance pool get: {e}");
            return "END Service temporarily unavailable. Please try again shortly.".into();
        }
    };
    let row = client
        .query_opt("SELECT solana_pubkey FROM users WHERE id = $1", &[&uid])
        .await
        .unwrap_or(None);
    let pubkey = match row {
        Some(row) => {
            let pk: Option<String> = row.get(0);
            match pk {
                Some(pk) => pk,
                None => return "END Wallet not set up.".into(),
            }
        }
        None => return "END Wallet not set up.".into(),
    };
    let rate = get_usd_to_ngn_rate(config).await;
    let sol = get_native_sol_balance(rpc, &pubkey).await;
    let mut lines: Vec<String> = vec!["END Your balance:".into()];
    for s in &config.stable_coins {
        let b = get_spl_token_balance(rpc, &pubkey, &s.mint).await;
        let ngn = usd_to_ngn(b, rate);
        lines.push(format!(
            "{}: {} (~{})",
            s.code,
            format_stable_qty_trimmed(b),
            format_ngn(ngn)
        ));
    }
    lines.push(format!("SOL: {}", format_sol(sol)));
    lines.push(format!("Rate: 1 USD = {}", format_ngn(rate)));
    lines.join("\n")
}
