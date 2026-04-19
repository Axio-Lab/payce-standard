use deadpool_postgres::Pool;
use redis::Client as RedisClient;

use crate::config::AppConfig;
use crate::services::solana_rpc::SolanaRpc;

use super::defi_earn;
use super::defi_swap;

pub async fn handle_defi(
    pool: &Pool,
    redis: &RedisClient,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    user_phone: &str,
    inputs: &[String],
) -> String {
    if inputs.len() == 1 {
        return "CON DeFi (powered by Jupiter)\n1. Swap Token\n2. Lend & Earn".into();
    }

    match inputs[1].as_str() {
        "1" => {
            defi_swap::handle_swap_token(pool, redis, rpc, config, user_id, user_phone, &inputs[2..])
                .await
        }
        "2" => {
            defi_earn::handle_lend_earn(pool, redis, rpc, config, user_id, user_phone, &inputs[2..])
                .await
        }
        _ => {
            "CON That option is not listed.\nDeFi (powered by Jupiter)\n1. Swap Token\n2. Lend & Earn\nUse 0. Back below to return to the main menu."
                .into()
        }
    }
}
