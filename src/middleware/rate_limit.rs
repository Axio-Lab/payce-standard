use redis::AsyncCommands;

use crate::config::AppConfig;

pub async fn is_rate_limited(
    redis: &redis::Client,
    identifier: &str,
    prefix: &str,
    max_requests: u32,
    window_seconds: i64,
) -> bool {
    let key = format!("{prefix}:{identifier}");
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("[RateLimit] Redis unavailable for {prefix}: {e}");
            return true;
        }
    };

    let current: u32 = match conn.incr(&key, 1u32).await {
        Ok(v) => v,
        Err(e) => {
            log::error!("[RateLimit] Redis incr failed for {prefix}: {e}");
            return true;
        }
    };
    if current == 1 {
        if let Err(e) = conn.expire::<_, ()>(&key, window_seconds).await {
            log::error!("[RateLimit] Redis expire failed for {prefix}: {e}");
            return true;
        }
    }
    current > max_requests
}

pub async fn is_phone_rate_limited(redis: &redis::Client, phone: &str, config: &AppConfig) -> bool {
    is_rate_limited(
        redis,
        phone,
        "rl:phone",
        config.rate_limit_max_per_phone,
        config.rate_limit_window_seconds,
    )
    .await
}

pub async fn is_ip_rate_limited(redis: &redis::Client, ip: &str, config: &AppConfig) -> bool {
    is_rate_limited(
        redis,
        ip,
        "rl:ip",
        config.rate_limit_max_per_ip,
        config.rate_limit_window_seconds,
    )
    .await
}
