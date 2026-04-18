use crate::config::AppConfig;
use crate::services::pin::verify_pin;

pub fn log_ussd_unexpected_shape(label: &str, inputs: &[String]) {
    log::warn!(
        "[USSD] {label}: unexpected segment shape len={} segment_lengths={:?}",
        inputs.len(),
        inputs.iter().map(|s| s.len()).collect::<Vec<_>>()
    );
}

pub async fn verify_pin_or_fail(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    config: &AppConfig,
    user_id: &str,
    pin: &str,
) -> Option<String> {
    match verify_pin(pool, redis, config, user_id, pin).await {
        Ok(r) if r.locked => Some(format!(
            "END Account locked due to too many failed attempts. Try again in {} minutes.",
            config.lockout_minutes
        )),
        Ok(r) if !r.success => {
            let remaining = r.attempts_remaining.unwrap_or(0);
            Some(format!("END Wrong PIN. {remaining} attempt(s) remaining."))
        }
        Ok(_) => None,
        Err(e) => {
            log::error!("[PIN] verify_pin internal error for user {user_id}: {e}");
            Some("END We could not verify your PIN right now. Please try again shortly.".into())
        }
    }
}
